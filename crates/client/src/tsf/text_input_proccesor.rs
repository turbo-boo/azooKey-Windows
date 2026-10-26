use std::collections::HashMap;

use crate::{engine::state::IMEState, globals::GUID_DISPLAY_ATTRIBUTE};

use super::{factory::TextServiceFactory_Impl, message_window::MessageWindow};
use windows::{
    core::{s, w, Interface as _, HRESULT},
    Win32::{
        Foundation::BOOL,
        System::LibraryLoader::{GetModuleHandleW, GetProcAddress},
        UI::TextServices::{
            ITfCategoryMgr, ITfKeyEventSink, ITfKeystrokeMgr, ITfLangBarItemButton,
            ITfLangBarItemMgr, ITfSource, ITfTextInputProcessorEx_Impl, ITfTextInputProcessor_Impl,
            ITfThreadMgr, ITfThreadMgrEventSink,
        },
    },
};

use anyhow::{Context, Result};

/// The category manager, obtained without COM. The text service registers
/// GUID_TFCAT_TIPCAP_COMLESS, so TSF may activate it in a thread that never
/// initialized COM, where CoCreateInstance fails. msctf.dll, which is
/// calling Activate, exports TF_CreateCategoryMgr for this case; the import
/// libraries do not, so it is looked up at run time.
fn category_mgr() -> Result<ITfCategoryMgr> {
    type CreateCategoryMgr = unsafe extern "system" fn(*mut *mut std::ffi::c_void) -> HRESULT;
    unsafe {
        let msctf = GetModuleHandleW(w!("msctf.dll"))?;
        let create = GetProcAddress(msctf, s!("TF_CreateCategoryMgr"))
            .context("msctf.dll does not export TF_CreateCategoryMgr")?;
        let create: CreateCategoryMgr = std::mem::transmute(create);
        let mut category_mgr = std::ptr::null_mut();
        create(&mut category_mgr).ok()?;
        // the out pointer carries a reference, which from_raw takes over
        Ok(ITfCategoryMgr::from_raw(category_mgr))
    }
}

impl ITfTextInputProcessor_Impl for TextServiceFactory_Impl {
    #[macros::anyhow]
    #[tracing::instrument(skip(self))]
    fn Activate(&self, ptim: Option<&ITfThreadMgr>, tid: u32) -> Result<()> {
        tracing::debug!("Activated with tid: {tid}");

        // Connect eagerly when possible. If the server is not up yet (e.g. right
        // after logon), keep activating: IMEState::ipc_service() reconnects on
        // the next key, and keys pass through to the app until then.
        if let Err(error) = IMEState::ipc_service() {
            tracing::warn!("IPC service is not available yet: {error:?}");
        }

        let mut text_service = self.borrow_mut()?;

        text_service.tid = tid;
        text_service.activation += 1;
        // Sinks registered below need this object's interfaces. TSF holds a
        // reference from Activate to Deactivate, so the self-reference kept
        // until Deactivate does not outlive it.
        text_service.this = Some(self.interface()?);
        // without it only F5 (AI conversion) fails; typing is unaffected
        text_service.message_window = MessageWindow::create(self)
            .inspect_err(|error| tracing::warn!("Cannot create the message window: {error:?}"))
            .ok();
        let thread_mgr = ptim.context("Thread manager is null")?;
        text_service.thread_mgr = Some(thread_mgr.clone());

        // initialize key event sink
        tracing::debug!("AdviseKeyEventSink");

        unsafe {
            thread_mgr.cast::<ITfKeystrokeMgr>()?.AdviseKeyEventSink(
                tid,
                &text_service.this::<ITfKeyEventSink>()?,
                BOOL::from(true),
            )?;
        };

        // initialize thread manager event sink
        tracing::debug!("AdviseThreadMgrEventSink");
        unsafe {
            let cookie = thread_mgr.cast::<ITfSource>()?.AdviseSink(
                &ITfThreadMgrEventSink::IID,
                &text_service.this::<ITfThreadMgrEventSink>()?,
            )?;
            text_service.thread_mgr_sink_cookie = Some(cookie);
        };

        // initialize text layout sink
        tracing::debug!("AdviseTextLayoutSink");
        let doc_mgr = unsafe { thread_mgr.GetFocus() };
        if let Ok(doc_mgr) = doc_mgr {
            text_service.advise_text_layout_sink(doc_mgr)?;
        }

        // initialize display attribute
        tracing::debug!("Initialize display attribute");
        let atom_map = unsafe {
            let mut map = HashMap::new();
            let atom = category_mgr()?.RegisterGUID(&GUID_DISPLAY_ATTRIBUTE)?;
            map.insert(GUID_DISPLAY_ATTRIBUTE, atom);
            map
        };

        text_service.display_attribute_atom = atom_map;

        // initialize langbar
        tracing::debug!("Initialize langbar");
        unsafe {
            thread_mgr
                .cast::<ITfLangBarItemMgr>()?
                .AddItem(&text_service.this::<ITfLangBarItemButton>()?)?;
        };
        drop(text_service);
        if let Ok(mode) = IMEState::get().map(|state| state.input_mode.clone()) {
            self.update_open_close(&mode);
        }

        tracing::debug!("Activate success");

        Ok(())
    }

