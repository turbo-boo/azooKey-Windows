//! A message-only window owned by the text service. Work finished on other
//! threads (the AI conversion) is queued here and the window is woken up,
//! because TSF objects may only be used on the thread that activated the
//! text service.

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, PoisonError},
};

use anyhow::{Context as _, Result};
use windows::{
    core::{w, PCWSTR},
    Win32::{
        Foundation::{
            GetLastError, ERROR_CLASS_ALREADY_EXISTS, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM,
        },
        UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, GetWindowLongPtrW, KillTimer,
            PostMessageW, RegisterClassW, SetTimer, SetWindowLongPtrW, UnregisterClassW,
            GWLP_USERDATA, HWND_MESSAGE, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WM_TIMER,
            WNDCLASSW,
        },
    },
};

use crate::{
    engine::composition::{Delivery, MagicResult},
    globals::DllModule,
};

use super::factory::TextServiceFactory;

const CLASS_NAME: PCWSTR = w!("AzookeyTextServiceMessages");
const WM_MAGIC_RESULT: u32 = WM_APP + 1;
/// Retries a delivery that found the text service in use. A timer, not a
/// posted message: a nested message loop running while the text service is
/// in use would dispatch a posted message again at once, over and over.
const RETRY_TIMER: usize = 1;
const RETRY_DELAY_MS: u32 = 50;

type Queue = Arc<Mutex<VecDeque<MagicResult>>>;

#[derive(Debug)]
pub struct MessageWindow {
    hwnd: HWND,
    hinstance: HINSTANCE,
    results: Queue,
}

/// Hands results to the text service's thread. Usable from any thread.
pub struct ResultSender {
    // HWND is not Send; the handle itself may be used from any thread
    hwnd: isize,
    results: Queue,
}

impl ResultSender {
    pub fn send(&self, result: MagicResult) {
        self.results
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push_back(result);
        // Fails once the text service has been deactivated and the window
        // destroyed; the result is then dropped with the queue.
        if let Err(error) =
            unsafe { PostMessageW(HWND(self.hwnd as _), WM_MAGIC_RESULT, WPARAM(0), LPARAM(0)) }
        {
            tracing::debug!("Text service is gone, dropping result: {error:?}");
        }
    }
}

impl MessageWindow {
    /// Creates the window on the current (TSF) thread. `owner` must outlive
    /// the window: the text service drops it in Deactivate.
    pub fn create(owner: &TextServiceFactory) -> Result<Self> {
        let hinstance = HINSTANCE(DllModule::get()?.hinst.context("Dll instance not found")?.0);

        unsafe {
            let class = WNDCLASSW {
                lpfnWndProc: Some(window_proc),
                hInstance: hinstance,
                lpszClassName: CLASS_NAME,
                ..Default::default()
            };
            // registered once per process; other threads' text services share it
            if RegisterClassW(&class) == 0 && GetLastError() != ERROR_CLASS_ALREADY_EXISTS {
                return Err(windows::core::Error::from_win32().into());
            }

            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                CLASS_NAME,
                PCWSTR::null(),
                WINDOW_STYLE::default(),
                0,
                0,
                0,
                0,
                HWND_MESSAGE,
                None,
                hinstance,
                None,
            )?;
            SetWindowLongPtrW(
                hwnd,
                GWLP_USERDATA,
                // isize on 64-bit; on 32-bit this is SetWindowLongW (i32)
                owner as *const TextServiceFactory as _,
            );

            Ok(Self {
                hwnd,
                hinstance,
                results: Queue::default(),
            })
        }
    }

    pub fn sender(&self) -> ResultSender {
        ResultSender {
            hwnd: self.hwnd.0 as isize,
            results: self.results.clone(),
        }
    }

    pub fn take_results(&self) -> Vec<MagicResult> {
        self.results
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .drain(..)
            .collect()
    }
}

impl Drop for MessageWindow {
    fn drop(&mut self) {
        unsafe {
            if let Err(error) = DestroyWindow(self.hwnd) {
                tracing::warn!("Failed to destroy the message window: {error:?}");
            }
            // Fails while text services on other threads still use the class;
            // the last one unregisters it.
            let _ = UnregisterClassW(CLASS_NAME, self.hinstance);
        }
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let retry = msg == WM_TIMER && wparam.0 == RETRY_TIMER;
    if msg == WM_MAGIC_RESULT || retry {
        if retry {
            let _ = KillTimer(hwnd, RETRY_TIMER);
        }
        let owner = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const TextServiceFactory;
        if let Some(owner) = owner.as_ref() {
            if owner.deliver_magic_results() == Delivery::Busy
                && SetTimer(hwnd, RETRY_TIMER, RETRY_DELAY_MS, None) == 0
            {
                tracing::warn!(
                    "Cannot schedule the AI result delivery: {:?}",
                    windows::core::Error::from_win32()
                );
            }
        }
        return LRESULT(0);
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}
