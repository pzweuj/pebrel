//! Real GPUI workspace/event routing with an owned in-memory SFTP peer. No SSH
//! authentication, user configuration writes or host filesystem cwd lookups.

use super::*;
use crate::gpui_shell::terminal::view::{TerminalLaunch, TerminalView};
use gpui::{BorrowAppContext as _, Entity, TestAppContext, VisualTestContext};
use gpui_component::Root;
use nebula_terminal::event::Event;
use russh_sftp::protocol::{File, Handle, Name, Status, StatusCode};
use std::cell::{Cell, RefCell};
use std::future::Future;
use std::sync::Mutex;
use std::task::{Context as TaskContext, Poll, Waker};
use std::time::{Duration, Instant};

thread_local! {
    static PUMP_ENABLED: Cell<bool> = const { Cell::new(false) };
    static NETWORK_WAITERS: RefCell<Vec<Waker>> = const { RefCell::new(Vec::new()) };
}

// A real Tokio peer must not wake GPUI's deterministic test scheduler from its
// worker thread. Poll the result on the test thread; the I/O and cancellation
// future are still the same production remote_call path.
pub(super) struct NetworkPump;

impl NetworkPump {
    fn new() -> Self {
        PUMP_ENABLED.set(true);
        Self
    }

    pub(super) fn enabled() -> bool {
        PUMP_ENABLED.get()
    }

    pub(super) async fn receive<T>(mut receiver: tokio::sync::oneshot::Receiver<T>) -> Option<T> {
        std::future::poll_fn(|cx| {
            let mut io_context = TaskContext::from_waker(Waker::noop());
            match std::pin::Pin::new(&mut receiver).poll(&mut io_context) {
                Poll::Ready(value) => Poll::Ready(value.ok()),
                Poll::Pending => {
                    NETWORK_WAITERS.with_borrow_mut(|waiters| {
                        if !waiters.iter().any(|waker| waker.will_wake(cx.waker())) {
                            waiters.push(cx.waker().clone());
                        }
                    });
                    Poll::Pending
                },
            }
        })
        .await
    }

    fn tick() {
        NETWORK_WAITERS
            .with_borrow_mut(|waiters| std::mem::take(waiters))
            .into_iter()
            .for_each(Waker::wake);
    }
}

impl Drop for NetworkPump {
    fn drop(&mut self) {
        PUMP_ENABLED.set(false);
        NETWORK_WAITERS.with_borrow_mut(Vec::clear);
    }
}

#[derive(Default)]
struct Store {
    requested: Vec<String>,
    gates: HashMap<String, Arc<tokio::sync::Semaphore>>,
    denied_once: HashSet<String>,
}

struct Peer {
    store: Arc<Mutex<Store>>,
    listed: HashSet<String>,
}

impl russh_sftp::server::Handler for Peer {
    type Error = StatusCode;

    fn unimplemented(&self) -> Self::Error {
        StatusCode::OpUnsupported
    }

    async fn opendir(&mut self, id: u32, path: String) -> Result<Handle, StatusCode> {
        let gate = {
            let mut store = self.store.lock().unwrap();
            store.requested.push(path.clone());
            if store.denied_once.remove(&path) {
                return Err(StatusCode::PermissionDenied);
            }
            store.gates.get(&path).cloned()
        };
        if let Some(gate) = gate {
            gate.acquire().await.map_err(|_| StatusCode::Failure)?.forget();
        }
        Ok(Handle { id, handle: format!("{id}:{path}") })
    }

    async fn readdir(&mut self, id: u32, handle: String) -> Result<Name, StatusCode> {
        if !self.listed.insert(handle) {
            return Err(StatusCode::Eof);
        }
        Ok(Name { id, files: vec![File::dummy("listing.txt")] })
    }

    async fn close(&mut self, id: u32, handle: String) -> Result<Status, StatusCode> {
        self.listed.remove(&handle);
        Ok(Status {
            id,
            status_code: StatusCode::Ok,
            error_message: String::new(),
            language_tag: String::new(),
        })
    }
}

