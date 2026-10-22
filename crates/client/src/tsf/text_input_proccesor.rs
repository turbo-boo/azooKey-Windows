use std::collections::HashMap;

use crate::{
    engine::state::IMEState,
    globals::{DllModule, GUID_DISPLAY_ATTRIBUTE},
};

use super::{factory::TextServiceFactory_Impl, message_window::MessageWindow};
use windows::{
    core::Interface as _,
    Win32::{
        Foundation::BOOL,
        System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER},
        UI::TextServices::{
            CLSID_TF_CategoryMgr, ITfCategoryMgr, ITfKeyEventSink, ITfKeystrokeMgr,
            ITfLangBarItemButton, ITfLangBarItemMgr, ITfSource, ITfTextInputProcessorEx_Impl,
            ITfTextInputProcessor_Impl, ITfThreadMgr, ITfThreadMgrEventSink,
        },
    },
};

use anyhow::{Context, Result};

impl ITfTextInputProcessor_Impl for TextServiceFactory_Impl {
    #[macros::anyhow]
    #[tracing::instrument]
    fn Activate(&self, ptim: Option<&ITfThreadMgr>, tid: u32) -> Result<()> {
        tracing::debug!("Activated with tid: {tid}");

        // add reference to the dll instance to prevent it from being unloaded
        let mut dll_instance = DllModule::get()?;
        dll_instance.add_ref();

        // Connect eagerly when possible. If the server is not up yet (e.g. right
        // after logon), keep activating: IMEState::ipc_service() reconnects on
        // the next key, and keys pass through to the app until then.
        if let Err(error) = IMEState::ipc_service() {
            tracing::warn!("IPC service is not available yet: {error:?}");
        }

        let mut text_service = self.borrow_mut()?;

        text_service.tid = tid;
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
            let category_mgr: ITfCategoryMgr =
                CoCreateInstance(&CLSID_TF_CategoryMgr, None, CLSCTX_INPROC_SERVER)?;

            let atom = category_mgr.RegisterGUID(&GUID_DISPLAY_ATTRIBUTE)?;
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

        tracing::debug!("Activate success");

        Ok(())
    }

    #[macros::anyhow]
    #[tracing::instrument]
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
        text_service.thread_mgr = None;
        text_service.thread_mgr_sink_cookie = None;
        // break the self-reference so TSF's release can free this instance
        text_service.this = None;
        // destroys the window; results still in flight are dropped
        text_service.message_window = None;
        drop(text_service);

        // remove reference to the dll instance
        check(
            "release dll reference",
            DllModule::get().map(|mut dll_instance| {
                dll_instance.release();
            }),
        );

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