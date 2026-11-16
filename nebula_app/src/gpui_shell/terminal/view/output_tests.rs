use super::startup_tests::{feed, open};
use super::*;
use gpui::TestAppContext;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

#[gpui::test]
fn pwd_enter_before_ssh_echo_completes_keeps_the_output_span(cx: &mut TestAppContext) {
    for prefix in ["", "p", "pw"] {
        for prompt in ["dev@host:~$ ".to_owned(), format!("{}$ ", "x".repeat(76))] {
            let (view, window, _) = open(cx);
            view.update(window, |view, cx| {
                let _input = view.install_remote_files_test_session("fixture@example.invalid", cx);
                feed(view, format!("{prompt}{prefix}").as_bytes());
                view.suggest.line_buf = "pwd".into();
                view.commit_line(cx);
                assert!(view.pending_pwd.is_some(), "prompt={prompt:?}, echo={prefix:?}");
                feed(
                    view,
                    format!("{}\r\n/srv/data\r\n{prompt}", &"pwd"[prefix.len()..]).as_bytes(),
                );
                view.process_event(TermEvent::CommandStart, cx);
                view.process_event(TermEvent::Wakeup, cx);
                assert_eq!(view.remote_cwd().as_deref(), Some("/srv/data"));
            });
        }
    }
}

#[gpui::test]
fn pwd_text_in_foreground_stdin_is_not_a_directory_query(cx: &mut TestAppContext) {
    for text in ["pwd", ">>> pwd", "echo /tmp/pwd"] {
        let (view, window, _) = open(cx);
        view.update(window, |view, cx| {
            let _input = view.install_remote_files_test_session("fixture@example.invalid", cx);
            view.mark_command_running();
            view.running_program = Some("cat".into());
            feed(view, text.as_bytes());
            view.suggest.line_buf = "pwd".into();
            view.commit_line(cx);
            assert!(view.pending_pwd.is_none(), "stdin {text:?}");
            feed(view, b"\r\n/unrelated/path\r\n");
            view.process_event(TermEvent::Wakeup, cx);
            assert!(view.remote_cwd().is_none());
        });
    }
}

#[gpui::test]
fn pwd_from_history_and_uninstrumented_prompts_survives_preexec_titles(cx: &mut TestAppContext) {
    for prompt in ["❯ ", "(base) [dev@host data]$ ", "dev@host:/srv/data$ "] {
        let (view, window, _) = open(cx);
        view.update(window, |view, cx| {
            let _input = view.install_remote_files_test_session("fixture@example.invalid", cx);
            for path in ["/srv/data", "/srv/other", "/srv/other"] {
                feed(view, format!("\r\n{prompt}pwd").as_bytes());
                // Up/Down history navigation invalidates the input mirror.
                crate::display::nebula_clear_line(&mut view.suggest);
                view.commit_line(cx);
                assert!(view.pending_pwd.is_some(), "capture recalled pwd at {prompt:?}");
                // A shell's preexec title can precede the output Wakeup.
                view.process_event(TermEvent::Title("NEBULA|/old||pwd".into()), cx);
                feed(view, format!("\r\n{path}\r\n{prompt}").as_bytes());
                // Protocol events are drained after the shared grid has already
                // advanced through the whole output batch.
                view.process_event(TermEvent::CommandStart, cx);
                view.process_event(TermEvent::Wakeup, cx);
                assert_eq!(view.remote_cwd().as_deref(), Some(path));
            }
        });
    }
}

#[gpui::test]
fn repeated_pwd_after_a_silent_cd_does_not_wait_for_the_activity_watchdog(cx: &mut TestAppContext) {
    for (initial, changed) in [
        ("dev@host:~$ ", "dev@host:/srv/data$ "),
        ("[dev@host ~]$ ", "[dev@host data]$ "),
        ("❯ ", "❯ "),
    ] {
        let (view, window, _) = open(cx);
        view.update(window, |view, cx| {
            let _input = view.install_remote_files_test_session("fixture@example.invalid", cx);
            for (command, prompt, output) in [
                ("pwd", initial, "/home/dev"),
                ("cd /srv/data", initial, ""),
                ("pwd", changed, "/srv/data"),
                ("pwd", changed, "/srv/data"),
            ] {
                feed(view, format!("\r\n{prompt}").as_bytes());
                if prompt == "❯ " {
                    let mut term = view.session.as_ref().unwrap().term.lock();
                    term.nebula_add_prompt_mark();
                    term.nebula_mark_prompt_input();
                }
                feed(view, command.as_bytes());
                view.suggest.line_buf = command.into();
                view.commit_line(cx);
                assert_eq!(view.suggest.last_committed, command);
                if command == "pwd" {
                    assert!(view.pending_pwd.is_some(), "each pwd needs a fresh capture");
                    feed(view, format!("\r\n{output}\r\n").as_bytes());
                    view.process_event(TermEvent::Wakeup, cx);
                    assert_eq!(view.remote_cwd().as_deref(), Some(output));
                }
            }
        });
    }
}

