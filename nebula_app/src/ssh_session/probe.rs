//! Connectivity and proxy probing for SSH host settings and network checks.

use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use russh::client;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use super::{
    AuthMethod, ClientSession, NoopSshEventHost, ResolvedRoute, SessionError, SharedSession,
    SshDestination, agent, authenticate_password, authenticated_route, authentication_plan,
    clear_secret, open_transport, resolve_connection_route, runtime, try_keyboard_interactive,
    try_private_key,
};
use crate::proxy_test::{ProxyTestFailure, ProxyTestOutcome, ProxyTestResult, ProxyTestRoute};

/// 编辑器草稿的连通性测试请求。带草稿密码/密钥而不是磁盘 profile——
/// 测试要回答「保存后能不能连上」，不是「上次保存的配置行不行」。
#[derive(Clone)]
pub struct SshTestRequest {
    /// Window-local monotonic id. The editor may be changed and tested again
    /// while an older network task is still finishing, so address equality is
    /// not sufficient to associate a result with its request.
    pub request_id: u64,
    pub destination: String,
    pub auth: crate::ssh_profiles::SshAuthMode,
    pub private_keys: Vec<PathBuf>,
    /// 密码框里的未保存草稿；`None`/空 = 只用密钥与已存凭据。
    pub password: Option<String>,
    pub connection: crate::ssh_profiles::SshConnectionOptions,
    pub proxy_password: Option<String>,
}

impl std::fmt::Debug for SshTestRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SshTestRequest")
            .field("request_id", &self.request_id)
            .field("destination", &self.destination)
            .field("auth", &self.auth)
            .field("private_keys", &self.private_keys)
            .field("connection", &self.connection)
            .field("password", &self.password.as_ref().map(|_| "[redacted]"))
            .field("proxy_password", &self.proxy_password.as_ref().map(|_| "[redacted]"))
            .finish()
    }
}

pub(super) const TEST_TIMEOUT: Duration = Duration::from_secs(12);

/// 一次草稿测试的完成数据。旧 winit 壳与 GPUI 设置页共用它：业务层只负责
/// 解析、代理和认证，具体 UI 决定怎样把结果投递回自己的事件循环。
#[derive(Debug, Clone)]
pub struct SshTestResult {
    pub request_id: u64,
    pub destination: String,
    pub ok: bool,
    pub message: String,
    pub elapsed_ms: u64,
}

/// 执行一次草稿测试。认证不弹 AskPass：草稿/已存密码可以回答明确的
/// keyboard-interactive 密码问题；未知主机密钥仍须用户在确认对话框中明确信任。
async fn run_test(request: SshTestRequest) -> SshTestResult {
    let started = std::time::Instant::now();
    let request_id = request.request_id;
    let raw = request.destination.clone();
    // Keep the short budget for local resolution; host-key confirmation is
    // user driven and therefore intentionally outside it.
    let route_outcome = tokio::time::timeout(TEST_TIMEOUT, async {
        let resolved = tokio::task::spawn_blocking({
            let raw = raw.clone();
            move || SshDestination::resolve(&raw)
        })
        .await
        .map_err(|err| -> SessionError {
            format!("SSH 地址解析任务失败: {err}").into()
        })??;
        let profile = crate::ssh_profiles::SshProfileAuth {
            destination: request.destination.clone(),
            auth: request.auth,
            private_keys: request.private_keys.clone(),
            label: None,
            icon: None,
            connection: request.connection.clone(),
        };
        let route =
            resolve_connection_route(&resolved, &profile, 0, request.proxy_password.clone())
                .await?;
        Ok::<_, SessionError>(route)
    })
    .await;
    let (ok, message) = match route_outcome {
        Ok(Ok(route)) => match test_connect(&route, &request).await {
            Ok(()) => (true, String::new()),
            Err(err) => (false, err.to_string()),
        },
        Ok(Err(err)) => (false, err.to_string()),
        Err(_) => (false, format!("连接超时（{} 秒无响应）", TEST_TIMEOUT.as_secs())),
    };
    SshTestResult {
        request_id,
        destination: raw,
        ok,
        message,
        elapsed_ms: started.elapsed().as_millis() as u64,
    }
}

