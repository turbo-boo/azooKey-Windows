use std::{
    cell::{Ref, RefCell, RefMut},
    ffi::c_void,
};

use windows::{
    core::{implement, IUnknown, Interface, GUID},
    Win32::{
        Foundation::{BOOL, CLASS_E_NOAGGREGATION, E_POINTER},
        System::Com::{IClassFactory, IClassFactory_Impl},
        UI::TextServices::{
            ITfCompositionSink, ITfDisplayAttributeProvider, ITfKeyEventSink, ITfLangBarItem,
            ITfLangBarItemButton, ITfLangBarItemSink, ITfSource, ITfTextInputProcessor,
            ITfTextInputProcessorEx, ITfTextLayoutSink, ITfThreadMgrEventSink,
        },
    },
};

use anyhow::Result;

use super::text_service::TextService;

#[derive(Default)]
#[implement(
    IClassFactory,
    ITfTextInputProcessor,
    ITfTextInputProcessorEx,
    ITfKeyEventSink,
    ITfThreadMgrEventSink,
    ITfTextLayoutSink,
    ITfCompositionSink,
    ITfDisplayAttributeProvider,
    ITfLangBarItem,
    ITfLangBarItemButton,
    ITfSource
)]
#[derive(Debug)]
pub struct TextServiceFactory {
    text_service: RefCell<TextService>,
    // The language bar's sink for our item, advised during AddItem. Kept
    // apart from the text service so advising works while it is borrowed.
    lang_bar_sink: RefCell<Option<ITfLangBarItemSink>>,
}

impl IClassFactory_Impl for TextServiceFactory_Impl {
    #[macros::anyhow]
    fn CreateInstance(
        &self,
        punkouter: Option<&IUnknown>,
        riid: *const GUID,
        ppvobject: *mut *mut c_void,
    ) -> Result<()> {
        if ppvobject.is_null() {
            return Err(windows::core::Error::from(E_POINTER).into());
        }
        unsafe { *ppvobject = std::ptr::null_mut() };

        if punkouter.is_some() {
            return Err(windows::core::Error::from(CLASS_E_NOAGGREGATION).into());
        }

        // TSF creates the text input processor and, through the CLSID
        // registered as GUID_TFCAT_DISPLAYATTRIBUTEPROVIDER, a separate
        // instance asked for ITfDisplayAttributeProvider. Hand out whatever
        // the object implements; anything else is E_NOINTERFACE.
        let object: IUnknown = TextServiceFactory::create()?;
        unsafe { object.query(riid, ppvobject).ok()? };

        Ok(())
    }

    #[macros::anyhow]
    fn LockServer(&self, _flock: BOOL) -> Result<()> {
        // The DLL is never unloaded while the process runs (see
        // DllCanUnloadNow), so there is nothing to keep loaded.
        Ok(())
    }
}

impl TextServiceFactory {
    pub fn create<I: Interface>() -> Result<I> {
        let factory = Self {
            text_service: RefCell::new(TextService::default()),
            lang_bar_sink: RefCell::default(),
        };

        let this = ITfTextInputProcessor::from(factory);
        Ok(this.cast::<I>()?)
    }

    pub fn borrow_mut(&self) -> Result<RefMut<TextService>> {
        Ok(self.text_service.try_borrow_mut()?)
    }

    pub fn borrow(&self) -> Result<Ref<TextService>> {
        Ok(self.text_service.try_borrow()?)
    }

    pub fn lang_bar_sink(&self) -> Result<RefMut<'_, Option<ITfLangBarItemSink>>> {
        Ok(self.lang_bar_sink.try_borrow_mut()?)
    }
}

impl TextServiceFactory_Impl {
    /// A new reference to this COM object as `I`.
    pub fn interface<I: Interface>(&self) -> Result<I> {
        Ok(unsafe { self.cast::<I>()? })
    }
}
