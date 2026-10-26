//! The processes behind the pipes the smoke test talks to.

use std::{
    os::windows::io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle},
    path::PathBuf,
};

use anyhow::{Context, Result};
use windows::{
    core::PWSTR,
    Win32::{
        Foundation::HANDLE,
        System::{
            Pipes::GetNamedPipeServerProcessId,
            Threading::{
                OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
                PROCESS_QUERY_LIMITED_INFORMATION,
            },
        },
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
