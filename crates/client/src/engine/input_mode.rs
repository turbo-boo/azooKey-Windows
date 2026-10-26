use crate::{
    engine::{client_action::ClientAction, composition::CompositionState, state::IMEState},
    tsf::{
        edit_session::IfRefused,
        factory::{TextServiceFactory, TextServiceFactory_Impl},
    },
};

use windows::{
    core::{Interface, GUID, VARIANT},
    Win32::UI::TextServices::{
        ITfCompartmentEventSink, ITfCompartmentEventSink_Impl, ITfCompartmentMgr, ITfSource,
        GUID_COMPARTMENT_KEYBOARD_INPUTMODE_CONVERSION, GUID_COMPARTMENT_KEYBOARD_OPENCLOSE,
        TF_CONVERSIONMODE_ALPHANUMERIC, TF_CONVERSIONMODE_FULLSHAPE, TF_CONVERSIONMODE_NATIVE,
        TF_CONVERSIONMODE_ROMAN, TF_LBI_ICON,
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
        // the change notifications this causes are not requests
        let _own = self.own_activity();
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

    /// Switches to the mode `mode` picks when the switch runs, from outside
    /// key handling (language bar, an application changing the open/close
    /// state), committing an active composition first like the keyboard
    /// toggle does, so TSF and the engine end it together.
    pub fn switch_input_mode(
        &self,
        mode: impl FnOnce() -> Result<InputMode> + 'static,
    ) -> Result<()> {
        self.run_outside_key_event(IfRefused::RunWithoutComposition, move |factory| {
            let mode = mode()?;
            let composing = factory.borrow()?.borrow_composition()?.state != CompositionState::None;
            let mut actions = Vec::new();
            if composing {
                actions.push(ClientAction::EndComposition);
            }
            actions.push(ClientAction::SetIMEMode(mode));
            factory.handle_action(&actions, CompositionState::None)
        })
    }

    /// Follows the open/close and conversion compartments of this thread,
    /// so a change made by the application (ImmSetOpenStatus, for one) or
    /// the system switches the input mode instead of being overwritten on
    /// the next focus change.
    pub fn advise_compartment_sinks(&self) -> Result<()> {
        let (thread_mgr, sink) = {
            let text_service = self.borrow()?;
            (
                text_service.thread_mgr()?,
                text_service.this::<ITfCompartmentEventSink>()?,
            )
        };
        let compartments = thread_mgr.cast::<ITfCompartmentMgr>()?;
        for guid in MODE_COMPARTMENTS {
            unsafe {
                let compartment = compartments.GetCompartment(&guid)?;
                let cookie = compartment
                    .cast::<ITfSource>()?
                    .AdviseSink(&ITfCompartmentEventSink::IID, &sink)?;
                self.borrow_mut()?
                    .compartment_sinks
                    .push((compartment, cookie));
            }
        }
        Ok(())
    }
}

/// The thread manager compartments that carry the input mode.
const MODE_COMPARTMENTS: [GUID; 2] = [
    GUID_COMPARTMENT_KEYBOARD_OPENCLOSE,
    GUID_COMPARTMENT_KEYBOARD_INPUTMODE_CONVERSION,
];

impl ITfCompartmentEventSink_Impl for TextServiceFactory_Impl {
    #[macros::anyhow]
    fn OnChange(&self, rguid: *const GUID) -> Result<()> {
        let changed = unsafe { *rguid };
        if !MODE_COMPARTMENTS.contains(&changed) || self.in_own_activity() {
            return Ok(());
        }

        let compartments = self.borrow()?.thread_mgr()?.cast::<ITfCompartmentMgr>()?;
        let read = |guid: GUID| -> Result<i32> {
            let value = unsafe { compartments.GetCompartment(&guid)?.GetValue()? };
            // VT_EMPTY until someone sets it
            Ok(i32::try_from(&value).unwrap_or(0))
        };
        let Some(mode) = InputMode::requested(
            read(GUID_COMPARTMENT_KEYBOARD_OPENCLOSE)?,
            read(GUID_COMPARTMENT_KEYBOARD_INPUTMODE_CONVERSION)?,
            changed == GUID_COMPARTMENT_KEYBOARD_OPENCLOSE,
        ) else {
            return Ok(());
        };
        if IMEState::get()?.input_mode == mode {
            return Ok(());
        }

        tracing::debug!("The open/close or conversion state asks for {mode:?}");
        self.switch_input_mode(move || Ok(mode))
    }
}

impl InputMode {
    /// The mode asked for by a change of the open/close state or of the
    /// conversion mode, given both values after the change. Opening means
    /// Kana and closing Latin. A conversion mode change only matters while
    /// open: with the native (Japanese) flag it is Kana, without it Latin.
    pub fn requested(open_close: i32, conversion: i32, open_close_changed: bool) -> Option<Self> {
        let kana = if open_close_changed {
            open_close != 0
        } else if open_close == 0 {
            return None;
        } else {
            conversion as u32 & TF_CONVERSIONMODE_NATIVE != 0
        };
        Some(if kana {
            InputMode::Kana
        } else {
            InputMode::Latin
        })
    }

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

#[cfg(test)]
mod tests {
    use super::InputMode;

    #[test]
    fn open_close_change_selects_the_mode() {
        let alphanumeric = InputMode::Latin.conversion_mode();
        // ImmSetOpenStatus(TRUE) while Latin had published "alphanumeric"
        assert_eq!(
            InputMode::requested(1, alphanumeric, true),
            Some(InputMode::Kana)
        );
        assert_eq!(
            InputMode::requested(0, InputMode::Kana.conversion_mode(), true),
            Some(InputMode::Latin)
        );
    }

    #[test]
    fn conversion_change_matters_only_while_open() {
        let kana = InputMode::Kana.conversion_mode();
        let alphanumeric = InputMode::Latin.conversion_mode();
        assert_eq!(InputMode::requested(0, kana, false), None);
        assert_eq!(
            InputMode::requested(1, alphanumeric, false),
            Some(InputMode::Latin)
        );
        assert_eq!(InputMode::requested(1, kana, false), Some(InputMode::Kana));
    }

    #[test]
    fn published_state_reads_back_as_the_same_mode() {
        // what update_open_close writes must not look like a switch request
        for mode in [InputMode::Kana, InputMode::Latin] {
            for open_close_changed in [true, false] {
                assert_eq!(
                    InputMode::requested(
                        mode.open_close(),
                        mode.conversion_mode(),
                        open_close_changed
                    )
                    .unwrap_or(InputMode::Latin),
                    mode
                );
            }
        }
    }
}
