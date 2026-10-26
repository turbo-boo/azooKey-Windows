use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context as _, Result};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use serde::Serialize;
use shared::ThemeConfig;
use tao::{
    event_loop::EventLoop,
    platform::windows::{WindowBuilderExtWindows, WindowExtWindows},
    window::{Window, WindowBuilder},
};
use windows::Win32::{
    Foundation::HWND,
    UI::WindowsAndMessaging::{
        SetWindowLongW, GWL_EXSTYLE, GWL_STYLE, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
        WS_POPUP,
    },
};
use wry::{WebContext, WebView, WebViewBuilder};

use crate::UserEvent;

pub fn create_candidate_window(event_loop: &EventLoop<UserEvent>) -> Result<Window> {
    let window = WindowBuilder::new()
        .with_decorations(false)
        .with_title("CandidateList")
        .with_focused(false)
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
    };

    Ok(window)
}

/// Background images larger than this are not shown: the image travels as a
/// base64 data URL through ExecuteScript.
const MAX_BACKGROUND_IMAGE_BYTES: u64 = 4 * 1024 * 1024;

/// Identifies a version of the background image file.
type ImageStamp = (PathBuf, SystemTime, u64);

/// Tracks the theme the candidate page shows, so that a Show (sent on every
/// key press during live conversion) only touches the web view when the
/// theme or the background image file changed, and the image is read and
/// encoded once per version of the file.
#[derive(Default)]
pub struct ThemeState {
    applied: Option<(ThemeConfig, Option<ImageStamp>)>,
    image: Option<(ImageStamp, Option<String>)>,
}

impl ThemeState {
    /// The argument for `applyTheme` when `theme` differs from the theme
    /// last returned, `None` when the page already shows it.
    pub fn payload_if_changed(&mut self, theme: &ThemeConfig) -> Option<String> {
        let stamp = image_stamp(&theme.background_image);
        if self
            .applied
            .as_ref()
            .is_some_and(|(applied, applied_stamp)| applied == theme && *applied_stamp == stamp)
        {
            return None;
        }
        let image = stamp.clone().and_then(|stamp| self.background_image(stamp));
        self.applied = Some((theme.clone(), stamp));
        Some(theme_payload(theme, image.as_deref()))
    }

    /// The page lost or never received the theme (it was (re)loaded, or
    /// applying failed): the next call returns the payload again.
    pub fn forget_applied(&mut self) {
        self.applied = None;
    }

    /// Sends `theme` to the candidate page unless it already shows it.
    pub fn apply(&mut self, webview: &WebView, theme: &ThemeConfig) {
        if let Some(payload) = self.payload_if_changed(theme) {
            if let Err(error) = webview.evaluate_script(&format!("applyTheme({payload})")) {
                eprintln!("Failed to apply candidate theme: {error}");
                self.forget_applied();
            }
        }
    }

    fn background_image(&mut self, stamp: ImageStamp) -> Option<String> {
        if let Some((cached_stamp, image)) = &self.image {
            if *cached_stamp == stamp {
                return image.clone();
            }
        }
        let image = read_background_image(&stamp.0, stamp.2);
        self.image = Some((stamp, image.clone()));
        image
    }
}

fn image_stamp(path: &str) -> Option<ImageStamp> {
    if path.is_empty() {
        return None;
    }
    let metadata = std::fs::metadata(path).ok()?;
    Some((
        PathBuf::from(path),
        metadata.modified().ok()?,
        metadata.len(),
    ))
}

/// The image as a data URL, or `None` when its type is not supported, it is
/// too large or it cannot be read.
fn read_background_image(path: &Path, len: u64) -> Option<String> {
    let mime = match path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
    {
        Some(ext) if ext == "png" => "image/png",
        Some(ext) if ext == "jpg" || ext == "jpeg" => "image/jpeg",
        Some(ext) if ext == "webp" => "image/webp",
        _ => return None,
    };
    if len > MAX_BACKGROUND_IMAGE_BYTES {
        eprintln!(
            "The background image {} is larger than {MAX_BACKGROUND_IMAGE_BYTES} bytes and is not shown",
            path.display()
        );
        return None;
    }
    match std::fs::read(path) {
        Ok(bytes) => Some(format!(
            "data:{mime};base64,{}",
            BASE64_STANDARD.encode(bytes)
        )),
        Err(error) => {
            eprintln!(
                "Cannot read the background image {}: {error}",
                path.display()
            );
            None
        }
    }
}

