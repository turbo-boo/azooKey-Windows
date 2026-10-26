use windows::{
    core::{implement, AsImpl, HRESULT, VARIANT},
    Win32::{
        Foundation::{E_FAIL, HWND, RECT},
        UI::{
            TextServices::{
                ITfComposition, ITfCompositionSink, ITfContext, ITfContextComposition,
                ITfEditSession, ITfEditSession_Impl, ITfInsertAtSelection, ITfRange,
                GUID_PROP_ATTRIBUTE, TF_AE_NONE, TF_ANCHOR_END, TF_ANCHOR_START,
                TF_ES_ASYNCDONTCARE, TF_ES_READ, TF_ES_READWRITE, TF_ES_SYNC, TF_E_DISCONNECTED,
                TF_E_LOCKED, TF_IAS_QUERYONLY, TF_SELECTION, TF_SELECTIONSTYLE, TF_ST_CORRECTION,
                TF_S_ASYNC, TS_E_READONLY,
            },
            WindowsAndMessaging::{KillTimer, SetTimer, USER_TIMER_MINIMUM},
        },
    },
};

use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    mem::ManuallyDrop,
    rc::Rc,
    time::Instant,
};

use anyhow::Result;

use crate::{
    engine::{client_action::ClientAction, composition::CompositionState, state::IMEState},
    extension::StringExt as _,
    globals::GUID_DISPLAY_ATTRIBUTE,
};

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

/// The document holding `composition`. The text service's own context follows
/// the last key event and can already belong to another document.
fn composition_context(composition: &ITfComposition) -> Result<ITfContext> {
    Ok(unsafe { composition.GetRange()?.GetContext()? })
}

/// The edit session that ends `composition` in `context`. The text already
/// in the composition is what gets committed; only the display attribute is
/// removed, and the caret is put after the committed text.
fn end_composition_session(
    composition: ITfComposition,
    context: ITfContext,
) -> Rc<dyn Fn(u32) -> Result<()>> {
    Rc::new(move |cookie| unsafe {
        let range: ITfRange = composition.GetRange()?;
        let prop = context.GetProperty(&GUID_PROP_ATTRIBUTE)?;
        prop.Clear(cookie, &range)?;

        range.Collapse(cookie, TF_ANCHOR_END)?;
        set_selection(&context, cookie, &range)?;

        composition.EndComposition(cookie)?;
        Ok(())
    })
}

/// What [`TextServiceFactory::run_outside_key_event`] does when the document
/// refuses to be edited.
#[derive(Clone, Copy, Debug)]
pub enum IfRefused {
    /// Let go of the composition and run the work anyway.
    RunWithoutComposition,
    /// Drop the work.
    Skip,
}

/// How often work refused with TF_E_LOCKED is tried again.
const LOCKED_RETRIES: u32 = 5;

/// The HRESULT of a failed request, when it came from Windows.
fn hresult(error: &anyhow::Error) -> Option<HRESULT> {
    error
        .downcast_ref::<windows::core::Error>()
        .map(|error| error.code())
}

thread_local! {
    /// Work waiting for its thread timer, by timer id.
    static DEFERRED: RefCell<HashMap<usize, Box<dyn FnOnce()>>> = RefCell::default();
}

/// Runs `work` on this thread once it is back in its message loop, from a
/// one-shot thread timer. Locks held by other text services are only held
/// while their calls run, so they are gone by then.
fn defer(work: Box<dyn FnOnce()>) -> Result<()> {
    let id = unsafe { SetTimer(None, 0, USER_TIMER_MINIMUM, Some(run_deferred)) };
    if id == 0 {
        return Err(windows::core::Error::from_win32().into());
    }
    DEFERRED.with(|deferred| deferred.borrow_mut().insert(id, work));
    Ok(())
}

unsafe extern "system" fn run_deferred(_hwnd: HWND, _msg: u32, id: usize, _time: u32) {
    let _ = KillTimer(None, id);
    // taken out first: the work may defer again
    let work = DEFERRED.with(|deferred| deferred.borrow_mut().remove(&id));
    if let Some(work) = work {
        work();
    }
}

impl TextServiceFactory {
    /// Runs `work` for an event other than a key stroke: a language bar
    /// click, a focus change or a background result.
    ///
    /// Without a TSF composition nothing in the document needs editing, so
    /// the work runs right away. With one, ending or rewriting it needs a
    /// read/write lock, which TSF grants synchronously only while a key is
    /// handled; the work then runs inside an asynchronous read/write session
    /// on the composition's document, now if possible and otherwise once the
    /// document can be locked, and the synchronous sessions it requests nest
    /// inside that lock. A queued session does nothing if the text service was
    /// deactivated or moved on to another composition in the meantime.
    ///
    /// If the document refuses the session (read-only, closed), the
    /// composition cannot be edited any more. `if_refused` decides whether the
    /// work still runs without it: state changes such as a mode switch should,
    /// work that only rewrites the composition should not. If the request
    /// comes while another text service holds the lock (TF_E_LOCKED), the
    /// work is tried again from the thread's message loop, when that lock is
    /// gone. On any other error the composition is kept and the error
    /// returned.
    pub fn run_outside_key_event(
        &self,
        if_refused: IfRefused,
        work: impl FnOnce(&TextServiceFactory) -> Result<()> + 'static,
    ) -> Result<()> {
        self.run_outside_key_event_with_retries(if_refused, work, LOCKED_RETRIES)
    }

