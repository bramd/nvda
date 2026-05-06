//! Port of `getNameForURL` from `nvdaHelper/vbufBase/utils.cpp:22`.
//!
//! Derives a short readable name from a URL by stripping the
//! protocol prefix (or returning the path / query / anchor fragment
//! for path-style URLs), then truncating to 30 characters.

/// Replacement-character ellipsis (U+2026) appended when the name is
/// truncated. The C++ original uses `L'\x2026'`.
const ELLIPSIS: u16 = 0x2026;

/// `true` when `bytes` (UTF-16 slice viewed as a string) starts with
/// `prefix` (ASCII), case-insensitive.
fn starts_with_ascii_ci(bytes: &[u16], prefix: &str) -> bool {
    if bytes.len() < prefix.len() {
        return false;
    }
    bytes.iter().zip(prefix.bytes()).all(|(a, b)| {
        // ASCII-only lowercase comparison: only u16s in 'A'..='Z'
        // need translating; everything else is compared as-is.
        let a_lower = if (b'A' as u16..=b'Z' as u16).contains(a) {
            *a + 32
        } else {
            *a
        };
        let b_lower = b.to_ascii_lowercase();
        a_lower == b_lower as u16
    })
}

/// Derive a readable name from a URL, mirroring
/// `getNameForURL`. Returns an empty `Vec<u16>` for URLs that don't
/// produce a useful name (e.g. `data:image/...`) or for empty input.
pub(crate) fn get_name_for_url(url: &[u16]) -> Vec<u16> {
    if url.is_empty() {
        return Vec::new();
    }

    // Find first ':' (protocol separator).
    let colon_pos = url.iter().position(|&c| c == b':' as u16);

    if let Some(cp) = colon_pos {
        // Check if this is a path-based protocol (`://`).
        let has_path_scheme = url.len() >= cp + 3
            && url[cp + 1] == b'/' as u16
            && url[cp + 2] == b'/' as u16;
        if !has_path_scheme {
            // Non-path protocol (javascript:, mailto:, data:, ...).
            // Special case: data:image/... -> empty (not useful).
            if starts_with_ascii_ci(url, "data:image/") {
                return Vec::new();
            }
            // Return everything after the colon.
            return url[cp + 1..].to_vec();
        }
    }

    // Path-style URL: extract the last path segment, then append the
    // query string and fragment if present.

    // `?` index (last occurrence -- C++ uses rfind).
    let query_start: Option<usize> =
        url.iter().rposition(|&c| c == b'?' as u16);
    // `#` index (last occurrence).
    let anchor_start: Option<usize> =
        url.iter().rposition(|&c| c == b'#' as u16);

    let query_len: Option<usize> = match (query_start, anchor_start) {
        (Some(qs), Some(ans)) if ans > qs => Some(ans - qs - 1),
        _ => None,
    };

    // Path end: just before `?`, just before `#`, or end of string.
    let path_end_excl = match (query_start, anchor_start) {
        (Some(qs), _) => qs,
        (None, Some(ans)) => ans,
        (None, None) => url.len(),
    };
    // Convert to inclusive index: pathEnd in C++ is the index of the
    // last *included* path char. `path_end_excl - 1` gives that, or
    // `None` if the path is empty.
    let mut path_end_incl: Option<usize> = path_end_excl.checked_sub(1);
    let mut strip_exten = true;
    if let Some(pe) = path_end_incl {
        if url[pe] == b'/' as u16 {
            // Trailing slash -> step back; this segment is not a
            // filename so don't strip extension.
            path_end_incl = pe.checked_sub(1);
            strip_exten = false;
        }
    }

    let (path_start, path_end_final): (usize, Option<usize>) =
        if let Some(pe) = path_end_incl {
            // Find start of last segment (last `/` <= pe).
            let path_start = url[..=pe].iter().rposition(|&c| c == b'/' as u16);
            let path_start = match path_start {
                None => 0, // single-segment URL
                Some(ps) => {
                    let next = ps + 1;
                    // If this URL has a hostname (`://`) and the
                    // hostname is the only path component, don't
                    // strip the extension.
                    if let Some(cp) = colon_pos {
                        if strip_exten && next == cp + 3 {
                            strip_exten = false;
                        }
                    }
                    next
                }
            };
            // Strip extension when applicable.
            let mut pe_final = pe;
            if strip_exten {
                if let Some(ext_start) =
                    url[..=pe].iter().rposition(|&c| c == b'.' as u16)
                {
                    if ext_start > path_start {
                        pe_final = ext_start - 1;
                    }
                }
            }
            (path_start, Some(pe_final))
        } else {
            (0, None)
        };

    let mut name: Vec<u16> = Vec::new();
    if let Some(pe) = path_end_final {
        if pe >= path_start {
            name.extend_from_slice(&url[path_start..=pe]);
        }
    }
    if let Some(qs) = query_start {
        name.push(b' ' as u16);
        let q_start = qs + 1;
        let q_end = match query_len {
            Some(len) => q_start + len,
            None => url.len(),
        };
        if q_end > q_start {
            name.extend_from_slice(&url[q_start..q_end]);
        }
    }
    if let Some(ans) = anchor_start {
        name.push(b' ' as u16);
        if ans + 1 < url.len() {
            name.extend_from_slice(&url[ans + 1..]);
        }
    }

    // Truncate to 30 characters with an ellipsis.
    if name.len() > 30 {
        name.truncate(30);
        name.push(ELLIPSIS);
    }
    name
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn empty_url() {
        assert_eq!(get_name_for_url(&w("")), w(""));
    }

    #[test]
    fn javascript_url_strips_protocol() {
        assert_eq!(
            get_name_for_url(&w("javascript:doSomething()")),
            w("doSomething()")
        );
    }

    #[test]
    fn mailto_strips_protocol() {
        assert_eq!(
            get_name_for_url(&w("mailto:user@example.com")),
            w("user@example.com")
        );
    }

    #[test]
    fn data_image_returns_empty() {
        assert_eq!(
            get_name_for_url(&w("data:image/png;base64,iVBORw0KGgo")),
            w("")
        );
    }

    #[test]
    fn data_image_case_insensitive() {
        assert_eq!(get_name_for_url(&w("DATA:IMAGE/PNG;base64,xx")), w(""));
    }

    #[test]
    fn http_url_extracts_filename() {
        assert_eq!(
            get_name_for_url(&w("https://example.com/path/file.html")),
            w("file")
        );
    }

    #[test]
    fn http_trailing_slash() {
        // Trailing slash -> use parent segment, don't strip extension.
        assert_eq!(
            get_name_for_url(&w("https://example.com/path/dir/")),
            w("dir")
        );
    }

    #[test]
    fn http_hostname_only_keeps_extension() {
        // The hostname is the last (and only) path component, so the
        // extension stays.
        assert_eq!(
            get_name_for_url(&w("https://example.com")),
            w("example.com")
        );
    }

    #[test]
    fn http_with_query() {
        assert_eq!(
            get_name_for_url(&w("https://example.com/file.html?a=1&b=2")),
            w("file a=1&b=2")
        );
    }

    #[test]
    fn http_with_anchor() {
        assert_eq!(
            get_name_for_url(&w("https://example.com/file.html#sect1")),
            w("file sect1")
        );
    }

    #[test]
    fn truncates_long_name() {
        let long_seg = "a".repeat(40);
        let url = format!("https://example.com/{long_seg}.html");
        let result = get_name_for_url(&w(&url));
        assert_eq!(result.len(), 31);
        assert_eq!(result[30], ELLIPSIS);
    }
}