fn theme_payload(theme: &ThemeConfig, background_image: Option<&str>) -> String {
    #[derive(Serialize)]
    struct ThemePayload<'a> {
        mode: &'a str,
        background_color: &'a str,
        text_color: &'a str,
        accent_color: &'a str,
        selection_background: &'a str,
        selection_border: &'a str,
        background_image: Option<&'a str>,
        custom_css_enabled: bool,
        custom_css: &'a str,
    }

    serde_json::to_string(&ThemePayload {
        mode: &theme.mode,
        background_color: &theme.background_color,
        text_color: &theme.text_color,
        accent_color: &theme.accent_color,
        selection_background: &theme.selection_background,
        selection_border: &theme.selection_border,
        background_image,
        custom_css_enabled: theme.custom_css_enabled,
        custom_css: &theme.custom_css,
    })
    .unwrap_or_else(|_| "{}".to_string())
}

pub fn create_candidate_webview(web_context: &mut WebContext) -> Result<WebViewBuilder<'_>> {
    let webview_builder = WebViewBuilder::with_web_context(web_context)
        .with_transparent(true)
        .with_html(
            r##"
        <html>
            <head>
                <style>
                    :root {
                        --candidate-background: #FFFFFF;
                        --candidate-text: #202020;
                        --candidate-muted: #636363;
                        --candidate-border: #E4E4E4;
                        --candidate-accent: #2CB5FF;
                        --candidate-selection-background: #D4F0FF;
                        --candidate-selection-border: #2CB5FF;
                        --candidate-scrollbar: #BCBCBC;
                    }
                    body, html {
                        overscroll-behavior: none;
                    }
                    body {
                        margin: 0;
                        padding: 7px;
                        color: var(--candidate-text);
                        filter: drop-shadow(3px 3px 3px rgba(0, 0, 0, 0.1));
                    }
                    main {
                        width: 100%;
                        height: 100%;
                        padding: 8px;
                        border: 1px solid var(--candidate-border);
                        border-radius: 10px;
                        background-color: var(--candidate-background);
                        background-size: cover;
                        background-position: center;
                        box-sizing: border-box;
                        display: flex;
                        flex-direction: column;
                    }
                    ol {
                        margin: 0;
                        padding: 0;
                        flex: 1;
                        overflow-y: auto;
                        scroll-snap-type: y proximity;
                        list-style-position: inside;
                        list-style-type: none;
                        counter-reset: number 0;
                        user-select: none;
                        cursor: pointer;
                    }
                    ol::-webkit-scrollbar {
                        width: 5px;
                    }
                    ol::-webkit-scrollbar-thumb {
                        background-color: var(--candidate-scrollbar);
                        border-radius: 10px;
                    }
                    li {
                        padding: 0.5rem;
                        font-size: 0.9rem;
                        display: flex;
                        align-items: center;
                        scroll-snap-align: start;
                        color: var(--candidate-text);
                    }
                    li::before {
                        content: counter(number);
                        counter-increment: number 1;
                        color: var(--candidate-muted);
                        font-weight: bold;
                        font-size: 0.75rem;
                        margin: 0 0.75rem 0 2px;
                        width: 0.75rem;
                    }
                    li[data-selected] {
                        background-color: var(--candidate-selection-background);
                        border-radius: 3px;
                        margin-right: 5px;
                        outline: 1px solid var(--candidate-selection-border);
                        outline-offset: -1px;
                    }
                    footer {
                        display: flex;
                        justify-content: space-between;
                        align-items: center;
                        padding: 8px 10px 5px 10px;
                        border-top: 1px solid var(--candidate-border);
                        font-size: 0.8rem;
                        user-select: none;
                    }
                </style>
                <style id="custom-theme"></style>
                <script>
                    function validHexColor(value) {
                        return typeof value === 'string' && /^#[0-9a-fA-F]{6}$/.test(value);
                    }

                    function applyTheme(theme) {
                        window.__candidateTheme = theme || {};
                        if (!document.documentElement) return;

                        const isDark = theme.mode === 'dark' ||
                            (theme.mode === 'system' && window.matchMedia('(prefers-color-scheme: dark)').matches);
                        const defaults = isDark
                            ? {
                                background: '#1E1E1E',
                                text: '#E0E0E0',
                                muted: '#BDBDBD',
                                border: '#424242',
                                accent: '#5C6BC0',
                                selectionBackground: '#3949AB',
                                selectionBorder: '#5C6BC0',
                                scrollbar: '#757575'
                            }
                            : {
                                background: '#FFFFFF',
                                text: '#202020',
                                muted: '#636363',
                                border: '#E4E4E4',
                                accent: '#2CB5FF',
                                selectionBackground: '#D4F0FF',
                                selectionBorder: '#2CB5FF',
                                scrollbar: '#BCBCBC'
                            };

                        const root = document.documentElement.style;
                        const accent = validHexColor(theme.accent_color) ? theme.accent_color : defaults.accent;
                        root.setProperty('--candidate-background', validHexColor(theme.background_color) ? theme.background_color : defaults.background);
                        root.setProperty('--candidate-text', validHexColor(theme.text_color) ? theme.text_color : defaults.text);
                        root.setProperty('--candidate-muted', defaults.muted);
                        root.setProperty('--candidate-border', defaults.border);
                        root.setProperty('--candidate-accent', accent);
                        root.setProperty('--candidate-selection-background', validHexColor(theme.selection_background) ? theme.selection_background : (validHexColor(theme.accent_color) ? theme.accent_color : defaults.selectionBackground));
                        root.setProperty('--candidate-selection-border', validHexColor(theme.selection_border) ? theme.selection_border : accent);
                        root.setProperty('--candidate-scrollbar', defaults.scrollbar);

                        const main = document.querySelector('main');
                        if (main) {
                            main.style.backgroundImage = theme.background_image ? `url("${theme.background_image}")` : 'none';
                        }

                        const customStyle = document.getElementById('custom-theme');
                        if (customStyle) {
                            customStyle.textContent = theme.custom_css_enabled ? (theme.custom_css || '') : '';
                        }

                        // Custom CSS can change the height of the rows.
                        if (document.readyState !== 'loading') adjustWindowSize();
                    }

                    function updateCandidates(candidates) {
                        const candidateList = document.getElementById('candidate-list');
                        const existingItems = Array.from(candidateList.children);

                        candidates.forEach((candidate, index) => {
                            if (existingItems[index]) {
                                existingItems[index].textContent = candidate;
                            } else {
                                const li = document.createElement('li');
                                li.textContent = candidate;
                                candidateList.appendChild(li);
                            }
                        });

                        while (candidateList.children.length > candidates.length) {
                            candidateList.removeChild(candidateList.lastChild);
                        }
                    }

                    function updateSelection(index) {
                        const candidateList = document.getElementById('candidate-list');
                        const selected = candidateList.querySelector('[data-selected]');
                        if (selected) selected.removeAttribute('data-selected');
                        if (!candidateList.children[index]) return;

                        candidateList.children[index].setAttribute('data-selected', '');
                        const itemHeight = candidateList.children[0]?.offsetHeight || 1;
                        const visibleItems = Math.floor(candidateList.clientHeight / itemHeight);
                        const groupSize = Math.max(1, Math.min(5, visibleItems || 5));
                        const groupIndex = Math.floor(index / groupSize);
                        const scrollToIndex = groupIndex * groupSize;
                        if (
                            index === scrollToIndex ||
                            !isElementInView(candidateList.children[index], candidateList)
                        ) {
                            candidateList.children[scrollToIndex]?.scrollIntoView({
                                behavior: "instant",
                                block: "start",
                                inline: "start"
                            });
                        }
                    }

                    function isElementInView(element, container) {
                        const containerRect = container.getBoundingClientRect();
                        const elementRect = element.getBoundingClientRect();
                        return elementRect.top >= containerRect.top &&
                            elementRect.bottom <= containerRect.bottom;
                    }

                    let reportedHeight = null;

                    // Measures the window height for five candidates with
                    // the current theme (custom CSS may change fonts and
                    // spacing) and reports it when it changed. Probe items
                    // are added and removed in the same task, so the shown
                    // candidates are kept and nothing is painted.
                    function adjustWindowSize() {
                        if (!document.body) return;
                        const candidateList = document.getElementById('candidate-list');
                        const probes = [];
                        for (let i = 0; i < 5; i++) {
                            const li = document.createElement('li');
                            li.textContent = `Item ${i + 1}`;
                            candidateList.appendChild(li);
                            probes.push(li);
                        }

                        const footer = document.querySelector('footer');
                        const main = document.querySelector('main');
                        const body = document.body;
                        const itemHeight = probes[0].getBoundingClientRect().height;
                        const candidateListHeight = itemHeight * 5;
                        const footerHeight = footer.getBoundingClientRect().height;
                        const mainStyle = window.getComputedStyle(main);
                        const bodyStyle = window.getComputedStyle(body);
                        const verticalSpace = (style) =>
                            parseFloat(style.paddingTop) + parseFloat(style.paddingBottom) +
                            parseFloat(style.borderTopWidth) + parseFloat(style.borderBottomWidth);
                        const totalHeight = Math.ceil(
                            candidateListHeight + footerHeight +
                            verticalSpace(mainStyle) + verticalSpace(bodyStyle)
                        );

                        probes.forEach((li) => li.remove());
                        if (totalHeight === reportedHeight) return;
                        reportedHeight = totalHeight;
                        window.ipc.postMessage(JSON.stringify({
                            type: 'resize',
                            height: totalHeight
                        }));
                    }

                    window.addEventListener('DOMContentLoaded', () => {
                        if (window.__candidateTheme) applyTheme(window.__candidateTheme);
                        // A theme sent before the page existed was lost.
                        window.ipc.postMessage(JSON.stringify({ type: 'ready' }));
                        setTimeout(adjustWindowSize, 50);
                        // Fonts named by custom CSS can finish loading later.
                        document.fonts.addEventListener('loadingdone', adjustWindowSize);
                    });

                    window.matchMedia('(prefers-color-scheme: dark)').addEventListener('change', () => {
                        if (window.__candidateTheme?.mode === 'system') {
                            applyTheme(window.__candidateTheme);
                        }
                    });
                </script>
            </head>
            <body>
                <main>
                    <ol id="candidate-list"></ol>
                    <footer>
                        <svg width="20" height="14" viewBox="0 0 22 16" fill="none" xmlns="http://www.w3.org/2000/svg">
                            <path d="M3.5 8C4.59202 9.04403 7.54398 10.3978 13.5068 9.93754M1.25349 5.39919C2.77722 0.413397 8.08911 0.79692 10.9673 1.24436C14.2687 1.71311 20.8969 3.82675 20.9985 8.53129C21.1255 14.412 13.1894 15.3069 10.0784 14.9233C6.96748 14.5398 -0.46071 13.0696 1.25349 5.39919Z" stroke="#838384" stroke-width="1.5" stroke-linecap="round"/>
                        </svg>
                    </footer>
                </main>
            </body>
        </html>"##,
        );

    Ok(webview_builder)
}

