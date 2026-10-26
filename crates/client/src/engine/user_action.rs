use crate::extension::VKeyExt;
use anyhow::Result;
use windows::Win32::{
    Foundation::{LPARAM, WPARAM},
    UI::Input::KeyboardAndMouse::{GetKeyboardState, ToUnicode, VK_SHIFT},
};

#[derive(Debug)]
pub enum UserAction {
    // what the key types: usually one character, but a keyboard layout can
    // produce several, or a character outside the BMP
    Input(String),
    Backspace,
    Enter,
    Space,
    Tab,
    Escape,
    Unknown,
    Navigation(Navigation),
    Function(Function),
    Number(i8),
    ToggleInputMode,
    InputModeOn,
    InputModeOff,
}

#[derive(Debug)]
pub enum Navigation {
    Up,
    Down,
    Left,
    Right,
}

#[derive(Debug)]
pub enum Function {
    Five,
    Six,
    Seven,
    Eight,
    Nine,
    Ten,
}

/// ToUnicode flag (bit 2, Windows 10 1607+): translate without changing the
/// keyboard state, i.e. without consuming or recording a dead key.
const TRANSLATE_KEEP_STATE: u32 = 0x4;

/// Enough for any layout: ligatures are at most a few characters, each at
/// most a surrogate pair.
const TRANSLATION_BUFFER_LEN: usize = 16;

/// What ToUnicode reported for a key.
#[derive(Debug, PartialEq)]
enum Translation {
    Text(String),
    /// An accent waiting for the next key (the buffer holds its spacing form).
    DeadKey,
    None,
}

/// Decodes ToUnicode's result: `written` is its return value, `buffer` the
/// UTF-16 output.
fn decode_translation(written: i32, buffer: &[u16]) -> Translation {
    let Ok(len) = usize::try_from(written) else {
        return Translation::DeadKey;
    };
    let Some(units) = buffer.get(..len) else {
        return Translation::None;
    };
    match String::from_utf16(units) {
        Ok(text) if !text.is_empty() && !text.chars().any(char::is_control) => {
            Translation::Text(text)
        }
        _ => Translation::None,
    }
}

fn scan_code(lparam: LPARAM) -> u32 {
    ((lparam.0 as usize >> 16) & 0xFF) as u32
}

fn translate(key_code: usize, lparam: LPARAM, flags: u32) -> Result<Translation> {
    let mut key_state = [0u8; 256];
    let mut buffer = [0u16; TRANSLATION_BUFFER_LEN];
    let written = unsafe {
        GetKeyboardState(&mut key_state)?;
        ToUnicode(
            key_code as u32,
            scan_code(lparam),
            Some(&key_state),
            &mut buffer,
            flags,
        )
    };
    Ok(decode_translation(written, &buffer))
}

/// Translates a key the text service eats the way the application's
/// TranslateMessage would have, so a pending dead key is used up and does
/// not combine with a later key that reaches the application.
pub fn consume_translation(wparam: WPARAM, lparam: LPARAM) {
    if let Err(error) = translate(wparam.0, lparam, 0) {
        tracing::warn!("Failed to translate the eaten key: {error:?}");
    }
}

