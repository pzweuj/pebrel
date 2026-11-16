//! Local X display used by SSH X11 forwarding.
//!
//! Windows prefers VcXsrv for application compatibility, with optional yserver. Other
//! platforms use the display already selected by `DISPLAY`. The server is a
//! separate process so its license stays outside `pebrel.exe`.

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::{Mutex, OnceLock};

#[cfg(all(test, windows))]
mod render_tests;
#[cfg(windows)]
mod windows_shell;

pub(crate) fn run_shell_helper_if_requested() -> Option<i32> {
    #[cfg(windows)]
    return windows_shell::run_if_requested();
    #[cfg(not(windows))]
    None
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DisplayTarget {
    Tcp {
        host: String,
        port: u16,
    },
    #[cfg_attr(windows, allow(dead_code))]
    Unix(PathBuf),
}

pub(crate) struct LocalDisplay {
    pub(crate) target: DisplayTarget,
    pub(crate) cookie: [u8; 16],
    pub(crate) lease: DisplayLease,
}

pub(crate) struct DisplayLease {
    active: bool,
    cookie: [u8; 16],
}

impl Drop for DisplayLease {
    fn drop(&mut self) {
        if self.active {
            release(self.cookie);
        }
    }
}

struct Running {
    users: usize,
    #[cfg_attr(not(windows), allow(dead_code))]
    child: Option<Child>,
    #[cfg_attr(not(windows), allow(dead_code))]
    group: Option<crate::platform::process::ProcessGroup>,
    authority: Option<PathBuf>,
}

struct AuthorityCleanup(Option<PathBuf>);
impl Drop for AuthorityCleanup {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Own one display lease for the resident GUI, without delaying its first frame.
pub(crate) struct Resident {
    stop: Option<std::sync::mpsc::Sender<()>>,
}

impl Resident {
    pub(crate) fn start() -> Self {
        #[cfg(windows)]
        {
            let (stop, stopped) = std::sync::mpsc::channel();
            match std::thread::Builder::new().name("x11-display".into()).spawn(
                move || match ensure() {
                    Ok(mut display) => {
                        log::info!("Local X11 display is ready: {:?}", display.target);
                        while matches!(
                            stopped.recv_timeout(std::time::Duration::from_secs(1)),
                            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
                        ) {
                            if let Ok(current) = ensure() {
                                display = current;
                            }
                        }
                        drop(display);
                    },
                    Err(error) => log::warn!("Local X11 display startup failed: {error}"),
                },
            ) {
                Ok(_) => return Self { stop: Some(stop) },
                Err(error) => log::warn!("Could not start the X11 display worker: {error}"),
            }
        }
        Self { stop: None }
    }
}

impl Drop for Resident {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

struct Shared {
    target: DisplayTarget,
    cookie: [u8; 16],
    running: Running,
}

impl Shared {
    fn release_user(&mut self, cookie: [u8; 16]) -> bool {
        if self.cookie != cookie {
            return false;
        }
        self.running.users = self.running.users.saturating_sub(1);
        self.running.users == 0
    }
}

fn state() -> &'static Mutex<Option<Shared>> {
    static STATE: OnceLock<Mutex<Option<Shared>>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(None))
}

pub(crate) fn ensure() -> Result<LocalDisplay, String> {
    let mut state = state().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let exited = state
        .as_mut()
        .and_then(|shared| shared.running.child.as_mut())
        .is_some_and(|child| matches!(child.try_wait(), Ok(Some(_))));
    if exited {
        if let Some(previous) = state.take() {
            stop(previous.running);
        }
    }
    if let Some(shared) = state.as_mut() {
        shared.running.users = shared.running.users.saturating_add(1);
        return Ok(LocalDisplay {
            target: shared.target.clone(),
            cookie: shared.cookie,
            lease: DisplayLease { active: true, cookie: shared.cookie },
        });
    }
    let started = start()?;
    *state = Some(started);
    let shared = state.as_ref().expect("display was just installed");
    Ok(LocalDisplay {
        target: shared.target.clone(),
        cookie: shared.cookie,
        lease: DisplayLease { active: true, cookie: shared.cookie },
    })
}

fn release(cookie: [u8; 16]) {
    let mut state = state().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let Some(shared) = state.as_mut() else { return };
    if !shared.release_user(cookie) {
        return;
    }
    if let Some(running) = state.take() {
        stop(running.running);
    }
}

fn stop(mut running: Running) {
    if let (Some(group), Some(child)) = (running.group.as_ref(), running.child.as_mut()) {
        group.terminate(child);
    } else if let Some(child) = running.child.as_mut() {
        let _ = child.kill();
    }
    if let Some(child) = running.child.as_mut() {
        let _ = child.wait();
    }
    if let Some(authority) = running.authority {
        let _ = std::fs::remove_file(authority);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Backend {
    Yserver,
    Vcxsrv,
}

impl Backend {
    fn filename(self) -> &'static str {
        match self {
            Self::Yserver => "yserver.exe",
            Self::Vcxsrv => "vcxsrv.exe",
        }
    }
}

#[cfg(windows)]
fn start() -> Result<Shared, String> {
    let candidates = server_candidates();
    if candidates.is_empty() {
        return Err(missing_server_message());
    }
    start_candidates(candidates, start_server)
}

#[cfg(any(windows, test))]
fn start_candidates(
    candidates: Vec<(Backend, PathBuf)>,
    mut launch: impl FnMut(Backend, &Path) -> Result<Shared, String>,
) -> Result<Shared, String> {
    let mut failures = Vec::new();
    for (backend, executable) in candidates {
        match launch(backend, &executable) {
            Ok(display) => return Ok(display),
            Err(error) => {
                log::warn!("{} failed: {error}", executable.display());
                failures.push(format!("{}: {error}", backend.filename()));
            },
        }
    }
    Err(failures.join("; "))
}

#[cfg(windows)]
fn start_server(backend: Backend, executable: &Path) -> Result<Shared, String> {
    let display = free_display()?;
    let cookie = random_cookie();
    let authority = authority_path()?;
    write_authority(&authority, display, &cookie)
        .map_err(|error| format!("could not write the X11 authority file: {error}"))?;
    let mut authority_cleanup = AuthorityCleanup(Some(authority.clone()));
    let log = authority.parent().unwrap_or(Path::new(".")).join(format!(
        "{}-{}-{display}.log",
        backend.filename(),
        std::process::id()
    ));
    let mut command = Command::new(&executable);
    command
        .current_dir(executable.parent().unwrap_or(Path::new(".")))
        .args(backend_args(backend, display, &authority, &log));
    if backend == Backend::Vcxsrv {
        windows_shell::configure(&mut command)?;
        // xkbcomp uses server-N.xkm in TEMP. Keep stale or concurrent displays
        // from colliding with this display's keyboard output.
        let temporary = authority.parent().ok_or("missing X11 runtime directory")?;
        command.env("TEMP", temporary).env("TMP", temporary);
    }
    let output_path =
        if backend == Backend::Yserver { log.clone() } else { log.with_extension("stderr.log") };
    let output = std::fs::File::create(output_path).map_err(|error| error.to_string())?;
    command.stdout(output.try_clone().map_err(|error| error.to_string())?).stderr(output);
    crate::platform::process::configure_process_group(&mut command);
    let mut child =
        command.spawn().map_err(|error| format!("could not start the X11 display: {error}"))?;
    let group = match crate::platform::process::ProcessGroup::attach(&child) {
        Ok(group) => group,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("could not own the X11 display: {error}"));
        },
    };
    let port = 6000 + display;
    if let Err(reason) = wait_for_port(port, &cookie, &mut child) {
        group.terminate(&mut child);
        let _ = child.wait();
        let detail = log_tail(&log);
        let detail = if detail.is_empty() { reason } else { format!("{reason} {detail}") };
        return Err(format!("the built-in X11 display did not open 127.0.0.1:{port}: {detail}"));
    }
    Ok(Shared {
        target: DisplayTarget::Tcp { host: "127.0.0.1".to_owned(), port },
        cookie,
        running: Running {
            users: 1,
            child: Some(child),
            group: Some(group),
            authority: authority_cleanup.0.take(),
        },
    })
}

