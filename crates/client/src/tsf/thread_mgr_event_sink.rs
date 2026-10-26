use windows::Win32::UI::TextServices::{
    ITfContext, ITfDocumentMgr, ITfThreadFocusSink_Impl, ITfThreadMgrEventSink_Impl,
};

use anyhow::Result;

use crate::engine::{client_action::ClientAction, composition::CompositionState, state::IMEState};

use super::{edit_session::IfRefused, factory::TextServiceFactory_Impl};

impl ITfThreadMgrEventSink_Impl for TextServiceFactory_Impl {
    #[macros::anyhow]
    fn OnInitDocumentMgr(&self, _pdim: Option<&ITfDocumentMgr>) -> Result<()> {
        Ok(())
    }

    #[macros::anyhow]
    fn OnUninitDocumentMgr(&self, _pdim: Option<&ITfDocumentMgr>) -> Result<()> {
        Ok(())
    }

    #[macros::anyhow]
    fn OnSetFocus(
        &self,
        focus: Option<&ITfDocumentMgr>,
        _prevfocus: Option<&ITfDocumentMgr>,
    ) -> Result<()> {
        // The steps are independent; above all the composition in the
        // document that lost focus must be ended even if a refresh fails.
        let mut first_error = None;
        let mut check = |step: &str, result: Result<()>| {
            if let Err(error) = result {
                tracing::error!("OnSetFocus: {step} failed: {error:?}");
                first_error.get_or_insert(error);
            }
        };

        check("language bar refresh", self.update_lang_bar());
        // the mode is process-wide; another thread may have changed it
        if let Ok(mode) = IMEState::get().map(|state| state.input_mode.clone()) {
            self.update_open_close(&mode);
        }

        // if focus is changed, the text layout sink should be updated
        if let Some(focus) = focus {
            check(
                "text layout sink",
                self.borrow_mut().and_then(|mut text_service| {
                    text_service.advise_text_layout_sink(focus.clone())
                }),
            );
        }

        check(
            "end composition",
            self.run_outside_key_event(IfRefused::RunWithoutComposition, |factory| {
                factory.handle_action(&[ClientAction::EndComposition], CompositionState::None)
            }),
        );

        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    #[macros::anyhow]
    fn OnPushContext(&self, _pic: Option<&ITfContext>) -> Result<()> {
        Ok(())
    }

    #[macros::anyhow]
    fn OnPopContext(&self, _pic: Option<&ITfContext>) -> Result<()> {
        Ok(())
    }
}

impl ITfThreadFocusSink_Impl for TextServiceFactory_Impl {
    /// The input mode is process-wide but the open/close state is per
    /// thread: after the mode was switched on another UI thread, this one
    /// still shows the old one. OnSetFocus above is not enough, since the
    /// thread can get the focus back without its focused document changing.
    #[macros::anyhow]
    fn OnSetThreadFocus(&self) -> Result<()> {
        if let Ok(mode) = IMEState::get().map(|state| state.input_mode.clone()) {
            self.update_open_close(&mode);
        }
        self.update_lang_bar()?;
        Ok(())
    }

    #[macros::anyhow]
    fn OnKillThreadFocus(&self) -> Result<()> {
        Ok(())
    }
}
