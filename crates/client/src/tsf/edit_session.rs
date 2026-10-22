use windows::{
    core::{implement, AsImpl, VARIANT},
    Win32::{
        Foundation::{E_FAIL, RECT},
        UI::TextServices::{
            ITfComposition, ITfCompositionSink, ITfContext, ITfContextComposition, ITfEditSession,
            ITfEditSession_Impl, ITfInsertAtSelection, ITfRange, GUID_PROP_ATTRIBUTE, TF_AE_NONE,
            TF_ANCHOR_END, TF_ANCHOR_START, TF_ES_ASYNCDONTCARE, TF_ES_READ, TF_ES_READWRITE,
            TF_ES_SYNC, TF_IAS_QUERYONLY, TF_SELECTION, TF_SELECTIONSTYLE, TF_ST_CORRECTION,
            TF_S_ASYNC,
        },
    },
};

use std::{cell::Cell, mem::ManuallyDrop, rc::Rc, time::Instant};

use anyhow::Result;

use crate::{engine::state::IMEState, extension::StringExt as _, globals::GUID_DISPLAY_ATTRIBUTE};

use super::factory::TextServiceFactory;

#[implement(ITfEditSession)]
struct EditSession<'a, T> {
    callback: Rc<dyn Fn(u32) -> anyhow::Result<T>>,
    // None until TSF has run the session
    pub result: Cell<Option<anyhow::Result<T>>>,
    phantom: std::marker::PhantomData<&'a T>,
}

/// Runs `callback` inside a synchronous read/write edit session and returns
/// its result. The callers continue with the composition right after this
/// call, so an edit session that TSF would only queue is reported as an
/// error instead of being silently skipped.
pub fn edit_session<T>(
    client_id: u32,
    context: ITfContext,
    callback: Rc<dyn Fn(u32) -> anyhow::Result<T>>,
) -> Result<T> {
    run_edit_session(client_id, context, callback, TF_ES_SYNC | TF_ES_READWRITE)?
        .ok_or_else(|| anyhow::anyhow!("synchronous edit session was not run"))
}

/// Runs `callback` in a read-only edit session, now if possible and queued
/// otherwise, for work whose result nobody waits for.
pub fn read_session(
    client_id: u32,
    context: ITfContext,
    callback: Rc<dyn Fn(u32) -> anyhow::Result<()>>,
) -> Result<()> {
    run_edit_session(client_id, context, callback, TF_ES_ASYNCDONTCARE | TF_ES_READ)?;
    Ok(())
}

fn run_edit_session<T>(
    client_id: u32,
    context: ITfContext,
    callback: Rc<dyn Fn(u32) -> anyhow::Result<T>>,
    flags: windows::Win32::UI::TextServices::TF_CONTEXT_EDIT_CONTEXT_FLAGS,
) -> Result<Option<T>> {
    let session: ITfEditSession = EditSession {
        callback,
        result: Cell::new(None),
        phantom: std::marker::PhantomData,
    }
    .into();

    // The outer result is whether the request was accepted; the returned
    // HRESULT is the edit session's own result.
    let session_result = unsafe { context.RequestEditSession(client_id, &session, flags)? };
    let session_impl = unsafe { session.as_impl() };
    match session_impl.result.take() {
        Some(result) => result.map(Some),
        None if session_result == TF_S_ASYNC => Ok(None),
        None => {
            session_result.ok()?;
            Ok(None)
        }
    }
}

impl<'a, T> ITfEditSession_Impl for EditSession_Impl<'a, T> {
    fn DoEditSession(&self, cookie: u32) -> windows::core::Result<()> {
        let result = (self.callback)(cookie);
        let hresult = match &result {
            Ok(_) => Ok(()),
            Err(error) => {
                tracing::error!("Edit session failed: {error:?}");
                Err(E_FAIL.into())
            }
        };
        // keep the detailed error for the requester
        self.result.set(Some(result));
        hresult
    }
}

