use windows::{
    core::{Interface as _, GUID},
    Win32::{
        Foundation::{BOOL, LPARAM, WPARAM},
        UI::{
            Input::KeyboardAndMouse::VK_CONTROL,
            TextServices::{
                ITfCompartmentMgr, ITfContext, ITfKeyEventSink_Impl, GUID_COMPARTMENT_EMPTYCONTEXT,
                GUID_COMPARTMENT_KEYBOARD_DISABLED,
            },
        },
    },
};

use crate::{
    engine::{
        client_action::ClientAction,
        composition::{ctrl_space_shortcut_enabled, CompositionState},
    },
    extension::VKeyExt as _,
};

use anyhow::Result;

use super::factory::TextServiceFactory_Impl;

fn should_intercept_control_shortcut(
    control_pressed: bool,
    key_code: usize,
    composition_active: bool,
    is_ime_toggle_shortcut: bool,
) -> bool {
    // Do not cancel on the modifier key itself. Wait for the actual shortcut key.
    const VK_CONTROL_CODE: usize = 0x11;
    const VK_LCONTROL_CODE: usize = 0xA2;
    const VK_RCONTROL_CODE: usize = 0xA3;

    control_pressed
        && composition_active
        && !is_ime_toggle_shortcut
        && !matches!(
            key_code,
            VK_CONTROL_CODE | VK_LCONTROL_CODE | VK_RCONTROL_CODE
        )
}

/// Whether the context says not to take keyboard input:
/// GUID_COMPARTMENT_KEYBOARD_DISABLED (keyboard disabled) or
/// GUID_COMPARTMENT_EMPTYCONTEXT (the context is empty), both nonzero
/// DWORDs in the context's own compartments. Keys then go to the
/// application untouched.
fn keyboard_disabled(context: &ITfContext) -> bool {
    let Ok(compartments) = context.cast::<ITfCompartmentMgr>() else {
        return false;
    };
    [
        GUID_COMPARTMENT_KEYBOARD_DISABLED,
        GUID_COMPARTMENT_EMPTYCONTEXT,
    ]
    .iter()
    .any(|guid| unsafe {
        compartments
            .GetCompartment(guid)
            .and_then(|compartment| compartment.GetValue())
            .ok()
            .and_then(|value| i32::try_from(&value).ok())
            .is_some_and(|value| value != 0)
    })
}

// sink (aka event listener) for key events
impl ITfKeyEventSink_Impl for TextServiceFactory_Impl {
    #[macros::anyhow]
    #[tracing::instrument(skip(self))]
    fn OnTestKeyDown(
        &self,
        pic: Option<&ITfContext>,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> Result<BOOL> {
        let _own = self.own_activity();
        if pic.is_some_and(keyboard_disabled) {
            return Ok(false.into());
        }

        // A control shortcut needs one pass through OnKeyDown so the active
        // composition can be cancelled before the application receives the key.
        // OnKeyDown returns FALSE afterwards, so the shortcut itself is not eaten.
        if should_intercept_control_shortcut(
            VK_CONTROL.is_pressed(),
            wparam.0,
            self.has_active_composition()?,
            ctrl_space_shortcut_enabled(wparam),
        ) {
            return Ok(true.into());
        }

        let result = self.process_key(pic, wparam, lparam)?.is_some();
        Ok(result.into())
    }

    #[macros::anyhow]
    #[tracing::instrument(skip(self))]
    fn OnKeyDown(&self, pic: Option<&ITfContext>, wparam: WPARAM, lparam: LPARAM) -> Result<BOOL> {
        let _own = self.own_activity();
        if pic.is_some_and(keyboard_disabled) {
            return Ok(false.into());
        }

        if should_intercept_control_shortcut(
            VK_CONTROL.is_pressed(),
            wparam.0,
            self.has_active_composition()?,
            ctrl_space_shortcut_enabled(wparam),
        ) {
            // Cancelling deletes the composition's text. One left in the
            // document that lost focus is committed instead: the shortcut
            // is meant for the document it was typed in.
            let action = match pic {
                Some(context) if self.composition_elsewhere(context)? => {
                    ClientAction::EndComposition
                }
                _ => ClientAction::CancelComposition,
            };
            if let Some(context) = pic {
                self.borrow_mut()?.context = Some(context.clone());
            }
            self.handle_action(&[action], CompositionState::None)?;
            // FALSE lets the application process Ctrl+A/C/V/etc.
            return Ok(false.into());
        }

        let result = self.handle_key(pic, wparam, lparam)?;
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
        assert!(should_intercept_control_shortcut(true, b'A' as usize, true, false));
        assert!(!should_intercept_control_shortcut(false, b'A' as usize, true, false));
        assert!(!should_intercept_control_shortcut(true, b'A' as usize, false, false));
        assert!(!should_intercept_control_shortcut(true, 0x20, true, true));
    }

    #[test]
    fn control_modifier_key_itself_does_not_cancel_composition() {
        assert!(!should_intercept_control_shortcut(true, 0x11, true, false));
        assert!(!should_intercept_control_shortcut(true, 0xA2, true, false));
        assert!(!should_intercept_control_shortcut(true, 0xA3, true, false));
    }
}
