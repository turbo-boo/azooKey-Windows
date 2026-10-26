use std::{
    io,
    os::windows::io::{AsRawHandle as _, FromRawHandle as _, IntoRawHandle as _, OwnedHandle},
};

use tokio::net::windows::named_pipe::NamedPipeClient;
use windows::{
    core::{HSTRING, PWSTR},
    Win32::{
        Foundation::{LocalFree, HANDLE, HLOCAL},
        Security::{
            Authorization::{ConvertSidToStringSidW, GetSecurityInfo, SE_KERNEL_OBJECT},
            EqualSid, GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation,
            IsWellKnownSid, TokenIntegrityLevel, TokenUser, WinBuiltinAdministratorsSid,
            WinLocalSystemSid, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
            TOKEN_MANDATORY_LABEL, TOKEN_QUERY, TOKEN_USER,
        },
        Storage::FileSystem::{
            CreateFileW, FILE_FLAG_OVERLAPPED, FILE_READ_ATTRIBUTES, FILE_READ_DATA,
            FILE_SHARE_NONE, FILE_WRITE_ATTRIBUTES, FILE_WRITE_DATA, OPEN_EXISTING, READ_CONTROL,
            SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT, SYNCHRONIZE,
        },
        System::{
            Pipes::{GetNamedPipeServerProcessId, GetNamedPipeServerSessionId},
            RemoteDesktop::ProcessIdToSessionId,
            Threading::{
                GetCurrentProcess, GetCurrentProcessId, OpenProcess, OpenProcessToken,
                PROCESS_QUERY_LIMITED_INFORMATION,
            },
        },
    },
};

/// Base name of the converter server's pipe.
pub const SERVER_PIPE: &str = "azookey_server";
/// Base name of the candidate window's pipe.
pub const UI_PIPE: &str = "azookey_ui";

/// Access mask granted to IPC clients by the servers' pipe DACL.
///
/// GENERIC_WRITE on a pipe includes FILE_CREATE_PIPE_INSTANCE, which would let
/// any client register a rogue server instance under the same name, so clients
/// get read/write data access (and READ_CONTROL to check the owner) only.
pub const CLIENT_ACCESS_SDDL: &str = "0x12019B";

/// `\\.\pipe\<base>-<user SID>-<session id>`. Every logon session of every
/// user runs its own servers, so the path names only those of the caller.
/// Sandboxed processes (AppContainer) run as the same user and session and
/// therefore compute the same path.
pub fn path(base: &str) -> io::Result<String> {
    let user = ProcessUser::current()?;
    let mut session = 0;
    unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut session)? };
    Ok(format!(r"\\.\pipe\{base}-{}-{session}", user.sid_string()?))
}

/// SID string of the user running this process, for the servers' DACL.
pub fn current_user_sid() -> io::Result<String> {
    ProcessUser::current()?.sid_string()
}

/// Mandatory integrity level (SECURITY_MANDATORY_MEDIUM_RID) of the user's
/// ordinary programs. Sandboxes (AppContainer, low integrity, restricted
/// tokens) run below it.
pub const MEDIUM_INTEGRITY: u32 = 0x2000;

/// Mandatory integrity level of `token` (opened with TOKEN_QUERY): the last
/// subauthority of its integrity SID, comparable with [`MEDIUM_INTEGRITY`].
pub fn token_integrity(token: HANDLE) -> io::Result<u32> {
    unsafe {
        let mut length = 0;
        // fails with ERROR_INSUFFICIENT_BUFFER and reports the size
        let _ = GetTokenInformation(token, TokenIntegrityLevel, None, 0, &mut length);
        // u64 keeps the SID pointer inside TOKEN_MANDATORY_LABEL aligned
        let mut buffer = vec![0u64; (length as usize).div_ceil(size_of::<u64>())];
        GetTokenInformation(
            token,
            TokenIntegrityLevel,
            Some(buffer.as_mut_ptr().cast()),
            (buffer.len() * size_of::<u64>()) as u32,
            &mut length,
        )?;
        let sid = (*buffer.as_ptr().cast::<TOKEN_MANDATORY_LABEL>()).Label.Sid;
        match *GetSidSubAuthorityCount(sid) {
            0 => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "the integrity SID has no subauthority",
            )),
            count => Ok(*GetSidSubAuthority(sid, u32::from(count) - 1)),
        }
    }
}

/// Opens the client end of the pipe at `path` requesting only the rights in
/// [`CLIENT_ACCESS_SDDL`]. tokio's `ClientOptions` always requests
/// GENERIC_WRITE, which that DACL intentionally denies.
///
/// The pipe name is predictable, so another user could create it first. The
/// connection is refused unless the pipe is owned by this user, SYSTEM or
/// Administrators (the owner of a server started elevated), and unless the
/// server passes [`check_server`].
pub fn open_client(path: &str) -> io::Result<NamedPipeClient> {
    let access = FILE_READ_DATA
        | FILE_WRITE_DATA
        | FILE_READ_ATTRIBUTES
        | FILE_WRITE_ATTRIBUTES
        | READ_CONTROL
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
    // closes the handle if the owner check fails
    let handle = unsafe { OwnedHandle::from_raw_handle(handle.0) };

    check_owner(HANDLE(handle.as_raw_handle()))?;
    check_server(HANDLE(handle.as_raw_handle()))?;

    unsafe { NamedPipeClient::from_raw_handle(handle.into_raw_handle()) }
}