fn browser_session(destination: &str) -> (SftpBrowseSession, Arc<Mutex<Store>>) {
    let store = Arc::new(Mutex::new(Store::default()));
    (browser_session_with_store(destination, store.clone()), store)
}

fn browser_session_with_store(destination: &str, store: Arc<Mutex<Store>>) -> SftpBrowseSession {
    let state = store.clone();
    let session = crate::ssh_session::runtime().unwrap().block_on(async move {
        let (client, server) = tokio::io::duplex(16 * 1024);
        tokio::spawn(russh_sftp::server::run(
            server,
            Peer { store: state, listed: HashSet::new() },
        ));
        russh_sftp::client::SftpSession::new_with_config(
            client,
            crate::ssh_sftp::limits::session_config(),
        )
        .await
        .unwrap()
    });
    SftpBrowseSession::from_test_session(destination.into(), session)
}

struct Fixture {
    _network_pump: NetworkPump,
    workspace: Entity<NebulaWorkspace>,
    window: VisualTestContext,
    terminals: Vec<Entity<TerminalView>>,
    stores: Vec<Arc<Mutex<Store>>>,
    _inputs: Vec<std::sync::mpsc::Receiver<nebula_terminal::event_loop::Msg>>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        for store in &self.stores {
            for gate in store.lock().unwrap().gates.values() {
                gate.close();
            }
        }
    }
}

fn reconnect(fixture: &mut Fixture, index: usize) {
    // 取消会回收真实子通道；重连也必须使用隔离协议端点，不触及用户 SSH 凭据。
    let session =
        browser_session_with_store("fixture@same-host.invalid", fixture.stores[index].clone());
    let pane = fixture.terminals[index].read_with(&fixture.window, |view, _| view.pane_id);
    fixture.workspace.update(&mut fixture.window, |workspace, _| {
        workspace.remote_browser.browse_sessions.insert(pane, session);
    });
}

fn open(cx: &mut TestAppContext) -> Fixture {
    let network_pump = NetworkPump::new();
    cx.update(|cx| {
        gpui_component::init(cx);
        crate::gpui_shell::init(cx, Default::default());
        super::super::windowing::initialize(cx, crate::runtime_api::RuntimeHub::new());
        cx.set_reduce_motion(true);
    });
    let mut result = None;
    let (_, window) = cx.add_window_view(|window, cx| {
        let workspace = cx.new(|cx| {
            NebulaWorkspace::new(
                window,
                None,
                None,
                1,
                crate::runtime_api::RuntimeHub::new(),
                super::super::windowing::WorkspaceStartup::Empty,
                super::super::windowing::WindowRole::Regular,
                cx,
            )
        });
        let mut terminals = Vec::new();
        let mut stores = Vec::new();
        let mut inputs = Vec::new();
        workspace.update(cx, |workspace, cx| {
            for cwd in ["/srv/Alpha 中", "/srv/Beta"] {
                let pane = workspace.new_pane(
                    (80, 24),
                    TerminalLaunch::Local {
                        cwd: None,
                        shell: Some(nebula_terminal::tty::Shell::new(
                            "pebrel-test-missing-shell-executable".into(),
                            vec![],
                        )),
                        shell_name: None,
                    },
                    None,
                    window,
                    cx,
                );
                let destination = "fixture@same-host.invalid";
                inputs.push(pane.view.update(cx, |view, cx| {
                    let input = view.install_remote_files_test_session(destination, cx);
                    view.remote_files_test_event(Event::CwdReport(cwd.into()), cx);
                    input
                }));
                let (session, store) = browser_session(destination);
                workspace.remote_browser.browse_sessions.insert(pane.id, session);
                stores.push(store);
                terminals.push(pane.view.clone());
                let focused = pane.id;
                workspace.tabs.push(super::super::WorkspaceTab::Terminal {
                    panes: vec![pane],
                    tree: nebula_split::SplitTree::Leaf(focused),
                    focused,
                    zoomed: false,
                    broadcast: false,
                });
            }
            workspace.details_panel.section = None;
            workspace.side_panel.open = true;
            workspace.side_panel.view = crate::display::side_panel::PanelView::Files;
            workspace.sync_side_panel_to_active(false, cx);
            cx.notify();
        });
        result = Some((workspace.clone(), terminals, stores, inputs));
        Root::new(workspace, window, cx)
    });
    let (workspace, terminals, stores, inputs) = result.unwrap();
    Fixture {
        _network_pump: network_pump,
        workspace,
        window: window.clone(),
        terminals,
        stores,
        _inputs: inputs,
    }
}

