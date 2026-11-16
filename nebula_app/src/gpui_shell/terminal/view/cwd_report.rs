//! Apply directory metadata without changing the reported filesystem path.

use gpui::EventEmitter as _;
use nebula_terminal::event::Event;
use nebula_terminal::grid::Dimensions as _;
use nebula_terminal::term::reading::{ReadingPoint, ReadingSpan};

/// Only an explicitly submitted pwd can turn screen output into directory metadata.
pub(super) struct PwdCapture {
    epoch: u64,
    line: usize,
    started: std::time::Instant,
}

impl PwdCapture {
    pub(super) fn start<T>(command: &str, term: &nebula_terminal::Term<T>) -> Option<Self> {
        let words: Vec<_> = command.split_whitespace().collect();
        if !matches!(words.as_slice(), ["pwd"] | ["pwd", "-L" | "-P"])
            || term.mode().contains(nebula_terminal::term::TermMode::ALT_SCREEN)
        {
            return None;
        }
        Some(Self {
            epoch: term.reading_epoch(),
            line: term.grid().scrolled_out()
                + term.history_size()
                + term.grid().cursor.point.line.0 as usize
                + 1,
            started: std::time::Instant::now(),
        })
    }

    fn read<T>(&self, term: &nebula_terminal::Term<T>) -> Result<Option<String>, ()> {
        if self.started.elapsed() > std::time::Duration::from_secs(10)
            || term.mode().contains(nebula_terminal::term::TermMode::ALT_SCREEN)
            || term.reading_epoch() != self.epoch
        {
            return Err(());
        }
        let end = term.grid().scrolled_out()
            + term.history_size()
            + term.grid().cursor.point.line.0 as usize;
        if end <= self.line {
            return Ok(None); // Wait for a complete output line, including across Wakeups.
        }
        let span = ReadingSpan {
            epoch: self.epoch,
            start: ReadingPoint { line: self.line, column: 0 },
            end: ReadingPoint { line: end, column: 0 },
        };
        let chunk = term.reading_chunk(span, None, 4096, 16384).map_err(|_| ())?;
        let mut path = String::new();
        for row in chunk.rows {
            path.push_str(&row.text);
            if row.hard_break {
                let path = path.trim_end_matches(' ');
                return if path.starts_with('/') && !path.chars().any(char::is_control) {
                    Ok(Some(path.to_owned()))
                } else {
                    Err(())
                };
            }
        }
        if chunk.next.is_some() { Err(()) } else { Ok(None) }
    }
}

impl super::TerminalView {
    pub(super) fn capture_submitted_pwd(&mut self) {
        self.pending_pwd = None;
        if self.ssh_destination.is_none()
            || self.effective_program_status().is_some()
            || self.agent_activity.hook_seen()
            || self
                .running_program
                .as_deref()
                .and_then(crate::ai_agents::AgentKind::parse)
                .is_some()
        {
            return;
        }
        let Some(session) = &self.session else { return };
        let term = session.term.lock();
        if term.mode().intersects(
            nebula_terminal::term::TermMode::ALT_SCREEN | nebula_terminal::term::TermMode::VI,
        ) {
            return;
        }
        // These are the only supported commands. A verified screen suffix also
        // covers Up/Down history recall without trusting a reconstructed key log.
        for command in ["pwd", "pwd -L", "pwd -P"] {
            let snapshot = crate::display::nebula_prompt_line_from_raw_grid(
                &term,
                term.grid().cursor.point,
                command,
                &self.suggest.suggest_env,
            );
            if snapshot.is_some_and(|line| line.input.trim() == command) {
                self.pending_pwd = PwdCapture::start(command, &term);
                return;
            }
        }
        // SSH echo may still be in flight when Enter is pressed. Only accept
        // a complete supported input mirror whose echoed prefix is at a proven
        // shell prompt; arbitrary foreground input remains ineligible.
        let command = self.suggest.line_buf.as_str();
        if !matches!(command, "pwd" | "pwd -L" | "pwd -P") {
            return;
        }
        for echoed in 0..command.len() {
            let at_prompt = if echoed == 0 {
                crate::display::nebula_shell_ready_from_raw_grid(&term, &self.suggest.suggest_env)
            } else {
                crate::display::nebula_prompt_line_from_raw_grid(
                    &term,
                    term.grid().cursor.point,
                    &command[..echoed],
                    &self.suggest.suggest_env,
                )
                .is_some_and(|line| line.input == command[..echoed])
            };
            if at_prompt {
                self.pending_pwd = PwdCapture::start(command, &term).map(|mut capture| {
                    let remaining = command.len() - echoed;
                    let column = term.grid().cursor.point.column.0
                        + usize::from(term.grid().cursor.input_needs_wrap);
                    capture.line += (column + remaining - 1) / term.columns();
                    capture
                });
                return;
            }
        }
    }