    fn run_outside_key_event_with_retries(
        &self,
        if_refused: IfRefused,
        work: impl FnOnce(&TextServiceFactory) -> Result<()> + 'static,
        retries: u32,
    ) -> Result<()> {
        let _own = self.own_activity();
        let (tid, activation, this, composition) = {
            let text_service = self.borrow()?;
            let composition = text_service.borrow_composition()?.tip_composition.clone();
            (
                text_service.tid,
                text_service.activation,
                text_service.this.clone(),
                composition,
            )
        };
        let (Some(this), Some(composition)) = (this, composition) else {
            return work(self);
        };
        let context = match composition_context(&composition) {
            Ok(context) => context,
            // ended under us (E_UNEXPECTED): there is nothing left to edit
            Err(error) => return self.refused(if_refused, work, error),
        };

        let work = Rc::new(Cell::new(Some(work)));
        let queued = work.clone();
        let requested = run_edit_session(
            tid,
            context.clone(),
            Rc::new({
                let this = this.clone();
                let composition = composition.clone();
                move |cookie| {
                    // `this` only keeps the object alive until a queued session
                    // runs; whether the work still applies is checked here.
                    let factory: &TextServiceFactory = unsafe { this.as_impl() };
                    let (active, tracked) = {
                        let text_service = factory.borrow()?;
                        let tip_composition =
                            text_service.borrow_composition()?.tip_composition.clone();
                        (
                            text_service.activation == activation,
                            tip_composition.as_ref() == Some(&composition),
                        )
                    };
                    match queued.take() {
                        Some(work) if active && tracked => {
                            let _own = factory.own_activity();
                            work(factory)
                        }
                        _ => {
                            tracing::debug!("Dropping work for an outdated composition");
                            if !tracked {
                                // Let go of in the meantime (deactivated, or
                                // the document could not be edited): end it
                                // here, or it stays open with its marking.
                                // Fails harmlessly if it has already ended.
                                let session =
                                    end_composition_session(composition.clone(), context.clone());
                                if let Err(error) = session(cookie) {
                                    tracing::debug!("Composition not ended: {error:?}");
                                }
                            }
                            Ok(())
                        }
                    }
                }
            }),
            TF_ES_ASYNCDONTCARE | TF_ES_READWRITE,
        );

        let error = match requested {
            // ran now, or queued: the session takes the work when it runs
            Ok(_) => return Ok(()),
            Err(error) => error,
        };
        let Some(work) = work.take() else {
            // the session ran and the work itself failed
            return Err(error);
        };
        // the work has not run
        match hresult(&error) {
            Some(code) if code == TF_E_LOCKED && retries > 0 => {
                tracing::debug!("Another text service holds the lock; trying again later");
                defer(Box::new(move || {
                    let factory: &TextServiceFactory = unsafe { this.as_impl() };
                    // Still applies if nothing else took over in between:
                    // the same activation, and the same composition or none.
                    let current = factory.borrow().is_ok_and(|text_service| {
                        text_service.activation == activation
                            && text_service.borrow_composition().is_ok_and(|current| {
                                current
                                    .tip_composition
                                    .as_ref()
                                    .map_or(true, |current| *current == composition)
                            })
                    });
                    if !current {
                        tracing::debug!("Dropping deferred work for an outdated composition");
                        return;
                    }
                    let result =
                        factory.run_outside_key_event_with_retries(if_refused, work, retries - 1);
                    if let Err(error) = result {
                        tracing::warn!("Deferred work failed: {error:?}");
                    }
                }))
            }
            Some(code) if code == TS_E_READONLY || code == TF_E_DISCONNECTED => {
                self.refused(if_refused, work, error)
            }
            // possibly passing (out of memory, a lock that stayed taken):
            // keep the composition, which can still be ended later
            _ => Err(error),
        }
    }

    /// The composition's document cannot be edited any more.
    fn refused(
        &self,
        if_refused: IfRefused,
        work: impl FnOnce(&TextServiceFactory) -> Result<()>,
        error: anyhow::Error,
    ) -> Result<()> {
        tracing::warn!("Cannot edit the composition any more: {error:?}");
        match if_refused {
            IfRefused::RunWithoutComposition => {
                self.borrow()?.borrow_mut_composition()?.tip_composition = None;
                work(self)
            }
            IfRefused::Skip => Ok(()),
        }
    }

    /// The client id, the TSF composition and the display attribute atom.
    /// No borrow of the text service is kept: the edit sessions using them
    /// can re-enter it, e.g. through OnCompositionTerminated when the
    /// application ends the composition in reaction to an edit.
    fn session_state(&self) -> Result<(u32, Option<ITfComposition>, Option<u32>)> {
        let text_service = self.borrow()?;
        let composition = text_service.borrow_composition()?.tip_composition.clone();
        let display_attribute = text_service
            .display_attribute_atom
            .get(&GUID_DISPLAY_ATTRIBUTE)
            .copied();
        Ok((text_service.tid, composition, display_attribute))
    }

