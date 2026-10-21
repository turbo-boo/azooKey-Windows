//! Checks that a pipe's DACL lets only SYSTEM, Administrators and the owner
//! create additional server instances. Anyone else holding
//! FILE_CREATE_PIPE_INSTANCE could register a rogue instance and receive IME
//! connections.

use anyhow::{bail, Context, Result};
use windows::{
    core::{HSTRING, PWSTR},
    Win32::{
        Foundation::{CloseHandle, LocalFree, HLOCAL},
        Security::{
            Authorization::{ConvertSidToStringSidW, GetSecurityInfo, SE_KERNEL_OBJECT},
            AclSizeInformation, GetAce, GetAclInformation, ACCESS_ALLOWED_ACE, ACE_HEADER, ACL,
            ACL_SIZE_INFORMATION, DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
        },
        Storage::FileSystem::{
            CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_NONE, OPEN_EXISTING, READ_CONTROL,
        },
        System::SystemServices::ACCESS_ALLOWED_ACE_TYPE,
    },
};

const FILE_CREATE_PIPE_INSTANCE: u32 = 0x4;
const GENERIC_ALL: u32 = 0x1000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
/// SYSTEM, BUILTIN\Administrators, OWNER RIGHTS
const TRUSTED: [&str; 3] = ["S-1-5-18", "S-1-5-32-544", "S-1-3-4"];

pub fn check_clients_cannot_create_instances(pipe: &str) -> Result<()> {
    let mut offenders = Vec::new();
    unsafe {
        let handle = CreateFileW(
            &HSTRING::from(pipe),
            READ_CONTROL.0,
            FILE_SHARE_NONE,
            None,
            OPEN_EXISTING,
            FILE_FLAGS_AND_ATTRIBUTES(0),
            None,
        )
        .context("open pipe for READ_CONTROL")?;

        let mut dacl: *mut ACL = std::ptr::null_mut();
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        let status = GetSecurityInfo(
            handle,
            SE_KERNEL_OBJECT,
            DACL_SECURITY_INFORMATION,
            None,
            None,
            Some(&mut dacl),
            None,
            Some(&mut descriptor),
        );
        let _ = CloseHandle(handle);
        status.ok().context("GetSecurityInfo")?;
        if dacl.is_null() {
            bail!("pipe has a NULL DACL (everyone has full access)");
        }

        let mut size = ACL_SIZE_INFORMATION::default();
        GetAclInformation(
            dacl,
            std::ptr::from_mut(&mut size).cast(),
            size_of::<ACL_SIZE_INFORMATION>() as u32,
            AclSizeInformation,
        )?;

        for index in 0..size.AceCount {
            let mut ace = std::ptr::null_mut();
            GetAce(dacl, index, &mut ace)?;
            if u32::from((*ace.cast::<ACE_HEADER>()).AceType) != ACCESS_ALLOWED_ACE_TYPE {
                continue;
            }
            let ace = &*ace.cast::<ACCESS_ALLOWED_ACE>();
            let sid = PSID(std::ptr::from_ref(&ace.SidStart).cast_mut().cast());
            let mut sid_string = PWSTR::null();
            ConvertSidToStringSidW(sid, &mut sid_string)?;
            let name = sid_string.to_string().unwrap_or_default();
            let _ = LocalFree(HLOCAL(sid_string.0.cast()));

            let risky = ace.Mask & (FILE_CREATE_PIPE_INSTANCE | GENERIC_ALL | GENERIC_WRITE);
            if risky != 0 && !TRUSTED.contains(&name.as_str()) {
                offenders.push(format!("{name} has access mask {:#x}", ace.Mask));
            }
        }

        let _ = LocalFree(HLOCAL(descriptor.0));
    }

    if offenders.is_empty() {
        Ok(())
    } else {
        bail!(
            "untrusted principals can create pipe instances: {}",
            offenders.join(", ")
        )
    }
}
