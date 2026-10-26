use std::{
    cell::{Cell, Ref, RefCell, RefMut},
    ffi::c_void,
};

use windows::{
    core::{implement, IUnknown, Interface, GUID},
    Win32::{
        Foundation::{BOOL, CLASS_E_NOAGGREGATION, E_POINTER},
        System::Com::{IClassFactory, IClassFactory_Impl},
        UI::TextServices::{
            ITfCompartmentEventSink, ITfCompositionSink, ITfDisplayAttributeProvider,
            ITfKeyEventSink, ITfLangBarItem, ITfLangBarItemButton, ITfLangBarItemSink, ITfSource,
            ITfTextInputProcessor, ITfTextInputProcessorEx, ITfTextLayoutSink, ITfThreadFocusSink,
            ITfThreadMgrEventSink,
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
    ITfThreadFocusSink,
    ITfTextLayoutSink,
    ITfCompositionSink,
    ITfDisplayAttributeProvider,
    ITfLangBarItem,
    ITfLangBarItemButton,
    ITfSource,
    ITfCompartmentEventSink
)]
#[derive(Debug)]
pub struct TextServiceFactory {
    text_service: RefCell<TextService>,
    // The language bar's sink for our item, advised during AddItem. Kept
    // apart from the text service so advising works while it is borrowed.
    lang_bar_sink: RefCell<Option<ITfLangBarItemSink>>,
    // Nesting depth of the text service's own work (key handling, mode
    // switches, ...). Compartment changes seen meanwhile are its own doing.
    own_activity: Cell<u32>,
}

/// Marks the text service's own work while alive; see
/// [`TextServiceFactory::own_activity`].
pub struct OwnActivity<'a>(&'a Cell<u32>);

impl Drop for OwnActivity<'_> {
    fn drop(&mut self) {
        self.0.set(self.0.get() - 1);
    }
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
            own_activity: Cell::new(0),
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

    /// Marks the text service's own work until the returned value is
    /// dropped. While any is in progress, changes of the open/close and
    /// conversion compartments are taken as its own doing (publishing the
    /// mode, or IMM32 reopening the IME when a composition starts) and not
    /// as a request from the application.
    pub fn own_activity(&self) -> OwnActivity<'_> {
        self.own_activity.set(self.own_activity.get() + 1);
        OwnActivity(&self.own_activity)
    }

    pub fn in_own_activity(&self) -> bool {
        self.own_activity.get() > 0
    }
}

impl TextServiceFactory_Impl {
    /// A new reference to this COM object as `I`.
    pub fn interface<I: Interface>(&self) -> Result<I> {
        Ok(unsafe { self.cast::<I>()? })
    }
}