#[gpui::test]
fn a_known_agent_prompt_does_not_rearm_pwd_even_with_an_input_boundary(cx: &mut TestAppContext) {
    let (view, window, _) = open(cx);
    view.update(window, |view, cx| {
        let _input = view.install_remote_files_test_session("fixture@example.invalid", cx);
        view.running_program = Some("codex".into());
        view.suggest.last_committed = "codex".into();
        view.suggest.pending_command_prompt = Some("❯".into());
        view.mark_command_running();
        feed(view, "❯ ".as_bytes());
        {
            let mut term = view.session.as_ref().unwrap().term.lock();
            term.nebula_add_prompt_mark();
            term.nebula_mark_prompt_input();
        }
        feed(view, b"pwd");
        view.suggest.line_buf = "pwd".into();
        view.commit_line(cx);
        assert!(view.pending_pwd.is_none());
        assert_eq!(view.suggest.last_committed, "codex");
        assert_eq!(view.running_program.as_deref(), Some("codex"));
    });
}

#[gpui::test]
fn submitted_pwd_updates_only_its_remote_pane_after_output(cx: &mut TestAppContext) {
    let (view, window, _) = open(cx);
    view.update(window, |view, cx| {
        let _input = view.install_remote_files_test_session("fixture@example.invalid", cx);
        feed(view, b"user@host:~$ pwd");
        view.suggest.line_buf = "pwd".into();
        view.commit_line(cx);
        assert!(view.pending_pwd.is_some());
        feed(view, b"\r\n/srv/Team Projects");
        view.process_event(TermEvent::Wakeup, cx);
        assert_eq!(view.remote_cwd(), None);
        feed(view, b"/data\r\nuser@host:~$ ");
        view.process_event(TermEvent::Wakeup, cx);
        assert_eq!(view.remote_cwd().as_deref(), Some("/srv/Team Projects/data"));
        assert!(view.pending_pwd.is_none());
        feed(view, b"echo /tmp\r\n/tmp\r\n$ ");
        view.process_event(TermEvent::Wakeup, cx);
        assert_eq!(view.remote_cwd().as_deref(), Some("/srv/Team Projects/data"));
    });
}

#[gpui::test]
fn runtime_submission_codex_prompt_does_not_depend_on_a_repaint(cx: &mut TestAppContext) {
    let (view, window, receiver) = open(cx);
    view.update(window, |view, cx| {
        view.running_program = Some("codex".into());
        view.runtime_prompt("继续".into(), true, cx).unwrap();
        assert!(matches!(receiver.try_recv().unwrap(), Msg::Input(bytes)
            if bytes.as_ref() == "继续\x1b[C\r".as_bytes()));
        assert!(view.pending_runtime_submit.is_none());
        assert_eq!(view.runtime_task_state(), crate::runtime_api::RuntimeTaskState::Running);
        view.process_event(TermEvent::Wakeup, cx);
        assert!(receiver.try_recv().is_err(), "a later repaint must not send Enter twice");
    });
}

#[gpui::test]
fn runtime_submission_codex_paste_and_no_submit_preserve_their_contracts(cx: &mut TestAppContext) {
    let (view, window, receiver) = open(cx);
    view.update(window, |view, cx| {
        view.running_program = Some("codex".into());
        view.runtime_prompt("draft".into(), false, cx).unwrap();
        assert!(
            matches!(receiver.try_recv().unwrap(), Msg::Input(bytes) if bytes.as_ref() == b"draft")
        );
        feed(view, b"\x1b[?2004h");
        view.runtime_paste("first\nsecond".into(), true, InputOrigin::Program, cx).unwrap();
        assert!(matches!(receiver.try_recv().unwrap(), Msg::Input(bytes)
            if bytes.as_ref() == b"\x1b[200~first\rsecond\x1b[201~\x1b[C\r"));
        assert!(view.pending_runtime_submit.is_none());
    });
}

