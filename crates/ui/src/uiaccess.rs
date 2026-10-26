// obtain useraccess privileges from system applications
// cf: https://github.com/killtimer0/uiaccess/

use std::{ffi::c_void, ptr::addr_of_mut};

use anyhow::Result;
use windows::{
    core::PWSTR,
    Win32::{
        Foundation::{
            CloseHandle, SetHandleInformation, BOOL, HANDLE, HANDLE_FLAG_INHERIT,
            INVALID_HANDLE_VALUE,
        },
        Security::{
            DuplicateTokenEx, GetTokenInformation, LookupPrivilegeValueW, PrivilegeCheck,
            RevertToSelf, SecurityAnonymous, SecurityImpersonation, SetTokenInformation,
            TokenImpersonation, TokenPrimary, TokenSessionId, TokenUIAccess, PRIVILEGE_SET,
            SE_TCB_NAME, TOKEN_ACCESS_MASK, TOKEN_ADJUST_DEFAULT, TOKEN_ASSIGN_PRIMARY,
            TOKEN_DUPLICATE, TOKEN_IMPERSONATE, TOKEN_QUERY,
        },
        System::{
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
                TH32CS_SNAPPROCESS,
            },
            Environment::GetCommandLineW,
            JobObjects::{
                AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
                SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
                JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            },
            SystemServices::PRIVILEGE_SET_ALL_NECESSARY,
            Threading::{
                CreateProcessAsUserW, GetCurrentProcess, GetExitCodeProcess, GetPriorityClass,
                GetStartupInfoW, OpenProcess, OpenProcessToken, ResumeThread, SetThreadToken,
                TerminateProcess, WaitForSingleObject, CREATE_SUSPENDED, INFINITE,
                PROCESS_CREATION_FLAGS, PROCESS_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION,
                STARTF_USESTDHANDLES, STARTUPINFOW,
            },
        },
    },
};

/// get token from current process
fn open_current_process_token() -> Result<HANDLE> {
    let mut h_token = HANDLE::default();
    unsafe {
        if let Ok(()) = OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_DUPLICATE | TOKEN_QUERY,
            &mut h_token,
        ) {
            Ok(h_token)
        } else {
            anyhow::bail!("OpenProcessToken failed");
        }
    }
}

/// check for ui access
pub fn check_for_ui_access() -> Result<bool> {
    let mut token_ui_access: BOOL = false.into();
    let mut token_len: u32 = 0;

    unsafe {
        let h_token = open_current_process_token()?;
        let success = GetTokenInformation(
            h_token,
            TokenUIAccess,
            Some(&mut token_ui_access as *mut _ as *mut _),
            std::mem::size_of::<BOOL>() as u32,
            &mut token_len,
        );
        let _ = CloseHandle(h_token);
        if let Ok(()) = success {
            Ok(token_ui_access.as_bool())
        } else {
            anyhow::bail!("GetTokenInformation failed {success:?}");
        }
    }
}

pub fn duplicate_winlogon_token(
    session_id: u32,
    desired_access: TOKEN_ACCESS_MASK,
    h_token: &mut HANDLE,
) -> Result<()> {
    let mut privilege_set = PRIVILEGE_SET {
        PrivilegeCount: 1,
        Control: PRIVILEGE_SET_ALL_NECESSARY,
        ..Default::default()
    };

    unsafe {
        LookupPrivilegeValueW(None, SE_TCB_NAME, &mut privilege_set.Privilege[0].Luid)?;

        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)?;
        anyhow::ensure!(
            snapshot != INVALID_HANDLE_VALUE,
            "CreateToolhelp32Snapshot failed"
        );

        let mut process_entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };

        let mut found = false;
        let mut next = Process32FirstW(snapshot, &mut process_entry);
        while next.is_ok() {
            // The wide-character entry holds any process name; the ANSI one
            // cannot represent names outside the code page.
            let name = &process_entry.szExeFile;
            let name = &name[..name.iter().position(|&c| c == 0).unwrap_or(name.len())];
            if String::from_utf16_lossy(name).eq_ignore_ascii_case("winlogon.exe") {
                found = duplicate_session_token(
                    process_entry.th32ProcessID,
                    session_id,
                    &mut privilege_set,
                    desired_access,
                    h_token,
                );
                if found {
                    break;
                }
            }
            next = Process32NextW(snapshot, &mut process_entry);
        }
        let _ = CloseHandle(snapshot);

        anyhow::ensure!(found, "winlogon token for session {session_id} not found");
    }

    Ok(())
}

