//! Port of `getTextFromIAccessible` from
//! `nvdaHelper/remote/textFromIAccessible.cpp`.
//!
//! For now this module exposes only the `is_empty_text` pure helper.
//! The full `get_text_from_iaccessible` port and its `extern "C"` shim
//! are added in a follow-up commit.

pub const OBJ_REPLACEMENT_CHAR: u16 = 0xFFFC;

/// Mirrors the C++ `isEmpty` helper in
/// `nvdaHelper/remote/textFromIAccessible.cpp:27`. A text run is "empty"
/// for our purposes if every character is either whitespace or the
/// embedded-object replacement character.
pub fn is_empty_text(chars: &[u16]) -> bool {
    chars.iter().all(|&c| c == OBJ_REPLACEMENT_CHAR || is_whitespace_w(c))
}

/// Mirrors the C runtime `iswspace` for the BMP characters NVDA actually
/// sees through BSTRs. The C++ code calls `iswspace` directly; we
/// implement the standard whitespace set ourselves to keep this a pure
/// Rust function (testable without the CRT).
fn is_whitespace_w(c: u16) -> bool {
    matches!(
        c,
        0x0009 // tab
        | 0x000A // line feed
        | 0x000B // vertical tab
        | 0x000C // form feed
        | 0x000D // carriage return
        | 0x0020 // space
        | 0x00A0 // no-break space (iswspace returns true for this in many locales,
                 // and NVDA encounters it from web content)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_slice_is_empty() {
        assert!(is_empty_text(&[]));
    }

    #[test]
    fn all_spaces_is_empty() {
        let chars: Vec<u16> = "    ".encode_utf16().collect();
        assert!(is_empty_text(&chars));
    }

    #[test]
    fn all_object_replacement_is_empty() {
        assert!(is_empty_text(&[OBJ_REPLACEMENT_CHAR; 5]));
    }

    #[test]
    fn mixed_spaces_and_object_replacement_is_empty() {
        let mut chars: Vec<u16> = " ".encode_utf16().collect();
        chars.push(OBJ_REPLACEMENT_CHAR);
        chars.extend("\t\n".encode_utf16());
        chars.push(OBJ_REPLACEMENT_CHAR);
        assert!(is_empty_text(&chars));
    }

    #[test]
    fn single_letter_is_not_empty() {
        let chars: Vec<u16> = "a".encode_utf16().collect();
        assert!(!is_empty_text(&chars));
    }

    #[test]
    fn whitespace_around_letter_is_not_empty() {
        let chars: Vec<u16> = "  a  ".encode_utf16().collect();
        assert!(!is_empty_text(&chars));
    }

    #[test]
    fn nbsp_alone_is_empty() {
        assert!(is_empty_text(&[0x00A0]));
    }
}