fn backend_args(backend: Backend, display: u16, authority: &Path, log: &Path) -> Vec<String> {
    match backend {
        Backend::Yserver => vec![
            format!(":{display}"),
            "--rootless".into(),
            "-auth".into(),
            authority.display().to_string(),
        ],
        Backend::Vcxsrv => server_args(display, authority, log),
    }
}

fn server_args(display: u16, authority: &Path, log: &Path) -> Vec<String> {
    // Current VcXsrv follows X.Org and does not listen on TCP unless asked.
    // Without this, the process can be alive while port 6000+n never opens.
    vec![
        format!(":{display}"),
        "-multiwindow".to_owned(),
        "-clipboard".to_owned(),
        "-silent-dup-error".to_owned(),
        "-noreset".to_owned(),
        "-listen".to_owned(),
        "tcp".to_owned(),
        "-nolisten".to_owned(),
        "inet6".to_owned(),
        "-auth".to_owned(),
        authority.display().to_string(),
        "-logfile".to_owned(),
        log.display().to_string(),
    ]
}

#[cfg(windows)]
fn missing_server_message() -> String {
    let expected = std::env::current_exe().ok().and_then(|executable| {
        executable.parent().map(|parent| parent.join("x11").join("vcxsrv.exe"))
    });
    match expected {
        Some(path) => format!(
            "Pebrel's X11 display is missing. Put yserver/yserver.exe or vcxsrv.exe, with its runtime files, under {}.",
            path.parent().unwrap_or(Path::new("x11")).display()
        ),
        None => "Pebrel's X11 display is missing. Put yserver or VcXsrv under x11/ beside the application.".to_owned(),
    }
}