fn check_owner(pipe: HANDLE) -> io::Result<()> {
    let user = ProcessUser::current()?;
    let mut owner = PSID::default();
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    unsafe {
        GetSecurityInfo(
            pipe,
            SE_KERNEL_OBJECT,
            OWNER_SECURITY_INFORMATION,
            Some(&mut owner),
            None,
            None,
            None,
            Some(&mut descriptor),
        )
        .ok()?;

        // `owner` points into `descriptor`
        let trusted = EqualSid(owner, user.sid()).is_ok()
            || IsWellKnownSid(owner, WinLocalSystemSid).as_bool()
            || IsWellKnownSid(owner, WinBuiltinAdministratorsSid).as_bool();
        LocalFree(HLOCAL(descriptor.0));

        if trusted {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "the pipe belongs to another user",
            ))
        }
    }
}

/// Any process of this user, a sandboxed one at low integrity included, can
/// create the pipe while the server is not running, and the owner check
/// cannot tell it from the server. It would receive what every application
/// types, elevated ones included, and answer with its own candidates. So the
/// server must run in this session at medium integrity or above.
///
/// Sandboxed clients (AppContainer, restricted tokens) are usually not
/// allowed to open the server's process and cannot tell the server from a
/// sandboxed impostor at all. Refusing them would break input in every
/// sandboxed application, so for them a server that cannot be inspected is
/// accepted; one that can be inspected must still be at medium integrity.
fn check_server(pipe: HANDLE) -> io::Result<()> {
    let refuse = |reason: &str| Err(io::Error::new(io::ErrorKind::PermissionDenied, reason));

    let mut server_session = 0;
    let mut session = 0;
    unsafe {
        GetNamedPipeServerSessionId(pipe, &mut server_session)?;
        ProcessIdToSessionId(GetCurrentProcessId(), &mut session)?;
    }
    if server_session != session {
        return refuse("the pipe is served from another session");
    }

    match server_integrity(pipe) {
        Ok(integrity) if integrity >= MEDIUM_INTEGRITY => Ok(()),
        Ok(_) => refuse("the pipe is served by a sandboxed process"),
        Err(_) if current_integrity()? < MEDIUM_INTEGRITY => Ok(()),
        Err(error) => Err(error),
    }
}

fn server_integrity(pipe: HANDLE) -> io::Result<u32> {
    unsafe {
        let mut process_id = 0;
        GetNamedPipeServerProcessId(pipe, &mut process_id)?;
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id)?;
        let process = OwnedHandle::from_raw_handle(process.0);
        let mut token = HANDLE::default();
        OpenProcessToken(HANDLE(process.as_raw_handle()), TOKEN_QUERY, &mut token)?;
        let token = OwnedHandle::from_raw_handle(token.0);
        token_integrity(HANDLE(token.as_raw_handle()))
    }
}

fn current_integrity() -> io::Result<u32> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)?;
        let token = OwnedHandle::from_raw_handle(token.0);
        token_integrity(HANDLE(token.as_raw_handle()))
    }
}

/// The TOKEN_USER of the current process.
struct ProcessUser {
    // u64 keeps the pointers inside TOKEN_USER aligned
    buffer: Vec<u64>,
}

impl ProcessUser {
    fn current() -> io::Result<Self> {
        unsafe {
            let mut token = HANDLE::default();
            OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)?;
            let owned_token = OwnedHandle::from_raw_handle(token.0);
            let token = HANDLE(owned_token.as_raw_handle());

            let mut length = 0;
            // fails with ERROR_INSUFFICIENT_BUFFER and reports the size
            let _ = GetTokenInformation(token, TokenUser, None, 0, &mut length);
            let mut buffer = vec![0u64; (length as usize).div_ceil(size_of::<u64>())];
            GetTokenInformation(
                token,
                TokenUser,
                Some(buffer.as_mut_ptr().cast()),
                (buffer.len() * size_of::<u64>()) as u32,
                &mut length,
            )?;
            Ok(Self { buffer })
        }
    }

    fn sid(&self) -> PSID {
        unsafe { (*self.buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid }
    }

    fn sid_string(&self) -> io::Result<String> {
        unsafe {
            let mut string = PWSTR::null();
            ConvertSidToStringSidW(self.sid(), &mut string)?;
            let sid = string.to_string();
            LocalFree(HLOCAL(string.0.cast()));
            sid.map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
        }
    }
}
