//! Restore and activate the already-running Antigravity desktop window.
use std::path::PathBuf;
use windows_sys::Win32::{
    Foundation::{CloseHandle, HWND, LPARAM},
    System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
    },
    UI::WindowsAndMessaging::{
        EnumWindows, GetForegroundWindow, GetWindow, GetWindowThreadProcessId, IsIconic,
        IsWindowVisible, SetForegroundWindow, ShowWindow, GW_OWNER, SW_RESTORE,
    },
};

pub(crate) const ACTIVATES_APPS: bool = true;
const ANTIGRAVITY: &str = "com.google.antigravity";

struct Windows {
    executable: PathBuf,
    handles: Vec<HWND>,
}

unsafe extern "system" fn collect(window: HWND, data: LPARAM) -> i32 {
    if IsWindowVisible(window) == 0 || !GetWindow(window, GW_OWNER).is_null() {
        return 1;
    }
    let state = &mut *(data as *mut Windows);
    let mut pid = 0;
    GetWindowThreadProcessId(window, &mut pid);
    let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
    if process.is_null() {
        return 1;
    }
    let mut path = vec![0; 32768];
    let mut length = path.len() as u32;
    let found = QueryFullProcessImageNameW(process, 0, path.as_mut_ptr(), &mut length) != 0;
    CloseHandle(process);
    if found
        && String::from_utf16_lossy(&path[..length as usize])
            .eq_ignore_ascii_case(&state.executable.to_string_lossy())
    {
        state.handles.push(window);
    }
    1
}

fn antigravity_windows() -> Vec<HWND> {
    let Some(local) = std::env::var_os("LOCALAPPDATA") else {
        return Vec::new();
    };
    let mut state = Windows {
        executable: PathBuf::from(local)
            .join("Programs")
            .join("antigravity")
            .join("Antigravity.exe"),
        handles: Vec::new(),
    };
    unsafe { EnumWindows(Some(collect), &mut state as *mut Windows as LPARAM) };
    state.handles
}

/// Other clients retain the existing unknown/running behavior.
pub(crate) fn app_running(bundle_id: &str) -> bool {
    bundle_id != ANTIGRAVITY || !antigravity_windows().is_empty()
}

pub(crate) fn activate_app(bundle_id: &str) -> bool {
    if bundle_id != ANTIGRAVITY {
        return false;
    }
    let windows = antigravity_windows();
    let [window] = windows.as_slice() else {
        return false;
    };
    unsafe {
        if IsIconic(*window) != 0 {
            ShowWindow(*window, SW_RESTORE);
        }
        SetForegroundWindow(*window) != 0 || GetForegroundWindow() == *window
    }
}
