/*
This file is a part of the NVDA project.
URL: https://www.nvaccess.org/
Copyright 2025 NV Access Limited.
    This program is free software: you can redistribute it and/or modify
    it under the terms of the GNU General Public License version 2.0, as published by
    the Free Software Foundation.
    This program is distributed in the hope that it will be useful,
    but WITHOUT ANY WARRANTY; without even the implied warranty of
    MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.
This license can be found at:
http://www.gnu.org/licenses/old-licenses/gpl-2.0.html
*/

use unicode_segmentation::UnicodeSegmentation;

/// Split text into grapheme clusters (visible characters).
///
/// Returns a Vec of string slices, one per extended grapheme cluster.
/// For example, "👨‍👩‍👧‍👦" (family emoji ZWJ sequence) is a single grapheme cluster.
///
/// Note: UAX#29 treats `\r\n` as a single grapheme cluster, but for text
/// navigation purposes (matching Uniscribe/Notepad behavior), we split it
/// into separate `\r` and `\n` characters.
pub fn split_at_character_boundaries(text: &str) -> Vec<&str> {
    let mut result = Vec::new();
    for grapheme in text.graphemes(true) {
        if grapheme == "\r\n" {
            // Split \r\n into separate characters for navigation purposes.
            result.push("\r");
            result.push("\n");
        } else {
            result.push(grapheme);
        }
    }
    result
}

/// Find the start and end str offsets of the grapheme cluster containing `offset`.
///
/// `offset` is a Python str offset (i.e. a Unicode scalar value / code point index).
/// Returns `(start, end)` where `start` is inclusive and `end` is exclusive.
/// If `offset` is at or beyond the end of the text, returns `(offset, offset + 1)`.
///
/// Note: `\r\n` is treated as two separate characters (matching Uniscribe behavior).
pub fn character_offsets(text: &str, offset: usize) -> (usize, usize) {
    let char_count = text.chars().count();
    if offset >= char_count {
        return (offset, offset + 1);
    }

    // Find which grapheme cluster contains this char offset
    let mut grapheme_start_char: usize = 0;
    for grapheme in text.graphemes(true) {
        if grapheme == "\r\n" {
            // Treat \r and \n as separate characters.
            if offset == grapheme_start_char {
                return (grapheme_start_char, grapheme_start_char + 1);
            }
            if offset == grapheme_start_char + 1 {
                return (grapheme_start_char + 1, grapheme_start_char + 2);
            }
            grapheme_start_char += 2;
            continue;
        }
        let grapheme_char_len = grapheme.chars().count();
        let grapheme_end_char = grapheme_start_char + grapheme_char_len;
        if offset < grapheme_end_char {
            return (grapheme_start_char, grapheme_end_char);
        }
        grapheme_start_char = grapheme_end_char;
    }

    // Fallback (shouldn't reach here)
    (offset, offset + 1)
}