#[cfg(windows)]
fn server_candidates() -> Vec<(Backend, PathBuf)> {
    let mut candidates = Vec::new();
    if let Ok(executable) = std::env::current_exe() {
        for backend in [Backend::Yserver, Backend::Vcxsrv] {
            if let Some(found) = executable
                .parent()
                .and_then(|parent| find_backend_under(&parent.join("x11"), backend))
            {
                candidates.push((backend, found));
            }
        }
    }
    for var in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(root) = std::env::var_os(var) {
            let installed = PathBuf::from(root).join("VcXsrv").join("vcxsrv.exe");
            if installed.is_file() {
                if !candidates.iter().any(|(_, path)| *path == installed) {
                    candidates.push((Backend::Vcxsrv, installed));
                }
            }
        }
    }
    order_candidates(&mut candidates, std::env::var("PEBREL_X11_BACKEND").ok().as_deref());
    candidates
}

#[cfg(any(windows, test))]
fn order_candidates(candidates: &mut [(Backend, PathBuf)], preference: Option<&str>) {
    let preferred = match preference {
        Some("yserver") => Backend::Yserver,
        Some("vcxsrv" | "auto") | None => Backend::Vcxsrv,
        Some(value) => {
            log::warn!("Unknown PEBREL_X11_BACKEND {value:?}; preferring VcXsrv");
            Backend::Vcxsrv
        },
    };
    candidates.sort_by_key(|(backend, _)| *backend != preferred);
}

fn server_beside(executable: &Path) -> Option<PathBuf> {
    find_server_under(&executable.parent()?.join("x11"))
}

/// Accept a flat runtime or one installer subdirectory under `x11/`.
fn find_server_under(directory: &Path) -> Option<PathBuf> {
    find_backend_under(directory, Backend::Vcxsrv)
}

fn find_backend_under(directory: &Path, backend: Backend) -> Option<PathBuf> {
    let direct = directory.join(backend.filename());
    if direct.is_file() {
        return Some(direct);
    }
    let entries = std::fs::read_dir(directory).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let nested = path.join(backend.filename());
        if nested.is_file() {
            return Some(nested);
        }
    }
    None
}

#[cfg(not(windows))]
fn start() -> Result<Shared, String> {
    let display = std::env::var("DISPLAY").map_err(|_| {
        "This session has no DISPLAY, so X11 forwarding has nowhere to draw.".to_owned()
    })?;
    let (target, number) = parse_display(&display)?;
    let cookie = read_authority_cookie(number.as_deref()).unwrap_or([0u8; 16]);
    Ok(Shared {
        target,
        cookie,
        running: Running { users: 1, child: None, group: None, authority: None },
    })
}

#[cfg(windows)]
fn free_display() -> Result<u16, String> {
    for display in 20..40 {
        let port = 6000 + display;
        if std::net::TcpListener::bind(("127.0.0.1", port)).is_ok() {
            return Ok(display);
        }
    }
    Err("no free local X11 display number".to_owned())
}