/// 启动测试并交给调用方异步等待结果。GPUI 没有旧壳的 winit `EventLoopProxy`，
/// 所以它通过这个 receiver 把结果安全地回写到自己的 Entity；网络与认证仍然
/// 完全复用同一条 Tokio SSH runtime。
pub fn start_test(
    request: SshTestRequest,
) -> io::Result<tokio::sync::oneshot::Receiver<SshTestResult>> {
    let (mut sender, receiver) = tokio::sync::oneshot::channel();
    runtime()?.spawn(async move {
        tokio::select! {
            biased;
            _ = sender.closed() => {},
            result = run_test(request) => { let _ = sender.send(result); },
        }
    });
    Ok(receiver)
}

/// 旧 winit 壳的事件投递适配器。测试实现本身在 [`run_test`]，避免两个壳
/// 各自维护解析、代理和认证的近似副本。
#[cfg(feature = "legacy-shell")]
pub fn spawn_test(
    request: SshTestRequest,
    proxy: winit::event_loop::EventLoopProxy<crate::event::Event>,
    window_id: winit::window::WindowId,
) -> io::Result<()> {
    runtime()?.spawn(async move {
        let result = run_test(request).await;
        let _ = proxy.send_event(crate::event::Event::new(
            crate::event::EventType::SshTestDone {
                request_id: result.request_id,
                destination: result.destination,
                ok: result.ok,
                message: result.message,
                elapsed_ms: result.elapsed_ms,
            },
            window_id,
        ));
    });
    Ok(())
}

// ---- 设置→网络：真实出网测试 ----

const NETWORK_TEST_HOST: &str = "example.com";
const NETWORK_TEST_PORT: u16 = 80;

trait NetworkTestStream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> NetworkTestStream for T {}

struct RetainedNetworkStream {
    stream: Box<dyn NetworkTestStream>,
    _jump_sessions: Vec<SharedSession>,
}

impl AsyncRead for RetainedNetworkStream {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
        buffer: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::pin::Pin::new(self.stream.as_mut()).poll_read(context, buffer)
    }
}

impl AsyncWrite for RetainedNetworkStream {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
        buffer: &[u8],
    ) -> std::task::Poll<io::Result<usize>> {
        std::pin::Pin::new(self.stream.as_mut()).poll_write(context, buffer)
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::pin::Pin::new(self.stream.as_mut()).poll_flush(context)
    }

    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::pin::Pin::new(self.stream.as_mut()).poll_shutdown(context)
    }
}

/// 一次出网测试的完成数据。旧 winit 壳经事件投递；GPUI 设置页走 oneshot，
/// 握手与 HTTP 探测仍是 [`proxy_test_once`] 这一条路径。
fn proxy_test_pair(
    outcome: Result<Result<ProxyTestRoute, ProxyTestFailure>, tokio::time::error::Elapsed>,
) -> ProxyTestOutcome {
    match outcome {
        Ok(Ok(route)) => ProxyTestOutcome::Success(route),
        Ok(Err(error)) => ProxyTestOutcome::Failed(error),
        Err(_) => {
            ProxyTestOutcome::Failed(ProxyTestFailure::Timeout { seconds: TEST_TIMEOUT.as_secs() })
        },
    }
}

async fn run_proxy_test(request_id: u64) -> ProxyTestResult {
    let started = std::time::Instant::now();
    let outcome = proxy_test_pair(tokio::time::timeout(TEST_TIMEOUT, proxy_test_once()).await);
    ProxyTestResult { request_id, outcome, elapsed_ms: started.elapsed().as_millis() as u64 }
}

