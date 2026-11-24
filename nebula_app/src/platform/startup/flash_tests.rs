//! Opt-in full GUI startup: verify its descendants do not show console windows.
use std::collections::{HashMap, HashSet};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetWindowThreadProcessId, IsWindowVisible,
};

unsafe extern "system" fn console_window(window: HWND, data: LPARAM) -> BOOL {
    // SAFETY: EnumWindows calls synchronously with our live output collection.
    unsafe {
        if IsWindowVisible(window) != 0 {
            let mut class = [0u16; 64];
            let len = GetClassNameW(window, class.as_mut_ptr(), class.len() as i32);
            if len > 0 && String::from_utf16_lossy(&class[..len as usize]) == "ConsoleWindowClass" {
                let mut pid = 0;
                GetWindowThreadProcessId(window, &mut pid);
                (&mut *(data as *mut HashSet<u32>)).insert(pid);
            }
        }
    }
    1
}

#[test]
#[ignore = "opens an isolated GUI; PEBREL_TEST_GUI_EXE must have bundled VcXsrv beside it"]
fn native_startup_has_no_visible_console_windows() {
    let executable = std::env::var_os("PEBREL_TEST_GUI_EXE").expect("explicit GUI executable");
    let root = tempfile::tempdir().unwrap();
    let config = root.path().join("config");
    let temporary = root.path().join("temporary");
    let codex = root.path().join("codex");
    for path in [&config, &temporary, &codex] {
        std::fs::create_dir(path).unwrap();
    }
    std::fs::write(config.join("pebrel_settings.txt"), "tray=0\nrestore_session=0\n").unwrap();
    let mut command = Command::new(executable);
    let output = std::fs::File::create(root.path().join("startup.log")).unwrap();
    command
        .env("PEBREL_CONFIG_DIR", &config)
        .env("CODEX_HOME", &codex)
        .env("TEMP", &temporary)
        .env("TMP", &temporary)
        .stdin(Stdio::null())
        .stdout(output.try_clone().unwrap())
        .stderr(output);
    crate::platform::process::configure_process_group(&mut command);
    let mut child = command.spawn().unwrap();
    let group = crate::platform::process::ProcessGroup::attach(&child).unwrap();
    let root_pid = child.id();
    let mut records = HashMap::new();
    let mut visible_consoles = HashSet::new();
    let deadline = Instant::now() + Duration::from_secs(12);
    while Instant::now() < deadline {
        for process in crate::platform::process_snapshot::snapshot().unwrap() {
            records.insert(process.pid, (process.parent, process.executable));
        }
        // SAFETY: callback and collection remain valid for this synchronous call.
        unsafe {
            EnumWindows(
                Some(console_window),
                (&mut visible_consoles as *mut HashSet<u32>) as LPARAM,
            )
        };
        std::thread::sleep(Duration::from_millis(10));
    }
    let final_processes = crate::platform::process_snapshot::snapshot().unwrap();
    let exited = child.try_wait().unwrap();
    group.terminate(&mut child);
    let _ = child.wait();
    let mut owned = HashSet::from([root_pid]);
    for _ in 0..16 {
        for (&pid, (parent, _)) in &records {
            if owned.contains(parent) {
                owned.insert(pid);
            }
        }
    }
    let flashes: Vec<_> = visible_consoles.intersection(&owned).collect();
    let vcxsrv_running = final_processes
        .iter()
        .any(|p| owned.contains(&p.pid) && p.executable.eq_ignore_ascii_case("vcxsrv.exe"));
    if !vcxsrv_running || !flashes.is_empty() {
        let descendants: Vec<_> = records.iter().filter(|(pid, _)| owned.contains(pid)).collect();
        eprintln!("GUI {root_pid}, exit={exited:?}, descendants={descendants:?}");
        eprintln!("Startup diagnostics retained at {}", root.keep().display());
    }
    assert!(flashes.is_empty(), "visible descendant console windows: {flashes:?}");
    assert!(vcxsrv_running, "VcXsrv must remain running");
    assert!(
        !final_processes
            .iter()
            .any(|p| p.parent == root_pid && p.executable.eq_ignore_ascii_case("yserver.exe"))
    );
}
