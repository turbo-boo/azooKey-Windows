use crate::{
    engine::user_action::UserAction,
    extension::VKeyExt as _,
    tsf::factory::{TextServiceFactory, TextServiceFactory_Impl},
};

use super::{
    client_action::{ClientAction, SetSelectionType, SetTextType},
    full_width::{to_fullwidth, to_halfwidth},
    input_mode::InputMode,
    ipc_service::Candidates,
    state::IMEState,
    text_util::{to_half_katakana, to_katakana},
    user_action::{Function, Navigation},
};
use shared::AppConfig;
use windows::Win32::{
    Foundation::{LPARAM, WPARAM},
    UI::{
        Input::KeyboardAndMouse::{
            VK_CONTROL, VK_LCONTROL, VK_LMENU, VK_MENU, VK_RCONTROL, VK_RMENU,
        },
        TextServices::{ITfComposition, ITfCompositionSink_Impl, ITfContext},
    },
};

use anyhow::Result;

#[derive(Default, Clone, PartialEq, Debug)]
pub enum CompositionState {
    #[default]
    None,
    Composing,
    Previewing,
    Selecting,
}

#[derive(Default, Clone, Debug)]
pub struct Composition {
    pub preview: String, // text to be previewed
    pub suffix: String,  // text to be appended after preview
    pub raw_input: String,
    pub raw_hiragana: String,

    pub corresponding_count: i32, // corresponding count of the preview

    pub selection_index: i32,
    pub candidates: Candidates,
    pub should_learn_candidate: bool,

    pub state: CompositionState,
    pub tip_composition: Option<ITfComposition>,
}

impl ITfCompositionSink_Impl for TextServiceFactory_Impl {
    #[macros::anyhow]
    fn OnCompositionTerminated(
        &self,
        _ecwrite: u32,
        _pcomposition: Option<&ITfComposition>,
    ) -> Result<()> {
        // if user clicked outside the composition, the composition will be terminated
        tracing::debug!("OnCompositionTerminated");

        let actions = vec![ClientAction::EndComposition];
        self.handle_action(&actions, CompositionState::None)?;

        Ok(())
    }
}

fn conversion_key_action(is_converted: bool) -> ClientAction {
    if is_converted {
        ClientAction::SetSelection(SetSelectionType::Down)
    } else {
        ClientAction::RequestConversion
    }
}

fn candidate_is_learnable(candidates: &Candidates, index: usize) -> bool {
    candidates.is_converted
        && candidates
            .candidate_ids
            .get(index)
            .is_some_and(|id| *id >= 0)
}

fn candidate_parts(candidates: &Candidates, index: i32) -> Option<(String, String, i32)> {
    let index = usize::try_from(index).ok()?;
    Some((
        candidates.texts.get(index)?.clone(),
        candidates.sub_texts.get(index)?.clone(),
        *candidates.corresponding_count.get(index)?,
    ))
}

fn next_selection_index(
    candidate_count: usize,
    current: i32,
    selection: &SetSelectionType,
) -> Option<i32> {
    if candidate_count == 0 {
        return None;
    }
    let last = i32::try_from(candidate_count - 1).ok()?;
    Some(match selection {
        SetSelectionType::Up => (current - 1).clamp(0, last),
        SetSelectionType::Down => (current + 1).clamp(0, last),
        SetSelectionType::Number(number) => (*number).clamp(0, last),
    })
}

fn is_ctrl_pressed() -> bool {
    VK_CONTROL.is_pressed() || VK_LCONTROL.is_pressed() || VK_RCONTROL.is_pressed()
}

fn is_alt_backquote_key(wparam: WPARAM, lparam: LPARAM, alt_pressed: bool) -> bool {
    const VK_OEM_3: usize = 0xC0;
    const SCAN_CODE_BACKQUOTE: usize = 0x29;
    const ALT_CONTEXT_BIT: usize = 0x2000_0000;

    let alt_context = ((lparam.0 as usize) & ALT_CONTEXT_BIT) != 0;
    let scan_code = ((lparam.0 as usize) >> 16) & 0xFF;
    let is_backquote = wparam.0 == VK_OEM_3 || scan_code == SCAN_CODE_BACKQUOTE;
    (alt_pressed || alt_context) && is_backquote
}

pub(crate) fn ctrl_space_shortcut_enabled(wparam: WPARAM) -> bool {
    if !is_ctrl_pressed() || wparam.0 != 0x20 {
        return false;
    }
    AppConfig::read().shortcuts.ctrl_space_toggle
}

impl TextServiceFactory {
    #[tracing::instrument]
    pub fn has_active_composition(&self) -> Result<bool> {
        let text_service = self.borrow()?;
        let active = {
            let composition = text_service.borrow_composition()?;
            composition.state != CompositionState::None
        };
        Ok(active)
    }

