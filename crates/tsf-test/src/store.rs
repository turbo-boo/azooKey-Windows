//! A minimal in-memory TSF document: plain text and one selection, behind the
//! lock protocol TSF requires (ITextStoreACP). The text service edits it
//! through TSF; the test reads it between keys, when nothing holds the lock.

use std::cell::RefCell;

use windows::{
    core::{implement, AsImpl, Error, IUnknown, Interface, Result, GUID, HRESULT, PCWSTR, PWSTR},
    Win32::{
        Foundation::{BOOL, E_INVALIDARG, E_NOTIMPL, HWND, POINT, RECT, S_OK},
        System::{
            Com::{IDataObject, FORMATETC},
            Ole::{CONNECT_E_ADVISELIMIT, CONNECT_E_NOCONNECTION},
        },
        UI::{
            TextServices::{
                ITextStoreACP, ITextStoreACPSink, ITextStoreACP_Impl, TEXT_STORE_LOCK_FLAGS,
                TF_IAS_NOQUERY, TF_IAS_QUERYONLY, TS_AE_END, TS_ATTRVAL, TS_DEFAULT_SELECTION,
                TS_E_INVALIDPOS, TS_E_NOLOCK, TS_E_SYNCHRONOUS, TS_LF_READWRITE, TS_LF_SYNC,
                TS_RT_PLAIN, TS_RUNINFO, TS_SELECTIONSTYLE, TS_SELECTION_ACP, TS_SS_NOHIDDENTEXT,
                TS_STATUS, TS_S_ASYNC, TS_TEXTCHANGE,
            },
            WindowsAndMessaging::GetWindowRect,
        },
    },
};

/// The text, read by the test while no lock is held.
#[derive(Clone, Default)]
pub struct Contents {
    pub text: Vec<u16>,
    /// start, end; the active end is always the end
    pub selection: (i32, i32),
}

impl Contents {
    pub fn text(&self) -> String {
        String::from_utf16_lossy(&self.text)
    }

    fn len(&self) -> i32 {
        self.text.len() as i32
    }

    fn valid(&self, start: i32, end: i32) -> bool {
        0 <= start && start <= end && end <= self.len()
    }

    /// Replaces `start..end` and puts the caret after the new text.
    fn replace(&mut self, start: i32, end: i32, new: &[u16]) -> TS_TEXTCHANGE {
        self.text.splice(start as usize..end as usize, new.iter().copied());
        let new_end = start + new.len() as i32;
        self.selection = (new_end, new_end);
        TS_TEXTCHANGE {
            acpStart: start,
            acpOldEnd: end,
            acpNewEnd: new_end,
        }
    }
}

#[derive(Default)]
struct State {
    contents: Contents,
    sink: Option<ITextStoreACPSink>,
    /// flags of the lock being granted, 0 while unlocked
    lock: u32,
    /// an asynchronous request made while locked, granted when it is released
    queued: u32,
}

#[implement(ITextStoreACP)]
pub struct TextStore {
    state: RefCell<State>,
    window: HWND,
}

impl TextStore {
    pub fn create(window: HWND) -> ITextStoreACP {
        TextStore {
            state: RefCell::default(),
            window,
        }
        .into()
    }
}

/// The contents of `store`, which must be a [`TextStore`].
pub fn contents(store: &ITextStoreACP) -> Contents {
    // SAFETY: only TextStore::create makes the stores this test uses
    let store: &TextStore = unsafe { store.as_impl() };
    store.state.borrow().contents.clone()
}

impl TextStore_Impl {
    fn require_lock(&self, write: bool) -> Result<()> {
        let lock = self.state.borrow().lock;
        let needed = if write { TS_LF_READWRITE.0 } else { 0 };
        if lock == 0 || lock & needed != needed {
            return Err(TS_E_NOLOCK.into());
        }
        Ok(())
    }