fn wait(fixture: &mut Fixture, condition: impl Fn(&Fixture) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        NetworkPump::tick();
        fixture.window.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });
        fixture.window.run_until_parked();
        if condition(fixture) {
            return;
        }
        assert!(Instant::now() < deadline, "remote browser regression timed out");
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn listed(fixture: &mut Fixture, path: &str) {
    wait(fixture, |fixture| {
        fixture.workspace.read_with(&fixture.window, |workspace, _| {
            !workspace.remote_browser.loading && workspace.remote_browser.path == path
        })
    });
    fixture.workspace.read_with(&fixture.window, |workspace, _| {
        assert_eq!(workspace.remote_browser.entries[0].path, format!("{path}/listing.txt"));
        assert!(workspace.remote_browser.error.is_none());
    });
}

fn report(fixture: &mut Fixture, terminal: usize, event: Event) {
    fixture.terminals[terminal].update(&mut fixture.window, |view, cx| {
        view.remote_files_test_event(event, cx);
    });
    fixture.window.run_until_parked();
}

fn focus(fixture: &mut Fixture, tab: usize) {
    fixture.workspace.update(&mut fixture.window, |workspace, cx| {
        workspace.active = tab;
        workspace.sync_side_panel_to_active(true, cx);
        cx.notify();
    });
    fixture.window.run_until_parked();
}

#[gpui::test]
fn explicit_pwd_snaps_the_focused_sftp_listing_back_to_the_reported_path(cx: &mut TestAppContext) {
    let mut fixture = open(cx);
    listed(&mut fixture, "/srv/Alpha 中");
    fixture.workspace.update(&mut fixture.window, |workspace, cx| {
        workspace.navigate_remote("/manual".into(), cx);
    });
    listed(&mut fixture, "/manual");
    fixture.terminals[0].update(&mut fixture.window, |view, cx| {
        view.remote_files_test_pwd("/srv/Alpha 中", cx);
    });
    listed(&mut fixture, "/srv/Alpha 中");
    for path in ["/srv/next", "/srv/next"] {
        fixture.terminals[0].update(&mut fixture.window, |view, cx| {
            view.remote_files_test_pwd(path, cx);
        });
        listed(&mut fixture, path);
    }
    fixture.terminals[1].update(&mut fixture.window, |view, cx| {
        view.remote_files_test_pwd("/background", cx);
    });
    fixture.window.run_until_parked();
    listed(&mut fixture, "/srv/next");
}

#[gpui::test]
fn opening_remote_files_and_shell_cwd_events_use_the_focused_pane(cx: &mut TestAppContext) {
    let mut fixture = open(cx);
    listed(&mut fixture, "/srv/Alpha 中");
    assert_eq!(fixture.stores[0].lock().unwrap().requested, ["/srv/Alpha 中"]);
    report(&mut fixture, 0, Event::CwdReport("/srv/next folder".into()));
    listed(&mut fixture, "/srv/next folder");
    report(&mut fixture, 0, Event::Title("NEBULA|/srv/title path|main|".into()));
    listed(&mut fixture, "/srv/title path");
    fixture.workspace.read_with(&fixture.window, |workspace, _| {
        assert!(workspace.side_panel.root().is_none(), "remote cwd must not become a host root");
    });
}