/// Duplicates the token of the winlogon process `process_id` when it runs in
/// `session_id` and holds SeTcbPrivilege. With RDP or fast user switching
/// there is one winlogon per session; only the one in our session is usable.
unsafe fn duplicate_session_token(
    process_id: u32,
    session_id: u32,
    privilege_set: &mut PRIVILEGE_SET,
    desired_access: TOKEN_ACCESS_MASK,
    h_token: &mut HANDLE,
) -> bool {
    let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id) else {
        return false;
    };

    let mut token = HANDLE::default();
    let opened = OpenProcessToken(process, TOKEN_QUERY | TOKEN_DUPLICATE, &mut token);
    let _ = CloseHandle(process);
    if opened.is_err() {
        return false;
    }

    let duplicated = (|| -> Result<bool> {
        let mut privilege_result = false.into();
        PrivilegeCheck(token, privilege_set, &mut privilege_result)?;
        if !privilege_result.as_bool() {
            return Ok(false);
        }

        let mut token_session_id: u32 = 0;
        let mut token_info_length: u32 = 0;
        GetTokenInformation(
            token,
            TokenSessionId,
            Some(addr_of_mut!(token_session_id) as *mut c_void),
            std::mem::size_of::<u32>() as u32,
            &mut token_info_length,
        )?;
        if token_session_id != session_id {
            return Ok(false);
        }

        DuplicateTokenEx(
            token,
            desired_access,
            None,
            SecurityImpersonation,
            TokenImpersonation,
            h_token,
        )?;
        Ok(true)
    })();
    let _ = CloseHandle(token);

    duplicated.unwrap_or_else(|error| {
        eprintln!("Cannot use the token of winlogon ({process_id}): {error}");
        false
    })
}

/// Returns a primary token of this process with UIAccess enabled. Setting
/// TokenUIAccess needs SeTcbPrivilege, which is borrowed from winlogon by
/// impersonating it; the impersonation ends before this returns.
pub fn create_uiaccess_token() -> Result<HANDLE> {
    unsafe {
        let mut token_self = HANDLE::default();
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_QUERY | TOKEN_DUPLICATE,
            &mut token_self,
        )?;

        let token = (|| -> Result<HANDLE> {
            let mut session_id = 0;
            let mut token_info_length = 0;
            GetTokenInformation(
                token_self,
                TokenSessionId,
                Some(addr_of_mut!(session_id) as *mut c_void),
                std::mem::size_of::<u32>() as u32,
                &mut token_info_length,
            )?;

            let mut system_token = HANDLE::default();
            duplicate_winlogon_token(session_id, TOKEN_IMPERSONATE, &mut system_token)?;
            let impersonated = SetThreadToken(None, system_token);
            let _ = CloseHandle(system_token);
            impersonated?;

            let token = (|| -> Result<HANDLE> {
                let mut token = HANDLE::default();
                DuplicateTokenEx(
                    token_self,
                    TOKEN_QUERY | TOKEN_DUPLICATE | TOKEN_ASSIGN_PRIMARY | TOKEN_ADJUST_DEFAULT,
                    None,
                    SecurityAnonymous,
                    TokenPrimary,
                    &mut token,
                )?;
                let ui_access: BOOL = true.into();
                if let Err(error) = SetTokenInformation(
                    token,
                    TokenUIAccess,
                    &ui_access as *const _ as *const c_void,
                    std::mem::size_of::<BOOL>() as u32,
                ) {
                    let _ = CloseHandle(token);
                    return Err(error.into());
                }
                Ok(token)
            })();

            // This thread goes on to run the UI when the relaunch fails; it
            // must never keep winlogon's identity.
            RevertToSelf().expect("failed to stop impersonating winlogon");
            token
        })();
        let _ = CloseHandle(token_self);
        token
    }
}

/// Set in the environment of the relaunched copy, so that a copy that
/// somehow did not get UIAccess runs the UI instead of relaunching again.
const RELAUNCHED_VARIABLE: &str = "AZOOKEY_UI_UIACCESS_RELAUNCH";

