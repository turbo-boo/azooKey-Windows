use crate::{
    engine::user_action::UserAction,
    extension::VKeyExt as _,
    tsf::{
        edit_session::IfRefused,
        factory::{TextServiceFactory, TextServiceFactory_Impl},
    },
};
use std::sync::atomic::{AtomicU64, Ordering};

use super::{
    client_action::{ClientAction, SetSelectionType, SetTextType},
    full_width::{to_fullwidth, to_fullwidth_latin, to_halfwidth},
    input_mode::InputMode,
    ipc_service::{magic_convert_in_background, Candidates, IPCService},
    state::IMEState,
    text_util::{to_half_katakana, to_katakana},
    user_action::{consume_translation, Function, Navigation},
};
use shared::AppConfig;
use windows::core::{IUnknown, Interface};
use windows::Win32::{
    Foundation::{LPARAM, WPARAM},
    UI::{
        Input::KeyboardAndMouse::{
            VK_CONTROL, VK_LCONTROL, VK_LMENU, VK_MENU, VK_RCONTROL, VK_RMENU,
        },
        TextServices::{
            ITfComposition, ITfCompositionSink_Impl, ITfContext, GUID_PROP_ATTRIBUTE,
        },
    },
};

use anyhow::{Context as _, Result};

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
    // the converter's input units (ComposingText.input_text), for F9/F10
    pub raw_input: String,
    pub raw_hiragana: String,

    pub corresponding_count: i32, // corresponding count of the preview

    pub selection_index: i32,
    pub candidates: Candidates,
    pub should_learn_candidate: bool,

    pub state: CompositionState,
    pub tip_composition: Option<ITfComposition>,
    // ComposingText.composition_id of the converter's composition holding
    // raw_input; None when the converter's content is not known to match
    // (no composition yet, or after a failure): the next edit clears the
    // converter and replays raw_input first
    pub composition_id: Option<u64>,
    // the AI conversion whose result the composition is waiting for
    pub magic_request: Option<MagicRequest>,
}

impl Composition {
    /// Composing and Previewing show text in a TSF composition; without one
    /// there is nothing left to edit.
    fn lost_tip_composition(&self) -> bool {
        self.tip_composition.is_none()
            && matches!(
                self.state,
                CompositionState::Composing | CompositionState::Previewing
            )
    }

    /// Forgets the composition's text, candidates and pending AI request.
    /// The TSF composition is left to the edit sessions that manage it.
    pub fn reset(&mut self) {
        let tip_composition = self.tip_composition.take();
        *self = Composition {
            tip_composition,
            ..Composition::default()
        };
    }
}

/// An AI conversion running in the background for the text `source`.
#[derive(Clone, Debug, PartialEq)]
pub struct MagicRequest {
    id: u64,
    source: String,
}

#[derive(Debug)]
pub struct MagicResult {
    request: MagicRequest,
    candidates: Result<Vec<String>>,
}

static NEXT_MAGIC_REQUEST: AtomicU64 = AtomicU64::new(0);

/// The text sent for AI conversion: what the composition currently shows.
fn magic_source(preview: &str, suffix: &str, raw_hiragana: &str) -> String {
    if preview.is_empty() {
        raw_hiragana.to_string()
    } else {
        format!("{preview}{suffix}")
    }
}

/// Whether the composition still waits for `request`: the user may have typed,
/// committed or pressed F5 again while the AI conversion was running.
fn magic_result_is_current(composition: &Composition, request: &MagicRequest) -> bool {
    composition.state != CompositionState::None
        && composition.magic_request.as_ref() == Some(request)
        && magic_source(
            &composition.preview,
            &composition.suffix,
            &composition.raw_hiragana,
        ) == request.source
}

