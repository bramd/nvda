//! Common IA2 types. Re-exports the `BSTR` / `HRESULT` aliases the rest
//! of the crate uses, plus structs from the IDL.

pub use windows::core::{BSTR, HRESULT, Result};
pub use windows::Win32::Foundation::{S_FALSE, S_OK};

/// Mirrors `IA2TextSegment` from `include/ia2/api/AccessibleText.idl:63`.
///
/// `text` is a server-allocated BSTR. Callers must `SysFreeString` it (the
/// `windows::core::BSTR` `Drop` impl does this automatically when this struct
/// is owned).
#[repr(C)]
#[derive(Default)]
pub struct IA2TextSegment {
    pub text: BSTR,
    pub start: i32,
    pub end: i32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{align_of, size_of};

    /// The struct is passed by pointer over the COM ABI. Its layout must
    /// match the C declaration: pointer (BSTR), 4-byte long, 4-byte long.
    /// On x86_64 that's 8 + 4 + 4 = 16 bytes.
    #[test]
    fn ia2_text_segment_layout() {
        assert_eq!(size_of::<IA2TextSegment>(), 16);
        assert_eq!(align_of::<IA2TextSegment>(), 8);
    }
}