#[gpui::test]
fn working_directory_copy_preserves_prompt_selection_and_utf8(cx: &mut TestAppContext) {
    let (view, window, _) = open(cx);
    let prompt = "    ~    15:30:27  ";
    view.update(window, |view, cx| {
        view.process_event(TermEvent::CwdReport("/home/用户/目录 with spaces".into()), cx);
        feed(view, prompt.as_bytes());
        let mut term = view.session.as_ref().unwrap().term.lock();
        term.selection = Some(Selection::new(
            SelectionType::Lines,
            TermPoint::new(Line(0), Column(0)),
            Side::Left,
        ));
    });
    window.update(|window, cx| {
        view.update(cx, |view, cx| {
            let selected = view.session.as_ref().unwrap().term.lock().selection_to_string();
            assert!(selected.as_deref().unwrap().contains(prompt));
            assert!(view.copy_working_directory(window, cx));
            assert_eq!(
                cx.read_from_clipboard().and_then(|item| item.text()),
                Some("/home/用户/目录 with spaces".into())
            );
            assert_eq!(view.session.as_ref().unwrap().term.lock().selection_to_string(), selected);
            assert!(view.copy_selection(false, window, cx));
            assert_eq!(cx.read_from_clipboard().and_then(|item| item.text()), selected);
            for unavailable in ["", "~", "relative/path", "/bad\npath"] {
                view.cwd = unavailable.into();
                assert!(!view.copy_working_directory(window, cx));
                assert_eq!(cx.read_from_clipboard().and_then(|item| item.text()), selected);
            }
        });
    });
}

#[gpui::test]
fn hidden_output_keeps_the_latest_grid_without_notifying_observers(cx: &mut TestAppContext) {
    let (view, window, _) = open(cx);
    window.run_until_parked();
    let notifications = Rc::new(Cell::new(0));
    let _subscription = window.update(|_, cx| {
        let notifications = notifications.clone();
        cx.observe(&view, move |_, _| notifications.set(notifications.get() + 1))
    });
    view.update(window, |view, cx| {
        // A visible but unfocused pane must still show output.
        view.cursor_window_active = false;
        view.cursor_pane_focused = false;
        view.process_event(TermEvent::Wakeup, cx);
    });
    assert_eq!(notifications.get(), 1);
    view.update(window, |view, cx| view.set_output_visible(false, cx));
    notifications.set(0);
    for index in 0..256 {
        view.update(window, |view, cx| {
            feed(view, format!("\r\x1b[2Kbackground-output-{index}").as_bytes());
            view.process_event(TermEvent::Wakeup, cx);
        });
    }
    assert_eq!(notifications.get(), 0, "hidden output must not invalidate chrome readers");
    view.update(window, |view, cx| {
        let snapshot = nebula_terminal::render::RenderSnapshot::capture(
            &view.session.as_ref().unwrap().term.lock(),
            &nebula_terminal::render::SnapshotConfig { rows: 24, cols: 80 },
        );
        let text: String = snapshot
            .segments
            .iter()
            .flat_map(|segment| &segment.cells)
            .map(|cell| cell.text.as_str())
            .collect();
        assert!(text.contains("background-output-255"));
        view.set_output_visible(true, cx);
    });
    assert_eq!(notifications.get(), 1, "reveal invalidates even after output has stopped");
    view.update(window, |view, cx| view.process_event(TermEvent::Wakeup, cx));
    assert_eq!(notifications.get(), 2, "visible output resumes invalidation");
}

#[gpui::test]
fn declaring_the_same_output_visibility_does_not_start_a_redraw_loop(cx: &mut TestAppContext) {
    let (view, window, _) = open(cx);
    window.run_until_parked();
    let notifications = Rc::new(Cell::new(0));
    let _subscription = window.update(|_, cx| {
        let notifications = notifications.clone();
        cx.observe(&view, move |_, _| notifications.set(notifications.get() + 1))
    });
    for visible in [true, true, false, false, true, true] {
        view.update(window, |view, cx| view.set_output_visible(visible, cx));
    }
    assert_eq!(notifications.get(), 1, "only hidden-to-visible needs invalidation");
}