/// The AI conversion still awaited after a key produced `actions`. Any handled
/// key (typing, Backspace, Escape, commit, selection) withdraws the pending
/// request, so its result cannot rewrite a composition the user has since
/// edited or replaced, even one showing the same text. Keys that produce no
/// action keep it; MagicConvert then sets a new one.
fn pending_magic_request(
    current: &Option<MagicRequest>,
    actions: &[ClientAction],
) -> Option<MagicRequest> {
    if actions.is_empty() {
        current.clone()
    } else {
        None
    }
}

impl ITfCompositionSink_Impl for TextServiceFactory_Impl {
    #[macros::anyhow]
    fn OnCompositionTerminated(
        &self,
        ecwrite: u32,
        pcomposition: Option<&ITfComposition>,
    ) -> Result<()> {
        // if user clicked outside the composition, the composition will be terminated
        tracing::debug!("OnCompositionTerminated");

        // TSF already ended the composition and lends its write cookie here;
        // a new edit session cannot be requested from inside this one. Clear
        // the marking through the composition's own document: the text
        // service's context follows the last key and can be another one.
        if let Some(composition) = pcomposition {
            let cleared = unsafe {
                composition.GetRange().and_then(|range| {
                    range
                        .GetContext()?
                        .GetProperty(&GUID_PROP_ATTRIBUTE)?
                        .Clear(ecwrite, &range)
                })
            };
            if let Err(error) = cleared {
                tracing::warn!("Failed to clear the terminated composition's marking: {error:?}");
            }
        }

        // The composition is gone whatever happened above: forget it and
        // reset the engine, which needs neither the document nor the server.
        {
            let text_service = self.borrow()?;
            let mut current = text_service.borrow_mut_composition()?;
            let ours = match (current.tip_composition.as_ref(), pcomposition) {
                (Some(tip), Some(terminated)) => same_object(tip, terminated),
                // already let go of; reset only an engine left composing
                (None, Some(_)) => current.lost_tip_composition(),
                (_, None) => true,
            };
            if !ours {
                // not the composition the engine is editing
                return Ok(());
            }
        }

        self.forget_composition()
    }
}

/// Whether `a` and `b` are the same COM object. Interface pointers identify
/// an object only through IUnknown; if that cannot be queried, they are
/// taken to be the same, which keeps the engine on its current composition.
fn same_object<T: Interface>(a: &T, b: &T) -> bool {
    match (a.cast::<IUnknown>(), b.cast::<IUnknown>()) {
        (Ok(a), Ok(b)) => a == b,
        _ => true,
    }
}

/// Hides the candidates and clears the converter after the TSF composition
/// ended. The engine has already forgotten the composition, so a failure
/// here must not undo that: the window shows candidates of a finished
/// composition at worst, and edit_converter clears the converter again
/// before the next composition uses it.
fn release_ended_composition(ipc_service: &mut IPCService) {
    let results = [
        ipc_service.hide_window(),
        ipc_service.set_candidates(vec![]),
        ipc_service.clear_text(),
    ];
    for error in results.into_iter().filter_map(Result::err) {
        tracing::warn!("Cleanup after the composition ended failed: {error:?}");
    }
}

/// Whether a response shows that the converter no longer holds the
/// composition with id `held`: the server restarted, or another connection
/// took the converter over and it started afresh.
fn converter_lost_composition(held: Option<u64>, response: &Candidates) -> bool {
    held.is_some_and(|held| held != response.composition_id)
}

/// Makes the converter hold exactly `raw_input` as a new composition. When a
/// connection takes the converter over, the server also forgets the text
/// before the composition, so `resend_context` sends it again before the
/// input is replayed. A new composition (no input yet) has just sent it.
fn restore_converter(
    ipc_service: &mut IPCService,
    raw_input: &str,
    resend_context: &mut impl FnMut(),
) -> Result<()> {
    ipc_service.clear_text()?;
    if !raw_input.is_empty() {
        resend_context();
        ipc_service.append_text(raw_input.to_string())?;
    }
    Ok(())
}