/// Places the caret at `range`. TSF does not take ownership of the range in
/// the selection, so the reference is released here on every path.
unsafe fn set_selection(context: &ITfContext, cookie: u32, range: &ITfRange) -> Result<()> {
    let selections = [TF_SELECTION {
        range: ManuallyDrop::new(Some(range.clone())),
        style: TF_SELECTIONSTYLE {
            ase: TF_AE_NONE,
            fInterimChar: false.into(),
        },
    }];
    let result = context.SetSelection(cookie, &selections);
    let [selection] = selections;
    drop(ManuallyDrop::into_inner(selection.range));
    Ok(result?)
}

impl TextServiceFactory {
    #[tracing::instrument]
    pub fn start_composition(&self) -> Result<()> {
        tracing::debug!("start_composition");

        let text_service = self.borrow_mut()?;
        let context = text_service.context()?;
        let context_composition = text_service.context::<ITfContextComposition>()?;
        let sink = text_service.this::<ITfCompositionSink>()?;
        let insert = text_service.context::<ITfInsertAtSelection>()?;

        let tip_exists = {
            let composition = text_service.borrow_composition()?;
            composition.tip_composition.is_some()
        };

        if tip_exists {
            self.end_composition()?;
            return Ok(());
        }

        let composition = edit_session::<ITfComposition>(
            text_service.tid,
            context,
            Rc::new({
                move |cookie| unsafe {
                    let range = insert.InsertTextAtSelection(cookie, TF_IAS_QUERYONLY, &[])?;
                    let composition =
                        context_composition.StartComposition(cookie, &range, &sink)?;

                    Ok(composition)
                }
            }),
        )?;

        tracing::debug!("Composition started {composition:?}");
        text_service.borrow_mut_composition()?.tip_composition = Some(composition);

        Ok(())
    }

    #[tracing::instrument]
    pub fn end_composition(&self) -> Result<()> {
        tracing::debug!("end_composition");
        let text_service = self.borrow()?;

        if let Some(composition) = text_service.borrow_composition()?.tip_composition.clone() {
            edit_session(
                text_service.tid,
                text_service.context()?,
                Rc::new({
                    let context = text_service.context::<ITfContext>()?;

                    move |cookie| unsafe {
                        // The text already in the composition is what gets
                        // committed; only the display attribute is removed.
                        let range: ITfRange = composition.GetRange()?;
                        let prop = context.GetProperty(&GUID_PROP_ATTRIBUTE)?;
                        prop.Clear(cookie, &range)?;

                        // put the caret after the committed text
                        range.Collapse(cookie, TF_ANCHOR_END)?;
                        set_selection(&context, cookie, &range)?;

                        composition.EndComposition(cookie)?;
                        Ok(())
                    }
                }),
            )?;
        } else {
            tracing::warn!("Composition is not started");
        }

        text_service.borrow_mut_composition()?.tip_composition = None;

        Ok(())
    }

    #[tracing::instrument]
    pub fn set_text(&self, text: &str, subtext: &str) -> Result<()> {
        let text_service = self.borrow()?;

        if let Some(composition) = text_service.borrow_composition()?.tip_composition.clone() {
            edit_session(
                text_service.tid,
                text_service.context()?,
                Rc::new({
                    // TSF positions count UTF-16 code units
                    let text_len = text.encode_utf16().count() as i32;

                    // unpadded is all you need!
                    let text = format!("{text}{subtext}").as_str().to_wide_16_unpadded();
                    let context = text_service.context::<ITfContext>()?;
                    let display_attribute_atom = text_service.display_attribute_atom.clone();

                    move |cookie| unsafe {
                        let range = composition.GetRange()?;
                        range.SetText(cookie, TF_ST_CORRECTION, &text)?;

                        // first, set the display attribute to the "text" part
                        let text_range = range.Clone()?;
                        text_range.Collapse(cookie, TF_ANCHOR_START)?;
                        let mut shifted: i32 = 0;
                        text_range.ShiftEnd(cookie, text_len, &mut shifted, std::ptr::null())?;
                        let display_attribute = display_attribute_atom.get(&GUID_DISPLAY_ATTRIBUTE);
                        if let Some(display_attribute) = display_attribute {
                            let pvar = VARIANT::from(*display_attribute as i32);
                            let prop = context.GetProperty(&GUID_PROP_ATTRIBUTE)?;
                            prop.SetValue(cookie, &text_range, &pvar)?;
                        }

                        range.Collapse(cookie, TF_ANCHOR_END)?;
                        set_selection(&context, cookie, &range)?;

                        Ok(())
                    }
                }),
            )?;
        } else {
            tracing::warn!("Composition is not started");
        }

        Ok(())
    }