#[cfg(windows)]
fn wait_for_port(port: u16, cookie: &[u8; 16], child: &mut Child) -> Result<(), String> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    for _ in 0..100 {
        if std::time::Instant::now() >= deadline {
            break;
        }
        if probe_display(port, cookie).is_ok() {
            return Ok(());
        }
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!("X server exited ({status})"));
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    Err("X server did not accept an authenticated X11 setup".to_owned())
}

#[cfg(windows)]
fn probe_display(port: u16, cookie: &[u8; 16]) -> io::Result<()> {
    let mut stream = std::net::TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        std::time::Duration::from_millis(100),
    )?;
    stream.set_read_timeout(Some(std::time::Duration::from_millis(100)))?;
    stream.set_write_timeout(Some(std::time::Duration::from_millis(100)))?;
    // X11 setup, little endian, version 11, MIT-MAGIC-COOKIE-1 plus 4-byte padding.
    stream.write_all(&[b'l', 0, 11, 0, 0, 0, 18, 0, 16, 0, 0, 0])?;
    stream.write_all(b"MIT-MAGIC-COOKIE-1")?;
    stream.write_all(&[0, 0])?;
    stream.write_all(cookie)?;
    let mut status = [0];
    stream.read_exact(&mut status)?;
    if status[0] != 1 {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "X11 setup rejected"));
    }
    Ok(())
}

fn log_tail(path: &Path) -> String {
    let Ok(text) = std::fs::read_to_string(path) else { return String::new() };
    let lines: Vec<&str> = text.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
    let start = lines.len().saturating_sub(6);
    lines[start..].join(" ")
}

#[cfg(windows)]
fn authority_path() -> Result<PathBuf, String> {
    let directory = std::env::temp_dir().join("pebrel-x11").join(format!(
        "display-{}-{:016x}",
        std::process::id(),
        u64::from_le_bytes(random_cookie()[..8].try_into().unwrap())
    ));
    std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    Ok(directory.join("Xauthority"))
}

#[cfg(windows)]
fn write_authority(path: &Path, display: u16, cookie: &[u8; 16]) -> io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    crate::platform::private_file::restrict_creation(&mut options);
    let mut file = options.open(path)?;
    let result = write_entry(&mut file, 0, &[127, 0, 0, 1], &display.to_string(), cookie)
        .and_then(|_| write_entry(&mut file, 65535, &[], &display.to_string(), cookie));
    if result.is_err() {
        drop(file);
        let _ = std::fs::remove_file(path);
    }
    result
}

fn write_entry(
    file: &mut impl Write,
    family: u16,
    address: &[u8],
    number: &str,
    cookie: &[u8],
) -> io::Result<()> {
    let name = b"MIT-MAGIC-COOKIE-1";
    file.write_all(&family.to_be_bytes())?;
    file.write_all(&(address.len() as u16).to_be_bytes())?;
    file.write_all(address)?;
    file.write_all(&(number.len() as u16).to_be_bytes())?;
    file.write_all(number.as_bytes())?;
    file.write_all(&(name.len() as u16).to_be_bytes())?;
    file.write_all(name)?;
    file.write_all(&(cookie.len() as u16).to_be_bytes())?;
    file.write_all(cookie)?;
    Ok(())
}

#[cfg(windows)]
fn random_cookie() -> [u8; 16] {
    let mut cookie = [0u8; 16];
    getrandom::fill(&mut cookie).expect("X11 cookie entropy");
    cookie
}

#[cfg(not(windows))]
fn parse_display(display: &str) -> Result<(DisplayTarget, Option<String>), String> {
    let display = display.trim();
    if display.starts_with('/') {
        let socket = PathBuf::from(display);
        if socket.exists() {
            return Ok((DisplayTarget::Unix(socket), Some("0".to_owned())));
        }
    }
    let display = display.trim_start_matches("unix:");
    let (host, rest) = display.rsplit_once(':').unwrap_or(("", display));
    let number_str = rest.split('.').next().unwrap_or(rest);
    let number: u16 =
        number_str.parse().map_err(|_| format!("DISPLAY {display:?} is not a display number"))?;
    if host.is_empty() || host == "unix" {
        let socket = PathBuf::from(format!("/tmp/.X11-unix/X{number}"));
        if socket.exists() {
            return Ok((DisplayTarget::Unix(socket), Some(number.to_string())));
        }
    }
    let host = if host.is_empty() || host == "unix" { "127.0.0.1" } else { host };
    Ok((
        DisplayTarget::Tcp { host: host.to_owned(), port: 6000 + number },
        Some(number.to_string()),
    ))
}