#[gpui::test]
fn material_icon_uses_a_fixed_slot_in_the_production_sftp_row(cx: &mut TestAppContext) {
    use gpui::{AppContext as _, IntoElement, Render};
    struct RowProbe(Entity<NebulaWorkspace>, SftpEntry);
    impl Render for RowProbe {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let mut folder = self.1.clone();
            folder.kind = SftpEntryKind::Directory;
            folder.name = "folder".into();
            folder.path.push_str("/folder");
            div()
                .w(px(320.0))
                .h(px(56.0))
                .flex()
                .flex_col()
                .child(self.0.update(cx, |workspace, cx| {
                    workspace.render_remote_row(self.1.clone(), (false, false), window, cx)
                }))
                .child(self.0.update(cx, |workspace, cx| {
                    workspace.render_remote_row(folder, (false, false), window, cx)
                }))
        }
    }
    let mut fixture = open(cx);
    listed(&mut fixture, "/srv/Alpha 中");
    let mut entry = fixture
        .workspace
        .read_with(&fixture.window, |workspace, _| workspace.remote_browser.entries[0].clone());
    // russh's File::dummy has directory attributes. This fixture also needs a file.
    entry.kind = SftpEntryKind::File;
    let owner = fixture.workspace.clone();
    let (_, window) = cx.add_window_view(|window, cx| {
        let row = cx.new(|_| RowProbe(owner, entry));
        Root::new(row, window, cx)
    });
    window.run_until_parked();
    window.update(|window, cx| window.draw(cx).clear(cx));
    let icon = window
        .debug_bounds("sftp-file-icon-file-icons/material/document.svg")
        .expect("Material icon in the production row");
    assert_eq!(icon.size, gpui::size(px(16.0), px(16.0)));
    let folder = window
        .debug_bounds("sftp-file-icon-file-icons/material/folder-base.svg")
        .expect("Material folder in the production row");
    assert_eq!(folder.size, icon.size);
}

#[gpui::test]
fn same_host_panes_keep_snapshots_and_background_cwd_until_focused(cx: &mut TestAppContext) {
    let mut fixture = open(cx);
    listed(&mut fixture, "/srv/Alpha 中");
    focus(&mut fixture, 1);
    listed(&mut fixture, "/srv/Beta");
    report(&mut fixture, 0, Event::CwdReport("/srv/background".into()));
    listed(&mut fixture, "/srv/Beta");
    assert_eq!(fixture.stores[0].lock().unwrap().requested, ["/srv/Alpha 中"]);
    focus(&mut fixture, 0);
    listed(&mut fixture, "/srv/background");
    focus(&mut fixture, 1);
    listed(&mut fixture, "/srv/Beta");
    assert_eq!(fixture.stores[1].lock().unwrap().requested, ["/srv/Beta"]);
}

#[gpui::test]
fn manual_browsing_survives_same_cwd_polls_but_follows_a_real_shell_change(
    cx: &mut TestAppContext,
) {
    let mut fixture = open(cx);
    listed(&mut fixture, "/srv/Alpha 中");
    fixture.workspace.update(&mut fixture.window, |workspace, cx| {
        workspace.navigate_remote("/manual".into(), cx);
    });
    listed(&mut fixture, "/manual");
    for _ in 0..3 {
        report(&mut fixture, 0, Event::Title("NEBULA|/srv/Alpha 中|different branch|".into()));
        fixture.workspace.update(&mut fixture.window, |workspace, cx| {
            assert!(!workspace.sync_side_panel_to_active(false, cx));
        });
    }
    listed(&mut fixture, "/manual");
    assert_eq!(fixture.stores[0].lock().unwrap().requested.len(), 2);
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    fixture.stores[0].lock().unwrap().gates.insert("/manual/child".into(), gate.clone());
    fixture.workspace.update(&mut fixture.window, |workspace, cx| {
        workspace.navigate_remote("/manual/child".into(), cx);
    });
    wait(&mut fixture, |fixture| {
        fixture.stores[0].lock().unwrap().requested.contains(&"/manual/child".into())
    });
    focus(&mut fixture, 1);
    listed(&mut fixture, "/srv/Beta");
    gate.add_permits(1);
    focus(&mut fixture, 0);
    listed(&mut fixture, "/manual");
    assert_eq!(fixture.stores[0].lock().unwrap().requested.len(), 3);
    reconnect(&mut fixture, 0);
    report(&mut fixture, 0, Event::CwdReport("/srv/moved".into()));
    listed(&mut fixture, "/srv/moved");
}

