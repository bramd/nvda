//! Field-node tree types for the vbuf storage port. Mirrors the
//! `VBufStorage_fieldNode_t` / `_controlFieldNode_t` /
//! `_textFieldNode_t` / `_referenceNode_t` class hierarchy as a
//! single `Node` with a `FieldNodeKind` discriminant.

use std::collections::BTreeMap;

use slotmap::new_key_type;

use super::ControlFieldIdentifier;

new_key_type! {
    /// Generational arena key. Mirrors what was a raw
    /// `VBufStorage_fieldNode_t*` in the C++ original. A stale key
    /// (slot reused after the original node was removed) yields
    /// `None` on lookup rather than aliasing into a different node.
    pub struct NodeKey;
}

/// A node in the vbuf tree. Common fields live on the outer struct;
/// kind-specific data lives in [`FieldNodeKind`].
pub struct Node {
    pub kind: FieldNodeKind,
    /// Length in characters this node spans in the buffer.
    pub length: i32,
    /// IA2 attribute map. The key/value strings are UTF-16 to match
    /// the C++ `std::map<std::wstring, std::wstring>` shape; the
    /// markup-generation code consumes them verbatim.
    pub attributes: BTreeMap<Vec<u16>, Vec<u16>>,

    pub parent: Option<NodeKey>,
    pub previous: Option<NodeKey>,
    pub next: Option<NodeKey>,
    pub first_child: Option<NodeKey>,
    pub last_child: Option<NodeKey>,

    /// `isBlock` -- forces a line break at start/end when computing
    /// line offsets.
    pub is_block: bool,
    /// `isHidden` -- searches skip this node.
    pub is_hidden: bool,
}

/// Kind-specific node data. Mirrors the C++ class hierarchy:
/// `Control` is the `controlFieldNode_t`, `Text` is the
/// `textFieldNode_t` leaf, `Reference` is the `referenceNode_t`
/// alias.
pub enum FieldNodeKind {
    Control(ControlFieldData),
    Text(TextFieldData),
    Reference(ReferenceData),
}

/// State unique to a control field node.
pub struct ControlFieldData {
    /// `(docHandle, ID)` identifier; immutable for the node's
    /// lifetime in the buffer.
    pub identifier: ControlFieldIdentifier,
    pub requires_parent_update: bool,
    pub allow_reuse_in_ancestor_update: bool,
    pub deny_reuse_if_previous_siblings_changed: bool,
    pub always_rerender_children: bool,
    pub always_rerender_descendants: bool,
}

/// State unique to a text field node.
pub struct TextFieldData {
    /// UTF-16 text content.
    pub text: Vec<u16>,
}

/// State unique to a reference node. References point at an existing
/// control field node in (typically) the backend's main buffer when
/// a temporary buffer is being built for partial re-rendering.
pub struct ReferenceData {
    /// The identifier of the referenced control field node. The
    /// referenced node lives in some other buffer (typically the
    /// backend's main buffer); resolution happens at consumption
    /// time.
    pub identifier: ControlFieldIdentifier,
    /// The actual key of the referenced node in its owner buffer.
    /// Caller is responsible for using this key with the right
    /// buffer.
    pub referenced: NodeKey,
}

impl Node {
    /// Construct a new control field node. Length defaults to 0;
    /// `is_block` per the C++ constructor.
    pub fn new_control(
        identifier: ControlFieldIdentifier,
        is_block: bool,
    ) -> Self {
        Self::new(
            FieldNodeKind::Control(ControlFieldData {
                identifier,
                requires_parent_update: false,
                allow_reuse_in_ancestor_update: true,
                deny_reuse_if_previous_siblings_changed: false,
                always_rerender_children: false,
                always_rerender_descendants: false,
            }),
            0,
            is_block,
        )
    }

    /// Construct a new text field node containing `text`.
    /// Length is set to the wide-char count of the text. `is_block`
    /// is `false` (text nodes don't force line breaks; the C++
    /// behaviour mirrors).
    pub fn new_text(text: Vec<u16>) -> Self {
        let length = text.len() as i32;
        Self::new(FieldNodeKind::Text(TextFieldData { text }), length, false)
    }

    /// Construct a new reference node aliasing `referenced`.
    pub fn new_reference(
        identifier: ControlFieldIdentifier,
        referenced: NodeKey,
    ) -> Self {
        Self::new(
            FieldNodeKind::Reference(ReferenceData {
                identifier,
                referenced,
            }),
            0,
            false,
        )
    }

    fn new(kind: FieldNodeKind, length: i32, is_block: bool) -> Self {
        Self {
            kind,
            length,
            attributes: BTreeMap::new(),
            parent: None,
            previous: None,
            next: None,
            first_child: None,
            last_child: None,
            is_block,
            is_hidden: false,
        }
    }

    /// Add or replace an attribute. Returns `true` (the C++ original
    /// returns `true` unconditionally; preserved for API parity).
    pub fn add_attribute(
        &mut self,
        name: &[u16],
        value: &[u16],
    ) -> bool {
        self.attributes.insert(name.to_vec(), value.to_vec());
        true
    }

    /// Look up an attribute. Returns `None` when the attribute is
    /// absent.
    pub fn get_attribute(&self, name: &[u16]) -> Option<&[u16]> {
        self.attributes.get(name).map(|v| v.as_slice())
    }
}