    /// Grants a lock: TSF reads and edits the document inside OnLockGranted,
    /// calling back into this store, so no borrow is held across it.
    fn grant(&self, flags: u32) -> HRESULT {
        let sink = {
            let mut state = self.state.borrow_mut();
            state.lock = flags;
            state.sink.clone()
        };
        let result = match sink {
            Some(sink) => unsafe { sink.OnLockGranted(TEXT_STORE_LOCK_FLAGS(flags)) }
                .map_or_else(|error| error.code(), |()| S_OK),
            None => TS_E_NOLOCK,
        };
        self.state.borrow_mut().lock = 0;
        result
    }
}

impl ITextStoreACP_Impl for TextStore_Impl {
    fn AdviseSink(&self, riid: *const GUID, punk: Option<&IUnknown>, _dwmask: u32) -> Result<()> {
        if unsafe { *riid } != ITextStoreACPSink::IID {
            return Err(E_INVALIDARG.into());
        }
        let sink: ITextStoreACPSink = punk.ok_or(Error::from(E_INVALIDARG))?.cast()?;
        let mut state = self.state.borrow_mut();
        match &state.sink {
            // advising the same sink again only changes the mask
            Some(current) if current.cast::<IUnknown>()? == sink.cast::<IUnknown>()? => Ok(()),
            Some(_) => Err(CONNECT_E_ADVISELIMIT.into()),
            None => {
                state.sink = Some(sink);
                Ok(())
            }
        }
    }

    fn UnadviseSink(&self, punk: Option<&IUnknown>) -> Result<()> {
        let mut state = self.state.borrow_mut();
        let current = state.sink.as_ref().ok_or(Error::from(CONNECT_E_NOCONNECTION))?;
        if Some(current.cast::<IUnknown>()?) != punk.map(|punk| punk.cast::<IUnknown>()).transpose()? {
            return Err(CONNECT_E_NOCONNECTION.into());
        }
        state.sink = None;
        Ok(())
    }

    fn RequestLock(&self, dwlockflags: u32) -> Result<HRESULT> {
        let wanted = dwlockflags & !TS_LF_SYNC;
        if self.state.borrow().lock != 0 {
            if dwlockflags & TS_LF_SYNC != 0 {
                return Ok(TS_E_SYNCHRONOUS);
            }
            // one queued request; a read-write one covers a read one
            let mut state = self.state.borrow_mut();
            state.queued |= wanted;
            return Ok(TS_S_ASYNC);
        }
        let result = self.grant(wanted);
        loop {
            let queued = std::mem::take(&mut self.state.borrow_mut().queued);
            if queued == 0 {
                break;
            }
            // an asynchronous request has no caller left to report to
            let _ = self.grant(queued);
        }
        Ok(result)
    }

    fn GetStatus(&self) -> Result<TS_STATUS> {
        Ok(TS_STATUS {
            dwDynamicFlags: 0,
            dwStaticFlags: TS_SS_NOHIDDENTEXT,
        })
    }

    fn QueryInsert(
        &self,
        acpteststart: i32,
        acptestend: i32,
        _cch: u32,
        pacpresultstart: *mut i32,
        pacpresultend: *mut i32,
    ) -> Result<()> {
        if !self.state.borrow().contents.valid(acpteststart, acptestend) {
            return Err(E_INVALIDARG.into());
        }
        unsafe {
            *pacpresultstart = acpteststart;
            *pacpresultend = acptestend;
        }
        Ok(())
    }

    fn GetSelection(
        &self,
        ulindex: u32,
        ulcount: u32,
        pselection: *mut TS_SELECTION_ACP,
        pcfetched: *mut u32,
    ) -> Result<()> {
        self.require_lock(false)?;
        if ulindex != 0 && ulindex != TS_DEFAULT_SELECTION {
            return Err(E_INVALIDARG.into());
        }
        let (start, end) = self.state.borrow().contents.selection;
        unsafe {
            *pcfetched = 0;
            if ulcount > 0 {
                *pselection = TS_SELECTION_ACP {
                    acpStart: start,
                    acpEnd: end,
                    style: TS_SELECTIONSTYLE {
                        ase: TS_AE_END,
                        fInterimChar: false.into(),
                    },
                };
                *pcfetched = 1;
            }
        }
        Ok(())
    }

