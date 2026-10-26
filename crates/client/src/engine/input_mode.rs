use crate::tsf::factory::TextServiceFactory;

use windows::{
    core::{Interface, VARIANT},
    Win32::UI::TextServices::{
        ITfCompartmentMgr, ITfLangBarItemButton, ITfLangBarItemMgr,
        GUID_COMPARTMENT_KEYBOARD_OPENCLOSE,
    },
};

use anyhow::Result;

#[derive(Default, Clone, PartialEq, Debug)]
pub enum InputMode {
    #[default]
    Latin,
    Kana,
}

impl TextServiceFactory {
    pub fn update_lang_bar(&self) -> Result<()> {
        // change the icon of the language bar item
        let text_service = self.borrow()?;
        let thread_mgr = text_service.thread_mgr()?;

        unsafe {
            thread_mgr
                .cast::<ITfLangBarItemMgr>()?
                .RemoveItem(&text_service.this::<ITfLangBarItemButton>()?)?;

            thread_mgr
                .cast::<ITfLangBarItemMgr>()?
                .AddItem(&text_service.this::<ITfLangBarItemButton>()?)?;
        };

        Ok(())
    }

    /// Publishes the input mode as the keyboard open/close state. IMM32
    /// applications read it through ImmGetOpenStatus; Sakura Editor, for one,
    /// places the composition window at the caret only while the IME is open,
    /// so it showed up at the top left of the screen.
    pub fn update_open_close(&self, mode: &InputMode) {
        let result: Result<()> = (|| {
            let (thread_mgr, tid) = {
                let text_service = self.borrow()?;
                (text_service.thread_mgr()?, text_service.tid)
            };
            let open = i32::from(*mode == InputMode::Kana);
            unsafe {
                let compartment = thread_mgr
                    .cast::<ITfCompartmentMgr>()?
                    .GetCompartment(&GUID_COMPARTMENT_KEYBOARD_OPENCLOSE)?;
                // called on every focus change: write only when it changes
                if i32::try_from(&compartment.GetValue()?).ok() != Some(open) {
                    compartment.SetValue(tid, &VARIANT::from(open))?;
                }
            }
            Ok(())
        })();

        if let Err(error) = result {
            tracing::warn!("Failed to publish the open/close state: {error:?}");
        }
    }
}