    pub fn process_key(
        &self,
        context: Option<&ITfContext>,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> Result<Option<(Vec<ClientAction>, CompositionState)>> {
        if context.is_none() {
            return Ok(None);
        };

        // Without the server nothing can be converted; let the key reach the app.
        if let Err(error) = IMEState::ipc_service() {
            tracing::debug!("Passing key through, IPC service unavailable: {error:?}");
            return Ok(None);
        }

        let ctrl_pressed = is_ctrl_pressed();
        let ctrl_space = ctrl_pressed && wparam.0 == 0x20;
        let alt_pressed = VK_MENU.is_pressed() || VK_LMENU.is_pressed() || VK_RMENU.is_pressed();
        let alt_backquote = !ctrl_pressed && is_alt_backquote_key(wparam, lparam, alt_pressed);

        if ctrl_space || alt_backquote {
            let shortcuts = AppConfig::read().shortcuts;
            if ctrl_space && !shortcuts.ctrl_space_toggle {
                return Ok(None);
            }
            if alt_backquote && !shortcuts.alt_backquote_toggle {
                return Ok(None);
            }
        }

        // Other Ctrl shortcuts are passed back to the application. key_event_sink
        // cancels an active composition first.
        if ctrl_pressed && !ctrl_space {
            return Ok(None);
        }

        #[allow(clippy::let_and_return)]
        let (composition, mode) = {
            let text_service = self.borrow()?;
            let composition = text_service.borrow_composition()?.clone();
            let mode = IMEState::get()?.input_mode.clone();
            (composition, mode)
        };

        let action = if ctrl_space || alt_backquote {
            UserAction::ToggleInputMode
        } else {
            UserAction::try_from(wparam.0)?
        };

        let explicit_input_mode = match &action {
            UserAction::InputModeOn => Some(InputMode::Kana),
            UserAction::InputModeOff => Some(InputMode::Latin),
            _ => None,
        };
        if let Some(target_mode) = explicit_input_mode {
            let actions = match composition.state {
                CompositionState::None => vec![ClientAction::SetIMEMode(target_mode)],
                CompositionState::Selecting => vec![
                    ClientAction::DismissPredictions,
                    ClientAction::SetIMEMode(target_mode),
                ],
                CompositionState::Composing | CompositionState::Previewing => vec![
                    ClientAction::EndComposition,
                    ClientAction::SetIMEMode(target_mode),
                ],
            };
            return Ok(Some((actions, CompositionState::None)));
        }

        let (transition, actions) = match composition.state {
            CompositionState::None => match action {
                UserAction::Input(char) if mode == InputMode::Kana => (
                    CompositionState::Composing,
                    vec![
                        ClientAction::StartComposition,
                        ClientAction::AppendText(char.to_string()),
                    ],
                ),
                UserAction::Number(number) if mode == InputMode::Kana => (
                    CompositionState::Composing,
                    vec![
                        ClientAction::StartComposition,
                        ClientAction::AppendText(number.to_string()),
                    ],
                ),
                UserAction::ToggleInputMode => (
                    CompositionState::None,
                    vec![match mode {
                        InputMode::Kana => ClientAction::SetIMEMode(InputMode::Latin),
                        InputMode::Latin => ClientAction::SetIMEMode(InputMode::Kana),
                    }],
                ),
                _ => {
                    return Ok(None);
                }
            },
            CompositionState::Composing => match action {
                UserAction::Input(char) => (
                    CompositionState::Composing,
                    vec![ClientAction::AppendText(char.to_string())],
                ),
                UserAction::Number(number) => (
                    CompositionState::Composing,
                    vec![ClientAction::AppendText(number.to_string())],
                ),
                UserAction::Backspace => (
                    CompositionState::Composing,
                    vec![ClientAction::RemoveText],
                ),
                UserAction::Enter => {
                    if composition.suffix.is_empty() {
                        if composition.should_learn_candidate {
                            (
                                CompositionState::Selecting,
                                vec![
                                    ClientAction::CommitCandidate,
                                    ClientAction::EndComposition,
                                    ClientAction::LoadPredictions,
                                ],
                            )
                        } else {
                            (
                                CompositionState::None,
                                vec![ClientAction::CommitCandidate, ClientAction::EndComposition],
                            )
                        }
                    } else {
                        (
                            CompositionState::Composing,
                            vec![
                                ClientAction::CommitCandidate,
                                ClientAction::ShrinkText("".to_string()),
                            ],
                        )
                    }
                }
                UserAction::Escape => (
                    CompositionState::None,
                    vec![ClientAction::CancelComposition],
                ),
                UserAction::Navigation(direction) => match direction {
                    Navigation::Right => (
                        CompositionState::Composing,
                        vec![ClientAction::MoveCursor(1)],
                    ),
                    Navigation::Left => (
                        CompositionState::Composing,
                        vec![ClientAction::MoveCursor(-1)],
                    ),
                    Navigation::Up => (
                        CompositionState::Previewing,
                        vec![ClientAction::SetSelection(SetSelectionType::Up)],
                    ),
                    Navigation::Down => (
                        CompositionState::Previewing,
                        vec![ClientAction::SetSelection(SetSelectionType::Down)],
                    ),
                },
                UserAction::ToggleInputMode => (
                    CompositionState::None,
                    vec![
                        ClientAction::EndComposition,
                        ClientAction::SetIMEMode(InputMode::Latin),
                    ],
                ),
                UserAction::Space | UserAction::Tab => (
                    CompositionState::Previewing,
                    vec![conversion_key_action(composition.candidates.is_converted)],
                ),
                UserAction::Function(key) => match key {
                    Function::Five => (
                        CompositionState::Previewing,
                        vec![ClientAction::MagicConvert],
                    ),
                    Function::Six => (
                        CompositionState::Previewing,
                        vec![ClientAction::SetTextWithType(SetTextType::Hiragana)],
                    ),
                    Function::Seven => (
                        CompositionState::Previewing,
                        vec![ClientAction::SetTextWithType(SetTextType::Katakana)],
                    ),
                    Function::Eight => (
                        CompositionState::Previewing,
                        vec![ClientAction::SetTextWithType(SetTextType::HalfKatakana)],
                    ),
                    Function::Nine => (
                        CompositionState::Previewing,
                        vec![ClientAction::SetTextWithType(SetTextType::FullLatin)],
                    ),
                    Function::Ten => (
                        CompositionState::Previewing,
                        vec![ClientAction::SetTextWithType(SetTextType::HalfLatin)],
                    ),
                },
                _ => {
                    return Ok(None);
                }
            },
            CompositionState::Previewing => match action {
                UserAction::Input(char) => (
                    CompositionState::Composing,
                    vec![
                        ClientAction::CommitCandidate,
                        ClientAction::ShrinkText(char.to_string()),
                    ],
                ),
                UserAction::Number(number) => (
                    CompositionState::Composing,
                    vec![
                        ClientAction::CommitCandidate,
                        ClientAction::ShrinkText(number.to_string()),
                    ],
                ),
                UserAction::Backspace => (
                    CompositionState::Composing,
                    vec![ClientAction::RemoveText],
                ),
                UserAction::Enter => {
                    if composition.suffix.is_empty() {
                        if composition.should_learn_candidate {
                            (
                                CompositionState::Selecting,
                                vec![
                                    ClientAction::CommitCandidate,
                                    ClientAction::EndComposition,
                                    ClientAction::LoadPredictions,
                                ],
                            )
                        } else {
                            (
                                CompositionState::None,
                                vec![ClientAction::CommitCandidate, ClientAction::EndComposition],
                            )
                        }
                    } else {
                        (
                            CompositionState::Composing,
                            vec![
                                ClientAction::CommitCandidate,
                                ClientAction::ShrinkText("".to_string()),
                            ],
                        )
                    }
                }
                UserAction::Escape => (
                    CompositionState::None,
                    vec![ClientAction::CancelComposition],
                ),
                UserAction::Navigation(direction) => match direction {
                    Navigation::Right => (
                        CompositionState::Composing,
                        vec![ClientAction::MoveCursor(1)],
                    ),
                    Navigation::Left => (
                        CompositionState::Composing,
                        vec![ClientAction::MoveCursor(-1)],
                    ),
                    Navigation::Up => (
                        CompositionState::Previewing,
                        vec![ClientAction::SetSelection(SetSelectionType::Up)],
                    ),
                    Navigation::Down => (
                        CompositionState::Previewing,
                        vec![ClientAction::SetSelection(SetSelectionType::Down)],
                    ),
                },
                UserAction::ToggleInputMode => (
                    CompositionState::None,
                    vec![
                        ClientAction::EndComposition,
                        ClientAction::SetIMEMode(InputMode::Latin),
                    ],
                ),
                UserAction::Space | UserAction::Tab => (
                    CompositionState::Previewing,
                    vec![conversion_key_action(composition.candidates.is_converted)],
                ),
                UserAction::Function(key) => match key {
                    Function::Five => (
                        CompositionState::Previewing,
                        vec![ClientAction::MagicConvert],
                    ),
                    Function::Six => (
                        CompositionState::Previewing,
                        vec![ClientAction::SetTextWithType(SetTextType::Hiragana)],
                    ),
                    Function::Seven => (
                        CompositionState::Previewing,
                        vec![ClientAction::SetTextWithType(SetTextType::Katakana)],
                    ),
                    Function::Eight => (
                        CompositionState::Previewing,
                        vec![ClientAction::SetTextWithType(SetTextType::HalfKatakana)],
                    ),
                    Function::Nine => (
                        CompositionState::Previewing,
                        vec![ClientAction::SetTextWithType(SetTextType::FullLatin)],
                    ),
                    Function::Ten => (
                        CompositionState::Previewing,
                        vec![ClientAction::SetTextWithType(SetTextType::HalfLatin)],
                    ),
                },
                _ => {
                    return Ok(None);
                }
            },
            CompositionState::Selecting => match action {
                UserAction::Input(char) if mode == InputMode::Kana => (
                    CompositionState::Composing,
                    vec![
                        ClientAction::DismissPredictions,
                        ClientAction::StartComposition,
                        ClientAction::AppendText(char.to_string()),
                    ],
                ),
                UserAction::Number(number) if mode == InputMode::Kana => (
                    CompositionState::Composing,
                    vec![
                        ClientAction::DismissPredictions,
                        ClientAction::StartComposition,
                        ClientAction::AppendText(number.to_string()),
                    ],
                ),
                UserAction::Enter => (
                    CompositionState::Selecting,
                    vec![ClientAction::AcceptPrediction],
                ),
                UserAction::Space | UserAction::Tab => (
                    CompositionState::Selecting,
                    vec![ClientAction::SetSelection(SetSelectionType::Down)],
                ),
                UserAction::Navigation(Navigation::Up) => (
                    CompositionState::Selecting,
                    vec![ClientAction::SetSelection(SetSelectionType::Up)],
                ),
                UserAction::Navigation(Navigation::Down) => (
                    CompositionState::Selecting,
                    vec![ClientAction::SetSelection(SetSelectionType::Down)],
                ),
                UserAction::Escape | UserAction::Backspace => (
                    CompositionState::None,
                    vec![ClientAction::DismissPredictions],
                ),
                UserAction::ToggleInputMode => (
                    CompositionState::None,
                    vec![
                        ClientAction::DismissPredictions,
                        ClientAction::SetIMEMode(match mode {
                            InputMode::Kana => InputMode::Latin,
                            InputMode::Latin => InputMode::Kana,
                        }),
                    ],
                ),
                _ => {
                    return Ok(None);
                }
            },
        };

        Ok(Some((actions, transition)))
    }

    #[tracing::instrument]
    pub fn handle_key(
        &self,
        context: Option<&ITfContext>,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> Result<bool> {
        if let Some(context) = context {
            self.borrow_mut()?.context = Some(context.clone());
        } else {
            return Ok(false);
        };

        if let Some((actions, transition)) = self.process_key(context, wparam, lparam)? {
            self.handle_action(&actions, transition)?;
        } else {
            return Ok(false);
        }

        Ok(true)
    }

    #[tracing::instrument]
    pub fn handle_action(
        &self,
        actions: &[ClientAction],
        transition: CompositionState,
    ) -> Result<()> {
        #[allow(clippy::let_and_return)]
        let (composition, mode) = {
            let text_service = self.borrow()?;
            let composition = text_service.borrow_composition()?.clone();
            let mode = IMEState::get()?.input_mode.clone();
            (composition, mode)
        };

        let mut preview = composition.preview.clone();
        let mut suffix = composition.suffix.clone();
        let mut raw_input = composition.raw_input.clone();
        let mut raw_hiragana = composition.raw_hiragana.clone();
        let mut corresponding_count = composition.corresponding_count.clone();
        let mut candidates = composition.candidates.clone();
        let mut should_learn_candidate = composition.should_learn_candidate;
        let mut selection_index = composition.selection_index;
        let mut ipc_service = IMEState::ipc_service()?;
        let mut transition = transition;

        // The text preceding the composition only changes when a composition
        // starts or a prefix is committed (ShrinkText), so do not pay an edit
        // session and a server round trip for it on every keystroke.
        if actions.contains(&ClientAction::StartComposition) {
            self.update_context(&preview)?;
        }

        for action in actions {
            match action {
                ClientAction::StartComposition => {
                    self.start_composition()?;
                    self.update_pos()?;
                }
                ClientAction::EndComposition => {
                    self.end_composition()?;
                    selection_index = 0;
                    corresponding_count = 0;
                    preview.clear();
                    suffix.clear();
                    raw_input.clear();
                    raw_hiragana.clear();
                    candidates = Candidates::default();
                    should_learn_candidate = false;
                    ipc_service.hide_window()?;
                    ipc_service.set_candidates(vec![])?;
                    ipc_service.clear_text()?;
                }
                ClientAction::CancelComposition => {
                    // Remove the marked text before ending the TSF composition so
                    // shortcut keys and Escape do not commit a partial conversion.
                    self.set_text("", "")?;
                    self.end_composition()?;

                    selection_index = 0;
                    corresponding_count = 0;
                    preview.clear();
                    suffix.clear();
                    raw_input.clear();
                    raw_hiragana.clear();
                    candidates = Candidates::default();
                    should_learn_candidate = false;
                    transition = CompositionState::None;

                    ipc_service.hide_window()?;
                    ipc_service.set_candidates(vec![])?;
                    ipc_service.clear_text()?;
                    if let Err(error) = ipc_service.clear_post_composition_predictions() {
                        tracing::warn!("Failed to clear prediction state while cancelling: {error:?}");
                    }
                }
                ClientAction::AppendText(text) => {
                    raw_input.push_str(&text);

                    let text = match mode {
                        InputMode::Kana => to_fullwidth(text, false),
                        InputMode::Latin => text.to_string(),
                    };

                    candidates = ipc_service.append_text(text.clone())?;
                    let hiragana = candidates.hiragana.clone();
                    let Some((text, sub_text, count)) =
                        candidate_parts(&candidates, selection_index)
                    else {
                        selection_index = 0;
                        corresponding_count = 0;
                        preview = hiragana.clone();
                        suffix.clear();
                        raw_hiragana = hiragana;
                        should_learn_candidate = false;
                        self.set_text(&preview, "")?;
                        ipc_service.hide_window()?;
                        ipc_service.set_candidates(vec![])?;
                        continue;
                    };

                    corresponding_count = count;
                    preview = text.clone();
                    suffix = sub_text.clone();
                    raw_hiragana = hiragana.clone();
                    should_learn_candidate =
                        candidate_is_learnable(&candidates, selection_index as usize);

                    self.set_text(&text, &sub_text)?;
                    if candidates.is_converted {
                        ipc_service.set_candidates(candidates.texts.clone())?;
                        ipc_service.set_selection(selection_index as i32)?;
                        ipc_service.show_window()?;
                    } else {
                        ipc_service.hide_window()?;
                        ipc_service.set_candidates(vec![])?;
                    }
                }
                ClientAction::RequestConversion => {
                    let converted = ipc_service.request_conversion()?;
                    if converted.texts.is_empty() {
                        continue;
                    }

                    candidates = converted;
                    selection_index = 0;
                    let Some((text, sub_text, count)) = candidate_parts(&candidates, 0) else {
                        ipc_service.hide_window()?;
                        ipc_service.set_candidates(vec![])?;
                        continue;
                    };
                    corresponding_count = count;
                    preview = text.clone();
                    suffix = sub_text.clone();
                    if !candidates.hiragana.is_empty() {
                        raw_hiragana = candidates.hiragana.clone();
                    }
                    should_learn_candidate = candidates
                        .candidate_ids
                        .first()
                        .is_some_and(|id| *id >= 0);

                    self.set_text(&text, &sub_text)?;
                    ipc_service.set_candidates(candidates.texts.clone())?;
                    ipc_service.set_selection(0)?;
                    ipc_service.show_window()?;
                    transition = CompositionState::Previewing;
                }
                ClientAction::CommitCandidate => {
                    if should_learn_candidate {
                        if let Some(candidate_id) =
                            candidates.candidate_ids.get(selection_index as usize)
                        {
                            ipc_service.commit_candidate(*candidate_id)?;
                        }
                    }
                    should_learn_candidate = false;
                }
                ClientAction::LoadPredictions => {
                    let predictions = match ipc_service.get_post_composition_predictions() {
                        Ok(predictions) => predictions,
                        Err(error) => {
                            tracing::warn!("Failed to load post-composition predictions: {error:?}");
                            candidates = Candidates::default();
                            transition = CompositionState::None;
                            ipc_service.hide_window()?;
                            ipc_service.set_candidates(vec![])?;
                            continue;
                        }
                    };
                    if predictions.texts.is_empty() {
                        candidates = Candidates::default();
                        transition = CompositionState::None;
                        ipc_service.hide_window()?;
                        ipc_service.set_candidates(vec![])?;
                    } else {
                        candidates = Candidates {
                            texts: predictions.texts.clone(),
                            sub_texts: vec![String::new(); predictions.texts.len()],
                            hiragana: String::new(),
                            corresponding_count: vec![0; predictions.texts.len()],
                            candidate_ids: predictions.prediction_ids,
                            is_converted: true,
                        };
                        selection_index = 0;
                        preview.clear();
                        suffix.clear();
                        raw_input.clear();
                        raw_hiragana.clear();
                        corresponding_count = 0;
                        should_learn_candidate = false;
                        ipc_service.set_candidates(candidates.texts.clone())?;
                        ipc_service.set_selection(0)?;
                        ipc_service.show_window()?;
                        transition = CompositionState::Selecting;
                    }
                }
                ClientAction::AcceptPrediction => {
                    let Some(text) = candidates.texts.get(selection_index as usize).cloned() else {
                        transition = CompositionState::None;
                        ipc_service.hide_window()?;
                        continue;
                    };
                    let Some(prediction_id) = candidates.candidate_ids.get(selection_index as usize).copied() else {
                        transition = CompositionState::None;
                        ipc_service.hide_window()?;
                        continue;
                    };

                    self.start_composition()?;
                    self.set_text(&text, "")?;
                    self.update_pos()?;
                    self.end_composition()?;

                    let predictions = match ipc_service.commit_prediction(prediction_id) {
                        Ok(predictions) => predictions,
                        Err(error) => {
                            tracing::warn!("Failed to update post-composition predictions: {error:?}");
                            candidates = Candidates::default();
                            selection_index = 0;
                            ipc_service.hide_window()?;
                            ipc_service.set_candidates(vec![])?;
                            transition = CompositionState::None;
                            continue;
                        }
                    };
                    if predictions.texts.is_empty() {
                        candidates = Candidates::default();
                        selection_index = 0;
                        ipc_service.hide_window()?;
                        ipc_service.set_candidates(vec![])?;
                        transition = CompositionState::None;
                    } else {
                        candidates = Candidates {
                            texts: predictions.texts.clone(),
                            sub_texts: vec![String::new(); predictions.texts.len()],
                            hiragana: String::new(),
                            corresponding_count: vec![0; predictions.texts.len()],
                            candidate_ids: predictions.prediction_ids,
                            is_converted: true,
                        };
                        selection_index = 0;
                        ipc_service.set_candidates(candidates.texts.clone())?;
                        ipc_service.set_selection(0)?;
                        ipc_service.show_window()?;
                        transition = CompositionState::Selecting;
                    }
                    preview.clear();
                    suffix.clear();
                    raw_input.clear();
                    raw_hiragana.clear();
                    corresponding_count = 0;
                    should_learn_candidate = false;
                }
                ClientAction::DismissPredictions => {
                    ipc_service.clear_post_composition_predictions()?;
                    candidates = Candidates::default();
                    selection_index = 0;
                    preview.clear();
                    suffix.clear();
                    raw_input.clear();
                    raw_hiragana.clear();
                    corresponding_count = 0;
                    should_learn_candidate = false;
                    ipc_service.hide_window()?;
                    ipc_service.set_candidates(vec![])?;
                }
                ClientAction::MagicConvert => {
                    let source = if preview.is_empty() {
                        raw_hiragana.clone()
                    } else {
                        format!("{preview}{suffix}")
                    };
                    let converted = match ipc_service.magic_convert(source) {
                        Ok(converted) => converted,
                        Err(error) => {
                            tracing::warn!("Magic conversion failed: {error:?}");
                            continue;
                        }
                    };
                    if !converted.is_empty() {
                        let count = raw_hiragana.chars().count() as i32;
                        candidates = Candidates {
                            texts: converted.clone(),
                            sub_texts: vec![String::new(); converted.len()],
                            hiragana: raw_hiragana.clone(),
                            corresponding_count: vec![count; converted.len()],
                            candidate_ids: vec![],
                            is_converted: true,
                        };
                        selection_index = 0;
                        corresponding_count = count;
                        preview = converted[0].clone();
                        suffix.clear();
                        should_learn_candidate = false;
                        self.set_text(&preview, "")?;
                        ipc_service.set_candidates(candidates.texts.clone())?;
                        ipc_service.set_selection(0)?;
                        ipc_service.show_window()?;
                    }
                }
                ClientAction::RemoveText => {
                    candidates = ipc_service.remove_text()?;
                    let hiragana = candidates.hiragana.clone();

                    // The visible candidate can have a different character count
                    // from the converter input. End the composition only after the
                    // converter itself reports that no input remains.
                    if hiragana.is_empty() {
                        self.set_text("", "")?;
                        self.end_composition()?;

                        selection_index = 0;
                        corresponding_count = 0;
                        preview.clear();
                        suffix.clear();
                        raw_input.clear();
                        raw_hiragana.clear();
                        candidates = Candidates::default();
                        should_learn_candidate = false;
                        transition = CompositionState::None;

                        ipc_service.hide_window()?;
                        ipc_service.set_candidates(vec![])?;
                        ipc_service.clear_text()?;
                        continue;
                    }

                    // The converter returns a fresh candidate list, so the
                    // previous selection index does not apply to it.
                    selection_index = 0;
                    let Some((text, sub_text, count)) =
                        candidate_parts(&candidates, selection_index)
                    else {
                        corresponding_count = 0;
                        preview = hiragana.clone();
                        suffix.clear();
                        raw_hiragana = hiragana;
                        should_learn_candidate = false;
                        self.set_text(&preview, "")?;
                        ipc_service.hide_window()?;
                        ipc_service.set_candidates(vec![])?;
                        continue;
                    };
                    corresponding_count = count;

                    raw_input = raw_input
                        .chars()
                        .take(corresponding_count as usize)
                        .collect();
                    preview = text.clone();
                    suffix = sub_text.clone();
                    raw_hiragana = hiragana;
                    should_learn_candidate = candidate_is_learnable(&candidates, selection_index as usize);

                    self.set_text(&text, &sub_text)?;
                    if candidates.is_converted {
                        ipc_service.set_candidates(candidates.texts.clone())?;
                        ipc_service.set_selection(selection_index as i32)?;
                        ipc_service.show_window()?;
                    } else {
                        ipc_service.hide_window()?;
                        ipc_service.set_candidates(vec![])?;
                    }
                }
                ClientAction::MoveCursor(_offset) => {
                    // TODO: I'll use azookey-kkc's composingText
                    // self.set_cursor(offset)?;
                }
                ClientAction::SetIMEMode(mode) => {
                    self.start_composition()?;
                    self.update_pos()?;
                    self.end_composition()?;

                    let mut ime_state = IMEState::get()?;
                    ime_state.input_mode = mode.clone();

                    // update the language bar
                    self.update_lang_bar()?;

                    let mode = match mode {
                        InputMode::Latin => "A",
                        InputMode::Kana => "あ",
                    };

                    ipc_service.set_input_mode(mode)?;

                    selection_index = 0;
                    corresponding_count = 0;
                    preview.clear();
                    suffix.clear();
                    raw_input.clear();
                    raw_hiragana.clear();
                    should_learn_candidate = false;
                    ipc_service.clear_text()?;
                }
                ClientAction::SetSelection(selection) => {
                    let candidates = {
                        let text_service = self.borrow()?;
                        let composition = text_service.borrow_composition()?.clone();
                        let candidates = composition.candidates.clone();
                        candidates
                    };

                    let texts = candidates.texts.clone();

                    let Some(next_index) =
                        next_selection_index(texts.len(), selection_index, selection)
                    else {
                        selection_index = 0;
                        should_learn_candidate = false;
                        ipc_service.hide_window()?;
                        ipc_service.set_candidates(vec![])?;
                        continue;
                    };
                    selection_index = next_index;

                    ipc_service.set_selection(selection_index)?;
                    if composition.state != CompositionState::Selecting {
                        let Some((text, sub_text, count)) =
                            candidate_parts(&candidates, selection_index)
                        else {
                            should_learn_candidate = false;
                            ipc_service.hide_window()?;
                            ipc_service.set_candidates(vec![])?;
                            continue;
                        };
                        let hiragana = candidates.hiragana.clone();
                        corresponding_count = count;

                        preview = text.clone();
                        suffix = sub_text.clone();
                        raw_hiragana = hiragana.clone();
                        should_learn_candidate =
                            candidate_is_learnable(&candidates, selection_index as usize);

                        self.set_text(&text, &sub_text)?;
                    }
                }
                ClientAction::ShrinkText(text) => {
                    // shrink text
                    raw_input.push_str(&text);
                    raw_input = raw_input
                        .chars()
                        .skip(corresponding_count as usize)
                        .collect();

                    let shrunk = ipc_service.shrink_text(corresponding_count)?;
                    let text = match mode {
                        InputMode::Kana => to_fullwidth(text, false),
                        InputMode::Latin => text.to_string(),
                    };
                    candidates = if text.is_empty() {
                        shrunk
                    } else {
                        ipc_service.append_text(text)?
                    };
                    selection_index = 0;

                    let hiragana = candidates.hiragana.clone();
                    let Some((text, sub_text, count)) =
                        candidate_parts(&candidates, selection_index)
                    else {
                        corresponding_count = 0;
                        let remaining = hiragana.clone();
                        self.shift_start(&preview, &remaining)?;
                        preview = remaining;
                        self.update_context(&preview)?;
                        suffix.clear();
                        raw_hiragana = hiragana;
                        should_learn_candidate = false;
                        ipc_service.hide_window()?;
                        ipc_service.set_candidates(vec![])?;
                        transition = CompositionState::Composing;
                        continue;
                    };
                    self.shift_start(&preview, &text)?;

                    corresponding_count = count;
                    preview = text.clone();
                    self.update_context(&preview)?;
                    suffix = sub_text.clone();
                    raw_hiragana = hiragana.clone();
                    should_learn_candidate =
                        candidate_is_learnable(&candidates, selection_index as usize);

                    if candidates.is_converted {
                        ipc_service.set_candidates(candidates.texts.clone())?;
                        ipc_service.set_selection(selection_index as i32)?;
                        ipc_service.show_window()?;
                    } else {
                        ipc_service.hide_window()?;
                        ipc_service.set_candidates(vec![])?;
                    }
                    self.update_pos()?;

                    transition = CompositionState::Composing;
                }
                ClientAction::SetTextWithType(set_type) => {
                    let text = match set_type {
                        SetTextType::Hiragana => raw_hiragana.clone(),
                        SetTextType::Katakana => to_katakana(&raw_hiragana),
                        SetTextType::HalfKatakana => to_half_katakana(&raw_hiragana),
                        SetTextType::FullLatin => to_fullwidth(&raw_input, true),
                        SetTextType::HalfLatin => to_halfwidth(&raw_input),
                    };

                    should_learn_candidate = false;
                    self.set_text(&text, "")?;
                }
            }
        }

        let text_service = self.borrow()?;
        let mut composition = text_service.borrow_mut_composition()?;

        composition.preview = preview.clone();
        composition.state = transition;
        composition.selection_index = selection_index;
        composition.raw_input = raw_input.clone();
        composition.raw_hiragana = raw_hiragana.clone();
        composition.candidates = candidates;
        composition.should_learn_candidate = should_learn_candidate;
        composition.suffix = suffix.clone();
        composition.corresponding_count = corresponding_count;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        candidate_is_learnable, candidate_parts, conversion_key_action, is_alt_backquote_key,
        next_selection_index,
    };
    use crate::engine::{
        client_action::{ClientAction, SetSelectionType},
        ipc_service::Candidates,
    };
    use windows::Win32::Foundation::{LPARAM, WPARAM};

    #[test]
    fn first_conversion_key_requests_candidates_when_live_conversion_is_off() {
        assert_eq!(
            conversion_key_action(false),
            ClientAction::RequestConversion
        );
        assert_eq!(
            conversion_key_action(true),
            ClientAction::SetSelection(SetSelectionType::Down)
        );
    }

    #[test]
    fn alt_backquote_accepts_vk_or_scan_code_with_alt_context() {
        const ALT_CONTEXT: isize = 0x2000_0000;
        const BACKQUOTE_SCAN: isize = 0x29 << 16;

        assert!(is_alt_backquote_key(WPARAM(0xC0), LPARAM(ALT_CONTEXT), false));
        assert!(is_alt_backquote_key(
            WPARAM(0),
            LPARAM(ALT_CONTEXT | BACKQUOTE_SCAN),
            false,
        ));
        assert!(!is_alt_backquote_key(WPARAM(0xC0), LPARAM(0), false));
    }

    #[test]
    fn candidate_parts_rejects_missing_or_misaligned_candidates() {
        let empty = Candidates::default();
        assert!(candidate_parts(&empty, 0).is_none());

        let malformed = Candidates {
            texts: vec!["候補".to_string()],
            sub_texts: vec![],
            corresponding_count: vec![1],
            ..Candidates::default()
        };
        assert!(candidate_parts(&malformed, 0).is_none());
        assert!(candidate_parts(&malformed, -1).is_none());
    }

    #[test]
    fn selection_index_is_clamped_and_empty_lists_are_rejected() {
        assert_eq!(next_selection_index(0, 0, &SetSelectionType::Down), None);
        assert_eq!(
            next_selection_index(3, 2, &SetSelectionType::Down),
            Some(2)
        );
        assert_eq!(next_selection_index(3, 0, &SetSelectionType::Up), Some(0));
        assert_eq!(
            next_selection_index(3, 0, &SetSelectionType::Number(99)),
            Some(2)
        );
    }

    #[test]
    fn synthetic_raw_candidate_is_never_learnable() {
        let raw = Candidates {
            texts: vec!["かな".to_string()],
            sub_texts: vec![String::new()],
            hiragana: "かな".to_string(),
            corresponding_count: vec![2],
            candidate_ids: vec![-1],
            is_converted: false,
        };
        assert!(!candidate_is_learnable(&raw, 0));

        let converted = Candidates {
            is_converted: true,
            candidate_ids: vec![0],
            ..raw
        };
        assert!(candidate_is_learnable(&converted, 0));
    }
}