/// Find the start and end str offsets of the word containing `offset`.
///
/// `offset` is a Python str offset (code point index).
/// Returns `(start, end)` where `start` is inclusive and `end` is exclusive.
///
/// Uses UAX#29 word boundaries from `unicode-segmentation`, with additional
/// whitespace-aware logic ported from the C++ implementation to handle
/// issue #1656: whitespace is included at the end of a word (trailing whitespace
/// belongs to the preceding word).
pub fn word_offsets(text: &str, offset: usize) -> (usize, usize) {
    let char_count = text.chars().count();
    if char_count == 0 {
        return (offset, offset + 1);
    }
    if offset >= char_count {
        return (offset, offset + 1);
    }

    // Get UAX#29 word boundaries as char offsets.
    // unicode_word_indices gives (byte_offset, word_str) for non-whitespace "words".
    // We need all word boundaries including whitespace, so use split_word_bound_indices.
    let segments: Vec<(usize, &str)> = text.split_word_bound_indices().collect();

    // Convert byte-based segment starts to char-based offsets
    let mut char_offset = 0usize;
    let mut segment_char_ranges: Vec<(usize, usize)> = Vec::new();
    for (_byte_idx, segment) in &segments {
        let seg_char_len = segment.chars().count();
        segment_char_ranges.push((char_offset, char_offset + seg_char_len));
        char_offset += seg_char_len;
    }

    // Find which segment contains our offset
    let mut seg_idx = 0;
    for (i, &(start, end)) in segment_char_ranges.iter().enumerate() {
        if offset >= start && offset < end {
            seg_idx = i;
            break;
        }
    }

    let (mut word_start, mut word_end) = segment_char_ranges[seg_idx];

    // Port the C++ whitespace-aware word boundary logic from issue #1656.
    //
    // The C++ code does two things:
    // 1. When searching backwards for word start: if we start in whitespace,
    //    skip it and find the non-whitespace word before it (whitespace belongs
    //    to the END of the previous word, not the start of the next).
    // 2. When searching forwards for word end: include trailing whitespace
    //    as part of the word.
    //
    // With UAX#29 split_word_bounds, whitespace is its own segment. So:
    // - If offset is in a whitespace segment, the word is the preceding
    //   non-whitespace segment + this whitespace segment.
    // - If offset is in a non-whitespace segment, the word is this segment
    //   + any immediately following whitespace segment.

    let segment_text = segments[seg_idx].1;
    let is_whitespace_segment = segment_text.chars().all(|c| c.is_whitespace());

    if is_whitespace_segment {
        // Offset is in whitespace. Look back for the preceding non-whitespace segment.
        if seg_idx > 0 {
            word_start = segment_char_ranges[seg_idx - 1].0;
        }
        // word_end is already at the end of the whitespace segment
    } else {
        // Offset is in a non-whitespace segment. Include trailing whitespace.
        if seg_idx + 1 < segments.len() {
            let next_text = segments[seg_idx + 1].1;
            if next_text.chars().all(|c| c.is_whitespace()) {
                word_end = segment_char_ranges[seg_idx + 1].1;
            }
        }
    }

    (word_start, word_end)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── split_at_character_boundaries ──

    #[test]
    fn test_split_ascii() {
        assert_eq!(split_at_character_boundaries("hello"), vec!["h", "e", "l", "l", "o"]);
    }

    #[test]
    fn test_split_empty() {
        let result: Vec<&str> = split_at_character_boundaries("");
        assert!(result.is_empty());
    }

    #[test]
    fn test_split_emoji_zwj() {
        // Family emoji: single grapheme cluster
        let result = split_at_character_boundaries("👨‍👩‍👧‍👦");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], "👨\u{200d}👩\u{200d}👧\u{200d}👦");
    }

    #[test]
    fn test_split_flag_emoji() {
        // Flag emoji (regional indicators)
        let result = split_at_character_boundaries("🇳🇱");
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn test_split_decomposed() {
        // é as e + combining acute accent
        let result = split_at_character_boundaries("e\u{0301}");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], "e\u{0301}");
    }

    #[test]
    fn test_split_surrogate_pairs() {
        // Characters outside BMP (would be surrogate pairs in UTF-16)
        let result = split_at_character_boundaries("𝕳𝖊𝖑𝖑𝖔");
        assert_eq!(result, vec!["𝕳", "𝖊", "𝖑", "𝖑", "𝖔"]);
    }

    #[test]
    fn test_split_mixed() {
        let result = split_at_character_boundaries("a👨‍👩‍👧‍👦b");
        assert_eq!(result.len(), 3);
        assert_eq!(result[0], "a");
        assert_eq!(result[2], "b");
    }

    #[test]
    fn test_split_cjk() {
        let result = split_at_character_boundaries("日本語");
        assert_eq!(result, vec!["日", "本", "語"]);
    }

    // ── character_offsets ──

    #[test]
    fn test_char_offsets_ascii() {
        assert_eq!(character_offsets("hello", 0), (0, 1));
        assert_eq!(character_offsets("hello", 4), (4, 5));
    }

    #[test]
    fn test_char_offsets_emoji_zwj() {
        // "a👨‍👩‍👧‍👦b" — the ZWJ family is 7 code points: man, ZWJ, woman, ZWJ, girl, ZWJ, boy
        let text = "a👨\u{200d}👩\u{200d}👧\u{200d}👦b";
        // offset 0 = 'a'
        assert_eq!(character_offsets(text, 0), (0, 1));
        // offset 1 = start of ZWJ sequence (should span the whole cluster)
        assert_eq!(character_offsets(text, 1), (1, 8));
        // offset 4 = middle of ZWJ sequence (still same cluster)
        assert_eq!(character_offsets(text, 4), (1, 8));
        // offset 8 = 'b'
        assert_eq!(character_offsets(text, 8), (8, 9));
    }

    #[test]
    fn test_char_offsets_decomposed() {
        // "e\u{0301}" — e + combining acute = 1 grapheme, 2 code points
        let text = "e\u{0301}x";
        assert_eq!(character_offsets(text, 0), (0, 2));
        assert_eq!(character_offsets(text, 1), (0, 2));
        assert_eq!(character_offsets(text, 2), (2, 3));
    }

    #[test]
    fn test_char_offsets_beyond_end() {
        assert_eq!(character_offsets("abc", 5), (5, 6));
    }

    #[test]
    fn test_split_crlf() {
        // \r\n should be split into separate characters for navigation
        let result = split_at_character_boundaries("one\r\ntwo");
        assert_eq!(result, vec!["o", "n", "e", "\r", "\n", "t", "w", "o"]);
    }

    #[test]
    fn test_char_offsets_crlf() {
        let text = "one\r\ntwo";
        assert_eq!(character_offsets(text, 3), (3, 4)); // \r
        assert_eq!(character_offsets(text, 4), (4, 5)); // \n
        assert_eq!(character_offsets(text, 5), (5, 6)); // t
    }

    // ── word_offsets ──

    #[test]
    fn test_word_simple() {
        let text = "hello world";
        // "hello" at offset 0: word is "hello " (trailing space)
        let (start, end) = word_offsets(text, 0);
        assert_eq!(start, 0);
        assert_eq!(end, 6); // "hello "

        // "world" at offset 6
        let (start, end) = word_offsets(text, 6);
        assert_eq!(start, 6);
        assert_eq!(end, 11);
    }

    #[test]
    fn test_word_whitespace_belongs_to_preceding() {
        let text = "hello world";
        // offset 5 = space — should belong to "hello " word
        let (start, end) = word_offsets(text, 5);
        assert_eq!(start, 0);
        assert_eq!(end, 6);
    }

    #[test]
    fn test_word_punctuation() {
        let text = "hello, world";
        // offset 0-4 = "hello"
        let (start, _end) = word_offsets(text, 0);
        assert_eq!(start, 0);
        // The exact end depends on UAX#29 — "hello" and "," are separate word segments
    }

    #[test]
    fn test_word_beyond_end() {
        assert_eq!(word_offsets("abc", 5), (5, 6));
    }

    #[test]
    fn test_word_empty() {
        assert_eq!(word_offsets("", 0), (0, 1));
    }

    #[test]
    fn test_word_multiple_spaces() {
        let text = "hello  world";
        // offset 5 = first space
        let (start, end) = word_offsets(text, 5);
        assert_eq!(start, 0);
        assert_eq!(end, 7); // "hello  "
    }
}