impl UserAction {
    /// Classifies a key. This runs for OnTestKeyDown and again for OnKeyDown,
    /// also for keys that then go to the application, so the translation
    /// must not change the keyboard state; see [`consume_translation`].
    pub fn from_key(wparam: WPARAM, lparam: LPARAM) -> Result<UserAction> {
        let key_code = wparam.0;
        let action = match key_code {
            0x08 => UserAction::Backspace, // VK_BACK
            0x09 => UserAction::Tab,       // VK_TAB
            0x0D => UserAction::Enter,     // VK_RETURN
            0x20 => UserAction::Space,     // VK_SPACE
            0x1B => UserAction::Escape,    // VK_ESCAPE

            0x25 => UserAction::Navigation(Navigation::Left), // VK_LEFT
            0x26 => UserAction::Navigation(Navigation::Up),   // VK_UP
            0x27 => UserAction::Navigation(Navigation::Right), // VK_RIGHT
            0x28 => UserAction::Navigation(Navigation::Down), // VK_DOWN

            0x30..=0x39 | 0x60..=0x69 if !VK_SHIFT.is_pressed() => {
                match key_code {
                    0x30 | 0x60 => UserAction::Number(0), // VK_0, VK_NUMPAD0
                    0x31 | 0x61 => UserAction::Number(1), // VK_1, VK_NUMPAD1
                    0x32 | 0x62 => UserAction::Number(2), // VK_2, VK_NUMPAD2
                    0x33 | 0x63 => UserAction::Number(3), // VK_3, VK_NUMPAD3
                    0x34 | 0x64 => UserAction::Number(4), // VK_4, VK_NUMPAD4
                    0x35 | 0x65 => UserAction::Number(5), // VK_5, VK_NUMPAD5
                    0x36 | 0x66 => UserAction::Number(6), // VK_6, VK_NUMPAD6
                    0x37 | 0x67 => UserAction::Number(7), // VK_7, VK_NUMPAD7
                    0x38 | 0x68 => UserAction::Number(8), // VK_8, VK_NUMPAD8
                    0x39 | 0x69 => UserAction::Number(9), // VK_9, VK_NUMPAD9
                    _ => UserAction::Unknown,
                }
            }

            0x16 => UserAction::InputModeOn,  // VK_IME_ON
            0x1A => UserAction::InputModeOff, // VK_IME_OFF

            0x74 => UserAction::Function(Function::Five), // VK_F5
            0x75 => UserAction::Function(Function::Six), // VK_F6
            0x76 => UserAction::Function(Function::Seven), // VK_F7
            0x77 => UserAction::Function(Function::Eight), // VK_F8
            0x78 => UserAction::Function(Function::Nine), // VK_F9
            0x79 => UserAction::Function(Function::Ten), // VK_F10

            0xF3 | 0xF4 => UserAction::ToggleInputMode, // Zenkaku/Hankaku

            // A dead key is left to the application: the system then
            // combines it with the next key, which arrives here translated.
            _ => match translate(key_code, lparam, TRANSLATE_KEEP_STATE)? {
                Translation::Text(text) => UserAction::Input(text),
                Translation::DeadKey | Translation::None => UserAction::Unknown,
            },
        };

        Ok(action)
    }
}

#[cfg(test)]
mod tests {
    use super::{decode_translation, Translation};

    #[test]
    fn translation_keeps_every_utf16_unit_written() {
        let mut buffer = [0u16; 16];
        let text: Vec<u16> = "'a".encode_utf16().collect();
        buffer[..text.len()].copy_from_slice(&text);
        assert_eq!(
            decode_translation(2, &buffer),
            Translation::Text("'a".to_string())
        );
        // only what the return value covers
        assert_eq!(
            decode_translation(1, &buffer),
            Translation::Text("'".to_string())
        );

        let emoji: Vec<u16> = "😀".encode_utf16().collect();
        assert_eq!(
            decode_translation(2, &emoji),
            Translation::Text("😀".to_string())
        );
    }

    #[test]
    fn dead_keys_and_unusable_output_are_not_input() {
        let accent: Vec<u16> = "´".encode_utf16().collect();
        assert_eq!(decode_translation(-1, &accent), Translation::DeadKey);
        assert_eq!(decode_translation(0, &[0; 4]), Translation::None);
        // half of a surrogate pair
        assert_eq!(decode_translation(1, &[0xD83D]), Translation::None);
        // control characters such as Ctrl+letter
        assert_eq!(decode_translation(1, &[0x01]), Translation::None);
        assert_eq!(decode_translation(4, &[0x61]), Translation::None);
    }
}