    fn SetSelection(&self, ulcount: u32, pselection: *const TS_SELECTION_ACP) -> Result<()> {
        self.require_lock(true)?;
        if ulcount != 1 {
            return Err(E_INVALIDARG.into());
        }
        let selection = unsafe { *pselection };
        let mut state = self.state.borrow_mut();
        if !state.contents.valid(selection.acpStart, selection.acpEnd) {
            return Err(TS_E_INVALIDPOS.into());
        }
        state.contents.selection = (selection.acpStart, selection.acpEnd);
        Ok(())
    }

    fn GetText(
        &self,
        acpstart: i32,
        acpend: i32,
        pchplain: PWSTR,
        cchplainreq: u32,
        pcchplainret: *mut u32,
        prgruninfo: *mut TS_RUNINFO,
        cruninforeq: u32,
        pcruninforet: *mut u32,
        pacpnext: *mut i32,
    ) -> Result<()> {
        self.require_lock(false)?;
        let state = self.state.borrow();
        let contents = &state.contents;
        let end = if acpend == -1 { contents.len() } else { acpend };
        if !contents.valid(acpstart, end) {
            return Err(TS_E_INVALIDPOS.into());
        }
        let count = ((end - acpstart) as u32).min(cchplainreq) as usize;
        let start = acpstart as usize;
        unsafe {
            if count > 0 && !pchplain.is_null() {
                std::ptr::copy_nonoverlapping(contents.text[start..].as_ptr(), pchplain.0, count);
            }
            *pcchplainret = count as u32;
            *pcruninforet = 0;
            if count > 0 && cruninforeq > 0 && !prgruninfo.is_null() {
                *prgruninfo = TS_RUNINFO {
                    uCount: count as u32,
                    r#type: TS_RT_PLAIN,
                };
                *pcruninforet = 1;
            }
            *pacpnext = acpstart + count as i32;
        }
        Ok(())
    }

    fn SetText(
        &self,
        _dwflags: u32,
        acpstart: i32,
        acpend: i32,
        pchtext: &PCWSTR,
        cch: u32,
    ) -> Result<TS_TEXTCHANGE> {
        self.require_lock(true)?;
        let new = text_slice(pchtext, cch);
        let mut state = self.state.borrow_mut();
        if !state.contents.valid(acpstart, acpend) {
            return Err(TS_E_INVALIDPOS.into());
        }
        Ok(state.contents.replace(acpstart, acpend, &new))
    }

    fn GetFormattedText(&self, _acpstart: i32, _acpend: i32) -> Result<IDataObject> {
        Err(E_NOTIMPL.into())
    }

    fn GetEmbedded(&self, _acppos: i32, _rguidservice: *const GUID, _riid: *const GUID) -> Result<IUnknown> {
        Err(E_NOTIMPL.into())
    }

    fn QueryInsertEmbedded(&self, _pguidservice: *const GUID, _pformatetc: *const FORMATETC) -> Result<BOOL> {
        Ok(false.into())
    }

    fn InsertEmbedded(
        &self,
        _dwflags: u32,
        _acpstart: i32,
        _acpend: i32,
        _pdataobject: Option<&IDataObject>,
    ) -> Result<TS_TEXTCHANGE> {
        Err(E_NOTIMPL.into())
    }

    fn InsertTextAtSelection(
        &self,
        dwflags: u32,
        pchtext: &PCWSTR,
        cch: u32,
        pacpstart: *mut i32,
        pacpend: *mut i32,
        pchange: *mut TS_TEXTCHANGE,
    ) -> Result<()> {
        self.require_lock(dwflags & TF_IAS_QUERYONLY.0 == 0)?;
        let mut state = self.state.borrow_mut();
        let (start, end) = state.contents.selection;
        if dwflags & TF_IAS_QUERYONLY.0 != 0 {
            unsafe {
                *pacpstart = start;
                *pacpend = end;
            }
            return Ok(());
        }
        let new = text_slice(pchtext, cch);
        let change = state.contents.replace(start, end, &new);
        unsafe {
            if dwflags & TF_IAS_NOQUERY.0 == 0 {
                *pacpstart = change.acpStart;
                *pacpend = change.acpNewEnd;
            }
            if !pchange.is_null() {
                *pchange = change;
            }
        }
        Ok(())
    }