#[cfg(test)]
mod tests {
    use super::{theme_payload, ThemeState, MAX_BACKGROUND_IMAGE_BYTES};
    use shared::ThemeConfig;

    fn payload(theme: &ThemeConfig) -> serde_json::Value {
        let payload = ThemeState::default().payload_if_changed(theme).unwrap();
        serde_json::from_str(&payload).unwrap()
    }

    #[test]
    fn theme_payload_preserves_safe_settings() {
        let theme = ThemeConfig {
            mode: "dark".to_string(),
            background_color: "#101010".to_string(),
            text_color: "#f0f0f0".to_string(),
            accent_color: "#336699".to_string(),
            selection_background: "#223344".to_string(),
            selection_border: "#445566".to_string(),
            background_image: String::new(),
            custom_css_enabled: true,
            custom_css: "main { opacity: 0.9; }".to_string(),
        };

        let value: serde_json::Value = serde_json::from_str(&theme_payload(&theme, None)).unwrap();
        assert_eq!(value["mode"], "dark");
        assert_eq!(value["background_color"], "#101010");
        assert_eq!(value["custom_css_enabled"], true);
        assert_eq!(value["custom_css"], "main { opacity: 0.9; }");
        assert!(value["background_image"].is_null());
    }

    #[test]
    fn unsupported_or_missing_background_image_is_not_exposed() {
        let mut theme = ThemeConfig::default();
        theme.background_image = "Z:/definitely-missing/background.bmp".to_string();

        assert!(payload(&theme)["background_image"].is_null());
    }