/// Runs ui.exe with UIAccess, so the candidate window can be shown above
/// elevated and immersive windows.
///
/// Returns `Ok(None)` when this process has UIAccess and should run the UI.
/// Otherwise this process starts a copy of itself with UIAccess, waits for it
/// and returns `Ok(Some(exit_code))` of the copy, which this process should
/// exit with: the launcher only knows this process, so it must live as long
/// as the real UI and report how it ended. The copy is in a job that is
/// closed with this process, so it does not outlive it either.
///
/// An error means the relaunch is not possible (a standard user cannot
/// borrow winlogon's privilege) and this process should run the UI without
/// UIAccess.
pub fn run_with_uiaccess() -> Result<Option<u32>> {
    if check_for_ui_access()? {
        println!("UIAccess is already enabled");
        return Ok(None);
    }
    anyhow::ensure!(
        std::env::var_os(RELAUNCHED_VARIABLE).is_none(),
        "the relaunched process did not get UIAccess"
    );

    let token_handle = create_uiaccess_token()?;
    let job = kill_on_close_job()
        .inspect_err(|error| eprintln!("The UIAccess process may outlive this one: {error}"))
        .ok();

    let mut startup_info = STARTUPINFOW::default();
    let mut process_info = PROCESS_INFORMATION::default();
    // CreateProcessAsUserW may write to the command line buffer.
    let mut command_line = unsafe { GetCommandLineW().as_wide() }.to_vec();
    command_line.push(0);
    std::env::set_var(RELAUNCHED_VARIABLE, "1");

    unsafe {
        GetStartupInfoW(&mut startup_info);
        // The launcher reads the UI's output through pipes passed as the
        // standard handles. Handing them on with STARTF_USESTDHANDLES needs
        // inheritable handles and inheritance turned on; otherwise the
        // copy's output is lost, or goes to whatever object later gets the
        // same handle value. The only other inheritable handles in this
        // process at this point are the ones the launcher passed along.
        let inherit_handles = startup_info.dwFlags.contains(STARTF_USESTDHANDLES);
        if inherit_handles {
            for handle in [
                startup_info.hStdInput,
                startup_info.hStdOutput,
                startup_info.hStdError,
            ] {
                if !handle.is_invalid() {
                    let _ =
                        SetHandleInformation(handle, HANDLE_FLAG_INHERIT.0, HANDLE_FLAG_INHERIT);
                }
            }
        }
        // The launcher raised this process's priority and exempted it from
        // EcoQoS for the real UI; neither carries over to a child by itself.
        let priority_class = PROCESS_CREATION_FLAGS(GetPriorityClass(GetCurrentProcess()));
        let created = CreateProcessAsUserW(
            token_handle,
            None,
            PWSTR(command_line.as_mut_ptr()),
            None,
            None,
            inherit_handles,
            CREATE_SUSPENDED | priority_class,
            None,
            None,
            &startup_info,
            &mut process_info,
        );
        let _ = CloseHandle(token_handle);
        std::env::remove_var(RELAUNCHED_VARIABLE);
        created?;

        if let Err(error) = shared::opt_out_of_power_throttling(process_info.hProcess) {
            eprintln!("Failed to disable power throttling: {error}");
        }
        if let Some(job) = job {
            if let Err(error) = AssignProcessToJobObject(job, process_info.hProcess) {
                eprintln!("The UIAccess process may outlive this one: {error}");
            }
        }
        if ResumeThread(process_info.hThread) == u32::MAX {
            let error = windows::core::Error::from_win32();
            let _ = TerminateProcess(process_info.hProcess, 1);
            let _ = CloseHandle(process_info.hThread);
            let _ = CloseHandle(process_info.hProcess);
            return Err(error.into());
        }
        let _ = CloseHandle(process_info.hThread);
        println!("Process created with UIAccess token");

        WaitForSingleObject(process_info.hProcess, INFINITE);
        let mut exit_code = 1;
        if let Err(error) = GetExitCodeProcess(process_info.hProcess, &mut exit_code) {
            eprintln!("Cannot read the exit code of the UIAccess process: {error}");
        }
        let _ = CloseHandle(process_info.hProcess);
        // The job handle stays open until this process exits.
        Ok(Some(exit_code))
    }
}

/// A job that terminates its processes when its last handle is closed.
fn kill_on_close_job() -> windows::core::Result<HANDLE> {
    unsafe {
        let job = CreateJobObjectW(None, None)?;
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if let Err(error) = SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &limits as *const _ as *const c_void,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        ) {
            let _ = CloseHandle(job);
            return Err(error);
        }
        Ok(job)
    }
}