    fn InsertEmbeddedAtSelection(
        &self,
        _dwflags: u32,
        _pdataobject: Option<&IDataObject>,
        _pacpstart: *mut i32,
        _pacpend: *mut i32,
        _pchange: *mut TS_TEXTCHANGE,
    ) -> Result<()> {
        Err(E_NOTIMPL.into())
    }

    // The document has no attributes of its own.
    fn RequestSupportedAttrs(&self, _dwflags: u32, _cfilterattrs: u32, _pafilterattrs: *const GUID) -> Result<()> {
        Ok(())
    }

    fn RequestAttrsAtPosition(
        &self,
        _acppos: i32,
        _cfilterattrs: u32,
        _pafilterattrs: *const GUID,
        _dwflags: u32,
    ) -> Result<()> {
        Ok(())
    }

    fn RequestAttrsTransitioningAtPosition(
        &self,
        _acppos: i32,
        _cfilterattrs: u32,
        _pafilterattrs: *const GUID,
        _dwflags: u32,
    ) -> Result<()> {
        Ok(())
    }

    fn FindNextAttrTransition(
        &self,
        _acpstart: i32,
        acphalt: i32,
        _cfilterattrs: u32,
        _pafilterattrs: *const GUID,
        _dwflags: u32,
        pacpnext: *mut i32,
        pffound: *mut BOOL,
        plfoundoffset: *mut i32,
    ) -> Result<()> {
        unsafe {
            *pacpnext = acphalt;
            *pffound = false.into();
            *plfoundoffset = 0;
        }
        Ok(())
    }

    fn RetrieveRequestedAttrs(&self, _ulcount: u32, _paattrvals: *mut TS_ATTRVAL, pcfetched: *mut u32) -> Result<()> {
        unsafe { *pcfetched = 0 };
        Ok(())
    }

    fn GetEndACP(&self) -> Result<i32> {
        self.require_lock(false)?;
        Ok(self.state.borrow().contents.len())
    }

    fn GetActiveView(&self) -> Result<u32> {
        Ok(1)
    }

    fn GetACPFromPoint(&self, _vcview: u32, _ptscreen: *const POINT, _dwflags: u32) -> Result<i32> {
        Err(E_NOTIMPL.into())
    }

    /// Characters are laid out 16 pixels apart on one line of the window.
    fn GetTextExt(
        &self,
        _vcview: u32,
        acpstart: i32,
        acpend: i32,
        prc: *mut RECT,
        pfclipped: *mut BOOL,
    ) -> Result<()> {
        self.require_lock(false)?;
        let window = self.GetScreenExt(1)?;
        unsafe {
            *prc = RECT {
                left: window.left + 8 + acpstart * 16,
                top: window.top + 8,
                right: window.left + 8 + acpend.max(acpstart + 1) * 16,
                bottom: window.top + 28,
            };
            *pfclipped = false.into();
        }
        Ok(())
    }

    fn GetScreenExt(&self, _vcview: u32) -> Result<RECT> {
        let mut rect = RECT::default();
        unsafe { GetWindowRect(self.window, &mut rect)? };
        Ok(rect)
    }

    fn GetWnd(&self, _vcview: u32) -> Result<HWND> {
        Ok(self.window)
    }
}

fn text_slice(text: &PCWSTR, count: u32) -> Vec<u16> {
    if count == 0 || text.is_null() {
        return Vec::new();
    }
    unsafe { std::slice::from_raw_parts(text.0, count as usize) }.to_vec()
}