#[gpui::test]
fn pending_listings_cannot_overwrite_newer_cwd_or_another_pane(cx: &mut TestAppContext) {
    let mut fixture = open(cx);
    listed(&mut fixture, "/srv/Alpha 中");
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    fixture.stores[0].lock().unwrap().gates.insert("/slow".into(), gate.clone());
    report(&mut fixture, 0, Event::CwdReport("/slow".into()));
    wait(&mut fixture, |fixture| {
        fixture.stores[0].lock().unwrap().requested.contains(&"/slow".into())
    });
    reconnect(&mut fixture, 0);
    report(&mut fixture, 0, Event::CwdReport("/newest".into()));
    fixture.workspace.update(&mut fixture.window, |workspace, cx| {
        let generation = workspace.remote_browser.generation;
        for _ in 0..3 {
            workspace.sync_side_panel_to_active(false, cx);
        }
        assert_eq!(
            workspace.remote_browser.generation, generation,
            "polls must not restart a pending listing"
        );
    });
    gate.add_permits(1);
    listed(&mut fixture, "/newest");

    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    fixture.stores[0].lock().unwrap().gates.insert("/old-pane".into(), gate.clone());
    report(&mut fixture, 0, Event::CwdReport("/old-pane".into()));
    wait(&mut fixture, |fixture| {
        fixture.stores[0].lock().unwrap().requested.contains(&"/old-pane".into())
    });
    focus(&mut fixture, 1);
    listed(&mut fixture, "/srv/Beta");
    gate.add_permits(1);
    fixture.window.run_until_parked();
    listed(&mut fixture, "/srv/Beta");
    fixture.stores[0].lock().unwrap().gates.remove("/old-pane");
    reconnect(&mut fixture, 0);
    focus(&mut fixture, 0);
    listed(&mut fixture, "/old-pane");
}

#[gpui::test]
fn failed_follow_keeps_the_old_listing_and_refresh_retries_the_failed_target(
    cx: &mut TestAppContext,
) {
    let mut fixture = open(cx);
    listed(&mut fixture, "/srv/Alpha 中");
    fixture.stores[0].lock().unwrap().denied_once.insert("/denied".into());
    report(&mut fixture, 0, Event::CwdReport("/denied".into()));
    wait(&mut fixture, |fixture| {
        fixture.workspace.read_with(&fixture.window, |workspace, _| {
            !workspace.remote_browser.loading && workspace.remote_browser.error.is_some()
        })
    });
    fixture.workspace.read_with(&fixture.window, |workspace, _| {
        assert_eq!(workspace.remote_browser.path, "/srv/Alpha 中");
        assert_eq!(workspace.remote_browser.retry_path.as_deref(), Some("/denied"));
    });
    report(&mut fixture, 0, Event::Title("NEBULA|/denied|main|".into()));
    assert_eq!(
        fixture.stores[0].lock().unwrap().requested.len(),
        2,
        "failed follow must not retry on every prompt"
    );

    // A failed subsystem is retired by the real browser adapter. Reconnect to
    // another owned SFTP peer before testing the explicit UI retry; no fixture
    // ever falls through to SSH authentication or the user's saved credentials.
    let (session, store) = browser_session("fixture@same-host.invalid");
    fixture.stores[0] = store;
    fixture.workspace.update(&mut fixture.window, |workspace, cx| {
        workspace
            .remote_browser
            .browse_sessions
            .insert(workspace.remote_browser.pane.unwrap(), session);
        workspace.remote_refresh(cx);
    });
    listed(&mut fixture, "/denied");
    assert_eq!(fixture.stores[0].lock().unwrap().requested, ["/denied"]);
}

