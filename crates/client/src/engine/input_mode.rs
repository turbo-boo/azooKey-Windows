use crate::tsf::factory::TextServiceFactory;

use windows::{
    core::{Interface, VARIANT},
    Win32::UI::TextServices::{
        ITfCompartmentMgr, GUID_COMPARTMENT_KEYBOARD_INPUTMODE_CONVERSION,
        GUID_COMPARTMENT_KEYBOARD_OPENCLOSE, TF_CONVERSIONMODE_ALPHANUMERIC,
        TF_CONVERSIONMODE_FULLSHAPE, TF_CONVERSIONMODE_NATIVE, TF_CONVERSIONMODE_ROMAN,
        TF_LBI_ICON,
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
    /// Asks the language bar to fetch the icon again (GetIcon) after the
    /// input mode changed.
    pub fn update_lang_bar(&self) -> Result<()> {
        // not advised: the item is not on the language bar, nothing to show
        let Some(sink) = self.lang_bar_sink()?.clone() else {
            return Ok(());
        };
        unsafe { sink.OnUpdate(TF_LBI_ICON)? };
        Ok(())
    }

    /// Publishes the input mode as the keyboard open/close state and the
    /// input mode conversion state. IMM32 applications read the former
    /// through ImmGetOpenStatus; Sakura Editor, for one, places the
    /// composition window at the caret only while the IME is open, so it
    /// showed up at the top left of the screen. The latter is what the
    /// text service promises by registering
    /// GUID_TFCAT_TIPCAP_INPUTMODECOMPARTMENT; the system input mode
    /// indicator reads it.
    pub fn update_open_close(&self, mode: &InputMode) {
        let result: Result<()> = (|| {
            let (thread_mgr, tid) = {
                let text_service = self.borrow()?;
                (text_service.thread_mgr()?, text_service.tid)
            };
            let compartments = thread_mgr.cast::<ITfCompartmentMgr>()?;
            for (guid, value) in [
                (GUID_COMPARTMENT_KEYBOARD_OPENCLOSE, mode.open_close()),
                (
                    GUID_COMPARTMENT_KEYBOARD_INPUTMODE_CONVERSION,
                    mode.conversion_mode(),
                ),
            ] {
                unsafe {
                    let compartment = compartments.GetCompartment(&guid)?;
                    // called on every focus change: write only when it changes
                    if i32::try_from(&compartment.GetValue()?).ok() != Some(value) {
                        compartment.SetValue(tid, &VARIANT::from(value))?;
                    }
                }
            }
            Ok(())
        })();

        if let Err(error) = result {
            tracing::warn!("Failed to publish the input mode: {error:?}");
        }
    }
}

impl InputMode {
    /// GUID_COMPARTMENT_KEYBOARD_OPENCLOSE: the IME is open in Kana mode.
    pub fn open_close(&self) -> i32 {
        i32::from(*self == InputMode::Kana)
    }

    /// GUID_COMPARTMENT_KEYBOARD_INPUTMODE_CONVERSION: romaji to full-width
    /// hiragana in Kana mode, plain alphanumeric input in Latin mode.
    pub fn conversion_mode(&self) -> i32 {
        let mode = match self {
            InputMode::Kana => {
                TF_CONVERSIONMODE_NATIVE | TF_CONVERSIONMODE_FULLSHAPE | TF_CONVERSIONMODE_ROMAN
            }
            InputMode::Latin => TF_CONVERSIONMODE_ALPHANUMERIC,
        };
        mode as i32
    }
}
