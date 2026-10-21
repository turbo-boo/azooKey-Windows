use std::io;

use tokio::net::windows::named_pipe::NamedPipeClient;
use windows::{
    core::HSTRING,
    Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAG_OVERLAPPED, FILE_READ_ATTRIBUTES, FILE_READ_DATA, FILE_SHARE_NONE,
        FILE_WRITE_ATTRIBUTES, FILE_WRITE_DATA, OPEN_EXISTING, SECURITY_IDENTIFICATION,
        SECURITY_SQOS_PRESENT, SYNCHRONIZE,
    },
};

/// Access mask granted to IPC clients by the servers' pipe DACL.
///
/// GENERIC_WRITE on a pipe includes FILE_CREATE_PIPE_INSTANCE, which would let
/// any client register a rogue server instance under the same name, so clients
/// get read/write data access only.
pub const CLIENT_ACCESS_SDDL: &str = "0x12019B";

/// Opens the client end of `\\.\pipe\<name>` requesting only the rights in
/// [`CLIENT_ACCESS_SDDL`]. tokio's `ClientOptions` always requests
/// GENERIC_WRITE, which that DACL intentionally denies.
pub fn open_client(path: &str) -> io::Result<NamedPipeClient> {
    let access = FILE_READ_DATA
        | FILE_WRITE_DATA
        | FILE_READ_ATTRIBUTES
        | FILE_WRITE_ATTRIBUTES
        | SYNCHRONIZE;

    let handle = unsafe {
        CreateFileW(
            &HSTRING::from(path),
            access.0,
            FILE_SHARE_NONE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
            None,
        )
    }
    .map_err(|error| {
        // Callers match on Win32 codes such as ERROR_PIPE_BUSY.
        let hresult = error.code().0 as u32;
        if hresult & 0xFFFF_0000 == 0x8007_0000 {
            io::Error::from_raw_os_error((hresult & 0xFFFF) as i32)
        } else {
            io::Error::from(error)
        }
    })?;

    unsafe { NamedPipeClient::from_raw_handle(handle.0) }
}