#[gpui::test]
fn copy_path_feedback_and_paste_jump_navigate_the_remote_drawer(cx: &mut TestAppContext) {
    let mut fixture = open(cx);
    listed(&mut fixture, "/srv/Alpha 中");
    fixture.window.update(|window, cx| window.draw(cx).clear(cx));

    let copy_bounds = fixture.window.debug_bounds("remote-files-copy-path").unwrap();
    fixture.window.simulate_click(copy_bounds.center(), gpui::Modifiers::default());
    fixture.window.run_until_parked();
    let copied = fixture
        .window
        .update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text()))
        .unwrap();
    assert_eq!(copied, "/srv/Alpha 中");
    assert!(fixture.workspace.read_with(&fixture.window, |workspace, cx| {
        workspace
            .remote_browser
            .copy_feedback
            .as_ref()
            .is_some_and(|feedback| feedback.read(cx).is_copied())
    }));
    cx.executor().advance_clock(
        crate::gpui_shell::copy_feedback::COPY_FEEDBACK_TTL + Duration::from_millis(1),
    );
    fixture.window.run_until_parked();
    assert!(!fixture.workspace.read_with(&fixture.window, |workspace, cx| {
        workspace
            .remote_browser
            .copy_feedback
            .as_ref()
            .is_some_and(|feedback| feedback.read(cx).is_copied())
    }));

    let path_bounds = fixture.window.debug_bounds("remote-files-path").unwrap();
    fixture.window.simulate_click(path_bounds.center(), gpui::Modifiers::default());
    fixture.window.run_until_parked();
    fixture.window.update(|window, cx| window.draw(cx).clear(cx));
    assert!(fixture.window.debug_bounds("remote-files-path-editor").is_some());

    fixture.window.update(|_, cx| {
        cx.write_to_clipboard(gpui::ClipboardItem::new_string("/srv/pasted".to_owned()));
    });
    let paste = if crate::platform::Platform::current() == crate::platform::Platform::MacOS {
        "cmd-v"
    } else {
        "ctrl-v"
    };
    fixture.window.simulate_keystrokes(paste);
    fixture.window.simulate_keystrokes("enter");
    listed(&mut fixture, "/srv/pasted");
    assert!(
        fixture
            .workspace
            .read_with(&fixture.window, |workspace, _| { workspace.remote_path.is_none() })
    );
}

#[gpui::test]
fn a_shell_without_cwd_reports_follows_the_next_unique_probe(cx: &mut TestAppContext) {
    const DESTINATION: &str = "fixture@same-host.invalid";
    let _probes = ProbeQueueGuard::new(DESTINATION);
    let mut fixture = open(cx);
    listed(&mut fixture, "/srv/Alpha 中");
    fixture.workspace.update(&mut fixture.window, |workspace, cx| {
        workspace.navigate_remote("/manual".into(), cx);
    });
    listed(&mut fixture, "/manual");
    let listed_before = fixture.stores[0].lock().unwrap().requested.len();

    fixture.window.update(|_, cx| {
        cx.update_global::<crate::gpui_shell::config::Settings, _>(|settings, _| {
            settings.sftp_follow_session = true;
        });
    });
    fixture.terminals[0].update(&mut fixture.window, |view, _| {
        view.forget_reported_cwd_for_test();
    });
    queue_test_probe(
        DESTINATION,
        crate::ssh_sftp::remote_cwd::RemoteCwd::Located("/srv/Alpha 中".into()),
    );
    fixture.workspace.update(&mut fixture.window, |workspace, cx| {
        workspace.remote_browser.cwd_probe_at = None;
        workspace.sync_side_panel_to_active(false, cx);
    });
    wait(&mut fixture, |fixture| {
        fixture
            .workspace
            .read_with(&fixture.window, |workspace, _| !workspace.remote_browser.cwd_probe_running)
    });
    fixture.workspace.read_with(&fixture.window, |workspace, _| {
        assert_eq!(workspace.remote_browser.path, "/manual");
    });
    assert_eq!(fixture.stores[0].lock().unwrap().requested.len(), listed_before);

    queue_test_probe(
        DESTINATION,
        crate::ssh_sftp::remote_cwd::RemoteCwd::Located("/srv/probed".into()),
    );
    fixture.workspace.update(&mut fixture.window, |workspace, cx| {
        workspace.remote_browser.cwd_probe_at = None;
        workspace.sync_side_panel_to_active(false, cx);
    });
    listed(&mut fixture, "/srv/probed");

    fixture.terminals[0].update(&mut fixture.window, |view, _| {
        view.forget_reported_cwd_for_test();
    });
    fixture.workspace.update(&mut fixture.window, |workspace, cx| {
        workspace.remote_browser.cwd_probe_at = None;
        workspace.sync_side_panel_to_active(false, cx);
    });
    wait(&mut fixture, |fixture| {
        fixture
            .workspace
            .read_with(&fixture.window, |workspace, _| !workspace.remote_browser.cwd_probe_running)
    });
    fixture.workspace.read_with(&fixture.window, |workspace, _| {
        assert_eq!(
            workspace.remote_browser.path, "/srv/probed",
            "an ambiguous probe must not move the listing"
        );
    });
}

