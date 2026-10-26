use windows::{
    core::{IUnknown, Interface as _, BSTR, GUID, PCWSTR},
    Win32::{
        Foundation::{BOOL, E_INVALIDARG, POINT, RECT},
        System::Ole::{CONNECT_E_ADVISELIMIT, CONNECT_E_CANNOTCONNECT, CONNECT_E_NOCONNECTION},
        UI::{
            TextServices::{
                ITfLangBarItemButton_Impl, ITfLangBarItemSink, ITfLangBarItem_Impl, ITfMenu,
                ITfSource_Impl, TfLBIClick, GUID_LBI_INPUTMODE, TF_LANGBARITEMINFO,
                TF_LBI_STYLE_BTN_BUTTON,
            },
            WindowsAndMessaging::{LoadImageW, HICON, IMAGE_ICON, LR_DEFAULTCOLOR},
        },
    },
};

use crate::{
    engine::{
        client_action::ClientAction, composition::CompositionState, input_mode::InputMode,
        state::IMEState, theme::get_theme,
    },
    globals::{DllModule, GUID_TEXT_SERVICE, TEXTSERVICE_LANGBARITEMSINK_COOKIE},
};

use anyhow::{Context as _, Result};

use super::{edit_session::IfRefused, factory::TextServiceFactory_Impl};

const INFO: TF_LANGBARITEMINFO = TF_LANGBARITEMINFO {
    clsidService: GUID_TEXT_SERVICE,
    guidItem: GUID_LBI_INPUTMODE,
    dwStyle: TF_LBI_STYLE_BTN_BUTTON,
    ulSort: 0,
    szDescription: [0; 32],
};

// you need to implement these three interfaces to create a language bar item
// if not, you will get E_FAIL error in ITfLangBarItemMgr::AddItem

impl ITfLangBarItem_Impl for TextServiceFactory_Impl {
    #[macros::anyhow]
    fn GetInfo(&self, p_info: *mut TF_LANGBARITEMINFO) -> Result<()> {
        unsafe {
            *p_info = INFO;
        }
        Ok(())
    }

    #[macros::anyhow]
    fn GetStatus(&self) -> Result<u32> {
        Ok(0)
    }

    #[macros::anyhow]
    fn Show(&self, _f_show: BOOL) -> Result<()> {
        Ok(())
    }

    // this will be shown as a tooltip when you hover the language bar item
    #[macros::anyhow]
    fn GetTooltipString(&self) -> Result<BSTR> {
        Ok(BSTR::default())
    }
}

impl ITfLangBarItemButton_Impl for TextServiceFactory_Impl {
    #[macros::anyhow]
    fn OnClick(&self, _click: TfLBIClick, _pt: &POINT, _prcarea: *const RECT) -> Result<()> {
        self.run_outside_key_event(IfRefused::RunWithoutComposition, |factory| {
            let mode = match IMEState::get()?.input_mode {
                InputMode::Latin => InputMode::Kana,
                InputMode::Kana => InputMode::Latin,
            };
            // Like the keyboard toggle, commit an active composition first so
            // TSF and the engine end it together.
            let composing =
                factory.borrow()?.borrow_composition()?.state != CompositionState::None;
            let mut actions = Vec::new();
            if composing {
                actions.push(ClientAction::EndComposition);
            }
            actions.push(ClientAction::SetIMEMode(mode));
            factory.handle_action(&actions, CompositionState::None)
        })
    }

    // this method should not be called
    #[macros::anyhow]
    fn InitMenu(&self, _pmenu: Option<&ITfMenu>) -> Result<()> {
        Ok(())
    }

    // this method should not be called
    #[macros::anyhow]
    fn OnMenuSelect(&self, _w_id: u32) -> Result<()> {
        Ok(())
    }

    #[macros::anyhow]
    fn GetIcon(&self) -> Result<HICON> {
        // copy what is needed instead of holding the locks across LoadImageW
        let hinst = DllModule::get()?.hinst.context("Dll instance not found")?;
        let input_mode = IMEState::get()?.input_mode.clone();
        let theme = get_theme()?;

        let icon_id = match input_mode {
            InputMode::Kana => {
                if theme {
                    102
                } else {
                    104
                }
            }
            InputMode::Latin => {
                if theme {
                    103
                } else {
                    105
                }
            }
        };

        unsafe {
            let handle = LoadImageW(
                hinst,
                PCWSTR(icon_id as *mut u16),
                IMAGE_ICON,
                0,
                0,
                LR_DEFAULTCOLOR,
            )?;

            Ok(HICON(handle.0))
        }
    }

    #[macros::anyhow]
    fn GetText(&self) -> Result<BSTR> {
        Ok(BSTR::default())
    }
}

impl ITfSource_Impl for TextServiceFactory_Impl {
    #[macros::anyhow]
    fn AdviseSink(&self, riid: *const GUID, punk: Option<&IUnknown>) -> Result<u32> {
        let riid = unsafe { *riid };

        if riid != ITfLangBarItemSink::IID {
            return Err(windows_core::Error::from(CONNECT_E_CANNOTCONNECT).into());
        }

        let sink: ITfLangBarItemSink = punk
            .ok_or_else(|| windows_core::Error::from(E_INVALIDARG))?
            .cast()?;

        // the item has a single connection point
        let mut advised = self.lang_bar_sink()?;
        if advised.is_some() {
            return Err(windows_core::Error::from(CONNECT_E_ADVISELIMIT).into());
        }
        // kept to tell the language bar when the icon changes
        *advised = Some(sink);

        Ok(TEXTSERVICE_LANGBARITEMSINK_COOKIE)
    }

    #[macros::anyhow]
    fn UnadviseSink(&self, dw_cookie: u32) -> Result<()> {
        let mut advised = self.lang_bar_sink()?;
        if dw_cookie != TEXTSERVICE_LANGBARITEMSINK_COOKIE || advised.is_none() {
            return Err(windows_core::Error::from(CONNECT_E_NOCONNECTION).into());
        }
        *advised = None;

        Ok(())
    }
}
