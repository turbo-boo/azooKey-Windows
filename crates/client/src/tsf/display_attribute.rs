use std::{
    cell::Cell,
    sync::atomic::{AtomicUsize, Ordering::Relaxed},
};
use windows::{
    core::{implement, BSTR, GUID},
    Win32::{
        Foundation::S_FALSE,
        UI::TextServices::{
            IEnumTfDisplayAttributeInfo, IEnumTfDisplayAttributeInfo_Impl,
            ITfDisplayAttributeInfo, ITfDisplayAttributeInfo_Impl,
            ITfDisplayAttributeProvider_Impl, TF_DISPLAYATTRIBUTE,
        },
    },
};

use anyhow::Result;

use crate::globals::{DISPLAY_ATTRIBUTE, GUID_DISPLAY_ATTRIBUTE};

use super::factory::TextServiceFactory_Impl;

// class for display attribute (color, bold, underline, etc.)
impl ITfDisplayAttributeProvider_Impl for TextServiceFactory_Impl {
    #[macros::anyhow]
    fn EnumDisplayAttributeInfo(&self) -> Result<IEnumTfDisplayAttributeInfo> {
        let enum_info = EnumDisplayAttributeInfo::new();
        Ok(enum_info.into())
    }

    #[macros::anyhow]
    fn GetDisplayAttributeInfo(
        &self,
        guid: *const windows_core::GUID,
    ) -> Result<ITfDisplayAttributeInfo> {
        let guid = unsafe { *guid };
        let attributes = EnumDisplayAttributeInfo::new();
        for attribute in attributes.attributes {
            if attribute.guid == guid {
                return Ok(attribute.into());
            }
        }
        anyhow::bail!("Display attribute not found");
    }
}

#[derive(Clone)]
#[implement(ITfDisplayAttributeInfo)]
pub struct DisplayAttributeInfo {
    pub guid: GUID,
    attribute: Cell<TF_DISPLAYATTRIBUTE>,
    attribute_backup: TF_DISPLAYATTRIBUTE,
}

impl DisplayAttributeInfo {
    pub fn new(guid: GUID, attribute: TF_DISPLAYATTRIBUTE) -> Self {
        DisplayAttributeInfo {
            guid,
            attribute: Cell::new(attribute),
            attribute_backup: attribute,
        }
    }
}

impl ITfDisplayAttributeInfo_Impl for DisplayAttributeInfo_Impl {
    #[macros::anyhow]
    fn GetAttributeInfo(&self, pda: *mut TF_DISPLAYATTRIBUTE) -> Result<()> {
        unsafe {
            *pda = self.attribute.get();
        }
        Ok(())
    }

    #[macros::anyhow]
    fn GetGUID(&self) -> Result<GUID> {
        Ok(self.guid)
    }

    #[macros::anyhow]
    fn Reset(&self) -> Result<()> {
        self.attribute.set(self.attribute_backup);
        Ok(())
    }

    #[macros::anyhow]
    fn GetDescription(&self) -> Result<BSTR> {
        Ok(BSTR::default())
    }

    #[macros::anyhow]
    fn SetAttributeInfo(&self, pda: *const TF_DISPLAYATTRIBUTE) -> Result<()> {
        unsafe {
            self.attribute.set(*pda);
        }
        Ok(())
    }
}

#[implement(IEnumTfDisplayAttributeInfo)]
pub struct EnumDisplayAttributeInfo {
    pub attributes: Vec<DisplayAttributeInfo>,
    index: AtomicUsize,
}

#[allow(clippy::new_without_default)]
impl EnumDisplayAttributeInfo {
    pub fn new() -> Self {
        let attributes = vec![DisplayAttributeInfo::new(
            GUID_DISPLAY_ATTRIBUTE,
            DISPLAY_ATTRIBUTE,
        )];

        EnumDisplayAttributeInfo {
            attributes,
            index: AtomicUsize::new(0),
        }
    }
}

impl IEnumTfDisplayAttributeInfo_Impl for EnumDisplayAttributeInfo_Impl {
    #[macros::anyhow]
    fn Clone(&self) -> Result<IEnumTfDisplayAttributeInfo> {
        let clone = EnumDisplayAttributeInfo::new();
        clone.index.store(self.index.load(Relaxed), Relaxed);
        Ok(clone.into())
    }

    // Implemented without #[macros::anyhow]: the end of the enumeration must be
    // reported as S_FALSE, which callers passing a NULL pcfetched rely on.
    fn Next(
        &self,
        ulcount: u32,
        rginfo: *mut Option<ITfDisplayAttributeInfo>,
        pcfetched: *mut u32,
    ) -> windows::core::Result<()> {
        let mut fetched = 0;
        let mut index = self.index.load(Relaxed);

        while fetched < ulcount && index < self.attributes.len() {
            let attribute = self.attributes[index].clone();
            unsafe { rginfo.add(fetched as usize).write(Some(attribute.into())) };
            fetched += 1;
            index += 1;
        }

        self.index.store(index, Relaxed);
        if !pcfetched.is_null() {
            unsafe { *pcfetched = fetched };
        }

        if fetched == ulcount {
            Ok(())
        } else {
            Err(S_FALSE.into())
        }
    }

    #[macros::anyhow]
    fn Reset(&self) -> Result<()> {
        self.index.store(0, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }

    #[macros::anyhow]
    fn Skip(&self, ulcount: u32) -> Result<()> {
        let index = self.index.load(Relaxed) + ulcount as usize;
        self.index.store(index, Relaxed);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::EnumDisplayAttributeInfo;
    use windows::{
        core::{Interface as _, HRESULT},
        Win32::{
            Foundation::{S_FALSE, S_OK},
            UI::TextServices::{IEnumTfDisplayAttributeInfo, ITfDisplayAttributeInfo},
        },
    };

    fn next(
        enumerator: &IEnumTfDisplayAttributeInfo,
        slots: &mut [Option<ITfDisplayAttributeInfo>],
        fetched: *mut u32,
    ) -> HRESULT {
        // Call through the vtable: the safe wrapper folds S_FALSE into Ok.
        unsafe {
            (enumerator.vtable().Next)(
                enumerator.as_raw(),
                slots.len() as u32,
                slots.as_mut_ptr().cast(),
                fetched,
            )
        }
    }

    #[test]
    fn next_accepts_null_fetched_pointer() {
        let enumerator: IEnumTfDisplayAttributeInfo = EnumDisplayAttributeInfo::new().into();
        let mut slot = [None];

        assert_eq!(next(&enumerator, &mut slot, std::ptr::null_mut()), S_OK);
        assert!(slot[0].is_some());
        // exhausted: no element, still no write through the NULL pointer
        let mut slot = [None];
        assert_eq!(next(&enumerator, &mut slot, std::ptr::null_mut()), S_FALSE);
        assert!(slot[0].is_none());
    }

    #[test]
    fn next_fills_each_slot_and_reports_short_reads() {
        let enumerator: IEnumTfDisplayAttributeInfo = EnumDisplayAttributeInfo::new().into();
        let available = EnumDisplayAttributeInfo::new().attributes.len();
        let mut slots = vec![None; available + 1];
        let mut fetched = u32::MAX;

        assert_eq!(next(&enumerator, &mut slots, &mut fetched), S_FALSE);
        assert_eq!(fetched as usize, available);
        assert!(slots[..available].iter().all(Option::is_some));
        assert!(slots[available].is_none());
    }
}