struct ProbeQueueGuard(&'static str);

impl ProbeQueueGuard {
    fn new(destination: &'static str) -> Self {
        clear_test_probes(destination);
        Self(destination)
    }
}

impl Drop for ProbeQueueGuard {
    fn drop(&mut self) {
        clear_test_probes(self.0);
    }
}

#[gpui::test]
fn sftp_follow_session_switch_pauses_and_snaps_back_when_enabled(cx: &mut TestAppContext) {
    use crate::gpui_shell::settings_fixture::{SettingsBytesGuard, lock_theme_studio};

    let _fixture_guard = lock_theme_studio();
    let _guard = SettingsBytesGuard::capture();
    let mut fixture = open(cx);
    listed(&mut fixture, "/srv/Alpha 中");
    fixture.window.update(|window, cx| window.draw(cx).clear(cx));

    let toggle = fixture.window.debug_bounds("nebula-switch-sftp-follow-session").unwrap();
    fixture.window.simulate_click(toggle.center(), gpui::Modifiers::default());
    fixture.window.run_until_parked();
    assert!(
        !fixture
            .workspace
            .read_with(&fixture.window, |workspace, cx| { workspace.sftp_follows_session(cx) })
    );

    report(&mut fixture, 0, Event::CwdReport("/srv/moved-while-paused".into()));
    fixture.window.update(|window, cx| window.draw(cx).clear(cx));
    assert_eq!(
        fixture
            .workspace
            .read_with(&fixture.window, |workspace, _| { workspace.remote_browser.path.clone() }),
        "/srv/Alpha 中"
    );

    let toggle = fixture.window.debug_bounds("nebula-switch-sftp-follow-session").unwrap();
    fixture.window.simulate_click(toggle.center(), gpui::Modifiers::default());
    listed(&mut fixture, "/srv/moved-while-paused");
}

#[gpui::test]
fn drop_upload_paths_uses_target_directory_and_rejects_when_unready(cx: &mut TestAppContext) {
    let mut fixture = open(cx);
    listed(&mut fixture, "/srv/Alpha 中");
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    fixture.stores[0].lock().unwrap().gates.insert("/srv/sub".into(), gate.clone());
    let local = tempfile::NamedTempFile::new().unwrap();
    let path = local.path().to_path_buf();
    let workspace = fixture.workspace.clone();

    fixture.window.update(|window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.remote_browser.loading = true;
            workspace.drop_upload_paths(vec![path.clone()], None, window, cx);
            assert!(workspace.remote_browser.error.is_some());
            workspace.remote_browser.loading = false;
            workspace.remote_browser.error = None;
            workspace.drop_upload_paths(vec![path], Some("/srv/sub".into()), window, cx);
            assert!(workspace.remote_browser.preflighting);
        });
    });
    wait(&mut fixture, |fixture| {
        fixture.stores[0].lock().unwrap().requested.contains(&"/srv/sub".to_owned())
    });
}
