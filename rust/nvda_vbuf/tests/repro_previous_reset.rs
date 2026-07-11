//! Scratch reproduction: mimic the storage call sequence fill_vbuf
//! emits for a small document with a multi-child container, both
//! WITHOUT the `previousNode=NULL` reset (current fill_vbuf) and WITH
//! it (C++ semantics). Demonstrates that the missing reset drops the
//! text of every non-first child.

use nvda_vbuf::storage::{Buffer, ControlFieldIdentifier};

fn cf(id: i32) -> ControlFieldIdentifier {
    ControlFieldIdentifier { doc_handle: 1, id }
}

fn w(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

fn full_text(b: &Buffer) -> String {
    let len = b.text_length();
    let mut out: Vec<u16> = Vec::new();
    if len > 0 {
        b.buffer_get_text_in_range(0, len, &mut out, false);
    }
    String::from_utf16(&out).unwrap()
}

/// Reproduces fill_vbuf as it is TODAY: block5 adds each child's text
/// with the *incoming sibling* `previous` (never reset to None after
/// the control node is created).
#[test]
fn buggy_sequence_drops_non_first_child_text() {
    let mut b = Buffer::new();
    // Document root (block).
    let doc = b
        .add_control_field_node(None, None, cf(1), true)
        .unwrap();

    // Child 1 "heading": control(child of doc, previous=None), then its
    // text with previous = the (reset) None.
    let h = b.add_control_field_node(Some(doc), None, cf(2), true).unwrap();
    // fill_vbuf for the heading: `previous` passed in was None (first
    // child), so its text add uses previous=None -> OK.
    let h_text = b.add_text_field_node(Some(h), None, w("Heading")).unwrap();
    let _ = h_text;

    // Child 2 "paragraph": control(child of doc, previous=heading).
    let p = b
        .add_control_field_node(Some(doc), Some(h), cf(3), true)
        .unwrap();
    // BUG: fill_vbuf passes the incoming sibling `previous` (the heading)
    // straight into block5, so the paragraph's text is added with
    // previous = heading, whose parent is `doc`, not `p`.
    let p_text =
        b.add_text_field_node(Some(p), Some(h), w("Body text"));
    assert!(
        p_text.is_none(),
        "storage should reject sibling-previous text add"
    );

    // Result: the paragraph is present (structure) but blank (no text).
    let text = full_text(&b);
    assert_eq!(text, "Heading", "only the first child's text survived: {text:?}");
}

/// The C++ semantics: after creating the new control node, `previous`
/// is reset to None, so each child's text is the first child of ITS
/// OWN parent.
#[test]
fn reset_sequence_renders_all_text() {
    let mut b = Buffer::new();
    let doc = b
        .add_control_field_node(None, None, cf(1), true)
        .unwrap();
    let h = b.add_control_field_node(Some(doc), None, cf(2), true).unwrap();
    let _ = b.add_text_field_node(Some(h), None, w("Heading")).unwrap();
    let p = b
        .add_control_field_node(Some(doc), Some(h), cf(3), true)
        .unwrap();
    // Reset: previous=None for the paragraph's own text child.
    let p_text = b.add_text_field_node(Some(p), None, w("Body text"));
    assert!(p_text.is_some());
    assert_eq!(full_text(&b), "HeadingBody text");
}