/// 启动出网测试并交给调用方异步等待。GPUI 没有 winit `EventLoopProxy`，
/// 所以通过这个 receiver 把结果回写到设置页 Entity；握手逻辑仍在
/// [`proxy_test_once`]。
pub fn start_proxy_test(
    request_id: u64,
) -> io::Result<tokio::sync::oneshot::Receiver<ProxyTestResult>> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    runtime()?.spawn(async move {
        let _ = sender.send(run_proxy_test(request_id).await);
    });
    Ok(receiver)
}

/// 使用与 SSH 新连接相同的全局配置解析和代理握手建立字节流，再请求一个
/// 真实 HTTP 页面。只探测代理端口并不能证明代理有出网能力，所以这里必须
/// 收到目标站点的 HTTP 状态行才算成功。
#[cfg(feature = "legacy-shell")]
pub fn spawn_proxy_test(
    request_id: u64,
    proxy: winit::event_loop::EventLoopProxy<crate::event::Event>,
    window_id: winit::window::WindowId,
) -> io::Result<()> {
    runtime()?.spawn(async move {
        let result = run_proxy_test(request_id).await;
        let _ = proxy.send_event(crate::event::Event::new(
            crate::event::EventType::ProxyTestDone {
                request_id: result.request_id,
                outcome: result.outcome,
                elapsed_ms: result.elapsed_ms,
            },
            window_id,
        ));
    });
    Ok(())
}

async fn proxy_test_once() -> Result<ProxyTestRoute, ProxyTestFailure> {
    let global = tokio::task::spawn_blocking(crate::ssh_proxy::SshProxyConfig::load_global)
        .await
        .map_err(|error| ProxyTestFailure::LoadSettings(error.to_string()))?;
    let link = global
        .resolve(None, NETWORK_TEST_HOST)
        .map_err(|error| ProxyTestFailure::InvalidSettings(error.to_string()))?;
    let route = match &link {
        Some(crate::ssh_proxy::ProxyLink::Server(server)) => {
            ProxyTestRoute::ProxyServer(server.display())
        },
        Some(crate::ssh_proxy::ProxyLink::Jump(target)) => ProxyTestRoute::SshJump(target.clone()),
        Some(crate::ssh_proxy::ProxyLink::Command(_)) => ProxyTestRoute::CustomCommand,
        None if global.mode == crate::ssh_proxy::ProxyMode::Custom => ProxyTestRoute::DirectAddress,
        None => ProxyTestRoute::Direct,
    };
    let mut stream = proxy_test_stream(link.as_ref()).await?;
    stream
        .write_all(
            b"GET / HTTP/1.1\r\nHost: example.com\r\nConnection: close\r\nUser-Agent: Nebula-Network-Test\r\n\r\n",
        )
        .await
        .map_err(|error| ProxyTestFailure::SendRequest(error.to_string()))?;
    stream.flush().await.map_err(|error| ProxyTestFailure::SendRequest(error.to_string()))?;

    let mut response = Vec::with_capacity(1024);
    let mut chunk = [0u8; 512];
    while response.len() < 2048 && !response.windows(2).any(|part| part == b"\r\n") {
        let read = stream
            .read(&mut chunk)
            .await
            .map_err(|error| ProxyTestFailure::ReadResponse(error.to_string()))?;
        if read == 0 {
            break;
        }
        response.extend_from_slice(&chunk[..read]);
    }
    let head = String::from_utf8_lossy(&response);
    let status_line = head.lines().next().unwrap_or_default();
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|value| value.parse::<u16>().ok())
        .ok_or_else(|| ProxyTestFailure::InvalidHttpStatusLine(status_line.to_owned()))?;
    if !(200..500).contains(&status) {
        return Err(ProxyTestFailure::HttpStatus { status });
    }
    Ok(route)
}

