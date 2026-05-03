//! Rust port of `IA2AttribsToMap` and `fetchIA2Attributes` from
//! `nvdaHelper/common/ia2utils.cpp`. Exposed via `extern "C"` callback
//! shims so the C++ wrappers in `ia2utils.cpp` can keep their existing
//! `std::map<std::wstring, std::wstring>&` API.

use std::collections::BTreeMap;

/// Parse an IA2-attributes string of the form `name:value;name:value;...`
/// into a sorted map.
///
/// - `:` separates key from value.
/// - `;` separates pairs.
/// - `\` escapes the next character (so `\:` is a literal colon, etc.).
/// - The trailing `;` is optional.
/// - Empty keys are dropped (mirrors the C++ behaviour at ia2utils.cpp:50).
/// - The `src` value is truncated if it starts with `data:` and contains
///   `base64,` (mirrors the C++ behaviour at ia2utils.cpp:62-74).
///
/// `BTreeMap<String, String>` is used (not `HashMap`) for deterministic
/// iteration in tests; the C++ side uses `std::map` which is also ordered.
pub fn parse_attribs(input: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut key = String::new();
    let mut value = String::new();
    let mut in_escape = false;
    let mut have_key = false;

    for ch in input.chars() {
        if in_escape {
            if have_key {
                value.push(ch);
            } else {
                key.push(ch);
            }
            in_escape = false;
        } else if ch == '\\' {
            in_escape = true;
        } else if ch == ':' && !have_key {
            have_key = true;
        } else if ch == ';' {
            if have_key && !key.is_empty() {
                out.insert(std::mem::take(&mut key), std::mem::take(&mut value));
            } else {
                key.clear();
                value.clear();
            }
            have_key = false;
        } else if have_key {
            value.push(ch);
        } else {
            key.push(ch);
        }
    }
    if have_key && !key.is_empty() {
        out.insert(key, value);
    }
    truncate_base64_src(&mut out);
    out
}

