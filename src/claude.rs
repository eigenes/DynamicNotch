//! Claude Code hook bridge.
//!
//! Claude Code runs hook commands with a JSON payload on stdin. Registered for
//! the `Stop` and `Notification` events, `dynamic-notch claude` turns that into
//! a peek ("Claude finished" / "Claude needs you"):
//!
//!   dynamic-notch claude              read the hook payload from stdin
//!   dynamic-notch claude stop|notification [message]   without a payload
//!   --always                          peek even if Claude's window is focused
//!
//! The hook process never starts the notch (a hook has to return quickly),
//! skips the peek while you are looking at the window Claude runs in, and
//! passes that window along so clicking the peek brings it to the front.

use std::io::Read;

use serde_json::Value;
use windows::core::BOOL;
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM};
use windows::Win32::Storage::FileSystem::{GetFileType, FILE_TYPE_DISK, FILE_TYPE_PIPE};
use windows::Win32::System::Console::{
    AttachConsole, FreeConsole, GetConsoleWindow, GetStdHandle, ATTACH_PARENT_PROCESS, STD_INPUT_HANDLE,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetAncestor, GetForegroundWindow, GetWindow, GetWindowTextLengthW, GetWindowThreadProcessId,
    IsWindowVisible, GA_ROOTOWNER, GW_OWNER,
};

use crate::ipc;
use crate::util::from_wide;

pub fn run(args: &[String]) {
    let always = args.iter().any(|a| a == "--always");
    let pos: Vec<&str> = args.iter().filter(|a| !a.starts_with("--")).map(String::as_str).collect();
    let payload = read_payload().unwrap_or(Value::Null);
    let field = |k: &str| payload.get(k).and_then(Value::as_str).map(str::to_string);

    let event = pos.first().map(|s| s.to_string()).or_else(|| field("hook_event_name")).unwrap_or_else(|| "Stop".into());
    let message = pos.get(1).map(|s| s.to_string()).or_else(|| field("message")).unwrap_or_default();
    let cwd = field("cwd").or_else(|| std::env::current_dir().ok().map(|d| d.to_string_lossy().into_owned()));
    let project = cwd.as_deref().map(folder_name).unwrap_or_default();

    let target = unsafe { claude_window() };
    if !always && target.map_or(false, |t| unsafe { is_foreground(t) }) {
        return; // the user is already looking at Claude
    }
    let hwnd = target.map_or(0, |h| h.0 as isize);
    ipc::send_to_running(&["claude".into(), event, project, message, hwnd.to_string()]);
}

/// Hook payload on stdin. Only read when stdin is a pipe or file, so running
/// the command by hand in a terminal never blocks.
fn read_payload() -> Option<Value> {
    unsafe {
        let h = GetStdHandle(STD_INPUT_HANDLE).ok()?;
        if h.is_invalid() || h.0.is_null() {
            return None;
        }
        let t = GetFileType(h);
        if t != FILE_TYPE_PIPE && t != FILE_TYPE_DISK {
            return None;
        }
    }
    let mut text = String::new();
    std::io::stdin().take(1 << 20).read_to_string(&mut text).ok()?;
    serde_json::from_str(&text).ok()
}

fn folder_name(path: &str) -> String {
    let p = path.trim_end_matches(['\\', '/']);
    p.rsplit(['\\', '/']).next().unwrap_or(p).to_string()
}

unsafe fn is_foreground(target: HWND) -> bool {
    let fg = GetForegroundWindow();
    !fg.is_invalid() && (fg == target || GetAncestor(fg, GA_ROOTOWNER) == target)
}

/// The window Claude Code runs in: the console of the hook's parent (Windows
/// Terminal, conhost), else the nearest ancestor process with a visible main
/// window (the Claude desktop app, VS Code, ...).
unsafe fn claude_window() -> Option<HWND> {
    if AttachConsole(ATTACH_PARENT_PROCESS).is_ok() {
        let c = GetConsoleWindow();
        let _ = FreeConsole();
        if !c.is_invalid() {
            for h in [GetAncestor(c, GA_ROOTOWNER), c] {
                if !h.is_invalid() && IsWindowVisible(h).as_bool() {
                    return Some(h);
                }
            }
        }
    }
    ancestors().into_iter().find_map(main_window)
}

/// Parent, grandparent, ... of this process (shell hosts excluded).
fn ancestors() -> Vec<u32> {
    let mut procs: Vec<(u32, u32, String)> = Vec::new();
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return Vec::new() };
        let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        if Process32FirstW(snap, &mut e).is_ok() {
            loop {
                let name = from_wide(&e.szExeFile).to_ascii_lowercase();
                procs.push((e.th32ProcessID, e.th32ParentProcessID, name));
                if Process32NextW(snap, &mut e).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
    }
    let mut out = Vec::new();
    let mut pid = std::process::id();
    for _ in 0..16 {
        let Some(&(_, parent, _)) = procs.iter().find(|p| p.0 == pid) else { break };
        let Some((_, _, name)) = procs.iter().find(|p| p.0 == parent) else { break };
        if parent == 0 || out.contains(&parent) || name == "explorer.exe" || name == "svchost.exe" {
            break;
        }
        out.push(parent);
        pid = parent;
    }
    out
}

/// First visible, unowned, titled top-level window of `pid`.
fn main_window(pid: u32) -> Option<HWND> {
    unsafe extern "system" fn cb(h: HWND, lp: LPARAM) -> BOOL {
        let st = &mut *(lp.0 as *mut (u32, Option<HWND>));
        let mut owner_pid = 0u32;
        GetWindowThreadProcessId(h, Some(&mut owner_pid));
        let unowned = GetWindow(h, GW_OWNER).map_or(true, |o| o.is_invalid());
        if owner_pid == st.0 && IsWindowVisible(h).as_bool() && unowned && GetWindowTextLengthW(h) > 0 {
            st.1 = Some(h);
            return false.into();
        }
        true.into()
    }
    let mut st: (u32, Option<HWND>) = (pid, None);
    unsafe {
        let _ = EnumWindows(Some(cb), LPARAM(&mut st as *mut _ as isize));
    }
    st.1
}

#[cfg(test)]
mod tests {
    use super::folder_name;

    #[test]
    fn project_names() {
        assert_eq!(folder_name(r"C:\Users\me\Documents\Tools\DynamicNotch"), "DynamicNotch");
        assert_eq!(folder_name("/home/me/app/"), "app");
        assert_eq!(folder_name("app"), "app");
    }
}
