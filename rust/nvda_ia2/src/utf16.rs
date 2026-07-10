//! Compile-time ASCII-to-UTF-16 conversion for constant string literals.
//!
//! `fill_vbuf` and the helpers it calls write dozens of constant
//! attribute names and values per node, across thousands of nodes per
//! render. Encoding those constants at compile time with [`utf16`]
//! avoids re-running `encode_utf16().collect()` (a fresh heap
//! allocation) on every call.

/// Convert a compile-time ASCII byte-string literal to a UTF-16 array.
///
/// Every byte must be ASCII (`< 0x80`); a non-ASCII byte triggers a
/// panic. In a `const` context that panic fires at compile time, so a
/// bad literal fails the build rather than producing a mangled value.
pub(crate) const fn utf16<const N: usize>(bytes: &[u8; N]) -> [u16; N] {
    let mut out = [0u16; N];
    let mut i = 0;
    while i < N {
        assert!(bytes[i] < 0x80, "utf16() requires ASCII bytes");
        out[i] = bytes[i] as u16;
        i += 1;
    }
    out
}
