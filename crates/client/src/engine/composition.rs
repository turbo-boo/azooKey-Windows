use std::cmp::{max, min};

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
use windows::Win32::{
    Foundation::WPARAM,
    UI::{
        Input::KeyboardAndMouse::VK_CONTROL,
        TextServices::{ITfComposition, ITfCompositionSink_Impl, ITfContext},
    },
};

use anyhow::{Context, Result};

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

impl TextServiceFactory {
    #[tracing::instrument]
    pub fn has_active_composition(&self) -> Result<bool> {
        let text_service = self.borrow()?;
        Ok(text_service.borrow_composition()?.state != CompositionState::None)
    }

    pub fn process_key(
        &self,
        context: Option<&ITfContext>,
        wparam: WPARAM,
    ) -> Result<Option<(Vec<ClientAction>, CompositionState)>> {
        if context.is_none() {
            return Ok(None);
        };

        // check shortcut keys
        if VK_CONTROL.is_pressed() {
            return Ok(None);
        }

        #[allow(clippy::let_and_return)]
        let (composition, mode) = {
            let text_service = self.borrow()?;
            let composition = text_service.borrow_composition()?.clone();
            let mode = IMEState::get()?.input_mode.clone();
            (composition, mode)
        };

        let action = UserAction::try_from(wparam.0)?;

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
                )
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
                    vec![ClientAction::SetSelection(SetSelectionType::Down)],
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
                )
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
                    vec![ClientAction::SetSelection(SetSelectionType::Down)],
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
    pub fn handle_key(&self, context: Option<&ITfContext>, wparam: WPARAM) -> Result<bool> {
        if let Some(context) = context {
            self.borrow_mut()?.context = Some(context.clone());
        } else {
            return Ok(false);
        };

        if let Some((actions, transition)) = self.process_key(context, wparam)? {
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
        let mut ipc_service = IMEState::get()?
            .ipc_service
            .clone()
            .context("ipc_service is None")?;
        let mut transition = transition;

        self.update_context(&preview)?;

        for action in actions {
            match action {
                ClientAction::StartComposition => {
                    self.start_composition()?;
                    self.update_pos()?;
                    ipc_service.show_window()?;
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
                    let text = candidates.texts[selection_index as usize].clone();
                    let sub_text = candidates.sub_texts[selection_index as usize].clone();
                    let hiragana = candidates.hiragana.clone();

                    corresponding_count = candidates.corresponding_count[selection_index as usize];

                    preview = text.clone();
                    suffix = sub_text.clone();
                    raw_hiragana = hiragana.clone();
                    should_learn_candidate =
                        candidates.candidate_ids.get(selection_index as usize).is_some();

                    self.set_text(&text, &sub_text)?;
                    ipc_service.set_candidates(candidates.texts.clone())?;
                    ipc_service.set_selection(selection_index as i32)?;
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
                        };
                        selection_index = 0;
                        corresponding_count = count;
                        preview = converted[0].clone();
                        suffix.clear();
                        should_learn_candidate = false;
                        self.set_text(&preview, "")?;
                        ipc_service.set_candidates(candidates.texts.clone())?;
                        ipc_service.set_selection(0)?;
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

                    let empty = "".to_string();
                    let text = candidates
                        .texts
                        .get(selection_index as usize)
                        .cloned()
                        .unwrap_or(empty.clone());
                    let sub_text = candidates
                        .sub_texts
                        .get(selection_index as usize)
                        .cloned()
                        .unwrap_or(empty.clone());
                    corresponding_count = candidates
                        .corresponding_count
                        .get(selection_index as usize)
                        .cloned()
                        .unwrap_or(0);

                    raw_input = raw_input
                        .chars()
                        .take(corresponding_count as usize)
                        .collect();
                    preview = text.clone();
                    suffix = sub_text.clone();
                    raw_hiragana = hiragana;
                    should_learn_candidate =
                        candidates.candidate_ids.get(selection_index as usize).is_some();

                    self.set_text(&text, &sub_text)?;
                    ipc_service.set_candidates(candidates.texts.clone())?;
                    ipc_service.set_selection(selection_index as i32)?;
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
                    let sub_texts = candidates.sub_texts.clone();

                    selection_index = match selection {
                        SetSelectionType::Up => max(0, selection_index - 1),
                        SetSelectionType::Down => min(texts.len() as i32 - 1, selection_index + 1),
                        SetSelectionType::Number(number) => *number,
                    };

                    ipc_service.set_selection(selection_index as i32)?;
                    if composition.state != CompositionState::Selecting {
                        let text = texts[selection_index as usize].clone();
                        let sub_text = sub_texts[selection_index as usize].clone();
                        let hiragana = candidates.hiragana.clone();
                        corresponding_count = candidates.corresponding_count[selection_index as usize];

                        preview = text.clone();
                        suffix = sub_text.clone();
                        raw_hiragana = hiragana.clone();
                        should_learn_candidate =
                            candidates.candidate_ids.get(selection_index as usize).is_some();

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

                    ipc_service.shrink_text(corresponding_count.clone())?;
                    let text = match mode {
                        InputMode::Kana => to_fullwidth(text, false),
                        InputMode::Latin => text.to_string(),
                    };
                    candidates = ipc_service.append_text(text)?;
                    selection_index = 0;

                    let text = candidates.texts[selection_index as usize].clone();
                    let sub_text = candidates.sub_texts[selection_index as usize].clone();
                    let hiragana = candidates.hiragana.clone();
                    self.shift_start(&preview, &text)?;

                    corresponding_count = candidates.corresponding_count[selection_index as usize];
                    preview = text.clone();
                    suffix = sub_text.clone();
                    raw_hiragana = hiragana.clone();
                    should_learn_candidate =
                        candidates.candidate_ids.get(selection_index as usize).is_some();

                    ipc_service.set_candidates(candidates.texts.clone())?;
                    ipc_service.set_selection(selection_index as i32)?;
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