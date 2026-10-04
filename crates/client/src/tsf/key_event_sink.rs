use windows::{
    core::GUID,
    Win32::{
        Foundation::{BOOL, LPARAM, WPARAM},
        UI::{
            Input::KeyboardAndMouse::VK_CONTROL,
            TextServices::{ITfContext, ITfKeyEventSink_Impl},
        },
    },
};

use crate::{
    engine::{client_action::ClientAction, composition::CompositionState},
    extension::VKeyExt as _,
};

use anyhow::Result;

use super::factory::TextServiceFactory_Impl;

fn should_intercept_control_shortcut(
    control_pressed: bool,
    key_code: usize,
    composition_active: bool,
) -> bool {
    // Do not cancel on the modifier key itself. Wait for the actual shortcut key.
    const VK_CONTROL_CODE: usize = 0x11;
    const VK_LCONTROL_CODE: usize = 0xA2;
    const VK_RCONTROL_CODE: usize = 0xA3;

    control_pressed
        && composition_active
        && !matches!(
            key_code,
            VK_CONTROL_CODE | VK_LCONTROL_CODE | VK_RCONTROL_CODE
        )
}

// sink (aka event listener) for key events
impl ITfKeyEventSink_Impl for TextServiceFactory_Impl {
    #[macros::anyhow]
    #[tracing::instrument]
    fn OnTestKeyDown(
        &self,
        pic: Option<&ITfContext>,
        wparam: WPARAM,
        _lparam: LPARAM,
    ) -> Result<BOOL> {
        // A control shortcut needs one pass through OnKeyDown so the active
        // composition can be cancelled before the application receives the key.
        // OnKeyDown returns FALSE afterwards, so the shortcut itself is not eaten.
        if should_intercept_control_shortcut(
            VK_CONTROL.is_pressed(),
            wparam.0,
            self.has_active_composition()?,
        ) {
            return Ok(true.into());
        }

        let result = self.process_key(pic, wparam)?.is_some();
        Ok(result.into())
    }

    #[macros::anyhow]
    #[tracing::instrument]
    fn OnKeyDown(&self, pic: Option<&ITfContext>, wparam: WPARAM, _lparam: LPARAM) -> Result<BOOL> {
        if should_intercept_control_shortcut(
            VK_CONTROL.is_pressed(),
            wparam.0,
            self.has_active_composition()?,
        ) {
            if let Some(context) = pic {
                self.borrow_mut()?.context = Some(context.clone());
            }
            self.handle_action(
                &[ClientAction::CancelComposition],
                CompositionState::None,
            )?;
            // FALSE lets the application process Ctrl+A/C/V/etc.
            return Ok(false.into());
        }

        let result = self.handle_key(pic, wparam)?;
        Ok(result.into())
    }

    #[macros::anyhow]
    fn OnTestKeyUp(
        &self,
        _pic: Option<&ITfContext>,
        _wparam: WPARAM,
        _lparam: LPARAM,
    ) -> Result<BOOL> {
        // same as OnTestKeyDown
        Ok(false.into())
    }

    #[macros::anyhow]
    fn OnKeyUp(&self, _pic: Option<&ITfContext>, _wparam: WPARAM, _lparam: LPARAM) -> Result<BOOL> {
        // this function is called when a key is released
        // but we handle key events in OnKeyDown function
        // so just return S_OK
        Ok(false.into())
    }

    #[macros::anyhow]
    fn OnPreservedKey(&self, _pic: Option<&ITfContext>, _rguid: *const GUID) -> Result<BOOL> {
        // this function is actually not used
        Ok(true.into())
    }

    #[macros::anyhow]
    fn OnSetFocus(&self, _fforeground: BOOL) -> Result<()> {
        Ok(())
    }
}


#[cfg(test)]
mod tests {
    use super::should_intercept_control_shortcut;

    #[test]
    fn control_shortcut_is_intercepted_only_with_active_composition() {
        assert!(should_intercept_control_shortcut(true, b'A' as usize, true));
        assert!(!should_intercept_control_shortcut(false, b'A' as usize, true));
        assert!(!should_intercept_control_shortcut(true, b'A' as usize, false));
    }

    #[test]
    fn control_modifier_key_itself_does_not_cancel_composition() {
        assert!(!should_intercept_control_shortcut(true, 0x11, true));
        assert!(!should_intercept_control_shortcut(true, 0xA2, true));
        assert!(!should_intercept_control_shortcut(true, 0xA3, true));
    }
}