    #[tracing::instrument]
    pub fn shift_start(&self, text: &str, subtext: &str) -> Result<()> {
        let text_service = self.borrow()?;

        if let Some(composition) = text_service.borrow_composition()?.tip_composition.clone() {
            edit_session(
                text_service.tid,
                text_service.context()?,
                Rc::new({
                    // TSF positions count UTF-16 code units
                    let text_len = text.encode_utf16().count() as i32;
                    let subtext = subtext.to_wide_16_unpadded();
                    let context = text_service.context::<ITfContext>()?;
                    let display_attribute_atom = text_service.display_attribute_atom.clone();

                    move |cookie| unsafe {
                        // first, shift the start of the composition
                        let range = composition.GetRange()?;
                        let mut shifted: i32 = 0;

                        // and clear the display attribute
                        let prop = context.GetProperty(&GUID_PROP_ATTRIBUTE)?;
                        prop.Clear(cookie, &range)?;

                        range.Collapse(cookie, TF_ANCHOR_START)?;
                        range.ShiftStart(cookie, text_len, &mut shifted, std::ptr::null())?;

                        composition.ShiftStart(cookie, &range)?;

                        // then, set the display attribute
                        let range = composition.GetRange()?;

                        range.SetText(cookie, TF_ST_CORRECTION, &subtext)?;

                        let display_attribute = display_attribute_atom.get(&GUID_DISPLAY_ATTRIBUTE);
                        if let Some(display_attribute) = display_attribute {
                            let pvar = VARIANT::from(*display_attribute as i32);
                            let prop = context.GetProperty(&GUID_PROP_ATTRIBUTE)?;
                            prop.SetValue(cookie, &range, &pvar)?;
                        }

                        range.Collapse(cookie, TF_ANCHOR_END)?;
                        set_selection(&context, cookie, &range)?;

                        Ok(())
                    }
                }),
            )?;
        } else {
            tracing::warn!("Composition is not started");
        }

        Ok(())
    }

    #[tracing::instrument]
    pub fn update_pos(&self) -> Result<()> {
        {
            let mut text_service = match self.borrow_mut() {
                Ok(text_service) => text_service,
                Err(error) => {
                    tracing::warn!("Skip update_pos due to borrow conflict: {error:?}");
                    return Ok(());
                }
            };

            if !text_service
                .update_pos_state
                .try_begin_update(Instant::now())
            {
                tracing::debug!("Skip re-entrant update_pos call");
                return Ok(());
            }
        }

        let result: Result<()> = (|| {
            let (tid, context, tip_composition) = {
                let text_service = self.borrow()?;
                let composition = text_service.borrow_composition()?;
                (
                    text_service.tid,
                    text_service.context::<ITfContext>()?,
                    composition.tip_composition.clone(),
                )
            };

            if let Some(tip_composition) = tip_composition {
                // Only moves the candidate window: run now if the document
                // can be read, otherwise when TSF grants the lock.
                read_session(
                    tid,
                    context.clone(),
                    Rc::new({
                        let context = context.clone();

                        move |cookie| unsafe {
                            let view = context.GetActiveView()?;
                            let range = tip_composition.GetRange()?;

                            let Some(mut ipc_service) = IMEState::get()?.ipc_service.clone() else {
                                return Ok(());
                            };

                            let mut rect = RECT::default();
                            let mut clipped = false.into();
                            view.GetTextExt(cookie, &range, &mut rect, &mut clipped)?;

                            ipc_service.set_window_position(
                                rect.top,
                                rect.left,
                                rect.bottom,
                                rect.right,
                            )?;

                            Ok(())
                        }
                    }),
                )?;
            }

            Ok(())
        })();

        match self.borrow_mut() {
            Ok(mut text_service) => {
                text_service.update_pos_state.finish_update(Instant::now());
            }
            Err(error) => {
                tracing::warn!("Failed to reset update_pos guard: {error:?}");
            }
        }

        if let Err(error) = result {
            tracing::warn!("Failed to update composition window position: {error:?}");
        }

        Ok(())
    }
}