/// Applies `edit` to the converter's copy of this composition and records
/// the converter's composition id and input. If the converter's content is
/// unknown, or the response shows it lost the composition, the composition
/// is replayed from `raw_input` (the input before this edit) so the edit
/// applies to the text the user typed, not to an empty or foreign one.
fn edit_converter(
    ipc_service: &mut IPCService,
    composition_id: &mut Option<u64>,
    raw_input: &mut String,
    mut resend_context: impl FnMut(),
    mut edit: impl FnMut(&mut IPCService) -> Result<Candidates>,
) -> Result<Candidates> {
    if composition_id.is_none() {
        restore_converter(ipc_service, raw_input, &mut resend_context)?;
    }
    let mut candidates = edit(ipc_service)?;
    if converter_lost_composition(*composition_id, &candidates) {
        tracing::warn!("The converter lost the composition; replaying its input");
        restore_converter(ipc_service, raw_input, &mut resend_context)?;
        candidates = edit(ipc_service)?;
    }
    *composition_id = Some(candidates.composition_id);
    *raw_input = candidates.input_text.clone();
    Ok(candidates)
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

/// What the composition shows after Backspace, given the fresh candidate list
/// returned by the converter. Always starts from the first candidate and falls
/// back to the remaining hiragana when there is none.
#[derive(Debug, PartialEq)]
struct RemovedTextView {
    text: String,
    sub_text: String,
    corresponding_count: i32,
    learnable: bool,
    show_candidates: bool,
}

fn view_after_remove(candidates: &Candidates) -> RemovedTextView {
    match candidate_parts(candidates, 0) {
        Some((text, sub_text, corresponding_count)) => RemovedTextView {
            text,
            sub_text,
            corresponding_count,
            learnable: candidate_is_learnable(candidates, 0),
            show_candidates: candidates.is_converted,
        },
        // the hiragana stands for the whole remaining input
        None => RemovedTextView {
            text: candidates.hiragana.clone(),
            sub_text: String::new(),
            corresponding_count: candidates.input_count,
            learnable: false,
            show_candidates: false,
        },
    }
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
    AppConfig::read_cached().shortcuts.ctrl_space_toggle
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
            let config = AppConfig::read_cached();
            let shortcuts = &config.shortcuts;
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
        let (mut composition, mode) = {
            let text_service = self.borrow()?;
            let composition = text_service.borrow_composition()?.clone();
            let mode = IMEState::get()?.input_mode.clone();
            (composition, mode)
        };
        // handle_key ends a composition of another document before it
        // handles the key, so classify the key as if none was active
        if let Some(context) = context {
            if self.composition_in_other_document(context)? {
                composition.reset();
            }
        }

        let action = if ctrl_space || alt_backquote {
            UserAction::ToggleInputMode
        } else {
            UserAction::from_key(wparam, lparam)?
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
                UserAction::Input(text) if mode == InputMode::Kana => (
                    CompositionState::Composing,
                    vec![
                        ClientAction::StartComposition,
                        ClientAction::AppendText(text),
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
                UserAction::Input(text) => (
                    CompositionState::Composing,
                    vec![ClientAction::AppendText(text)],
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
                    // azooKey-Desktop consumes Left/Right while composing; the
                    // caret stays at the end of the composition.
                    Navigation::Right | Navigation::Left => (CompositionState::Composing, vec![]),
                    Navigation::Up => (
                        CompositionState::Previewing,
                        vec![ClientAction::SetSelection(SetSelectionType::Up)],
                    ),
                    // azooKey-Desktop: Down enters candidate selection like Space
                    Navigation::Down => (
                        CompositionState::Previewing,
                        vec![ClientAction::RequestConversion],
                    ),
                },
                UserAction::ToggleInputMode => (
                    CompositionState::None,
                    vec![
                        ClientAction::EndComposition,
                        ClientAction::SetIMEMode(InputMode::Latin),
                    ],
                ),
                // As in azooKey-Desktop, the first Space/Tab always asks for
                // conversion: live-converted text was produced without rich
                // candidates, so the candidate list is generated afresh.
                UserAction::Space | UserAction::Tab => (
                    CompositionState::Previewing,
                    vec![ClientAction::RequestConversion],
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
                UserAction::Input(text) => (
                    CompositionState::Composing,
                    vec![
                        ClientAction::CommitCandidate,
                        ClientAction::ShrinkText(text),
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
                    // consumed without moving, as in Composing
                    Navigation::Right | Navigation::Left => (CompositionState::Previewing, vec![]),
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
                UserAction::Input(text) if mode == InputMode::Kana => (
                    CompositionState::Composing,
                    vec![
                        ClientAction::DismissPredictions,
                        ClientAction::StartComposition,
                        ClientAction::AppendText(text),
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
            self.end_composition_in_other_document(context)?;
            self.borrow_mut()?.context = Some(context.clone());
        } else {
            return Ok(false);
        };

        if let Some((actions, transition)) = self.process_key(context, wparam, lparam)? {
            consume_translation(wparam, lparam);
            self.handle_action(&actions, transition)?;
        } else {
            return Ok(false);
        }

        Ok(true)
    }

    /// Whether the engine's TSF composition lives in another document than
    /// `context`. After a focus change the end of the old composition may be
    /// queued until its document can be locked, while keys already arrive in
    /// the new document.
    fn composition_in_other_document(&self, context: &ITfContext) -> Result<bool> {
        let tip_composition = self.borrow()?.borrow_composition()?.tip_composition.clone();
        let Some(tip_composition) = tip_composition else {
            return Ok(false);
        };
        let owner = unsafe {
            tip_composition
                .GetRange()
                .and_then(|range| range.GetContext())
        };
        Ok(match owner {
            Ok(owner) => !same_object(&owner, context),
            // the composition has ended
            Err(_) => true,
        })
    }

    /// Ends a composition of another document before a key in `context` is
    /// handled, so the key cannot edit it: its text is committed there, or,
    /// if that document cannot be edited now, the engine lets go of it.
    fn end_composition_in_other_document(&self, context: &ITfContext) -> Result<()> {
        if !self.composition_in_other_document(context)? {
            return Ok(());
        }
        if let Err(error) = self.end_composition() {
            tracing::warn!("Cannot end the composition in the other document: {error:?}");
        }
        self.forget_composition()
    }

    /// Forgets the TSF composition and resets the engine. The candidate
    /// window and the converter are cleaned up through the existing
    /// connection only: connecting here could stall the application, and
    /// without a connection there is nothing to clean up.
    fn forget_composition(&self) -> Result<()> {
        {
            let text_service = self.borrow()?;
            let mut composition = text_service.borrow_mut_composition()?;
            composition.tip_composition = None;
            composition.reset();
        }

        let ipc_service = IMEState::get()
            .ok()
            .and_then(|state| state.ipc_service.clone());
        if let Some(mut ipc_service) = ipc_service {
            release_ended_composition(&mut ipc_service);
        }
        Ok(())
    }

    /// Applies AI conversion results queued by the background requests.
    /// Runs on the TSF thread, from the text service's message window.
    pub fn deliver_magic_results(&self) {
        let results = match self.borrow() {
            Ok(text_service) => match text_service.message_window.as_ref() {
                Some(window) => window.take_results(),
                None => return,
            },
            Err(error) => {
                // left queued; the next delivery picks them up
                tracing::warn!("Cannot deliver AI conversion results now: {error:?}");
                return;
            }
        };

        for result in results {
            // The staleness check runs when the edit session does, so a result
            // queued behind the document lock is still checked against the
            // composition it would replace.
            // An AI result that cannot be written into the document is
            // dropped: showing it would let Enter commit text the document
            // never received.
            let applied = self.run_outside_key_event(IfRefused::Skip, move |factory| {
                factory.apply_magic_result(result)
            });
            if let Err(error) = applied {
                tracing::warn!("Failed to show AI conversion candidates: {error:?}");
            }
        }
    }

    fn apply_magic_result(&self, result: MagicResult) -> Result<()> {
        let composition = self.borrow()?.borrow_composition()?.clone();
        if !magic_result_is_current(&composition, &result.request) {
            tracing::debug!("Dropping AI conversion result for an outdated composition");
            return Ok(());
        }

        let converted = match result.candidates {
            Ok(converted) if !converted.is_empty() => converted,
            Ok(_) => return Ok(()),
            Err(error) => {
                tracing::warn!("Magic conversion failed: {error:?}");
                return Ok(());
            }
        };
        self.handle_action(
            &[ClientAction::ShowMagicCandidates(converted)],
            composition.state,
        )
    }

    /// Sends the converter the text before the composition again. The
    /// document still shows `preview` and `suffix` before the caret.
    fn resend_context(&self, preview: &str, suffix: &str) {
        if let Err(error) = self.update_context(&format!("{preview}{suffix}")) {
            tracing::warn!("Failed to send the text before the composition: {error:?}");
        }
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
        let mut composition_id = composition.composition_id;
        let mut raw_hiragana = composition.raw_hiragana.clone();
        let mut corresponding_count = composition.corresponding_count.clone();
        let mut candidates = composition.candidates.clone();
        let mut should_learn_candidate = composition.should_learn_candidate;
        let mut selection_index = composition.selection_index;
        let mut magic_request = pending_magic_request(&composition.magic_request, actions);
        let mut ipc_service = IMEState::ipc_service()?;
        let mut transition = transition;

        let applied = (|| -> Result<()> {
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
                        composition_id = None;
                        raw_hiragana.clear();
                        candidates = Candidates::default();
                        should_learn_candidate = false;
                        release_ended_composition(&mut ipc_service);
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
                        composition_id = None;
                        raw_hiragana.clear();
                        candidates = Candidates::default();
                        should_learn_candidate = false;
                        transition = CompositionState::None;

                        release_ended_composition(&mut ipc_service);
                        if let Err(error) = ipc_service.clear_post_composition_predictions() {
                            tracing::warn!("Failed to clear prediction state while cancelling: {error:?}");
                        }
                    }
                    ClientAction::AppendText(text) => {
                        let text = match mode {
                            InputMode::Kana => to_fullwidth(text),
                            InputMode::Latin => text.to_string(),
                        };

                        candidates = edit_converter(
                            &mut ipc_service,
                            &mut composition_id,
                            &mut raw_input,
                            || self.resend_context(&preview, &suffix),
                            |ipc| ipc.append_text(text.clone()),
                        )?;
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
                        let converted = edit_converter(
                            &mut ipc_service,
                            &mut composition_id,
                            &mut raw_input,
                            || self.resend_context(&preview, &suffix),
                            |ipc| ipc.request_conversion(),
                        )?;
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
                                ..Candidates::default()
                            };
                            selection_index = 0;
                            preview.clear();
                            suffix.clear();
                            raw_input.clear();
                            composition_id = None;
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
                                ..Candidates::default()
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
                        composition_id = None;
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
                        composition_id = None;
                        raw_hiragana.clear();
                        corresponding_count = 0;
                        should_learn_candidate = false;
                        ipc_service.hide_window()?;
                        ipc_service.set_candidates(vec![])?;
                    }
                    ClientAction::MagicConvert => {
                        // The HTTP API can take up to a minute: request it in the
                        // background and show the candidates when they arrive
                        // (ShowMagicCandidates), unless the composition changed.
                        let source = magic_source(&preview, &suffix, &raw_hiragana);
                        let request = MagicRequest {
                            id: NEXT_MAGIC_REQUEST.fetch_add(1, Ordering::Relaxed),
                            source: source.clone(),
                        };
                        let sender = self
                            .borrow()?
                            .message_window
                            .as_ref()
                            .context("message window is not available")?
                            .sender();
                        magic_request = Some(request.clone());
                        magic_convert_in_background(source, move |candidates| {
                            sender.send(MagicResult {
                                request,
                                candidates,
                            })
                        })?;
                    }
                    ClientAction::ShowMagicCandidates(converted) => {
                        magic_request = None;
                        // The AI candidate replaces the whole composition, so it
                        // covers every input unit (not the number of kana).
                        let count = candidates.input_count;
                        candidates = Candidates {
                            texts: converted.clone(),
                            sub_texts: vec![String::new(); converted.len()],
                            hiragana: raw_hiragana.clone(),
                            corresponding_count: vec![count; converted.len()],
                            candidate_ids: vec![],
                            is_converted: true,
                            input_count: count,
                            input_text: raw_input.clone(),
                            composition_id: candidates.composition_id,
                        };
                        selection_index = 0;
                        corresponding_count = count;
                        preview = converted.first().cloned().unwrap_or_default();
                        suffix.clear();
                        should_learn_candidate = false;
                        self.set_text(&preview, "")?;
                        ipc_service.set_candidates(candidates.texts.clone())?;
                        ipc_service.set_selection(0)?;
                        ipc_service.show_window()?;
                    }
                    ClientAction::RemoveText => {
                        // The converter may rewrite romaji it keeps into kana
                        // ("sha" -> "し"): only its input says what remains, and
                        // edit_converter takes raw_input from it.
                        candidates = edit_converter(
                            &mut ipc_service,
                            &mut composition_id,
                            &mut raw_input,
                            || self.resend_context(&preview, &suffix),
                            |ipc| ipc.remove_text(),
                        )?;
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
                            composition_id = None;
                            raw_hiragana.clear();
                            candidates = Candidates::default();
                            should_learn_candidate = false;
                            transition = CompositionState::None;

                            release_ended_composition(&mut ipc_service);
                            continue;
                        }

                        // The converter returns a fresh candidate list, so the
                        // previous selection index does not apply to it.
                        selection_index = 0;
                        let view = view_after_remove(&candidates);
                        corresponding_count = view.corresponding_count;
                        preview = view.text;
                        suffix = view.sub_text;
                        raw_hiragana = hiragana;
                        should_learn_candidate = view.learnable;

                        self.set_text(&preview, &suffix)?;
                        if view.show_candidates {
                            ipc_service.set_candidates(candidates.texts.clone())?;
                            ipc_service.set_selection(selection_index)?;
                            ipc_service.show_window()?;
                        } else {
                            ipc_service.hide_window()?;
                            ipc_service.set_candidates(vec![])?;
                        }
                    }
                    ClientAction::SetIMEMode(mode) => {
                        // An empty composition only locates the caret for the mode
                        // indicator; the mode must still switch when the document
                        // cannot be edited right now.
                        match self.start_composition() {
                            Ok(()) => {
                                self.update_pos()?;
                                self.end_composition()?;
                            }
                            Err(error) => {
                                tracing::warn!("Cannot locate the caret for the indicator: {error:?}")
                            }
                        }
                        // After the indicator composition: in IMM32 applications
                        // starting one opens the IME again.
                        self.update_open_close(mode);

                        // Hold the process-wide state only for the assignment: the
                        // language bar (GetIcon) and other threads' keys read it
                        // while the COM and IPC calls below run.
                        IMEState::get()?.input_mode = mode.clone();

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
                        composition_id = None;
                        raw_hiragana.clear();
                        should_learn_candidate = false;
                        // the next composition clears the converter anyway
                        if let Err(error) = ipc_service.clear_text() {
                            tracing::warn!("Failed to clear the converter: {error:?}");
                        }
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
                        let shrunk = edit_converter(
                            &mut ipc_service,
                            &mut composition_id,
                            &mut raw_input,
                            || self.resend_context(&preview, &suffix),
                            |ipc| ipc.shrink_text(corresponding_count),
                        )?;
                        let text = match mode {
                            InputMode::Kana => to_fullwidth(text),
                            InputMode::Latin => text.to_string(),
                        };
                        candidates = if text.is_empty() {
                            shrunk
                        } else {
                            edit_converter(
                                &mut ipc_service,
                                &mut composition_id,
                                &mut raw_input,
                                || self.resend_context(&preview, &suffix),
                                |ipc| ipc.append_text(text.clone()),
                            )?
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
                            // the converter holds kana-mode symbols in their
                            // Japanese form ("ー"); F9 shows the ASCII key's
                            SetTextType::FullLatin => to_fullwidth_latin(&to_halfwidth(&raw_input)),
                            SetTextType::HalfLatin => to_halfwidth(&raw_input),
                        };

                        // The displayed text now stands for the whole composition;
                        // keep preview/suffix and the commit range in sync so the
                        // next key commits exactly what is shown.
                        preview = text;
                        suffix.clear();
                        corresponding_count = candidates.input_count;
                        should_learn_candidate = false;
                        self.set_text(&preview, "")?;
                    }
                }
            }
            Ok(())
        })();

        let text_service = self.borrow()?;
        let mut composition = text_service.borrow_mut_composition()?;

        if let Err(error) = applied {
            // The actions stopped part way. The converter may have applied
            // an edit whose result was never shown: replay the known input
            // before the next edit. A TSF composition that already ended
            // cannot be continued: forget it instead of composing without it.
            composition.composition_id = None;
            if composition.lost_tip_composition() {
                composition.reset();
                drop(composition);
                drop(text_service);
                release_ended_composition(&mut ipc_service);
            }
            return Err(error);
        }

        composition.preview = preview.clone();
        composition.state = transition;
        composition.selection_index = selection_index;
        composition.raw_input = raw_input.clone();
        composition.composition_id = composition_id;
        composition.raw_hiragana = raw_hiragana.clone();
        composition.candidates = candidates;
        composition.should_learn_candidate = should_learn_candidate;
        composition.suffix = suffix.clone();
        composition.corresponding_count = corresponding_count;
        composition.magic_request = magic_request;

        // The application can end the composition while an edit session
        // runs (OnCompositionTerminated), after which the actions continued
        // without one.
        if composition.lost_tip_composition() {
            composition.reset();
            drop(composition);
            drop(text_service);
            release_ended_composition(&mut ipc_service);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        candidate_is_learnable, candidate_parts, conversion_key_action, converter_lost_composition,
        is_alt_backquote_key, magic_result_is_current, next_selection_index, pending_magic_request,
        view_after_remove, Composition, CompositionState, MagicRequest,
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
            input_count: 2,
            input_text: "kana".to_string(),
            composition_id: 1,
        };
        assert!(!candidate_is_learnable(&raw, 0));

        let converted = Candidates {
            is_converted: true,
            candidate_ids: vec![0],
            ..raw
        };
        assert!(candidate_is_learnable(&converted, 0));
    }

    #[test]
    fn backspace_shows_first_candidate_of_the_fresh_list() {
        // After Backspace the server returns only the unconverted text (as
        // azooKey-Desktop does), even if the 3rd candidate was selected before.
        // "kyou": 4 input units for 3 kana
        let after_remove = Candidates {
            texts: vec!["きょう".to_string()],
            sub_texts: vec![String::new()],
            hiragana: "きょう".to_string(),
            corresponding_count: vec![4],
            candidate_ids: vec![-1],
            is_converted: false,
            input_count: 4,
            input_text: "kyou".to_string(),
            composition_id: 1,
        };

        let view = view_after_remove(&after_remove);
        assert_eq!(view.text, "きょう");
        assert_eq!(view.corresponding_count, 4);
        assert!(!view.learnable);
        assert!(!view.show_candidates);
    }

    #[test]
    fn backspace_without_candidates_keeps_remaining_hiragana_visible() {
        // "kyo": 3 input units
        let after_remove = Candidates {
            hiragana: "きょ".to_string(),
            input_count: 3,
            ..Candidates::default()
        };

        let view = view_after_remove(&after_remove);
        assert_eq!(view.text, "きょ");
        assert_eq!(view.sub_text, "");
        // committing it later must consume all of the remaining input
        assert_eq!(view.corresponding_count, 3);
    }

    #[test]
    fn ai_result_applies_only_to_the_composition_that_requested_it() {
        let request = MagicRequest {
            id: 1,
            source: "きょう".to_string(),
        };
        let waiting = Composition {
            raw_hiragana: "きょう".to_string(),
            state: CompositionState::Composing,
            magic_request: Some(request.clone()),
            ..Composition::default()
        };
        assert!(magic_result_is_current(&waiting, &request));

        // typed another character while waiting
        let typed = Composition {
            raw_hiragana: "きょうは".to_string(),
            ..waiting.clone()
        };
        assert!(!magic_result_is_current(&typed, &request));

        // pressed F5 again: only the newer request's result is shown
        let newer = Composition {
            magic_request: Some(MagicRequest { id: 2, ..request.clone() }),
            ..waiting.clone()
        };
        assert!(!magic_result_is_current(&newer, &request));

        // committed or cancelled
        let ended = Composition {
            state: CompositionState::None,
            ..waiting
        };
        assert!(!magic_result_is_current(&ended, &request));
    }

    #[test]
    fn ai_result_is_dropped_after_cancelling_and_retyping_the_same_text() {
        let request = MagicRequest {
            id: 1,
            source: "か".to_string(),
        };
        let waiting = Some(request.clone());

        // F5 on "か", then Escape before the result arrives
        let after_escape =
            pending_magic_request(&waiting, &[ClientAction::CancelComposition]);
        // typing "か" again starts a new composition showing the same text
        let after_retyping = pending_magic_request(
            &after_escape,
            &[
                ClientAction::StartComposition,
                ClientAction::AppendText("k".to_string()),
            ],
        );
        let retyped = Composition {
            raw_hiragana: "か".to_string(),
            state: CompositionState::Composing,
            magic_request: after_retyping,
            ..Composition::default()
        };
        assert!(!magic_result_is_current(&retyped, &request));

        // a key that does nothing (e.g. Left while composing) keeps waiting
        assert_eq!(pending_magic_request(&waiting, &[]), waiting);
    }

    #[test]
    fn composition_is_replayed_only_when_the_converter_changed_it() {
        let response = Candidates {
            composition_id: 7,
            ..Candidates::default()
        };
        // the first edit of a composition adopts the converter's id
        assert!(!converter_lost_composition(None, &response));
        assert!(!converter_lost_composition(Some(7), &response));
        // server restart, or another connection took the converter over
        assert!(converter_lost_composition(Some(6), &response));
    }

    #[test]
    fn composing_without_a_tsf_composition_is_reset() {
        let composing = Composition {
            preview: "かな".to_string(),
            raw_input: "kana".to_string(),
            composition_id: Some(3),
            state: CompositionState::Composing,
            magic_request: Some(MagicRequest {
                id: 1,
                source: "かな".to_string(),
            }),
            ..Composition::default()
        };
        assert!(composing.lost_tip_composition());

        let mut reset = composing;
        reset.reset();
        assert_eq!(reset.state, CompositionState::None);
        assert!(reset.preview.is_empty() && reset.raw_input.is_empty());
        assert_eq!(reset.composition_id, None);
        assert_eq!(reset.magic_request, None);

        // predictions are shown without a TSF composition
        let selecting = Composition {
            state: CompositionState::Selecting,
            ..Composition::default()
        };
        assert!(!selecting.lost_tip_composition());
    }
}
