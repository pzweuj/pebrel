//! VcXsrv uses CRT system() for xkbcomp. Its GUI parent cannot pass
//! CREATE_NO_WINDOW to that call, so scope COMSPEC to our early GUI bridge.

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt as _;
use std::os::windows::process::CommandExt as _;
use std::process::{Command, Stdio};

const HELPER: &str = "PEBREL_INTERNAL_X11_SHELL";

pub(super) fn configure(command: &mut Command) -> Result<(), String> {
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    #[cfg(test)]
    let executable = std::env::var_os("PEBREL_X11_HELPER_EXE")
        .map(std::path::PathBuf::from)
        .unwrap_or(executable);
    command.env("COMSPEC", executable).env(HELPER, "1");
    Ok(())
}

// Preserve the original /c tail, including redirection and quoted paths. Parsing
// it through args_os and quoting it again would change cmd.exe's grammar.
fn command_tail(line: &[u16]) -> Option<&[u16]> {
    let space = |c| c == u16::from(b' ') || c == u16::from(b'\t');
    let start = line.iter().position(|&c| !space(c))?;
    let mut quoted = false;
    let end = line[start..].iter().position(|&c| {
        if c == u16::from(b'"') {
            quoted = !quoted;
        }
        !quoted && space(c)
    })? + start;
    let tail = &line[end..];
    let tail = &tail[tail.iter().position(|&c| !space(c))?..];
    (tail.len() > 3 && tail[0] == u16::from(b'/') && matches!(tail[1], 67 | 99) && space(tail[2]))
        .then_some(tail)
}

pub(super) fn run_if_requested() -> Option<i32> {
    if std::env::var_os(HELPER).as_deref() != Some(std::ffi::OsStr::new("1")) {
        return None;
    }
    // SAFETY: GetCommandLineW returns the process-owned, NUL-terminated buffer.
    let line = unsafe {
        let start = windows_sys::Win32::System::Environment::GetCommandLineW();
        let mut len = 0;
        while len < 32768 && *start.add(len) != 0 {
            len += 1;
        }
        std::slice::from_raw_parts(start, len)
    };
    let Some(tail) = command_tail(line) else { return Some(2) };
    let Some(root) = std::env::var_os("SystemRoot") else { return Some(2) };
    let shell = std::path::PathBuf::from(root).join("System32").join("cmd.exe");
    let mut command = Command::new(&shell);
    command.args(["/d"]).raw_arg(OsString::from_wide(tail));
    command.env_remove(HELPER).env("COMSPEC", &shell);
    command.stdin(Stdio::inherit()).stdout(Stdio::inherit()).stderr(Stdio::inherit());
    Some(match crate::platform::process::hidden_command(&mut command).status() {
        Ok(status) => status.code().unwrap_or(1),
        Err(error) => {
            eprintln!("X11 keyboard command failed: {error}");
            1
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_tail_preserves_quotes_redirection_and_unicode() {
        let tail = r#"/c ""D:\X Server\xkbcomp.exe" "键盘 文件" > "out file" 2>&1""#;
        let line: Vec<_> =
            format!("\"D:\\App Folder\\pebrel.exe\" {tail}").encode_utf16().collect();
        assert_eq!(OsString::from_wide(command_tail(&line).unwrap()), OsString::from(tail));
        for invalid in ["pebrel.exe", "pebrel.exe /k echo no", "pebrel.exe --gpui", "pebrel.exe /c"]
        {
            assert!(command_tail(&invalid.encode_utf16().collect::<Vec<_>>()).is_none());
        }
    }

    #[test]
    #[ignore = "requires a built GUI executable in PEBREL_X11_HELPER_EXE"]
    fn real_gui_shell_bridge_preserves_output_redirection_and_exit_code() {
        let executable = std::env::var_os("PEBREL_X11_HELPER_EXE").expect("GUI helper executable");
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("键盘 output.txt");
        let mut command = Command::new(executable);
        command.env(HELPER, "1").arg("/c").raw_arg(format!(
            "echo keyboard-data>\"{}\" & echo stderr-data 1>&2 & exit /b 7",
            destination.display()
        ));
        let output = crate::platform::process::hidden_command(&mut command).output().unwrap();
        assert_eq!(output.status.code(), Some(7));
        assert!(output.stdout.is_empty());
        assert_eq!(String::from_utf8(output.stderr).unwrap().trim(), "stderr-data");
        assert_eq!(std::fs::read_to_string(destination).unwrap().trim(), "keyboard-data");
    }
}