#[cfg(not(windows))]
fn read_authority_cookie(display_number: Option<&str>) -> Option<[u8; 16]> {
    let path = std::env::var_os("XAUTHORITY")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".Xauthority")))?;
    let bytes = std::fs::read(path).ok()?;
    parse_xauthority_cookie(&bytes, display_number)
}

#[cfg(not(windows))]
fn parse_xauthority_cookie(bytes: &[u8], display_number: Option<&str>) -> Option<[u8; 16]> {
    let mut cursor = 0usize;
    let mut fallback = None;
    let take_slice = |cursor: &mut usize, len: usize| -> Option<&[u8]> {
        let slice = bytes.get(*cursor..*cursor + len)?;
        *cursor += len;
        Some(slice)
    };
    let take_u16 = |cursor: &mut usize| -> Option<usize> {
        let pair = bytes.get(*cursor..*cursor + 2)?;
        *cursor += 2;
        Some(usize::from(u16::from_be_bytes([pair[0], pair[1]])))
    };
    while cursor + 2 <= bytes.len() {
        let _family = take_u16(&mut cursor)?;
        let addr_len = take_u16(&mut cursor)?;
        let _addr = take_slice(&mut cursor, addr_len)?;
        let num_len = take_u16(&mut cursor)?;
        let num = take_slice(&mut cursor, num_len)?;
        let name_len = take_u16(&mut cursor)?;
        let name = take_slice(&mut cursor, name_len)?;
        let data_len = take_u16(&mut cursor)?;
        let data = take_slice(&mut cursor, data_len)?;
        if name == b"MIT-MAGIC-COOKIE-1" && data.len() == 16 {
            let mut cookie = [0u8; 16];
            cookie.copy_from_slice(data);
            if display_number.is_some_and(|wanted| num == wanted.as_bytes()) {
                return Some(cookie);
            }
            if fallback.is_none() {
                fallback = Some(cookie);
            }
        }
    }
    fallback
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compatibility_backend_is_preferred_unless_yserver_is_requested() {
        for (preference, preferred) in [
            (None, Backend::Vcxsrv),
            (Some("auto"), Backend::Vcxsrv),
            (Some("vcxsrv"), Backend::Vcxsrv),
            (Some("yserver"), Backend::Yserver),
            (Some("invalid"), Backend::Vcxsrv),
        ] {
            let mut candidates = vec![
                (Backend::Yserver, "bundled-yserver".into()),
                (Backend::Vcxsrv, "bundled-vcxsrv".into()),
                (Backend::Vcxsrv, "installed-vcxsrv".into()),
            ];
            order_candidates(&mut candidates, preference);
            assert_eq!(candidates[0].0, preferred);
            let vcxsrv: Vec<_> = candidates.iter().filter(|(b, _)| *b == Backend::Vcxsrv).collect();
            assert_eq!(vcxsrv[0].1, Path::new("bundled-vcxsrv"));
            assert_eq!(vcxsrv[1].1, Path::new("installed-vcxsrv"));
        }
    }

    fn sample() -> Shared {
        Shared {
            target: DisplayTarget::Tcp { host: "127.0.0.1".into(), port: 6020 },
            cookie: [7; 16],
            running: Running { users: 2, child: None, group: None, authority: None },
        }
    }

    #[test]
    fn old_display_leases_cannot_stop_a_replacement_server() {
        let mut display = sample();
        assert!(!display.release_user([1; 16]));
        assert_eq!(display.running.users, 2);
        assert!(!display.release_user([7; 16]));
        assert!(display.release_user([7; 16]));
    }

    #[test]
    fn a_failed_yserver_falls_back_to_vcxsrv_and_keeps_failure_details() {
        let candidates = || {
            vec![(Backend::Yserver, "yserver.exe".into()), (Backend::Vcxsrv, "vcxsrv.exe".into())]
        };
        let mut tried = Vec::new();
        let result = start_candidates(candidates(), |backend, _| {
            tried.push(backend);
            if backend == Backend::Yserver {
                Err("Vulkan unavailable".into())
            } else {
                Ok(sample())
            }
        });
        assert!(result.is_ok());
        assert_eq!(tried, [Backend::Yserver, Backend::Vcxsrv]);
        let failure = start_candidates(candidates(), |_, _| Err("not ready".into())).err().unwrap();
        assert!(failure.contains("yserver.exe") && failure.contains("vcxsrv.exe"));
        let args = backend_args(Backend::Yserver, 21, Path::new("auth"), Path::new("log"));
        assert_eq!(args, [":21", "--rootless", "-auth", "auth"]);
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "launches the real optional X display; set PEBREL_Y_SERVER_TEST_EXE"]
    fn real_yserver_authenticates_and_releases_its_process() {
        let executable =
            std::env::var_os("PEBREL_Y_SERVER_TEST_EXE").expect("explicit test runtime");
        let display = start_server(Backend::Yserver, Path::new(&executable)).unwrap();
        let DisplayTarget::Tcp { port, .. } = display.target else { panic!("TCP display") };
        let pid = display.running.child.as_ref().unwrap().id();
        probe_display(port, &display.cookie).unwrap();
        assert!(probe_display(port, &[0; 16]).is_err(), "wrong cookies must not connect");
        probe_display(port, &display.cookie).unwrap();
        let authority = display.running.authority.clone().unwrap();
        stop(display.running);
        assert!(!authority.exists());
        assert!(
            std::net::TcpStream::connect(("127.0.0.1", port)).is_err(),
            "owned server {pid} must stop"
        );
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "requires the verified runtime beside the test executable under x11/yserver"]
    fn real_resident_display_survives_the_last_ssh_lease_and_stops_with_its_owner() {
        assert!(std::env::var_os("PEBREL_TEST_RESIDENT_X11").is_some());
        let resident = Resident::start();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(12);
        let (port, cookie) = loop {
            if let Some(display) = state().lock().unwrap().as_ref() {
                let DisplayTarget::Tcp { port, .. } = display.target else { panic!("TCP display") };
                break (port, display.cookie);
            }
            assert!(std::time::Instant::now() < deadline, "resident display did not start");
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        let shell = ensure().unwrap();
        drop(shell);
        probe_display(port, &cookie).unwrap();
        assert!(state().lock().unwrap().is_some(), "the GUI still owns a lease");
        drop(resident);
        while state().lock().unwrap().is_some() {
            assert!(std::time::Instant::now() < deadline, "resident display did not stop");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(std::net::TcpStream::connect(("127.0.0.1", port)).is_err());
    }

    #[test]
    fn server_args_listen_on_tcp_and_keep_the_display_cookie() {
        let authority = Path::new("auth-dir").join("Xauthority");
        let log = Path::new("auth-dir").join("vcxsrv-20.log");
        let args = server_args(20, &authority, &log);
        assert_eq!(args[0], ":20");
        assert!(args.iter().any(|arg| arg == "-noreset"));
        assert!(
            has_pair(&args, "-listen", "tcp"),
            "VcXsrv does not open port 6020 unless TCP listen is requested"
        );
        assert!(has_pair(&args, "-auth", &authority.display().to_string()));
        assert!(has_pair(&args, "-logfile", &log.display().to_string()));
    }

    fn has_pair(args: &[String], left: &str, right: &str) -> bool {
        args.windows(2).any(|pair| pair[0] == left && pair[1] == right)
    }

    #[test]
    fn an_installer_subdirectory_still_counts_as_the_bundled_server() {
        let root = std::env::temp_dir().join(format!("pebrel-x11-layout-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let nested = root.join("x11").join("VcXsrv");
        std::fs::create_dir_all(&nested).unwrap();
        let executable = nested.join("vcxsrv.exe");
        std::fs::write(&executable, b"not a real server").unwrap();
        let launcher = root.join("pebrel.exe");
        std::fs::write(&launcher, b"launcher").unwrap();
        assert_eq!(server_beside(&launcher).as_deref(), Some(executable.as_path()));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn log_tail_keeps_the_last_server_lines() {
        let path = std::env::temp_dir().join(format!("pebrel-x11-log-{}", std::process::id()));
        std::fs::write(&path, "first\n\nWelcome\nFatal: no TCP listener\n").unwrap();
        assert_eq!(log_tail(&path), "first Welcome Fatal: no TCP listener");
        let _ = std::fs::remove_file(&path);
    }
}