#[gpui::test]
fn hidden_terminals_keep_title_progress_bell_and_exit_events(cx: &mut TestAppContext) {
    let (view, window, _) = open(cx);
    let received = Rc::new(RefCell::new(Vec::new()));
    let _subscription = window.update(|_, cx| {
        let received = received.clone();
        cx.subscribe(&view, move |_, event, _| {
            let label = match event {
                TerminalViewEvent::TitleChanged => "title",
                TerminalViewEvent::ProgressChanged(_) => "progress",
                TerminalViewEvent::Bell => "bell",
                TerminalViewEvent::Exited => "exit",
                _ => return,
            };
            received.borrow_mut().push(label);
        })
    });
    view.update(window, |view, cx| {
        view.set_output_visible(false, cx);
        view.process_event(TermEvent::Title("background task".into()), cx);
        view.process_event(TermEvent::Progress { state: 1, value: Some(25) }, cx);
        view.process_event(TermEvent::Bell, cx);
        view.process_event(TermEvent::Exit, cx);
    });
    assert_eq!(*received.borrow(), ["title", "progress", "bell", "exit"]);
}

#[gpui::test]
fn hidden_wakeup_still_flushes_shell_commands_and_pending_enter(cx: &mut TestAppContext) {
    let (view, window, receiver) = open(cx);
    view.update(window, |view, cx| {
        view.set_output_visible(false, cx);
        view.run_command("echo background-check".into(), cx);
        assert!(receiver.try_recv().is_err(), "wait for the real prompt");
        feed(view, b"\x1b]133;A\x07user@host:/tmp$ ");
        view.process_event(TermEvent::Wakeup, cx);
        assert!(view.pending_shell_command.is_none());
        assert!(matches!(receiver.try_recv().unwrap(), Msg::Input(bytes)
            if bytes.as_ref() == b"echo background-check"));
        assert!(receiver.try_recv().is_err(), "Enter waits for echo");
        feed(view, b"echo background-check");
        view.process_event(TermEvent::Wakeup, cx);
        assert!(matches!(receiver.try_recv().unwrap(), Msg::Input(bytes)
            if bytes.as_ref() == b"\r"));
        assert!(view.pending_runtime_submit.is_none());
    });
}

#[gpui::test]
fn ssh_exit_and_failure_cancel_pending_forward_and_hide_remote_actions(cx: &mut TestAppContext) {
    let (view, window, _) = open(cx);
    for failed in [false, true] {
        view.update(window, |view, cx| {
            view.exited = None;
            view.ssh_destination = Some("fixture@localhost".into());
            view.apply_ssh_stage(crate::ssh_session::SshStage::Ready, cx);
            assert_eq!(view.ready_ssh_destination(), Some("fixture@localhost"));
            view.port_forward_task =
                Some(cx.spawn(async |_, _| std::future::pending::<()>().await));
            if failed {
                view.apply_ssh_stage(
                    crate::ssh_session::SshStage::Failed("disconnected".into()),
                    cx,
                );
            } else {
                view.process_event(TermEvent::Exit, cx);
            }
            assert!(view.port_forward_task.is_none());
            assert!(view.ready_ssh_destination().is_none());
        });
    }
}

/// Issue #806: a remote `rz` left the pane in ZMODEM raw mode with no way out.
#[gpui::test]
fn remote_zmodem_offer_is_cancelled_on_the_pty_and_reported_once(cx: &mut TestAppContext) {
    let (view, window, _) = open(cx);
    let reported = Rc::new(Cell::new(0));
    let _subscription = window.update(|_, cx| {
        let reported = reported.clone();
        cx.subscribe(&view, move |_, event, _| {
            if matches!(event, TerminalViewEvent::ZmodemCancelled) {
                reported.set(reported.get() + 1);
            }
        })
    });
    view.update(window, |view, cx| {
        let (session, writes, mut events, proxy) =
            crate::gpui_shell::terminal::session::test_session_with_events();
        view.session = Some(session);
        let mut stream = nebula_terminal::event_loop::StreamProcessor::default();
        let wire = b"rz waiting to receive.**\x18B0100000023be50\r\x8a\x11";
        for chunk in wire.chunks(7) {
            stream.feed(&mut view.session.as_ref().unwrap().term.lock(), &proxy, chunk);
        }
        while let Ok(event) = events.try_recv() {
            view.process_event(event, cx);
        }
        let written: Vec<u8> = writes
            .try_iter()
            .flat_map(|msg| match msg {
                Msg::Input(bytes) => bytes.into_owned(),
                _ => Vec::new(),
            })
            .collect();
        assert_eq!(written, nebula_terminal::zmodem::ABORT_SEQUENCE.as_bytes());
    });
    assert_eq!(reported.get(), 1);
}
