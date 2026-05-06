//! Rust port of `nvdaHelper/vbufBase/storage.{cpp,h}`.
//!
//! Phase 6b deliverable -- the per-node tree data structure plus the
//! `Buffer` that owns it. The C++ original uses raw pointers
//! (`parent` / `previous` / `next` / `firstChild` / `lastChild`) on
//! each node and lets the buffer manage allocation. The Rust port
//! flips that to an arena: `Buffer` holds a `slotmap::SlotMap<NodeKey,
//! Node>`; each node refers to its neighbours by `Option<NodeKey>`.
//! Generational keys catch use-after-free as `None` on lookup rather
//! than dangling-pointer UB.
//!
//! This module is built but not yet wired in. The existing C++
//! storage (`nvdaHelper/vbufBase/storage.cpp`) is still the live
//! implementation reached through `c_shim.cpp`. Phase 6d flips the
//! C-shim to call into this module instead.

mod node;

pub use node::{ControlFieldData, FieldNodeKind, Node, NodeKey, TextFieldData};

use std::collections::BTreeMap;

use slotmap::SlotMap;

/// The `(docHandle, ID)` pair that uniquely identifies a control
/// field node in a buffer. Mirrors
/// `VBufStorage_controlFieldNodeIdentifier_t`.
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
pub struct ControlFieldIdentifier {
    pub doc_handle: i32,
    pub id: i32,
}

/// A vbuf storage buffer: an arena of nodes plus a root pointer plus
/// an identifier index for fast lookup. Mirrors
/// `VBufStorage_buffer_t`.
pub struct Buffer {
    nodes: SlotMap<NodeKey, Node>,
    root: Option<NodeKey>,
    /// Maps `(docHandle, id)` to the matching control field node so
    /// `getControlFieldNodeWithIdentifier` is `O(log n)` rather than
    /// a tree walk. Built up as control field nodes are added; pruned
    /// on removal.
    by_identifier: BTreeMap<ControlFieldIdentifier, NodeKey>,
}

impl Buffer {
    /// Construct an empty buffer. The C++ default constructor.
    pub fn new() -> Self {
        Self {
            nodes: SlotMap::with_key(),
            root: None,
            by_identifier: BTreeMap::new(),
        }
    }

    /// Look up a control field node by `(docHandle, ID)`. Returns
    /// `None` if no such node exists in this buffer. Mirrors
    /// `VBufStorage_buffer_t::getControlFieldNodeWithIdentifier`.
    pub fn get_control_field_node_with_identifier(
        &self,
        doc_handle: i32,
        id: i32,
    ) -> Option<NodeKey> {
        self.by_identifier
            .get(&ControlFieldIdentifier { doc_handle, id })
            .copied()
    }

    /// Borrow a node by key. Returns `None` if the key is stale or
    /// belongs to a different buffer.
    pub fn get(&self, key: NodeKey) -> Option<&Node> {
        self.nodes.get(key)
    }

    /// Borrow a node mutably. See [`Buffer::get`].
    pub fn get_mut(&mut self, key: NodeKey) -> Option<&mut Node> {
        self.nodes.get_mut(key)
    }

    /// Whether `key` belongs to this buffer (the C++
    /// `isNodeInBuffer`).
    pub fn contains(&self, key: NodeKey) -> bool {
        self.nodes.contains_key(key)
    }

    /// The root node of this buffer, if any. The C++ doesn't expose a
    /// dedicated getter -- `firstChild`/`lastChild` walk handles the
    /// empty case there. Rust exposes the root explicitly.
    pub fn root(&self) -> Option<NodeKey> {
        self.root
    }

    /// Add a new control field node as a child of `parent` (or as the
    /// root when `parent` is `None`), positioned after `previous` in
    /// its sibling list (or as the first child when `previous` is
    /// `None`). Mirrors
    /// `VBufStorage_buffer_t::addControlFieldNode`.
    ///
    /// Returns the new node's key, or `None` if the resulting tree
    /// would be invalid (e.g. duplicate identifier, unknown parent /
    /// previous, sibling/parent mismatch).
    pub fn add_control_field_node(
        &mut self,
        parent: Option<NodeKey>,
        previous: Option<NodeKey>,
        identifier: ControlFieldIdentifier,
        is_block: bool,
    ) -> Option<NodeKey> {
        if self.by_identifier.contains_key(&identifier) {
            return None;
        }
        if !self.validate_insertion_anchor(parent, previous) {
            return None;
        }
        let node = Node::new_control(identifier, is_block);
        let key = self.nodes.insert(node);
        self.by_identifier.insert(identifier, key);
        self.link(parent, previous, key);
        Some(key)
    }

    /// Add a new text field node. Same anchor semantics as
    /// `add_control_field_node`. Mirrors
    /// `VBufStorage_buffer_t::addTextFieldNode`.
    pub fn add_text_field_node(
        &mut self,
        parent: Option<NodeKey>,
        previous: Option<NodeKey>,
        text: Vec<u16>,
    ) -> Option<NodeKey> {
        if !self.validate_insertion_anchor(parent, previous) {
            return None;
        }
        let node = Node::new_text(text);
        let key = self.nodes.insert(node);
        self.link(parent, previous, key);
        Some(key)
    }