    #[tracing::instrument]
    pub fn start_composition(&self) -> Result<()> {
        tracing::debug!("start_composition");

        // end_composition borrows the text service itself, so check without
        // holding a borrow
        let tip_exists = self.borrow()?.borrow_composition()?.tip_composition.is_some();
        if tip_exists {
            self.end_composition()?;
        }

        let (tid, context, context_composition, sink, insert) = {
            let text_service = self.borrow()?;
            (
                text_service.tid,
                text_service.context()?,
                text_service.context::<ITfContextComposition>()?,
                text_service.this::<ITfCompositionSink>()?,
                text_service.context::<ITfInsertAtSelection>()?,
            )
        };

        let composition = edit_session::<ITfComposition>(
            tid,
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
        self.borrow()?.borrow_mut_composition()?.tip_composition = Some(composition);

        Ok(())
    }

    /// Ends the TSF composition, committing its text, and forgets it. It is
    /// forgotten even if the document cannot be edited: the composition
    /// holds this object as its sink, so keeping it would keep both alive,
    /// and the engine would go on editing a composition it cannot end.
    #[tracing::instrument]
    pub fn end_composition(&self) -> Result<()> {
        tracing::debug!("end_composition");
        let tid = self.borrow()?.tid;
        let composition = self.take_tip_composition()?;

        if let Some(composition) = composition {
            let context = composition_context(&composition)?;
            edit_session(
                tid,
                context.clone(),
                end_composition_session(composition, context),
            )?;
        } else {
            tracing::warn!("Composition is not started");
        }

        Ok(())
    }

    /// Ends the composition when the text service is deactivated, outside
    /// key handling, where TSF rarely grants a synchronous lock. The TSF
    /// composition is ended now if the document can be locked, otherwise
    /// when TSF runs the queued session; the engine, the candidate window
    /// and the server's text are reset right away either way.
    pub fn end_composition_on_deactivate(&self) -> Result<()> {
        let tid = self.borrow()?.tid;
        let tip_composition = self.take_tip_composition()?;
        let composing = self.has_active_composition()?;

        let mut result = Ok(());
        if let Some(composition) = tip_composition {
            let ended = composition_context(&composition).and_then(|context| {
                run_edit_session(
                    tid,
                    context.clone(),
                    end_composition_session(composition, context),
                    TF_ES_ASYNCDONTCARE | TF_ES_READWRITE,
                )
            });
            if let Err(error) = ended {
                tracing::warn!("Cannot end the composition in the document: {error:?}");
                result = Err(error);
            }
        }

        if composing {
            // without a TSF composition EndComposition only resets the rest
            self.handle_action(&[ClientAction::EndComposition], CompositionState::None)?;
        }
        result
    }

    fn take_tip_composition(&self) -> Result<Option<ITfComposition>> {
        Ok(self
            .borrow()?
            .borrow_mut_composition()?
            .tip_composition
            .take())
    }

    #[tracing::instrument]
    pub fn set_text(&self, text: &str, subtext: &str) -> Result<()> {
        let (tid, composition, display_attribute) = self.session_state()?;

        if let Some(composition) = composition {
            edit_session(
                tid,
                composition_context(&composition)?,
                Rc::new({
                    // TSF positions count UTF-16 code units
                    let text_len = text.encode_utf16().count() as i32;

                    // unpadded is all you need!
                    let text = format!("{text}{subtext}").as_str().to_wide_16_unpadded();
                    let context = composition_context(&composition)?;

                    move |cookie| unsafe {
                        let range = composition.GetRange()?;
                        range.SetText(cookie, TF_ST_CORRECTION, &text)?;

                        // first, set the display attribute to the "text" part.
                        // SetValue fails on an empty range, which deleting the
                        // last character produces.
                        if text_len > 0 {
                            let text_range = range.Clone()?;
                            text_range.Collapse(cookie, TF_ANCHOR_START)?;
                            let mut shifted: i32 = 0;
                            text_range.ShiftEnd(
                                cookie,
                                text_len,
                                &mut shifted,
                                std::ptr::null(),
                            )?;
                            if let Some(display_attribute) = display_attribute {
                                let pvar = VARIANT::from(display_attribute as i32);
                                let prop = context.GetProperty(&GUID_PROP_ATTRIBUTE)?;
                                prop.SetValue(cookie, &text_range, &pvar)?;
                            }
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
        let (tid, composition, display_attribute) = self.session_state()?;

        if let Some(composition) = composition {
            edit_session(
                tid,
                composition_context(&composition)?,
                Rc::new({
                    // TSF positions count UTF-16 code units
                    let text_len = text.encode_utf16().count() as i32;
                    let subtext = subtext.to_wide_16_unpadded();
                    let context = composition_context(&composition)?;

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

                        if let Some(display_attribute) = display_attribute {
                            let pvar = VARIANT::from(display_attribute as i32);
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
