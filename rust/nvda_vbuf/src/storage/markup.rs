//! XML markup generation for the vbuf storage tree. Mirrors
//! `generateMarkupOpeningTag`, `generateMarkupClosingTag`,
//! `generateAttributesForMarkupOpeningTag` from
//! `nvdaHelper/vbufBase/storage.cpp` plus the
//! `appendCharToXML` / `sanitizeXMLAttribName` helpers in
//! `nvdaHelper/common/xml.h`.
//!
//! Output is byte-identical to the C++ original (modulo the
//! BTreeMap-vs-std::map iteration order, which is sorted in both
//! cases).

use core::fmt::Write;

use super::node::{FieldNodeKind, NodeKey};
use super::Buffer;

/// Append `c` to `out` with XML-character escaping. Mirrors
/// `appendCharToXML` from `nvdaHelper/common/xml.h`. `is_attribute`
/// changes the fallback for invalid XML characters: replacement
/// character in attributes, `<unich value="N"/>` element in body.
pub(crate) fn append_char_to_xml(c: u16, out: &mut Vec<u16>, is_attribute: bool) {
    match c {
        // L'"'
        0x22 => out.extend_from_slice(&utf16_lit("&quot;")),
        // L'<'
        0x3c => out.extend_from_slice(&utf16_lit("&lt;")),
        // L'>'
        0x3e => out.extend_from_slice(&utf16_lit("&gt;")),
        // L'&'
        0x26 => out.extend_from_slice(&utf16_lit("&amp;")),
        _ => {
            let valid = matches!(c, 0x9 | 0xA | 0xD)
                || (0x20..=0xD7FF).contains(&c)
                || (0xE000..=0xFFFD).contains(&c);
            if valid {
                out.push(c);
            } else if is_attribute {
                out.push(0xfffd); // U+FFFD REPLACEMENT CHARACTER
            } else {
                let mut s = String::new();
                let _ = write!(s, "<unich value=\"{}\" />", c);
                out.extend(s.encode_utf16());
            }
        }
    }
}

/// Append `name` to `out` after replacing every `' '` (U+0020) with
/// `'_'`. Mirrors `sanitizeXMLAttribName`.
pub(crate) fn append_sanitized_attrib_name(name: &[u16], out: &mut Vec<u16>) {
    for &c in name {
        if c == b' ' as u16 {
            out.push(b'_' as u16);
        } else {
            out.push(c);
        }
    }
}