async fn proxy_test_stream(
    link: Option<&crate::ssh_proxy::ProxyLink>,
) -> Result<Box<dyn NetworkTestStream>, ProxyTestFailure> {
    use crate::ssh_proxy::ProxyLink;
    match link {
        Some(ProxyLink::Server(server)) => {
            crate::ssh_proxy::connect(server, NETWORK_TEST_HOST, NETWORK_TEST_PORT)
                .await
                .map(|stream| Box::new(stream) as Box<dyn NetworkTestStream>)
                .map_err(|error| ProxyTestFailure::ProxyServer {
                    server: server.display(),
                    error: error.to_string(),
                })
        },
        Some(ProxyLink::Command(command)) => {
            crate::ssh_proxy::connect_command(command, NETWORK_TEST_HOST, NETWORK_TEST_PORT)
                .await
                .map(|stream| Box::new(stream) as Box<dyn NetworkTestStream>)
                .map_err(|error| ProxyTestFailure::CustomCommand(error.to_string()))
        },
        Some(ProxyLink::Jump(spec)) => {
            let (jump_destination, jump_profile) = tokio::task::spawn_blocking({
                let spec = spec.clone();
                move || {
                    let destination = SshDestination::resolve(&spec).map_err(|error| {
                        ProxyTestFailure::JumpResolve {
                            target: spec.clone(),
                            error: error.to_string(),
                        }
                    })?;
                    let path = crate::display::nebula_data_dir().join("ssh_profiles.json");
                    let profile = crate::ssh_profiles::SshProfiles::load(&path)
                        .map_err(|error| ProxyTestFailure::JumpResolve {
                            target: spec.clone(),
                            error: format!("读取 SSH 主机配置失败: {error}"),
                        })?
                        .for_destination(&spec);
                    Ok::<_, ProxyTestFailure>((destination, profile))
                }
            })
            .await
            .map_err(|error| ProxyTestFailure::JumpTask(error.to_string()))??;
            let route = resolve_connection_route(&jump_destination, &jump_profile, 1, None)
                .await
                .map_err(|error| ProxyTestFailure::JumpConnect {
                target: spec.clone(),
                error: error.to_string(),
            })?;
            let jump = authenticated_route(&route, None::<&NoopSshEventHost>, true, false)
                .await
                .map_err(|error| ProxyTestFailure::JumpConnect {
                    target: spec.clone(),
                    error: error.to_string(),
                })?;
            let channel = jump
                .session
                .channel_open_direct_tcpip(
                    NETWORK_TEST_HOST,
                    u32::from(NETWORK_TEST_PORT),
                    "127.0.0.1",
                    0,
                )
                .await
                .map_err(|error| ProxyTestFailure::JumpChannel {
                    target: spec.clone(),
                    error: error.to_string(),
                })?;
            let mut sessions = jump.jump_sessions;
            sessions.push(jump.session);
            Ok(Box::new(RetainedNetworkStream {
                stream: Box::new(channel.into_stream()),
                _jump_sessions: sessions,
            }))
        },
        None => tokio::net::TcpStream::connect((NETWORK_TEST_HOST, NETWORK_TEST_PORT))
            .await
            .map(|stream| Box::new(stream) as Box<dyn NetworkTestStream>)
            .map_err(|error| ProxyTestFailure::Direct(error.to_string())),
    }
}

pub(super) async fn test_connect(
    route: &ResolvedRoute,
    request: &SshTestRequest,
) -> Result<(), SessionError> {
    let config = Arc::new(client::Config {
        inactivity_timeout: None,
        keepalive_interval: None,
        keepalive_max: 3,
        ..Default::default()
    });
    let mut transport = open_transport(route, config, true, true).await?;
    test_authenticate(&mut transport.session, &route.destination, request).await
}

/// 无人值守版认证沿用正式连接的计划；草稿密码只替代其中的保存密码。明确的
/// keyboard-interactive 密码问题可以复用已有密码；OTP 与「连接时询问」在这里
/// 跳过——测试不能弹框，也不能把真实的二次验证误报成配置错误。
pub(super) async fn test_authenticate(
    session: &mut ClientSession,
    destination: &SshDestination,
    request: &SshTestRequest,
) -> Result<(), SessionError> {
    let mut using_agent = false;
    match tokio::time::timeout(
        TEST_TIMEOUT,
        test_authentication_plan(session, destination, request, &mut using_agent),
    )
    .await
    {
        Ok(result) => result,
        Err(_) if using_agent => Err(agent::timed_out()),
        Err(_) => Err(format!("认证超时（{} 秒无响应）", TEST_TIMEOUT.as_secs()).into()),
    }
}

