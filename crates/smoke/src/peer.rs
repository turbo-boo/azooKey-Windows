//! The processes behind the pipes the smoke test talks to.

use std::{
    os::windows::io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle},
    path::PathBuf,
};

use anyhow::{Context, Result};
use windows::{
    core::{HSTRING, PCWSTR, PWSTR},
    Win32::{
        Foundation::{HANDLE, HWND},
        System::{
            Pipes::GetNamedPipeServerProcessId,
            Threading::{
                OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
                PROCESS_QUERY_LIMITED_INFORMATION,
            },
        },
        UI::WindowsAndMessaging::{FindWindowExW, GetWindowThreadProcessId, IsWindowVisible},
    },
};

/// The process serving `pipe`: its ID and executable.
pub fn pipe_server(pipe: &str) -> Result<(u32, PathBuf)> {
    let client = shared::pipe::open_client(pipe).with_context(|| format!("open {pipe}"))?;
    let mut process_id = 0;
    unsafe {
        GetNamedPipeServerProcessId(HANDLE(client.as_raw_handle()), &mut process_id)
            .context("GetNamedPipeServerProcessId")?;
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id)
            .with_context(|| format!("open process {process_id}"))?;
        let process = OwnedHandle::from_raw_handle(process.0);
        let mut name = vec![0u16; 32768];
        let mut length = name.len() as u32;
        QueryFullProcessImageNameW(
            HANDLE(process.as_raw_handle()),
            PROCESS_NAME_WIN32,
            PWSTR(name.as_mut_ptr()),
            &mut length,
        )
        .with_context(|| format!("image name of process {process_id}"))?;
        let image = String::from_utf16_lossy(&name[..length as usize]);
        Ok((process_id, PathBuf::from(image)))
    }
}

/// Whether the top-level window titled `title` of process `process_id` is
/// visible; `None` when the process has no such window.
pub fn window_visible(process_id: u32, title: &str) -> Option<bool> {
    let title = HSTRING::from(title);
    let mut after = HWND::default();
    unsafe {
        while let Ok(window) = FindWindowExW(HWND::default(), after, PCWSTR::null(), &title) {
            let mut owner = 0;
            GetWindowThreadProcessId(window, Some(&mut owner));
            if owner == process_id {
                return Some(IsWindowVisible(window).as_bool());
            }
            after = window;
        }
    }
    None
}