/// Compile-time ASCII string to UTF-16. The byte slice must be ASCII.
fn utf16_lit(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

impl Buffer {
    /// Append the text content of `key`'s subtree, restricted to
    /// `[start_offset, end_offset)`, to `out` -- with XML markup.
    /// Mirrors `getTextInRange` with `useMarkup=true`.
    pub fn get_text_in_range_with_markup(
        &self,
        key: NodeKey,
        start_offset: i32,
        end_offset: i32,
        out: &mut Vec<u16>,
    ) {
        if !self.contains(key) {
            return;
        }
        let n = &self.nodes[key];
        if n.length == 0 {
            return;
        }
        debug_assert!(start_offset >= 0);
        debug_assert!(start_offset < end_offset);
        debug_assert!(end_offset <= n.length);

        self.write_markup_opening_tag(key, start_offset, end_offset, out);

        match &n.kind {
            FieldNodeKind::Text(data) => {
                let s = start_offset as usize;
                let e = end_offset as usize;
                for &c in &data.text[s..e] {
                    append_char_to_xml(c, out, false);
                }
            }
            _ => {
                let mut child_start: i32 = 0;
                let mut child = n.first_child;
                while let Some(ckey) = child {
                    let child_node = &self.nodes[ckey];
                    let child_length = child_node.length;
                    let child_end = child_start + child_length;
                    if child_end > start_offset && end_offset > child_start {
                        let sub_start =
                            (start_offset.max(child_start)) - child_start;
                        let sub_end = (end_offset - child_start)
                            .min(child_length);
                        self.get_text_in_range_with_markup(
                            ckey, sub_start, sub_end, out,
                        );
                    }
                    child_start += child_length;
                    child = child_node.next;
                }
            }
        }

        self.write_markup_closing_tag(key, out);
    }

    /// Write `<tagname attr1="v1" attr2="v2" ...>` for the given
    /// node. Mirrors `generateMarkupOpeningTag` +
    /// `generateAttributesForMarkupOpeningTag`.
    fn write_markup_opening_tag(
        &self,
        key: NodeKey,
        start_offset: i32,
        end_offset: i32,
        out: &mut Vec<u16>,
    ) {
        out.push(b'<' as u16);
        self.write_tag_name(key, out);
        out.push(b' ' as u16);
        self.write_attributes_for_opening_tag(
            key, start_offset, end_offset, out,
        );
        out.push(b'>' as u16);
    }

    /// Write `</tagname>` for the given node. Mirrors
    /// `generateMarkupClosingTag`.
    fn write_markup_closing_tag(&self, key: NodeKey, out: &mut Vec<u16>) {
        out.push(b'<' as u16);
        out.push(b'/' as u16);
        self.write_tag_name(key, out);
        out.push(b'>' as u16);
    }

    /// Write the per-kind tag name: "control", "text", or
    /// "reference".
    fn write_tag_name(&self, key: NodeKey, out: &mut Vec<u16>) {
        let n = &self.nodes[key];
        let s: &str = match &n.kind {
            FieldNodeKind::Control(_) => "control",
            FieldNodeKind::Text(_) => "text",
            FieldNodeKind::Reference(_) => "reference",
        };
        out.extend(s.encode_utf16());
    }

    /// Write the attribute string for an opening tag. The C++
    /// generates a fixed prefix of `_startOfNode`, `_endOfNode`,
    /// `_offsetFromStartOfNode`, `_offsetFromEndOfNode`, `isBlock`,
    /// `isHidden`, `_childcount`, `_childcontrolcount`,
    /// `_indexInParent`, `_parentChildCount`; control field nodes
    /// additionally prepend `controlIdentifier_docHandle` and
    /// `controlIdentifier_ID`. Each user attribute is then appended
    /// as `name="value" ` with the name sanitized and value
    /// XML-escaped.
    fn write_attributes_for_opening_tag(
        &self,
        key: NodeKey,
        start_offset: i32,
        end_offset: i32,
        out: &mut Vec<u16>,
    ) {
        let n = &self.nodes[key];

        // Control field nodes prepend their identifier.
        match &n.kind {
            FieldNodeKind::Control(data) => {
                let mut s = String::new();
                let _ = write!(
                    s,
                    "controlIdentifier_docHandle=\"{}\" controlIdentifier_ID=\"{}\" ",
                    data.identifier.doc_handle, data.identifier.id
                );
                out.extend(s.encode_utf16());
            }
            FieldNodeKind::Reference(data) => {
                // The C++ `referenceNode_t` inherits from
                // `controlFieldNode_t` so its identifier prefix
                // also fires.
                let mut s = String::new();
                let _ = write!(
                    s,
                    "controlIdentifier_docHandle=\"{}\" controlIdentifier_ID=\"{}\" ",
                    data.identifier.doc_handle, data.identifier.id
                );
                out.extend(s.encode_utf16());
            }
            FieldNodeKind::Text(_) => {}
        }

        // Common per-node prefix.
        let start_of_node = (start_offset == 0) as i32;
        let end_of_node = (end_offset >= n.length) as i32;
        let offset_from_start = start_offset;
        let offset_from_end = (n.length - end_offset).max(0);
        let is_block = n.is_block as i32;
        let is_hidden = n.is_hidden as i32;

        // Walk siblings + children for the *count attributes.
        let mut child_count = 0;
        let mut child_control_count = 0;
        let mut child = n.first_child;
        while let Some(ckey) = child {
            child_count += 1;
            let cn = &self.nodes[ckey];
            if cn.length > 0 && cn.first_child.is_some() {
                child_control_count += 1;
            }
            child = cn.next;
        }
        let mut index_in_parent = 0;
        let mut parent_child_count = 1;
        let mut prev = n.previous;
        while let Some(pk) = prev {
            index_in_parent += 1;
            parent_child_count += 1;
            prev = self.nodes[pk].previous;
        }
        let mut next = n.next;
        while let Some(nk) = next {
            parent_child_count += 1;
            next = self.nodes[nk].next;
        }

        let mut s = String::new();
        let _ = write!(
            s,
            "_startOfNode=\"{}\" _endOfNode=\"{}\" \
             _offsetFromStartOfNode=\"{}\" _offsetFromEndOfNode=\"{}\" \
             isBlock=\"{}\" isHidden=\"{}\" \
             _childcount=\"{}\" _childcontrolcount=\"{}\" \
             _indexInParent=\"{}\" _parentChildCount=\"{}\" ",
            start_of_node,
            end_of_node,
            offset_from_start,
            offset_from_end,
            is_block,
            is_hidden,
            child_count,
            child_control_count,
            index_in_parent,
            parent_child_count
        );
        out.extend(s.encode_utf16());

        // User attributes: name (sanitized) ="value" (xml-escaped) `space`.
        for (name, value) in &n.attributes {
            append_sanitized_attrib_name(name, out);
            out.push(b'=' as u16);
            out.push(b'"' as u16);
            for &c in value {
                append_char_to_xml(c, out, true);
            }
            out.push(b'"' as u16);
            out.push(b' ' as u16);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::*;
    use super::*;

    fn cf(doc_handle: i32, id: i32) -> ControlFieldIdentifier {
        ControlFieldIdentifier { doc_handle, id }
    }

    fn collect(b: &Buffer, key: NodeKey, start: i32, end: i32) -> String {
        let mut out: Vec<u16> = Vec::new();
        b.get_text_in_range_with_markup(key, start, end, &mut out);
        String::from_utf16(&out).expect("valid utf16")
    }

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn append_char_xml_escapes_special_chars() {
        let mut out: Vec<u16> = Vec::new();
        for c in "&<>\"".encode_utf16() {
            append_char_to_xml(c, &mut out, false);
        }
        assert_eq!(String::from_utf16(&out).unwrap(), "&amp;&lt;&gt;&quot;");
    }

    #[test]
    fn append_char_xml_keeps_normal_text() {
        let mut out: Vec<u16> = Vec::new();
        for c in "hello".encode_utf16() {
            append_char_to_xml(c, &mut out, false);
        }
        assert_eq!(String::from_utf16(&out).unwrap(), "hello");
    }

    #[test]
    fn append_char_xml_invalid_in_attribute_replaced() {
        let mut out: Vec<u16> = Vec::new();
        // U+0001 is invalid; in attribute mode it becomes U+FFFD.
        append_char_to_xml(0x1, &mut out, true);
        assert_eq!(out, vec![0xfffd]);
    }

    #[test]
    fn append_char_xml_invalid_in_body_unich() {
        let mut out: Vec<u16> = Vec::new();
        append_char_to_xml(0x1, &mut out, false);
        assert_eq!(
            String::from_utf16(&out).unwrap(),
            "<unich value=\"1\" />"
        );
    }

    #[test]
    fn sanitize_attrib_name_replaces_spaces() {
        let mut out: Vec<u16> = Vec::new();
        append_sanitized_attrib_name(&w("a b c"), &mut out);
        assert_eq!(String::from_utf16(&out).unwrap(), "a_b_c");
    }

    #[test]
    fn markup_for_text_node_in_control() {
        let mut b = Buffer::new();
        let root = b.add_control_field_node(None, None, cf(7, 42), true).unwrap();
        let _t = b.add_text_field_node(Some(root), None, w("hi")).unwrap();
        let s = collect(&b, root, 0, 2);
        // Verify the prefix and the text node appears wrapped:
        // <control controlIdentifier_docHandle="7" controlIdentifier_ID="42" ...>
        //   <text ...>hi</text>
        // </control>
        assert!(s.starts_with("<control controlIdentifier_docHandle=\"7\" controlIdentifier_ID=\"42\" "));
        assert!(s.contains("<text "));
        assert!(s.contains(">hi</text>"));
        assert!(s.ends_with("</control>"));
        assert!(s.contains("isBlock=\"1\""));
    }

    #[test]
    fn markup_escapes_text_special_chars() {
        let mut b = Buffer::new();
        let root = b.add_control_field_node(None, None, cf(1, 1), false).unwrap();
        let _t = b.add_text_field_node(Some(root), None, w("a<b&c>")).unwrap();
        let s = collect(&b, root, 0, 6);
        // The text body should be escaped.
        assert!(s.contains(">a&lt;b&amp;c&gt;</text>"));
    }

    #[test]
    fn markup_user_attributes_are_xml_escaped() {
        let mut b = Buffer::new();
        let root = b.add_control_field_node(None, None, cf(1, 1), false).unwrap();
        let n = b.get_mut(root).unwrap();
        n.add_attribute(&w("data-foo"), &w("a\"b"));
        let _t = b.add_text_field_node(Some(root), None, w("x")).unwrap();
        let s = collect(&b, root, 0, 1);
        // " inside attribute value -> &quot;
        assert!(s.contains("data-foo=\"a&quot;b\""));
    }

    #[test]
    fn markup_attribute_name_sanitized() {
        let mut b = Buffer::new();
        let root = b.add_control_field_node(None, None, cf(1, 1), false).unwrap();
        let n = b.get_mut(root).unwrap();
        // Attribute name with a space.
        n.add_attribute(&w("a b"), &w("v"));
        let _t = b.add_text_field_node(Some(root), None, w("x")).unwrap();
        let s = collect(&b, root, 0, 1);
        // Space replaced with underscore.
        assert!(s.contains("a_b=\"v\""));
    }
}
