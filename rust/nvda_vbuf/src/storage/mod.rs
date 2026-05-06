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
        let length = node.length;
        let key = self.nodes.insert(node);
        self.link(parent, previous, key);
        self.bump_ancestor_lengths(key, length);
        Some(key)
    }

    /// Add a new reference node aliasing an existing control field
    /// node. Mirrors
    /// `VBufStorage_buffer_t::addReferenceNodeToBuffer`. The
    /// reference node's identifier matches the target's; this means
    /// the call fails when the buffer already contains a node with
    /// that identifier (which is precisely the case the C++ rejects
    /// via `addControlFieldNode`).
    ///
    /// `referenced` is the key of the target node, which typically
    /// lives in a different (backend-owned) buffer. The caller is
    /// responsible for using that key with the right buffer when
    /// resolving.
    pub fn add_reference_node(
        &mut self,
        parent: Option<NodeKey>,
        previous: Option<NodeKey>,
        identifier: ControlFieldIdentifier,
        referenced: NodeKey,
    ) -> Option<NodeKey> {
        if self.by_identifier.contains_key(&identifier) {
            return None;
        }
        if !self.validate_insertion_anchor(parent, previous) {
            return None;
        }
        let node = Node::new_reference(identifier, referenced);
        let key = self.nodes.insert(node);
        self.by_identifier.insert(identifier, key);
        self.link(parent, previous, key);
        Some(key)
    }

    /// Walk up from `descendant` via the parent chain; returns
    /// `true` if `parent` is encountered. Mirrors
    /// `VBufStorage_buffer_t::isDescendantNode`.
    pub fn is_descendant_node(
        &self,
        parent: NodeKey,
        descendant: NodeKey,
    ) -> bool {
        if !self.contains(parent) || !self.contains(descendant) {
            return false;
        }
        let mut cur = self.nodes[descendant].parent;
        while let Some(key) = cur {
            if key == parent {
                return true;
            }
            cur = self.nodes[key].parent;
        }
        false
    }

    /// Remove `node` from the buffer. When `remove_descendants` is
    /// `true`, the entire subtree is dropped; otherwise the node's
    /// children are re-parented to its parent.
    ///
    /// Returns `false` for stale keys, or when attempting to remove
    /// the root without cascading. Mirrors
    /// `VBufStorage_buffer_t::removeFieldNode`.
    pub fn remove(
        &mut self,
        key: NodeKey,
        remove_descendants: bool,
    ) -> bool {
        if !self.contains(key) {
            return false;
        }
        if Some(key) == self.root && !remove_descendants {
            // Root removal must cascade -- no parent to inherit
            // children to.
            return false;
        }

        // Snapshot the links and length we need before mutating.
        let snapshot = {
            let n = &self.nodes[key];
            NodeSnapshot {
                length: n.length,
                parent: n.parent,
                previous: n.previous,
                next: n.next,
                first_child: n.first_child,
                last_child: n.last_child,
            }
        };

        // 1. Collapse ancestor lengths when this node's length is
        // disappearing from the tree (cascade case, or leaf node).
        let length_disappearing = remove_descendants
            || snapshot.first_child.is_none();
        if length_disappearing && snapshot.length > 0 {
            let mut a = snapshot.parent;
            while let Some(akey) = a {
                let n = &mut self.nodes[akey];
                n.length -= snapshot.length;
                debug_assert!(n.length >= 0, "ancestor length went negative");
                a = n.parent;
            }
        }

        // 2. Re-link siblings / parent. If we're keeping descendants,
        // they slide in where this node used to be.
        let new_next_for_prev =
            if !remove_descendants && snapshot.first_child.is_some() {
                snapshot.first_child
            } else {
                snapshot.next
            };
        let new_prev_for_next =
            if !remove_descendants && snapshot.last_child.is_some() {
                snapshot.last_child
            } else {
                snapshot.previous
            };
        if let Some(nkey) = snapshot.next {
            self.nodes[nkey].previous = new_prev_for_next;
        } else if let Some(pkey) = snapshot.parent {
            self.nodes[pkey].last_child = new_prev_for_next;
        }
        if let Some(pkey) = snapshot.previous {
            self.nodes[pkey].next = new_next_for_prev;
        } else if let Some(parent_key) = snapshot.parent {
            self.nodes[parent_key].first_child = new_next_for_prev;
        }

        // 3. Adopt children when keeping the subtree.
        if !remove_descendants {
            let mut child = snapshot.first_child;
            while let Some(ckey) = child {
                self.nodes[ckey].parent = snapshot.parent;
                child = self.nodes[ckey].next;
            }
            if let Some(fc) = snapshot.first_child {
                self.nodes[fc].previous = snapshot.previous;
            }
            if let Some(lc) = snapshot.last_child {
                self.nodes[lc].next = snapshot.next;
            }
        }

        // 4. Drop root when applicable.
        if Some(key) == self.root {
            self.root = None;
        }

        // 5. Remove the actual nodes from the arena.
        if remove_descendants {
            self.remove_subtree_arena(key);
        } else {
            self.remove_node_arena(key);
        }
        true
    }

    /// Empty the buffer entirely. Mirrors
    /// `VBufStorage_buffer_t::clearBuffer`.
    pub fn clear(&mut self) {
        self.nodes.clear();
        self.by_identifier.clear();
        self.root = None;
    }

    /// Bump every ancestor's length by `delta` (positive) when a new
    /// child is inserted whose length contributes to the tree.
    fn bump_ancestor_lengths(&mut self, key: NodeKey, delta: i32) {
        if delta == 0 {
            return;
        }
        let mut a = self.nodes[key].parent;
        while let Some(akey) = a {
            let n = &mut self.nodes[akey];
            n.length += delta;
            a = n.parent;
        }
    }

    /// Remove a single node from the arena and prune its identifier
    /// from the lookup map.
    fn remove_node_arena(&mut self, key: NodeKey) {
        if let Some(removed) = self.nodes.remove(key) {
            match removed.kind {
                FieldNodeKind::Control(data) => {
                    // Only prune by_identifier when the entry still
                    // points at *this* node -- a reference node
                    // sharing the identifier may have overwritten
                    // it earlier (shouldn't happen with the current
                    // uniqueness check, but defend against it).
                    if self.by_identifier.get(&data.identifier).copied()
                        == Some(key)
                    {
                        self.by_identifier.remove(&data.identifier);
                    }
                }
                FieldNodeKind::Reference(data) => {
                    if self.by_identifier.get(&data.identifier).copied()
                        == Some(key)
                    {
                        self.by_identifier.remove(&data.identifier);
                    }
                }
                FieldNodeKind::Text(_) => {}
            }
        }
    }

    /// Remove `root` and every descendant from the arena. Walks the
    /// subtree depth-first and removes nodes after collecting their
    /// keys (so we don't read freed slots).
    fn remove_subtree_arena(&mut self, root: NodeKey) {
        let mut to_remove: Vec<NodeKey> = Vec::new();
        let mut stack: Vec<NodeKey> = vec![root];
        while let Some(key) = stack.pop() {
            to_remove.push(key);
            let mut child = self.nodes[key].first_child;
            while let Some(ckey) = child {
                stack.push(ckey);
                child = self.nodes[ckey].next;
            }
        }
        for key in to_remove {
            self.remove_node_arena(key);
        }
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

/// Snapshot of a node's links / length, captured before mutation in
/// [`Buffer::remove`] to avoid reading the slot after `slotmap`
/// invalidates it.
struct NodeSnapshot {
    length: i32,
    parent: Option<NodeKey>,
    previous: Option<NodeKey>,
    next: Option<NodeKey>,
    first_child: Option<NodeKey>,
    last_child: Option<NodeKey>,
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

    #[test]
    fn is_descendant_node_walks_parent_chain() {
        let mut b = Buffer::new();
        let root = b.add_control_field_node(None, None, cf(1, 1), true).unwrap();
        let child = b
            .add_control_field_node(Some(root), None, cf(1, 2), false)
            .unwrap();
        let grandchild = b
            .add_control_field_node(Some(child), None, cf(1, 3), false)
            .unwrap();
        let sibling = b
            .add_control_field_node(Some(root), Some(child), cf(1, 4), false)
            .unwrap();
        assert!(b.is_descendant_node(root, child));
        assert!(b.is_descendant_node(root, grandchild));
        assert!(b.is_descendant_node(child, grandchild));
        assert!(!b.is_descendant_node(child, sibling));
        assert!(!b.is_descendant_node(child, root)); // self/ancestor
        assert!(!b.is_descendant_node(child, child)); // node is not its own descendant
    }

    #[test]
    fn add_reference_node_aliases_target() {
        let mut backend_buf = Buffer::new();
        let target = backend_buf
            .add_control_field_node(None, None, cf(1, 99), true)
            .unwrap();
        let target_ident = ControlFieldIdentifier {
            doc_handle: 1,
            id: 99,
        };
        // Add the reference into a *separate* temp buffer; the
        // target lives in `backend_buf`.
        let mut temp_buf = Buffer::new();
        let temp_root = temp_buf
            .add_control_field_node(None, None, cf(1, 1), true)
            .unwrap();
        let reference_key = temp_buf
            .add_reference_node(Some(temp_root), None, target_ident, target)
            .expect("reference inserts");
        let ref_node = temp_buf.get(reference_key).unwrap();
        match &ref_node.kind {
            FieldNodeKind::Reference(data) => {
                assert_eq!(data.identifier, target_ident);
                assert_eq!(data.referenced, target);
            }
            _ => panic!("expected Reference"),
        }
        // The reference is discoverable by identifier within temp_buf.
        assert_eq!(
            temp_buf.get_control_field_node_with_identifier(1, 99),
            Some(reference_key)
        );
    }

    #[test]
    fn add_reference_node_rejects_duplicate_identifier_in_buffer() {
        let mut b = Buffer::new();
        let root = b
            .add_control_field_node(None, None, cf(1, 1), true)
            .unwrap();
        // The buffer already has identifier (1,1); a reference with
        // the same identifier is rejected.
        assert_eq!(
            b.add_reference_node(
                Some(root),
                None,
                ControlFieldIdentifier {
                    doc_handle: 1,
                    id: 1,
                },
                root
            ),
            None
        );
    }

    #[test]
    fn remove_leaf_text_node() {
        let mut b = Buffer::new();
        let root = b
            .add_control_field_node(None, None, cf(1, 1), true)
            .unwrap();
        let txt = b
            .add_text_field_node(Some(root), None, "hello".encode_utf16().collect())
            .unwrap();
        // Root absorbed the text node's length.
        assert_eq!(b.get(root).unwrap().length, 5);
        assert!(b.remove(txt, false));
        assert!(!b.contains(txt));
        // Length collapsed back to zero.
        assert_eq!(b.get(root).unwrap().length, 0);
        // Root's first_child / last_child cleared.
        assert_eq!(b.get(root).unwrap().first_child, None);
        assert_eq!(b.get(root).unwrap().last_child, None);
    }

    #[test]
    fn remove_root_requires_cascade() {
        let mut b = Buffer::new();
        let root = b
            .add_control_field_node(None, None, cf(1, 1), true)
            .unwrap();
        assert!(!b.remove(root, false));
        assert!(b.remove(root, true));
        assert_eq!(b.root(), None);
        assert!(!b.contains(root));
    }

    #[test]
    fn remove_subtree_cascade() {
        let mut b = Buffer::new();
        let root = b
            .add_control_field_node(None, None, cf(1, 1), true)
            .unwrap();
        let mid = b
            .add_control_field_node(Some(root), None, cf(1, 2), false)
            .unwrap();
        let leaf = b
            .add_text_field_node(Some(mid), None, "abc".encode_utf16().collect())
            .unwrap();
        assert!(b.remove(mid, true));
        assert!(!b.contains(mid));
        assert!(!b.contains(leaf));
        assert_eq!(b.get(root).unwrap().first_child, None);
        assert_eq!(b.get(root).unwrap().length, 0);
        // Identifier (1,2) is gone.
        assert_eq!(b.get_control_field_node_with_identifier(1, 2), None);
    }

    #[test]
    fn remove_without_cascade_adopts_children() {
        let mut b = Buffer::new();
        let root = b
            .add_control_field_node(None, None, cf(1, 1), true)
            .unwrap();
        let mid = b
            .add_control_field_node(Some(root), None, cf(1, 2), false)
            .unwrap();
        let grandchild = b
            .add_control_field_node(Some(mid), None, cf(1, 3), false)
            .unwrap();
        // Remove `mid` without cascading -- grandchild should
        // become a direct child of root.
        assert!(b.remove(mid, false));
        assert!(!b.contains(mid));
        assert!(b.contains(grandchild));
        assert_eq!(b.get(grandchild).unwrap().parent, Some(root));
        assert_eq!(b.get(root).unwrap().first_child, Some(grandchild));
        assert_eq!(b.get(root).unwrap().last_child, Some(grandchild));
    }

    #[test]
    fn clear_buffer_drops_everything() {
        let mut b = Buffer::new();
        let root = b
            .add_control_field_node(None, None, cf(1, 1), true)
            .unwrap();
        let _txt = b
            .add_text_field_node(Some(root), None, "x".encode_utf16().collect())
            .unwrap();
        b.clear();
        assert_eq!(b.root(), None);
        assert!(!b.contains(root));
        assert_eq!(b.get_control_field_node_with_identifier(1, 1), None);
    }
}
