use anyhow::{Context as _, Result};
use tao::{
    event_loop::{EventLoop, EventLoopProxy},
    platform::windows::{WindowBuilderExtWindows, WindowExtWindows},
    window::{Window, WindowBuilder},
};
use windows::Win32::{
    Foundation::HWND,
    UI::WindowsAndMessaging::{
        SetWindowLongW, SetWindowPos, GWL_EXSTYLE, GWL_STYLE, SWP_FRAMECHANGED, SWP_NOACTIVATE,
        SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
        WS_POPUP,
    },
};
use wry::{WebContext, WebView, WebViewBuilder};

use crate::UserEvent;

pub fn create_indicator_window(event_loop: &EventLoop<UserEvent>) -> Result<Window> {
    let window = WindowBuilder::new()
        .with_decorations(false)
        .with_title("Indicator")
        .with_focused(false)
        // Shown only for a moment after the input mode changes.
        .with_visible(false)
        .with_undecorated_shadow(false)
        .with_transparent(true)
        .build(&event_loop)
        .context("Failed to create window")?;

    let hwnd = window.hwnd() as *mut std::ffi::c_void;

    // set extended window style
    // https://docs.microsoft.com/en-us/windows/win32/winmsg/extended-window-styles
    // https://docs.microsoft.com/en-us/windows/win32/winmsg/window-styles
    unsafe {
        let exnewstyle = WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0 | WS_EX_TOPMOST.0;
        SetWindowLongW(HWND(hwnd), GWL_EXSTYLE, exnewstyle as i32);

        let style = WS_POPUP.0;
        SetWindowLongW(HWND(hwnd), GWL_STYLE, style as i32);

        // Style changes take effect only after SWP_FRAMECHANGED.
        let _ = SetWindowPos(
            HWND(hwnd),
            None,
            0,
            0,
            0,
            0,
            SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    };

    Ok(window)
}

pub fn create_indicator_webview(
    window: &Window,
    web_context: &mut WebContext,
    proxy: EventLoopProxy<UserEvent>,
) -> Result<WebView> {
    let webview = WebViewBuilder::with_web_context(web_context)
        .with_transparent(true)
        .with_ipc_handler(move |message| {
            if message.body() == "ready" {
                let _ = proxy.send_event(UserEvent::IndicatorPageReady);
            }
        })
        .with_html(
            r##"
        <html>
            <head>
                <style>
                    body, html {
                        overscroll-behavior: none;
                    }
                    body {
                        margin: 0;
                        padding: 7px;
                        filter: drop-shadow(3px 3px 3px rgba(0, 0, 0, 0.1));
                    }
                    main {
                        width: 100%;
                        height: 100%;
                        border: 1px solid #2CB5FF;
                        border-radius: 8px;
                        background-color: #FFFFFF;
                        box-sizing: border-box;
                        display: flex;
                        justify-content: center;
                        align-items: center;
                    }

                    html.dark body {
                        color: #FFFFFF;
                    }
                    html.dark main {
                        border: 1px solid #5C6BC0;
                        background-color: #1E1E1E;
                    }
                </style>
                <script>
                    let themeMode = 'system';
                    const systemDark = window.matchMedia('(prefers-color-scheme: dark)');

                    function updateColorScheme() {
                        const dark = themeMode === 'dark' ||
                            (themeMode === 'system' && systemDark.matches);
                        document.documentElement.classList.toggle('dark', dark);
                    }

                    // mode: the theme.mode setting, "system", "light" or "dark".
                    function applyTheme(mode) {
                        themeMode = mode;
                        updateColorScheme();
                    }

                    function updateInputMethod(text) {
                        document.querySelector('main').innerText = text;
                    }

                    systemDark.addEventListener('change', updateColorScheme);
                    updateColorScheme();
                    window.addEventListener('DOMContentLoaded', () => {
                        // A theme sent before the page existed was lost.
                        window.ipc.postMessage('ready');
                    });
                </script>
            </head>
            <body style="margin: 0;">
                <main>
                    あ
                </main>
            </body>
        </html>"##,
        )
        .build(&window)
        .context("Failed to create webview")?;

    Ok(webview)
}

/// Script that shows `mode` in the indicator. The mode string arrives over IPC,
/// so it is passed as a JSON string literal and cannot inject script.
pub fn input_mode_script(mode: &str) -> String {
    format!("updateInputMethod({})", serde_json::Value::from(mode))
}

/// Script that applies the `theme.mode` setting to the indicator.
fn theme_script(theme_mode: &str) -> String {
    format!("applyTheme({})", serde_json::Value::from(theme_mode))
}

/// Tracks the `theme.mode` the indicator page shows, so it is only sent
/// when it changed.
#[derive(Default)]
pub struct IndicatorTheme {
    applied: Option<String>,
}

impl IndicatorTheme {
    /// The page was (re)loaded and lost the mode sent before.
    pub fn forget_applied(&mut self) {
        self.applied = None;
    }

    pub fn apply(&mut self, webview: &WebView, theme_mode: &str) {
        if self.applied.as_deref() == Some(theme_mode) {
            return;
        }
        match webview.evaluate_script(&theme_script(theme_mode)) {
            Ok(()) => self.applied = Some(theme_mode.to_string()),
            Err(error) => {
                eprintln!("Failed to apply indicator theme: {error}");
                self.applied = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{input_mode_script, theme_script};

    #[test]
    fn theme_mode_is_passed_as_a_string_literal() {
        assert_eq!(theme_script("dark"), r#"applyTheme("dark")"#);
        assert_eq!(theme_script(r#"x");("#), r#"applyTheme("x\");(")"#);
    }

    #[test]
    fn input_mode_is_passed_as_a_single_string_literal() {
        assert_eq!(input_mode_script("あ"), r#"updateInputMethod("あ")"#);

        let hostile = r#"A"); location.href = "https://example.com"; ("#;
        let script = input_mode_script(hostile);
        let argument = script
            .strip_prefix("updateInputMethod(")
            .and_then(|rest| rest.strip_suffix(')'))
            .expect("one call");
        assert_eq!(serde_json::from_str::<String>(argument).unwrap(), hostile);
    }
}