    #[test]
    fn supported_background_image_is_embedded_as_data_url() {
        let path =
            std::env::temp_dir().join(format!("azookey-theme-test-{}.png", std::process::id()));
        std::fs::write(&path, [0x89, b'P', b'N', b'G']).unwrap();

        let mut theme = ThemeConfig::default();
        theme.background_image = path.to_string_lossy().into_owned();

        assert!(payload(&theme)["background_image"]
            .as_str()
            .is_some_and(|value| value.starts_with("data:image/png;base64,")));

        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn oversized_background_image_is_not_exposed() {
        let path = std::env::temp_dir().join(format!(
            "azookey-theme-large-test-{}.png",
            std::process::id()
        ));
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(MAX_BACKGROUND_IMAGE_BYTES + 1).unwrap();
        drop(file);

        let mut theme = ThemeConfig::default();
        theme.background_image = path.to_string_lossy().into_owned();

        assert!(payload(&theme)["background_image"].is_null());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn unchanged_theme_is_sent_once() {
        let mut state = ThemeState::default();
        let mut theme = ThemeConfig::default();
        assert!(state.payload_if_changed(&theme).is_some());
        assert!(state.payload_if_changed(&theme).is_none());

        theme.mode = "dark".to_string();
        assert!(state.payload_if_changed(&theme).is_some());
        assert!(state.payload_if_changed(&theme).is_none());

        state.forget_applied();
        assert!(state.payload_if_changed(&theme).is_some());
    }
}
