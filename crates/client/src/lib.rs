mod engine;
mod extension;
mod globals;
mod macros;
mod register;
mod trace;
mod tracing_chrome;
mod tsf;

use std::{ffi::c_void, sync::Mutex};

use globals::{DllModule, DLL_INSTANCE, GUID_TEXT_SERVICE};
use register::{CLSIDMgr, CategoryMgr, ProfileMgr};
use tsf::factory::TextServiceFactory;
use windows::{
    core::{IUnknown, Interface as _, GUID, HRESULT},
    Win32::{
        Foundation::{
            BOOL, CLASS_E_CLASSNOTAVAILABLE, E_NOINTERFACE, E_POINTER, FALSE, HMODULE, S_FALSE,
            TRUE,
        },
        System::{Com::IClassFactory, Ole::SELFREG_E_CLASS, SystemServices::DLL_PROCESS_ATTACH},
    },
};
// -- Dll Export Functions --
// The IME DLL needs to implement the following four functions to operate as a COM server.

#[no_mangle]
pub extern "system" fn DllMain(
    hinst: HMODULE,
    fdw_reason: u32,
    _lpv_reserved: *mut c_void,
) -> BOOL {
    if fdw_reason == DLL_PROCESS_ATTACH {
        let result: anyhow::Result<()> = (|| {
            let mut dll_instance = DllModule::new();
            dll_instance.hinst = Some(hinst);
            DLL_INSTANCE
                .set(Mutex::new(dll_instance))
                .map_err(|e| anyhow::anyhow!(format!("{:?}", e)))?;
            Ok(())
        })();

        // Debug builds only. The logger opens a file and installs the global
        // subscriber, which must not happen under the loader lock, so a
        // thread does it once DllMain has returned. Failing to start that
        // thread only means there is no log; thread::spawn would panic here
        // instead, and a panic cannot leave DllMain.
        if cfg!(debug_assertions) {
            let _ = std::thread::Builder::new().spawn(|| {
                let _ = trace::setup_logger();
            });
        }

        tracing::debug!("DllMain");

        check_err!(result, TRUE, FALSE)
    } else {
        // Nothing to undo on DLL_PROCESS_DETACH. At process termination the
        // other threads are already gone and may have left any lock taken;
        // on FreeLibrary (only regsvr32 does that, see DllCanUnloadNow) the
        // process-wide state is simply discarded with the DLL.
        TRUE
    }
}

#[no_mangle]
/// # Safety
/// This function uses raw pointers
pub unsafe extern "system" fn DllGetClassObject(
    rclsid: *const GUID,
    riid: *const GUID,
    ppv: *mut *mut c_void,
) -> HRESULT {
    // Return a class factory to obtain the tsf TextService
    // This function will be called only once when applications request the TextService
    // So, You have to reopen the application to apply the changes in the TextService
    // https://zenn.dev/link/comments/d918e46723da80
    tracing::debug!("DllGetClassObject");

    if ppv.is_null() || rclsid.is_null() || riid.is_null() {
        return E_POINTER;
    }
    // the out pointer must be NULL on every failure
    *ppv = std::ptr::null_mut();

    if *rclsid != GUID_TEXT_SERVICE {
        return CLASS_E_CLASSNOTAVAILABLE;
    }

    // The class object is only handed out as IClassFactory or IUnknown,
    // even though the type also implements the text service interfaces.
    if *riid != IClassFactory::IID && *riid != IUnknown::IID {
        return E_NOINTERFACE;
    }
    let factory: IClassFactory = TextServiceFactory::default().into();
    factory.query(riid, ppv)
}

#[no_mangle]
pub extern "system" fn DllRegisterServer() -> HRESULT {
    // Register the CLSID of the TextService
    // Called when the DLL is registered using regsvr32
    tracing::debug!("DllRegisterServer");

    let result: anyhow::Result<()> = (|| {
        let dll_path = DllModule::get_path()?;

        ProfileMgr::register(&dll_path)?;
        CLSIDMgr::register(&dll_path)?;
        CategoryMgr::register()?;

        Ok(())
    })();

    // to show the error, SELFREG_E_CLASS is needed
    check_err!(result, SELFREG_E_CLASS)
}

#[no_mangle]
pub extern "system" fn DllUnregisterServer() -> HRESULT {
    // Unregister the CLSID of the TextService
    // Called when the DLL is unregistered using regsvr32
    tracing::debug!("DllUnregisterServer");

    let result: anyhow::Result<()> = (|| {
        ProfileMgr::unregister()?;
        CLSIDMgr::unregister()?;
        CategoryMgr::unregister()?;

        Ok(())
    })();

    check_err!(result, SELFREG_E_CLASS)
}

#[no_mangle]
pub extern "system" fn DllCanUnloadNow() -> HRESULT {
    // The DLL stays loaded until the process exits. Counting COM objects
    // would not make unloading safe: process-wide state outlives them,
    // notably the server connection whose candidate-window dispatcher
    // thread runs code from this DLL, the message window class and, in
    // debug builds, the global tracing subscriber.
    S_FALSE
}