    #[macros::anyhow]
    #[tracing::instrument(skip(self))]
    fn Deactivate(&self) -> Result<()> {
        tracing::debug!("Deactivated");

        // Every registration must be removed even if an earlier step fails,
        // e.g. when the server has already exited.
        let mut failed = false;
        let mut check = |step: &str, result: Result<()>| {
            if let Err(error) = result {
                tracing::error!("Deactivate: {step} failed: {error:?}");
                failed = true;
            }
        };

        check("end composition", self.end_composition());

        // persist learning memory when the text service is deactivated
        check(
            "flush learning",
            (|| {
                let ipc_service = IMEState::get()?.ipc_service.clone();
                if let Some(mut ipc_service) = ipc_service {
                    ipc_service.flush_learning()?;
                }
                Ok(())
            })(),
        );

        let mut text_service = self.borrow_mut()?;
        if let Some(thread_mgr) = text_service.thread_mgr.clone() {
            tracing::debug!("UnadviseKeyEventSink");
            check(
                "unadvise key event sink",
                (|| unsafe {
                    thread_mgr
                        .cast::<ITfKeystrokeMgr>()?
                        .UnadviseKeyEventSink(text_service.tid)?;
                    Ok(())
                })(),
            );

            tracing::debug!("Remove langbar");
            check(
                "remove langbar item",
                (|| unsafe {
                    thread_mgr
                        .cast::<ITfLangBarItemMgr>()?
                        .RemoveItem(&text_service.this::<ITfLangBarItemButton>()?)?;
                    Ok(())
                })(),
            );

            tracing::debug!("UnadviseThreadMgrEventSink");
            if let Some(cookie) = text_service.thread_mgr_sink_cookie.take() {
                check(
                    "unadvise thread manager event sink",
                    (|| unsafe {
                        thread_mgr.cast::<ITfSource>()?.UnadviseSink(cookie)?;
                        Ok(())
                    })(),
                );
            }
        }

        tracing::debug!("UnadviseTextLayoutSink");
        check(
            "unadvise text layout sink",
            text_service.unadvise_text_layout_sink(),
        );

        text_service.display_attribute_atom.clear();
        text_service.tid = 0;
        text_service.activation += 1;
        text_service.thread_mgr = None;
        text_service.thread_mgr_sink_cookie = None;
        // break the self-reference so TSF's release can free this instance
        text_service.this = None;
        // destroys the window; results still in flight are dropped
        text_service.message_window = None;
        drop(text_service);

        if failed {
            anyhow::bail!("Deactivate completed with errors");
        }
        tracing::debug!("Deactivate success");

        Ok(())
    }
}

impl ITfTextInputProcessorEx_Impl for TextServiceFactory_Impl {
    #[macros::anyhow]
    fn ActivateEx(&self, ptim: Option<&ITfThreadMgr>, tid: u32, _dwflags: u32) -> Result<()> {
        // called when the text service is activated
        // if this function is implemented, the Activate() function won't be called
        // so we need to call the Activate function manually
        tracing::debug!("Activated(Ex) with tid: {tid}");
        self.Activate(ptim, tid)?;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use std::{
        sync::{mpsc, Mutex},
        time::Duration,
    };

    use windows::Win32::{
        System::LibraryLoader::GetModuleHandleW, UI::TextServices::ITfTextInputProcessor,
    };

    use crate::{
        globals::{DllModule, DLL_INSTANCE},
        tsf::factory::TextServiceFactory,
    };

    #[test]
    fn activate_returns_instead_of_waiting_on_its_own_lock() {
        let (done, finished) = mpsc::channel();
        // COM objects stay on the thread that created them
        std::thread::spawn(move || {
            let mut dll = DllModule::new();
            dll.hinst = Some(unsafe { GetModuleHandleW(None) }.unwrap());
            let _ = DLL_INSTANCE.set(Mutex::new(dll));

            let tip: ITfTextInputProcessor = TextServiceFactory::create().unwrap();
            // Without a thread manager Activate fails, but only after it has
            // taken every lock it uses (DLL module, message window).
            let result = unsafe { tip.Activate(None, 1) };
            done.send(result.is_err()).unwrap();
        });

        let returned = finished.recv_timeout(Duration::from_secs(30));
        assert_eq!(returned, Ok(true), "Activate did not return");
    }
}