async fn test_authentication_plan(
    session: &mut ClientSession,
    destination: &SshDestination,
    request: &SshTestRequest,
    using_agent: &mut bool,
) -> Result<(), SessionError> {
    if session.authenticate_none(&destination.user).await?.success() {
        return Ok(());
    }
    let has_draft_password =
        request.password.as_deref().is_some_and(|password| !password.is_empty());
    let plan =
        authentication_plan(request.auth, &request.private_keys, &destination.identity_files);
    let mut interactive_skipped = false;
    let mut stored_password = None;
    let mut loaded_stored_password = false;
    let mut local_key_errors = Vec::new();
    let mut credential_was_attempted = false;
    let mut agent_attempt = None;
    for method in plan {
        *using_agent = matches!(method, AuthMethod::Agent);
        match method {
            AuthMethod::PrivateKey(path) => {
                if matches!(agent_attempt, Some(agent::Attempt::PartialSuccess { .. })) {
                    continue;
                }
                // allow_prompt=false：spawn_test 承诺绝不弹框，密钥口令也
                // 不例外——受口令保护且无已存口令的密钥记为本地问题。
                if try_private_key(session, destination, &path, false, &mut local_key_errors)
                    .await?
                {
                    clear_secret(&mut stored_password);
                    return Ok(());
                }
            },
            AuthMethod::Agent => {
                let attempt =
                    agent::authenticate(session, destination, &request.private_keys).await?;
                if attempt == agent::Attempt::Authenticated {
                    return Ok(());
                }
                agent_attempt = Some(attempt);
            },
            AuthMethod::StoredPassword => {
                // A non-empty draft is the user's explicit answer for this
                // test. Do not let an older credential-manager value turn a
                // wrong draft into a misleading success.
                if has_draft_password {
                    credential_was_attempted = true;
                    let password = request.password.as_deref().unwrap_or_default();
                    if authenticate_password(session, &destination.user, password.as_bytes())
                        .await?
                    {
                        return Ok(());
                    }
                    continue;
                }
                if !loaded_stored_password {
                    stored_password =
                        crate::ssh_credentials::load_stored_password(&destination.original)?;
                    loaded_stored_password = true;
                }
                if let Some(password) = stored_password.as_deref() {
                    credential_was_attempted = true;
                    if authenticate_password(session, &destination.user, password).await? {
                        clear_secret(&mut stored_password);
                        return Ok(());
                    }
                }
            },
            AuthMethod::KeyboardInteractive => {
                let password = request
                    .password
                    .as_deref()
                    .filter(|password| !password.is_empty())
                    .map(str::as_bytes)
                    .or_else(|| stored_password.as_deref());
                let attempt =
                    try_keyboard_interactive(session, destination, password, false).await?;
                credential_was_attempted |= attempt.used_password;
                interactive_skipped |= attempt.prompt_required;
                if attempt.success {
                    clear_secret(&mut stored_password);
                    return Ok(());
                }
            },
            AuthMethod::PromptPassword => {
                // 已经拿草稿/存储密码试过时，继续弹框不会让这次“无人值守测试”
                // 更可信；直接报告认证失败。完全没有凭据时才说明正式连接需要询问。
                interactive_skipped |= !credential_was_attempted;
            },
        }
    }
    clear_secret(&mut stored_password);
    let message = if !local_key_errors.is_empty() {
        format!("私钥无法使用：{}", local_key_errors.join("；"))
    } else if interactive_skipped {
        "服务器可达，但此配置需要连接时交互输入（密码/MFA），测试无法替你完成".into()
    } else {
        "认证未通过：请检查密码、私钥或服务器端授权".into()
    };
    Err(agent::with_diagnostic(message, agent_attempt.as_ref()).into())
}