    /// `(parent, previous)` validation:
    /// * `parent` must be an existing control field node (or `None`
    ///   when there is no current root).
    /// * `previous`, if present, must already be a child of `parent`.
    fn validate_insertion_anchor(
        &self,
        parent: Option<NodeKey>,
        previous: Option<NodeKey>,
    ) -> bool {
        match parent {
            None => self.root.is_none() && previous.is_none(),
            Some(p_key) => {
                let p_node = match self.nodes.get(p_key) {
                    Some(n) => n,
                    None => return false,
                };
                if !matches!(p_node.kind, FieldNodeKind::Control(_)) {
                    return false;
                }
                if let Some(prev_key) = previous {
                    let prev_node = match self.nodes.get(prev_key) {
                        Some(n) => n,
                        None => return false,
                    };
                    if prev_node.parent != Some(p_key) {
                        return false;
                    }
                }
                true
            }
        }
    }

    /// Link a freshly-allocated node into the tree at the
    /// `(parent, previous)` anchor. The anchor is assumed valid.
    fn link(
        &mut self,
        parent: Option<NodeKey>,
        previous: Option<NodeKey>,
        new_key: NodeKey,
    ) {
        // Determine `next` from previous's current next, or from
        // parent's firstChild when previous is None.
        let next = match (parent, previous) {
            (None, _) => None, // root: no siblings
            (Some(p_key), None) => self.nodes[p_key].first_child,
            (Some(_), Some(prev_key)) => self.nodes[prev_key].next,
        };

        // Update the new node's links.
        {
            let n = &mut self.nodes[new_key];
            n.parent = parent;
            n.previous = previous;
            n.next = next;
        }

        // Update neighbours.
        if let Some(prev_key) = previous {
            self.nodes[prev_key].next = Some(new_key);
        }
        if let Some(next_key) = next {
            self.nodes[next_key].previous = Some(new_key);
        }
        match parent {
            None => self.root = Some(new_key),
            Some(p_key) => {
                let p = &mut self.nodes[p_key];
                if previous.is_none() {
                    p.first_child = Some(new_key);
                }
                if next.is_none() {
                    p.last_child = Some(new_key);
                }
            }
        }
    }
}

impl Default for Buffer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cf(doc_handle: i32, id: i32) -> ControlFieldIdentifier {
        ControlFieldIdentifier { doc_handle, id }
    }

    #[test]
    fn empty_buffer_has_no_root() {
        let b = Buffer::new();
        assert_eq!(b.root(), None);
    }

    #[test]
    fn add_root_control_field_node() {
        let mut b = Buffer::new();
        let root = b
            .add_control_field_node(None, None, cf(1, 1), true)
            .expect("first add");
        assert_eq!(b.root(), Some(root));
        assert_eq!(
            b.get_control_field_node_with_identifier(1, 1),
            Some(root)
        );
        let n = b.get(root).unwrap();
        assert_eq!(n.parent, None);
        assert_eq!(n.previous, None);
        assert_eq!(n.next, None);
    }

    #[test]
    fn second_root_rejected() {
        let mut b = Buffer::new();
        let _root = b.add_control_field_node(None, None, cf(1, 1), true);
        // Adding a second root (parent=None) when one exists is
        // rejected.
        assert_eq!(
            b.add_control_field_node(None, None, cf(1, 2), true),
            None
        );
    }

    #[test]
    fn add_child_to_control_field() {
        let mut b = Buffer::new();
        let root = b.add_control_field_node(None, None, cf(1, 1), true).unwrap();
        let child = b
            .add_control_field_node(Some(root), None, cf(1, 2), false)
            .unwrap();
        assert_eq!(b.get(root).unwrap().first_child, Some(child));
        assert_eq!(b.get(root).unwrap().last_child, Some(child));
        assert_eq!(b.get(child).unwrap().parent, Some(root));
    }

    #[test]
    fn add_text_field_after_sibling() {
        let mut b = Buffer::new();
        let root = b.add_control_field_node(None, None, cf(1, 1), true).unwrap();
        let first_child =
            b.add_text_field_node(Some(root), None, "abc".encode_utf16().collect())
                .unwrap();
        let second_child = b
            .add_text_field_node(
                Some(root),
                Some(first_child),
                "def".encode_utf16().collect(),
            )
            .unwrap();
        // Sibling order: first_child -> second_child.
        assert_eq!(b.get(first_child).unwrap().next, Some(second_child));
        assert_eq!(b.get(second_child).unwrap().previous, Some(first_child));
        assert_eq!(b.get(root).unwrap().first_child, Some(first_child));
        assert_eq!(b.get(root).unwrap().last_child, Some(second_child));
    }

    #[test]
    fn duplicate_identifier_rejected() {
        let mut b = Buffer::new();
        let root = b.add_control_field_node(None, None, cf(1, 1), true).unwrap();
        // Duplicate identifier should be rejected even with a valid
        // parent.
        assert_eq!(
            b.add_control_field_node(Some(root), None, cf(1, 1), false),
            None
        );
    }

    #[test]
    fn previous_must_share_parent() {
        let mut b = Buffer::new();
        let root = b.add_control_field_node(None, None, cf(1, 1), true).unwrap();
        let other = b.add_control_field_node(Some(root), None, cf(1, 2), false).unwrap();
        // Try to add a node under root with a "previous" that's not
        // a child of root (in this case, root itself is not a child
        // of root).
        assert_eq!(
            b.add_control_field_node(Some(root), Some(root), cf(1, 3), false),
            None
        );
        // But "previous" being `other` (a child of root) is valid.
        assert!(b
            .add_control_field_node(Some(root), Some(other), cf(1, 3), false)
            .is_some());
    }
}