    pub(super) fn observe_pwd_output(&mut self, cx: &mut gpui::Context<Self>) {
        let Some(capture) = &self.pending_pwd else { return };
        let result =
            self.session.as_ref().ok_or(()).and_then(|session| capture.read(&session.term.lock()));
        match result {
            Ok(Some(path)) => {
                self.pending_pwd = None;
                self.process_event(Event::CwdReport(path), cx);
                cx.emit(super::TerminalViewEvent::RemoteDirectoryQueried);
            },
            Err(()) => self.pending_pwd = None,
            Ok(None) => {},
        }
    }
}

pub(super) fn apply(current: &mut String, event: &Event) -> bool {
    let reported = match event {
        Event::CwdReport(path) => Some(path.as_str()),
        Event::Title(title) => {
            title.strip_prefix("NEBULA|").and_then(|rest| rest.split('|').next())
        },
        _ => None,
    };
    let Some(path) = reported else { return false };
    if path.chars().any(char::is_control)
        || !(path.starts_with('/') || std::path::Path::new(path).is_absolute())
        || current.as_str() == path
    {
        return false;
    }
    current.clear();
    current.push_str(path);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn terminal() -> nebula_terminal::Term<nebula_terminal::event::VoidListener> {
        struct Size;
        impl nebula_terminal::grid::Dimensions for Size {
            fn total_lines(&self) -> usize {
                8
            }
            fn screen_lines(&self) -> usize {
                8
            }
            fn columns(&self) -> usize {
                20
            }
        }
        nebula_terminal::Term::new(Default::default(), &Size, nebula_terminal::event::VoidListener)
    }

    fn feed(term: &mut nebula_terminal::Term<nebula_terminal::event::VoidListener>, text: &str) {
        let mut parser = nebula_terminal::vte::ansi::Processor::<
            nebula_terminal::vte::ansi::StdSyncHandler,
        >::default();
        parser.advance(term, text.as_bytes());
    }

    #[test]
    fn pwd_waits_for_a_complete_path_and_joins_wrapped_output() {
        let mut term = terminal();
        feed(&mut term, "user@host:~$ pwd");
        let capture = PwdCapture::start("pwd", &term).unwrap();
        feed(&mut term, "\r\n/home/用户");
        assert_eq!(capture.read(&term), Ok(None));
        feed(&mut term, "/directory with spaces/project\r\n$ ");
        assert_eq!(
            capture.read(&term),
            Ok(Some("/home/用户/directory with spaces/project".into()))
        );
    }

    #[test]
    fn pwd_does_not_consume_old_paths_other_commands_or_errors() {
        let mut term = terminal();
        feed(&mut term, "/old/path\r\n$ pwd");
        for command in ["cat path", "echo /tmp", "pwd; echo /other", "pwd > file", "sudo pwd"] {
            assert!(PwdCapture::start(command, &term).is_none());
        }
        let capture = PwdCapture::start("pwd -P", &term).unwrap();
        assert_eq!(capture.read(&term), Ok(None));
        feed(&mut term, "\r\npwd: permission denied\r\n$ ");
        assert_eq!(capture.read(&term), Err(()));
    }

    #[test]
    fn cwd_events_preserve_absolute_paths_verbatim() {
        for path in ["/", "/srv/Team's \"App\" ", "/home/guest/new project"] {
            let mut current = "/original".to_owned();
            let report = Event::CwdReport(path.to_owned());
            assert!(apply(&mut current, &report));
            assert_eq!(current, path);
            assert!(!apply(&mut current, &report));
        }
    }

    #[test]
    fn shell_title_events_preserve_the_directory_field() {
        let mut current = "/original".to_owned();
        let path = "/srv/Team's \"App\" ";
        assert!(apply(&mut current, &Event::Title(format!("NEBULA|{path}| main | hx "))));
        assert_eq!(current, path);
        assert!(!apply(&mut current, &Event::Title("editor title".to_owned())));
        assert_eq!(current, path);
    }

    #[test]
    fn invalid_directory_events_keep_the_last_known_directory() {
        for path in ["", "relative", " /srv/app", "/srv/app\r", "\n/srv/app", "/srv/\x1bapp"] {
            for event in
                [Event::CwdReport(path.to_owned()), Event::Title(format!("NEBULA|{path}||"))]
            {
                let mut current = "/last known".to_owned();
                assert!(!apply(&mut current, &event), "{path:?}");
                assert_eq!(current, "/last known");
            }
        }
    }

    #[cfg(windows)]
    #[test]
    fn local_windows_directory_events_keep_drive_and_unc_paths() {
        for path in [r"C:\Users\Test User", r"\\server\share\project"] {
            let mut current = String::new();
            assert!(apply(&mut current, &Event::CwdReport(path.to_owned())));
            assert_eq!(current, path);
        }
    }
}