fn truncate_base64_src(map: &mut BTreeMap<String, String>) {
    const PREFIX: &str = "data:";
    const NEEDLE: &str = "base64,";
    if let Some(src) = map.get_mut("src") {
        if src.starts_with(PREFIX) {
            if let Some(pos) = src.find(NEEDLE) {
                let truncate_at = pos + NEEDLE.len();
                src.truncate(truncate_at);
                src.push_str("<truncated>");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn empty_input_yields_empty_map() {
        assert_eq!(parse_attribs(""), BTreeMap::new());
    }

    #[test]
    fn single_pair_with_trailing_semicolon() {
        assert_eq!(parse_attribs("foo:bar;"), map(&[("foo", "bar")]));
    }

    #[test]
    fn single_pair_without_trailing_semicolon() {
        assert_eq!(parse_attribs("foo:bar"), map(&[("foo", "bar")]));
    }

    #[test]
    fn multiple_pairs() {
        assert_eq!(
            parse_attribs("a:1;b:2;c:3;"),
            map(&[("a", "1"), ("b", "2"), ("c", "3")])
        );
    }

    #[test]
    fn empty_value_is_preserved() {
        assert_eq!(parse_attribs("foo:;"), map(&[("foo", "")]));
    }

    #[test]
    fn empty_key_is_dropped() {
        // Mirrors C++ behaviour: !key.empty() guards the insert.
        assert_eq!(parse_attribs(":bar;"), BTreeMap::new());
    }

    #[test]
    fn escaped_colon_in_value() {
        assert_eq!(parse_attribs("foo:a\\:b;"), map(&[("foo", "a:b")]));
    }

    #[test]
    fn escaped_semicolon_in_value() {
        assert_eq!(parse_attribs("foo:a\\;b;"), map(&[("foo", "a;b")]));
    }

    #[test]
    fn escaped_backslash_in_value() {
        assert_eq!(parse_attribs("foo:a\\\\b;"), map(&[("foo", "a\\b")]));
    }

    #[test]
    fn escaped_colon_in_key() {
        assert_eq!(parse_attribs("a\\:b:val;"), map(&[("a:b", "val")]));
    }

    #[test]
    fn duplicate_key_keeps_last_value() {
        // Mirrors std::map's `attribsMap[key] = str` overwrite semantics.
        assert_eq!(parse_attribs("k:v1;k:v2;"), map(&[("k", "v2")]));
    }

    #[test]
    fn src_data_base64_is_truncated() {
        assert_eq!(
            parse_attribs("src:data:image/png;base64,iVBORw0KGgo;"),
            // Note: the `;` inside the base64 part isn't escaped, so the
            // semicolon ends the value at `data:image/png`. This matches
            // the C++ parser; the test documents the behaviour. The
            // truncation only fires when the value (post-parse) still
            // starts with `data:` and contains `base64,`.
            // Adjusted expectation: the value at this point is just
            // "data:image/png" -- no `base64,`, so no truncation occurs.
            map(&[("src", "data:image/png")]),
        );
    }

    #[test]
    fn src_with_escaped_semicolon_truncated() {
        assert_eq!(
            parse_attribs("src:data:image/png\\;base64,iVBORw0KGgo;"),
            map(&[("src", "data:image/png;base64,<truncated>")]),
        );
    }

    #[test]
    fn src_without_data_prefix_not_truncated() {
        assert_eq!(
            parse_attribs("src:http://example.com/img.png;"),
            map(&[("src", "http://example.com/img.png")]),
        );
    }

    #[test]
    fn non_src_data_value_not_truncated() {
        assert_eq!(
            parse_attribs("href:data:text/plain\\;base64,abc;"),
            map(&[("href", "data:text/plain;base64,abc")]),
        );
    }
}

// ---------------------------------------------------------------------------
// extern "C" shim
// ---------------------------------------------------------------------------

/// Callback invoked once per attribute. `key_ptr`/`val_ptr` point to UTF-16
/// code units (without a NUL terminator); `key_len`/`val_len` are code-unit
/// counts. Both pointers are valid only for the duration of the call.
pub type AttribCallback = unsafe extern "C" fn(
    ctx: *mut core::ffi::c_void,
    key_ptr: *const u16,
    key_len: usize,
    val_ptr: *const u16,
    val_len: usize,
);

/// C-callable replacement for `IA2AttribsToMap`.
///
/// `input_ptr` / `input_len` point to a UTF-16 attributes string. The shim
/// parses it and invokes `cb(ctx, key, key_len, val, val_len)` once per
/// attribute. The C++ wrapper in `ia2utils.cpp` uses this to populate the
/// caller's `std::map<std::wstring, std::wstring>&`.
///
/// # Safety
/// - `input_ptr` must be valid for `input_len` u16s, or null when `input_len`
///   is 0.
/// - `cb` must be a valid function pointer; `ctx` is opaque user data passed
///   through to `cb` unchanged.
#[no_mangle]
pub unsafe extern "C" fn nvda_ia2_attribs_to_map(
    input_ptr: *const u16,
    input_len: usize,
    ctx: *mut core::ffi::c_void,
    cb: AttribCallback,
) {
    let input = if input_ptr.is_null() || input_len == 0 {
        String::new()
    } else {
        let slice = std::slice::from_raw_parts(input_ptr, input_len);
        String::from_utf16_lossy(slice)
    };
    let map = parse_attribs(&input);
    for (k, v) in map {
        let k_utf16: Vec<u16> = k.encode_utf16().collect();
        let v_utf16: Vec<u16> = v.encode_utf16().collect();
        cb(
            ctx,
            k_utf16.as_ptr(),
            k_utf16.len(),
            v_utf16.as_ptr(),
            v_utf16.len(),
        );
    }
}

#[cfg(test)]
mod shim_tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    thread_local! {
        static COLLECTED: RefCell<BTreeMap<String, String>> = const { RefCell::new(BTreeMap::new()) };
    }

    unsafe extern "C" fn collect_cb(
        _ctx: *mut core::ffi::c_void,
        key_ptr: *const u16,
        key_len: usize,
        val_ptr: *const u16,
        val_len: usize,
    ) {
        let key = String::from_utf16_lossy(std::slice::from_raw_parts(key_ptr, key_len));
        let val = String::from_utf16_lossy(std::slice::from_raw_parts(val_ptr, val_len));
        COLLECTED.with(|c| { c.borrow_mut().insert(key, val); });
    }

    #[test]
    fn shim_invokes_callback_per_pair() {
        COLLECTED.with(|c| c.borrow_mut().clear());
        let input: Vec<u16> = "a:1;b:2;".encode_utf16().collect();
        unsafe {
            nvda_ia2_attribs_to_map(
                input.as_ptr(),
                input.len(),
                core::ptr::null_mut(),
                collect_cb,
            );
        }
        COLLECTED.with(|c| {
            let m = c.borrow();
            assert_eq!(m.get("a"), Some(&"1".to_string()));
            assert_eq!(m.get("b"), Some(&"2".to_string()));
            assert_eq!(m.len(), 2);
        });
    }

    #[test]
    fn shim_handles_null_input() {
        COLLECTED.with(|c| c.borrow_mut().clear());
        unsafe {
            nvda_ia2_attribs_to_map(core::ptr::null(), 0, core::ptr::null_mut(), collect_cb);
        }
        COLLECTED.with(|c| assert!(c.borrow().is_empty()));
    }
}
