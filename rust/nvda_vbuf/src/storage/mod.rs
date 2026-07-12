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

mod markup;
mod node;

pub use node::{ControlFieldData, FieldNodeKind, Node, NodeKey, TextFieldData};

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};

use regex::Regex;
use slotmap::SlotMap;

/// The `(docHandle, ID)` pair that uniquely identifies a control
/// field node in a buffer. Mirrors
/// `VBufStorage_controlFieldNodeIdentifier_t`.
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
pub struct ControlFieldIdentifier {
    pub doc_handle: i32,
    pub id: i32,
}

/// Result of `Buffer::locate_control_field_node_at_offset`. Holds
/// the deepest control (or reference) field that contains the
/// requested offset together with its `(start, end)` range and its
/// `(docHandle, ID)` identifier.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub struct LocateControlFieldResult {
    pub node: NodeKey,
    pub start: i32,
    pub end: i32,
    pub doc_handle: i32,
    pub id: i32,
}

/// Result of `Buffer::buffer_locate_text_field_node_at_offset`. Holds
/// the text-field node that contains the requested offset together
/// with its `(start, end)` range in the buffer's rendered text. Unlike
/// [`LocateControlFieldResult`] there is no `(docHandle, ID)` -- text
/// field nodes carry no identifier.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub struct LocateTextFieldResult {
    pub node: NodeKey,
    pub start: i32,
    pub end: i32,
}

/// Direction for an attribute search. Mirrors
/// `VBufStorage_findDirection_t` in `nvdaHelper/vbufBase/storage.h`.
/// The discriminants match the C++ enum order (and the constants in
/// `source/virtualBuffers/__init__.py`): `forward = 0`, `back = 1`,
/// `up = 2`.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum FindDirection {
    /// Depth-first forward from (but excluding) the node at the start
    /// offset.
    Forward,
    /// Depth-first backward from (but excluding) the node at the start
    /// offset, skipping the enclosing parent match at the offset.
    Back,
    /// Walk up the ancestor chain looking for a matching enclosing
    /// node.
    Up,
}

/// Result of [`Buffer::find_node_by_attributes`]: the matching node
/// plus its `(start, end)` offsets in the buffer's rendered text.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub struct FindNodeResult {
    pub node: NodeKey,
    pub start: i32,
    pub end: i32,
}

/// Direction for tree-order traversal. Mirrors `TreeDirection` in
/// `nvdaHelper/vbufBase/storage.h`.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum TreeDirection {
    /// Depth-first forward: first child, then siblings of ancestors.
    Forward,
    /// Inverse of `Forward`: previous sibling's deepest descendant,
    /// or parent if no previous sibling.
    Back,
    /// Backward but with the same first-step shape as Forward (last
    /// child first, then previous siblings).
    SymmetricalBack,
}

/// A vbuf storage buffer: an arena of nodes plus a root pointer plus
/// an identifier index for fast lookup. Mirrors
/// `VBufStorage_buffer_t` and the storage-relevant state of
/// `VBufBackend_t` (pending / working invalid subtree lists).
pub struct Buffer {
    nodes: SlotMap<NodeKey, Node>,
    root: Option<NodeKey>,
    /// Maps `(docHandle, id)` to the matching control field node so
    /// `getControlFieldNodeWithIdentifier` is `O(log n)` rather than
    /// a tree walk. Built up as control field nodes are added; pruned
    /// on removal.
    by_identifier: BTreeMap<ControlFieldIdentifier, NodeKey>,
    /// `selectionStart` from the C++ buffer. The selection is the
    /// half-open range `[selectionStart, selectionStart +
    /// selectionLength)` over the rendered text length.
    selection_start: i32,
    selection_length: i32,
    /// Control field nodes that have been invalidated and are
    /// awaiting re-render on the next update tick. The C++ original
    /// keeps this on `VBufBackend_t` since only backends invalidate;
    /// we co-locate it with the buffer because the keys reference
    /// the buffer's arena.
    pending_invalid: Vec<NodeKey>,
    /// Subset of `pending_invalid` currently being processed by an
    /// in-flight `update()` call. The C++ keeps both lists so that a
    /// reuse query during render can detect "this node was invalid
    /// at the start of this tick".
    working_invalid: Vec<NodeKey>,
    /// Cache of compiled regexes for `find_node_by_attributes`, keyed
    /// by the raw `regexp` input. Quick-nav (H/K/…) reissues the same
    /// pattern on every keypress, and `Regex::new` is comparatively
    /// expensive (~20 µs), so caching turns a repeated search into a
    /// map lookup + cheap `Regex` clone (Arc-backed). The C++ original
    /// recompiles its `std::wregex` on every call; this is a pure win
    /// on top of parity. Regex output depends only on the pattern, not
    /// on buffer content, so no invalidation is needed on edits.
    /// Bounded (cleared past `REGEX_CACHE_CAP`) so find-in-page text
    /// patterns can't grow it without limit.
    regex_cache: RefCell<HashMap<Vec<u16>, Regex>>,
}

/// Max distinct patterns retained in [`Buffer::regex_cache`] before it
/// is cleared. Comfortably covers every quick-nav key plus a run of
/// find-in-page searches; the clear is O(1)-amortised.
const REGEX_CACHE_CAP: usize = 64;

impl Buffer {
    /// Construct an empty buffer. The C++ default constructor.
    pub fn new() -> Self {
        Self {
            nodes: SlotMap::with_key(),
            root: None,
            by_identifier: BTreeMap::new(),
            selection_start: 0,
            selection_length: 0,
            pending_invalid: Vec::new(),
            working_invalid: Vec::new(),
            regex_cache: RefCell::new(HashMap::new()),
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
        self.selection_start = 0;
        self.selection_length = 0;
        self.pending_invalid.clear();
        self.working_invalid.clear();
    }

    /// Whether there are pending invalid subtrees waiting for the
    /// next update tick. Used by the gecko_ia2 isRootDocAlive
    /// fast-path: a pending update means the document is still
    /// alive even if we haven't checked COM yet.
    pub fn pending_invalid_subtrees_empty(&self) -> bool {
        self.pending_invalid.is_empty()
    }

    /// Move every pending invalidation into the working list and
    /// return the list as a snapshot for the caller to render.
    /// Mirrors the C++ `pendingInvalidSubtreesList.swap(working
    /// InvalidSubtreesList)` at the start of `VBufBackend_t::update`.
    ///
    /// Returns the snapshot rather than borrowing the working list
    /// because the caller typically iterates while the buffer is
    /// being mutated by render side-effects.
    pub fn take_pending_into_working(&mut self) -> Vec<NodeKey> {
        // The C++ `swap` puts `pendingInvalidSubtreesList`'s contents
        // into `workingInvalidSubtreesList`. We achieve the same by
        // moving pending into working. Working should be empty when
        // this is called (between update ticks).
        debug_assert!(
            self.working_invalid.is_empty(),
            "take_pending_into_working called while working list is \
             non-empty -- update() loop invariant violated"
        );
        std::mem::swap(&mut self.pending_invalid, &mut self.working_invalid);
        self.working_invalid.clone()
    }

    /// Drain the working list. Mirrors the C++
    /// `workingInvalidSubtreesList.clear()` at the end of update().
    pub fn clear_working(&mut self) {
        self.working_invalid.clear();
    }

    /// Remove `key` from the working list if present. Used by
    /// renders that take responsibility for an invalidated subtree
    /// (the caller's render call will produce the new content for
    /// it). Returns whether it was present.
    pub fn remove_from_working(&mut self, key: NodeKey) -> bool {
        if let Some(idx) =
            self.working_invalid.iter().position(|&k| k == key)
        {
            self.working_invalid.remove(idx);
            true
        } else {
            false
        }
    }

    /// Whether the working list still has entries to process.
    /// Useful for asserting that update completed.
    pub fn working_invalid_empty(&self) -> bool {
        self.working_invalid.is_empty()
    }

    /// Atomically replace one or more subtrees in `self` with the
    /// contents of fully-rendered temp buffers. Mirrors
    /// `VBufStorage_buffer_t::replaceSubtrees`. Each entry in `map`
    /// is a `(target_key, temp_buffer)` pair: `target_key` is the
    /// subtree in `self` to be replaced; `temp_buffer` is owned and
    /// dropped after its contents move into `self`.
    ///
    /// Returns `true` when every replacement succeeded, `false`
    /// when one or more failed (logged callsite-side; failures are
    /// continued past so other entries still apply).
    ///
    /// Each temp buffer's reference nodes (which point at existing
    /// nodes in `self`) are resolved before the target replacement
    /// by physically moving the referenced subtrees from `self`
    /// into the temp at the reference's slot. This matches the
    /// C++ original's two-pass behaviour
    /// (`storage.cpp:660-672`).
    pub fn replace_subtrees(
        &mut self,
        mut map: Vec<(NodeKey, Buffer)>,
    ) -> bool {
        // 1. Save the selection ancestor chain so we can re-anchor
        // after the mutations. The chain runs from outermost
        // ancestor (root-side) to innermost (selection-side); each
        // entry pairs an identifier with the relative selection
        // start within that ancestor.
        let selection_anchor = self.snapshot_selection_anchor();

        // 1b. Resolve reference nodes in each temp buffer by
        // physically moving the referenced subtree from self to
        // temp at the reference's position. Mirrors
        // storage.cpp:660-672. Done in reverse insertion / tree
        // order so each reference's saved (parent, previous)
        // remains valid (the still-untouched references and their
        // neighbours haven't been mutated yet).
        for (_target, temp_buffer) in map.iter_mut() {
            self.resolve_references_in_temp(temp_buffer);
        }

        // 2. For each (target, temp_buffer): record target's
        // (parent, previous) in self, remove the target with
        // cascade, handle by_identifier collisions, then move the
        // temp buffer's root into self at the saved anchor.
        let mut all_ok = true;
        for (target_key, mut temp_buffer) in map {
            if !self.contains(target_key) {
                all_ok = false;
                continue;
            }
            let parent = self.nodes[target_key].parent;
            let previous = self.nodes[target_key].previous;
            // Remove the target, including descendants. The C++
            // calls removeFieldNode(target) with default
            // removeDescendants=true.
            if !self.remove(target_key, true) {
                all_ok = false;
                continue;
            }
            let temp_root = match temp_buffer.root {
                Some(r) => r,
                None => continue, // empty temp -- nothing to insert
            };
            // Resolve identifier collisions BEFORE the move.
            // Mirrors the C++ post-merge by_identifier reconciliation
            // (lines 716-739) but applied per-replacement so the
            // move can carry its identifiers in cleanly.
            let temp_idents: Vec<ControlFieldIdentifier> = temp_buffer
                .by_identifier
                .keys()
                .copied()
                .collect();
            for ident in temp_idents {
                if let Some(existing) =
                    self.by_identifier.get(&ident).copied()
                {
                    // C++: removeFieldNode(existing, false)
                    // (children get adopted by the removed node's
                    // parent).
                    self.remove(existing, false);
                }
            }
            if self
                .move_subtree_from(
                    &mut temp_buffer,
                    temp_root,
                    parent,
                    previous,
                )
                .is_none()
            {
                all_ok = false;
            }
            // temp_buffer is dropped at end of iteration.
        }

        // 3. Re-anchor the selection.
        if let Some(anchor) = selection_anchor {
            self.restore_selection(anchor);
        }

        all_ok
    }

    /// Walk `temp`'s tree depth-first to collect every reference
    /// node, then iterate the list in reverse and replace each
    /// reference with the actual referenced subtree (moved out of
    /// `self`).
    ///
    /// The C++ original uses a `referenceNodes` insertion-order
    /// list on each buffer; we don't track that and instead rebuild
    /// the list via a tree walk. The order is depth-first preorder,
    /// reversed -- equivalent to insertion-order-reversed for any
    /// renderer that inserts references as it descends (which the
    /// gecko_ia2 fillVBuf does).
    ///
    /// Returns the count of references successfully resolved.
    fn resolve_references_in_temp(&mut self, temp: &mut Buffer) -> usize {
        // Collect reference keys depth-first preorder.
        let mut refs: Vec<NodeKey> = Vec::new();
        if let Some(root) = temp.root {
            let mut stack: Vec<NodeKey> = vec![root];
            while let Some(k) = stack.pop() {
                if matches!(temp.nodes[k].kind, FieldNodeKind::Reference(_)) {
                    refs.push(k);
                }
                // Push children in reverse so we visit them in
                // tree order on subsequent pops.
                let mut children: Vec<NodeKey> = Vec::new();
                let mut child = temp.nodes[k].first_child;
                while let Some(ck) = child {
                    children.push(ck);
                    child = temp.nodes[ck].next;
                }
                for c in children.into_iter().rev() {
                    stack.push(c);
                }
            }
        }
        // Reverse to mirror the C++ reference-list reverse-iteration.
        refs.reverse();

        let mut resolved = 0;
        for ref_key in refs {
            if !temp.contains(ref_key) {
                continue;
            }
            let parent = temp.nodes[ref_key].parent;
            let previous = temp.nodes[ref_key].previous;
            let referenced = match &temp.nodes[ref_key].kind {
                FieldNodeKind::Reference(d) => d.referenced,
                _ => continue,
            };
            // Drop the reference. Cascade is fine -- references
            // never have descendants in the gecko_ia2 flow.
            temp.remove(ref_key, true);
            // The referenced node lives in self. If it's already
            // been moved out (e.g. by an earlier reference in
            // *another* temp buffer in the same map), skip.
            if !self.contains(referenced) {
                continue;
            }
            if temp
                .move_subtree_from(self, referenced, parent, previous)
                .is_some()
            {
                resolved += 1;
            }
        }
        resolved
    }

    /// Snapshot the chain of (control-field-identifier,
    /// relative-selection-start) pairs from the deepest control
    /// field at the current selection position up through every
    /// ancestor. Used by `replace_subtrees` to re-anchor the
    /// selection after the tree has been mutated.
    fn snapshot_selection_anchor(
        &self,
    ) -> Option<Vec<(ControlFieldIdentifier, i32)>> {
        if self.text_length() == 0 {
            return None;
        }
        // Locate the control field at selectionStart.
        let (text_key, _rel) =
            self.locate_text_field_node_at_offset(
                self.root?,
                self.selection_start.max(0).min(self.text_length() - 1),
            )?;
        // Start from the text node's parent (the deepest control
        // field) and walk up.
        let text_offset =
            self.calculate_offset_in_tree(text_key);
        let mut relative = self.selection_start - text_offset;
        let mut chain: Vec<(ControlFieldIdentifier, i32)> = Vec::new();
        let mut cur = self.nodes[text_key].parent;
        while let Some(c) = cur {
            let ident = match &self.nodes[c].kind {
                FieldNodeKind::Control(d) => d.identifier,
                FieldNodeKind::Reference(d) => d.identifier,
                FieldNodeKind::Text(_) => break,
            };
            // Push during the deepest-to-root walk, then reverse
            // after the loop to get the root-first order (O(depth)
            // instead of the O(depth^2) of inserting at index 0).
            chain.push((ident, relative));
            // Accumulate previous-sibling lengths so the relative
            // offset stays correct as we walk up.
            let mut prev = self.nodes[c].previous;
            while let Some(p) = prev {
                relative += self.nodes[p].length;
                prev = self.nodes[p].previous;
            }
            cur = self.nodes[c].parent;
        }
        // The walk collected deepest-first; reverse to root-first.
        chain.reverse();
        if chain.is_empty() {
            None
        } else {
            Some(chain)
        }
    }

    /// Restore the selection to the deepest ancestor in `chain`
    /// that still exists in `self`, clamping to that ancestor's
    /// length. Mirrors the closing block of
    /// `VBufStorage_buffer_t::replaceSubtrees`.
    fn restore_selection(
        &mut self,
        chain: Vec<(ControlFieldIdentifier, i32)>,
    ) {
        let mut last_key: Option<NodeKey> = None;
        let mut last_relative: i32 = 0;
        for (ident, relative) in chain {
            let key = match self
                .by_identifier
                .get(&ident)
                .copied()
            {
                Some(k) => k,
                None => break,
            };
            // The C++ stops if currentAncestor.parent != lastAncestor
            // -- i.e. the chain has been reshuffled.
            if self.nodes[key].parent != last_key {
                break;
            }
            last_key = Some(key);
            last_relative = relative;
        }
        if let Some(key) = last_key {
            if let Some((start, _end)) = self.field_node_offsets(key) {
                let length = self.nodes[key].length;
                let clamp = (length - 1).max(0);
                self.selection_start = start + last_relative.min(clamp);
                // C++ doesn't touch selectionLength here; we leave
                // it alone too.
            }
        }
    }

    /// Move a subtree rooted at `source_root` from `source` into
    /// `self`. The subtree is detached from any tree links in
    /// `source` (the caller must have already unlinked
    /// `source_root` from its parent / siblings or `source_root`
    /// must have been a free root) and inserted into `self` at the
    /// `(parent, previous)` anchor. Returns the new root key in
    /// `self`, or `None` on validation failure.
    ///
    /// All descendant nodes of `source_root` are also moved. Their
    /// keys change (slotmap keys are arena-scoped); a key remapping
    /// table is built and used to translate every parent / sibling
    /// / child pointer to the new arena.
    ///
    /// `by_identifier` entries for moved control / reference nodes
    /// are transferred from `source` to `self`. **Identifier
    /// collisions are not handled here -- the caller must remove
    /// the colliding node in `self` before calling.** When a
    /// collision occurs, the existing entry in `self.by_identifier`
    /// is silently overwritten, which leaves the previously-
    /// existing node orphaned in the arena.
    pub fn move_subtree_from(
        &mut self,
        source: &mut Buffer,
        source_root: NodeKey,
        parent: Option<NodeKey>,
        previous: Option<NodeKey>,
    ) -> Option<NodeKey> {
        if !source.contains(source_root) {
            return None;
        }
        // The new parent must be an existing control field in self
        // (or None for root), and previous (if Some) must be a
        // current child of parent in self.
        if !self.validate_insertion_anchor(parent, previous) {
            return None;
        }

        // 0. Unlink source_root from its parent / siblings in source
        // so source's tree stays consistent after the move. When
        // source_root is source.root, this is a no-op (no parent or
        // sibling links to fix); otherwise we splice neighbours.
        let s_parent = source.nodes[source_root].parent;
        let s_prev = source.nodes[source_root].previous;
        let s_next = source.nodes[source_root].next;
        if let Some(prev_key) = s_prev {
            source.nodes[prev_key].next = s_next;
        } else if let Some(parent_key) = s_parent {
            source.nodes[parent_key].first_child = s_next;
        }
        if let Some(next_key) = s_next {
            source.nodes[next_key].previous = s_prev;
        } else if let Some(parent_key) = s_parent {
            source.nodes[parent_key].last_child = s_prev;
        }
        // Collapse ancestor lengths in source.
        let moved_length = source.nodes[source_root].length;
        if moved_length > 0 {
            let mut a = s_parent;
            while let Some(akey) = a {
                let n = &mut source.nodes[akey];
                n.length -= moved_length;
                debug_assert!(n.length >= 0);
                a = n.parent;
            }
        }

        // 1. Collect source keys depth-first.
        let mut source_keys: Vec<NodeKey> = Vec::new();
        let mut stack: Vec<NodeKey> = vec![source_root];
        while let Some(k) = stack.pop() {
            source_keys.push(k);
            let mut child = source.nodes[k].first_child;
            while let Some(ck) = child {
                stack.push(ck);
                child = source.nodes[ck].next;
            }
        }

        // 2. Drain source -> insert into self, building a remap.
        let mut remap: BTreeMap<NodeKey, NodeKey> = BTreeMap::new();
        for src_key in &source_keys {
            let node = source
                .nodes
                .remove(*src_key)
                .expect("collected key must still exist");
            // Pull the identifier (if any) from source and forward
            // it to self below.
            let ident = match &node.kind {
                FieldNodeKind::Control(d) => Some(d.identifier),
                FieldNodeKind::Reference(d) => Some(d.identifier),
                FieldNodeKind::Text(_) => None,
            };
            if let Some(i) = ident {
                source.by_identifier.remove(&i);
            }
            let new_key = self.nodes.insert(node);
            remap.insert(*src_key, new_key);
            if let Some(i) = ident {
                self.by_identifier.insert(i, new_key);
            }
        }

        // 3. Re-link parent/sibling/child pointers via remap. Only
        // links that point to keys in `source_keys` (i.e. inside the
        // moved subtree) get translated; links to nodes outside the
        // subtree are cleared (the C++ caller normally orphans the
        // subtree before move so external links are already None,
        // but defend against stale pointers).
        for src_key in &source_keys {
            let new_key = remap[src_key];
            let n = &mut self.nodes[new_key];
            n.parent = translate_link(n.parent, &remap);
            n.previous = translate_link(n.previous, &remap);
            n.next = translate_link(n.next, &remap);
            n.first_child = translate_link(n.first_child, &remap);
            n.last_child = translate_link(n.last_child, &remap);
        }

        // 4. Source's root may have been the moved subtree's root;
        // clear it.
        if source.root == Some(source_root) {
            source.root = None;
        }

        // 5. Reset the new root's external links (parent/prev/next)
        // before linking it into self. These would have pointed at
        // siblings or parent in source; in self they're determined
        // by the (parent, previous) anchor.
        let new_root = remap[&source_root];
        {
            let n = &mut self.nodes[new_root];
            n.parent = None;
            n.previous = None;
            n.next = None;
        }
        self.link(parent, previous, new_root);
        let length = self.nodes[new_root].length;
        self.bump_ancestor_lengths(new_root, length);
        Some(new_root)
    }

    /// Mark `key` as invalidated, scheduling its subtree for
    /// re-render on the next update tick. Mirrors
    /// `VBufBackend_t::invalidateSubtree` exactly:
    ///
    /// 1. If `key` requires its parent to update, walk up to the
    ///    closest ancestor that doesn't require its parent to
    ///    update; mark intermediates as not-reusable. Invalidate
    ///    that ancestor instead.
    /// 2. If `key` is already invalidated, no-op.
    /// 3. If `key` is a descendant of an already-invalidated node,
    ///    mark `key` and any intermediate ancestors as
    ///    not-reusable so the in-flight render won't reuse stale
    ///    state.
    /// 4. If `key` is an ancestor of any already-invalidated node,
    ///    remove those (they're subsumed by the new invalidation)
    ///    after marking each as non-reusable through to the new
    ///    ancestor.
    /// 5. Push `key` onto the pending list.
    ///
    /// Returns `false` when the key is stale or not in the buffer.
    pub fn invalidate_subtree(&mut self, key: NodeKey) -> bool {
        if !self.contains(key) {
            return false;
        }

        // Step 1: Walk up to the closest non-requires-parent-update
        // ancestor. The C++ mutates `node` in place; we mirror via a
        // local.
        let mut effective_key = key;
        loop {
            let n = &self.nodes[effective_key];
            let requires = match &n.kind {
                FieldNodeKind::Control(d) => d.requires_parent_update,
                FieldNodeKind::Reference(_) => false,
                FieldNodeKind::Text(_) => false,
            };
            if !requires {
                break;
            }
            // Mark this node as not reusable (it's about to be
            // bypassed by invalidating its parent).
            if let FieldNodeKind::Control(d) =
                &mut self.nodes[effective_key].kind
            {
                d.allow_reuse_in_ancestor_update = false;
            }
            match self.nodes[effective_key].parent {
                Some(p) => effective_key = p,
                None => break,
            }
        }

        // Step 2: already invalidated -> no-op.
        // Step 3: descendant of an invalidated node -> mark
        // intermediates and bail with `true`.
        // Snapshot the pending list to release the borrow before
        // we mutate the arena via mark_nonreusable_if_in_ancestor.
        let pending_snapshot: Vec<NodeKey> = self.pending_invalid.clone();
        for existing in &pending_snapshot {
            if *existing == effective_key {
                return true;
            }
            if self
                .mark_nonreusable_if_in_ancestor(effective_key, *existing)
            {
                return true;
            }
        }

        // Step 4: ancestor of any pending node -> mark them as
        // non-reusable and remove them from the list (they'll be
        // covered by the new invalidation).
        let target = effective_key;
        let mut to_drop: Vec<usize> = Vec::new();
        for (idx, &existing) in pending_snapshot.iter().enumerate() {
            if self.mark_nonreusable_if_in_ancestor(existing, target) {
                to_drop.push(idx);
            }
        }
        for &idx in to_drop.iter().rev() {
            self.pending_invalid.remove(idx);
        }

        // Step 5: enqueue.
        self.pending_invalid.push(effective_key);
        true
    }

    /// Walk parents of `node` until either `ancestor` is encountered
    /// (return `true`, marking every intermediate as
    /// `allow_reuse_in_ancestor_update = false`) or the root is
    /// reached without finding `ancestor` (return `false`, no
    /// mutation). Mirrors the recursive C++
    /// `markNodeAsNonreusableIfInAncestor`.
    fn mark_nonreusable_if_in_ancestor(
        &mut self,
        node: NodeKey,
        ancestor: NodeKey,
    ) -> bool {
        // Walk up collecting keys; we'll mark them only on success.
        let mut chain: Vec<NodeKey> = Vec::new();
        let mut cur = self.nodes[node].parent;
        let mut found = false;
        while let Some(p) = cur {
            if p == ancestor {
                found = true;
                break;
            }
            chain.push(p);
            cur = self.nodes[p].parent;
        }
        if !found {
            return false;
        }
        // Mark `node` plus every intermediate as not-reusable.
        if let FieldNodeKind::Control(d) = &mut self.nodes[node].kind {
            d.allow_reuse_in_ancestor_update = false;
        }
        for k in chain {
            if let FieldNodeKind::Control(d) = &mut self.nodes[k].kind {
                d.allow_reuse_in_ancestor_update = false;
            }
        }
        true
    }

    /// Try to reuse an existing control field node from this buffer
    /// when rendering a temporary subtree. The caller is rendering
    /// a node into a temp buffer with `(parent, previous)` already
    /// set up there; this method asks "is there an equivalent
    /// existing node in the main buffer with `(doc_handle, id)`
    /// that we can reuse?".
    ///
    /// Returns `Some(existing_key)` when the existing node is
    /// reuse-eligible. Mirrors `VBufBackend_t::reuseExistingNodeIn
    /// Render`.
    ///
    /// Returns `None` when:
    /// * `parent` is None (root nodes can't be reused).
    /// * `parent`'s `always_rerender_descendants` or
    ///   `always_rerender_children` is true.
    /// * No node with `(doc_handle, id)` exists in this buffer.
    /// * The existing node has no parent (it's the root).
    /// * The existing node's parent has `always_rerender_descendants`,
    ///   in which case we propagate that flag down to the existing
    ///   node and refuse reuse.
    /// * The existing node has `always_rerender_descendants` set.
    /// * The existing node has `allow_reuse_in_ancestor_update == false`.
    /// * The existing node has `deny_reuse_if_previous_siblings_changed
    ///   == true` and the previous control field of the new render
    ///   doesn't match the previous control field of the existing
    ///   node (a sibling has been added, removed, or moved).
    /// * The existing node is in `working_invalid` (it was already
    ///   marked invalid for re-render this tick). The method also
    ///   removes it from `working_invalid` in that case so the
    ///   caller's render takes responsibility.
    ///
    /// `previous` is from the *temp* buffer (because that's where
    /// the new render is going); we don't dereference it through
    /// `self`, only walk its previous chain in `temp_buffer`.
    pub fn reuse_existing_node_in_render(
        &mut self,
        temp_buffer: &Buffer,
        parent: Option<NodeKey>,
        previous: Option<NodeKey>,
        doc_handle: i32,
        id: i32,
    ) -> Option<NodeKey> {
        let parent = parent?;
        // The parent here is in the temp buffer (because the temp
        // render is the one supplying it). Read its rerender flags
        // from temp_buffer.
        if let Some(p_node) = temp_buffer.nodes.get(parent) {
            if let FieldNodeKind::Control(p_data) = &p_node.kind {
                if p_data.always_rerender_descendants
                    || p_data.always_rerender_children
                {
                    return None;
                }
            }
        } else {
            return None;
        }

        let existing = self.get_control_field_node_with_identifier(
            doc_handle, id,
        )?;
        let existing_parent = self.nodes[existing].parent?;

        // If the existing parent has alwaysRerenderDescendants,
        // propagate the flag down to existing and refuse reuse.
        let parent_always = matches!(
            &self.nodes[existing_parent].kind,
            FieldNodeKind::Control(d) if d.always_rerender_descendants
        );
        if parent_always {
            if let FieldNodeKind::Control(d) =
                &mut self.nodes[existing].kind
            {
                d.always_rerender_descendants = true;
            }
        }

        let existing_data = match &self.nodes[existing].kind {
            FieldNodeKind::Control(d) => d,
            // Reference nodes hit the by_identifier index too in
            // theory, but reuse semantics only apply to true control
            // field nodes per the C++ original (which uses dynamic_cast).
            _ => return None,
        };

        if existing_data.always_rerender_descendants {
            return None;
        }
        if !existing_data.allow_reuse_in_ancestor_update {
            return None;
        }
        let deny_on_sibling_change =
            existing_data.deny_reuse_if_previous_siblings_changed;
        // (existing_data borrow ends here.)

        if deny_on_sibling_change {
            // Find the previous control-field-like sibling in the
            // temp render's `previous` chain.
            let prev_temp_cf = walk_back_to_control_field(temp_buffer, previous);
            // Resolve through reference nodes: if the previous in
            // the temp render is a reference node, dereference it
            // to the actual control field.
            let prev_temp_resolved = match prev_temp_cf {
                Some(pk) => match &temp_buffer.nodes[pk].kind {
                    FieldNodeKind::Reference(rd) => Some(rd.referenced),
                    FieldNodeKind::Control(_) => Some(pk),
                    _ => None,
                },
                None => None,
            };
            // The C++ checks: if prev_temp_cf is a controlField but
            // not a referenceNode, return None (= the previous is a
            // newly-added node).
            if matches!(
                prev_temp_cf.and_then(|k| {
                    temp_buffer.nodes.get(k).map(|n| &n.kind)
                }),
                Some(FieldNodeKind::Control(_))
            ) {
                return None;
            }

            // Find the previous control field in self, walking from
            // `existing.previous` backward.
            let prev_existing = walk_back_to_control_field(
                self,
                self.nodes[existing].previous,
            );
            if prev_temp_resolved != prev_existing {
                return None;
            }
        }

        // If existing was already in working_invalid, the caller
        // takes responsibility -- remove it from working_invalid
        // and refuse reuse (the caller will render it).
        if let Some(idx) =
            self.working_invalid.iter().position(|&k| k == existing)
        {
            self.working_invalid.remove(idx);
            return None;
        }

        Some(existing)
    }

    /// Total rendered text length of the buffer (the root node's
    /// length, or 0 when empty). Mirrors
    /// `VBufStorage_buffer_t::getTextLength`.
    pub fn text_length(&self) -> i32 {
        match self.root {
            Some(root) => self.nodes[root].length,
            None => 0,
        }
    }

    /// Read the current selection range as `(start, end)`. Mirrors
    /// `VBufStorage_buffer_t::getSelectionOffsets` -- the values are
    /// clamped against `[0, text_length()]` even if the underlying
    /// `selection_start` / `selection_length` were set when the
    /// buffer had different content.
    pub fn selection_offsets(&self) -> (i32, i32) {
        let max_end = self.text_length();
        let start = self.selection_start.max(0);
        let end = (self.selection_start + self.selection_length).min(max_end);
        (start, end)
    }

    /// Set the current selection range to the half-open
    /// `[start, end)`. Returns `false` (no mutation) for negative or
    /// inverted ranges. Mirrors
    /// `VBufStorage_buffer_t::setSelectionOffsets`.
    pub fn set_selection_offsets(
        &mut self,
        start_offset: i32,
        end_offset: i32,
    ) -> bool {
        if start_offset < 0 || end_offset < 0 || end_offset < start_offset {
            return false;
        }
        self.selection_start = start_offset;
        self.selection_length = end_offset - start_offset;
        true
    }

    /// Compute the start and end of the line containing the given
    /// `offset`. Mirrors
    /// `VBufStorage_buffer_t::getLineOffsets`.
    ///
    /// `max_line_length` (when > 0) wraps long lines on whitespace
    /// boundaries. `use_screen_layout` controls whether the walk
    /// crosses control field boundaries (when `false`, the search
    /// stops at the enclosing text node's parent).
    ///
    /// Returns `None` for an empty buffer, an out-of-range `offset`,
    /// or any other internal failure to locate the initial node.
    pub fn line_offsets(
        &self,
        offset: i32,
        max_line_length: i32,
        use_screen_layout: bool,
    ) -> Option<(i32, i32)> {
        let root = self.root?;
        if offset >= self.nodes[root].length || offset < 0 {
            return None;
        }
        let (init_node, init_rel) =
            self.locate_text_field_node_at_offset(root, offset)?;
        let init_buffer_start = offset - init_rel;
        let init_buffer_end =
            init_buffer_start + self.nodes[init_node].length;

        // The block-element ancestor we don't cross during the
        // forward / backward walks. Walk parents until we find a
        // block node; None when no enclosing block exists.
        let limit_block_node = self.nearest_block_ancestor(init_node);

        let mut possible_breaks: BTreeSet<i32> = BTreeSet::new();
        let mut line_end = init_buffer_end;
        let mut line_start = init_buffer_start;

        // ----- forward search -----
        {
            let mut node = init_node;
            let mut relative = init_rel;
            let mut buffer_start = init_buffer_start;
            let mut buffer_end = init_buffer_end;
            loop {
                possible_breaks.insert(buffer_start);
                possible_breaks.insert(buffer_end);
                let n = &self.nodes[node];
                if n.length > 0 && n.first_child.is_none() {
                    line_end = buffer_end;
                    if let Some(text) = node_text_slice(n) {
                        let mut last_was_space = false;
                        let mut found_hard_break = false;
                        for i in (relative as usize)..(n.length as usize) {
                            let c = text[i];
                            // CR (not followed by LF) or LF -> hard break
                            let is_cr = c == b'\r' as u16
                                && (i + 1 >= n.length as usize
                                    || text[i + 1] != b'\n' as u16);
                            let is_lf = c == b'\n' as u16;
                            if is_cr || is_lf {
                                line_end = buffer_start + i as i32 + 1;
                                found_hard_break = true;
                                break;
                            }
                            if is_whitespace_w(c) {
                                last_was_space = true;
                            } else {
                                if last_was_space {
                                    possible_breaks
                                        .insert(buffer_start + i as i32);
                                }
                                last_was_space = false;
                            }
                        }
                        if found_hard_break {
                            break;
                        }
                    }
                }
                // Advance forward; bail if we'd cross a block edge or
                // (in non-screen-layout mode) a control field.
                let step = self.next_node_in_tree(
                    node,
                    TreeDirection::Forward,
                    limit_block_node,
                );
                let (next_key, next_rel) = match step {
                    Some(p) => p,
                    None => break,
                };
                let next = &self.nodes[next_key];
                let blocked_by_control =
                    !use_screen_layout && next.first_child.is_some();
                if blocked_by_control || next.is_block {
                    break;
                }
                buffer_start += next_rel;
                buffer_end = buffer_start + next.length;
                relative = 0;
                node = next_key;
            }
        }

        // ----- backward search -----
        {
            let mut node = init_node;
            let mut relative = init_rel;
            let mut buffer_start = init_buffer_start;
            let mut buffer_end = init_buffer_end;
            loop {
                possible_breaks.insert(buffer_start);
                possible_breaks.insert(buffer_end);
                let n = &self.nodes[node];
                if n.length > 0 && n.first_child.is_none() {
                    line_start = buffer_start;
                    if let Some(text) = node_text_slice(n) {
                        let mut last_was_space = false;
                        let mut found_hard_break = false;
                        for i in (0..relative as usize).rev() {
                            let c = text[i];
                            let is_cr = c == b'\r' as u16
                                && (i + 1 >= n.length as usize
                                    || text[i + 1] != b'\n' as u16);
                            let is_lf = c == b'\n' as u16;
                            if is_cr || is_lf {
                                line_start =
                                    buffer_start + i as i32 + 1;
                                found_hard_break = true;
                                break;
                            }
                            if is_whitespace_w(c) {
                                if !last_was_space {
                                    possible_breaks.insert(
                                        buffer_start + i as i32 + 1,
                                    );
                                }
                                last_was_space = true;
                            } else {
                                last_was_space = false;
                            }
                        }
                        if found_hard_break {
                            break;
                        }
                    }
                }
                // For backward search, the limit shifts: when not
                // using screen layout, we stop crossing this node's
                // parent (preventing leaving the enclosing control
                // field).
                let limit = if use_screen_layout {
                    limit_block_node
                } else {
                    self.nodes[node].parent
                };
                let step = self.next_node_in_tree(
                    node,
                    TreeDirection::SymmetricalBack,
                    limit,
                );
                let (prev_key, prev_rel) = match step {
                    Some(p) => p,
                    None => break,
                };
                let prev = &self.nodes[prev_key];
                if prev.is_block {
                    break;
                }
                buffer_start += prev_rel;
                buffer_end = buffer_start + prev.length;
                relative = prev.length;
                node = prev_key;
            }
        }

        // ----- maxLineLength wrapping -----
        if max_line_length > 0 {
            let mut real_breaks: BTreeSet<i32> = BTreeSet::new();
            real_breaks.insert(line_start);
            real_breaks.insert(line_end);
            let mut i = line_start;
            let mut line_char_counter = 0;
            while i < line_end {
                if line_char_counter == max_line_length {
                    // Snap back to the nearest possible break before
                    // i, but only if it's still within max_line_length
                    // characters of the wrap point.
                    if !possible_breaks.is_empty() {
                        if let Some(&candidate) = possible_breaks
                            .range(..i)
                            .next_back()
                        {
                            if candidate > i - max_line_length {
                                i = candidate;
                            }
                        }
                    }
                    real_breaks.insert(i);
                    line_char_counter = 0;
                }
                i += 1;
                line_char_counter += 1;
            }
            // Final line bounds: greatest realBreak <= offset and
            // smallest realBreak > offset.
            let after = real_breaks
                .range((offset + 1)..)
                .next()
                .copied()
                .unwrap_or(line_end);
            let before = real_breaks
                .range(..=offset)
                .next_back()
                .copied()
                .unwrap_or(line_start);
            line_end = after;
            line_start = before;
        }

        Some((line_start, line_end))
    }

    /// Walk parent pointers to find the nearest ancestor whose
    /// `is_block` flag is true. Returns `None` if there's no
    /// enclosing block ancestor (e.g. the node is at the top of
    /// the tree without a block-bearing parent).
    fn nearest_block_ancestor(&self, key: NodeKey) -> Option<NodeKey> {
        let mut cur = self.nodes[key].parent;
        while let Some(c) = cur {
            if self.nodes[c].is_block {
                return Some(c);
            }
            cur = self.nodes[c].parent;
        }
        None
    }

    /// Buffer-level wrapper for `get_text_in_range`. Mirrors
    /// `VBufStorage_buffer_t::getTextInRange`. Validates the offsets
    /// against the root's length and starts the walk at the root.
    /// Returns `false` for an empty buffer or out-of-range
    /// arguments; `true` otherwise (with `out` populated).
    pub fn buffer_get_text_in_range(
        &self,
        start_offset: i32,
        end_offset: i32,
        out: &mut Vec<u16>,
        use_markup: bool,
    ) -> bool {
        let root = match self.root {
            Some(r) => r,
            None => return false,
        };
        let length = self.nodes[root].length;
        if start_offset < 0 || start_offset >= end_offset || end_offset > length {
            return false;
        }
        if use_markup {
            self.get_text_in_range_with_markup(
                root, start_offset, end_offset, out,
            );
        } else {
            self.get_text_in_range(root, start_offset, end_offset, out);
        }
        true
    }

    /// Append the text content of `key`'s subtree, restricted to the
    /// half-open range `[start_offset, end_offset)`, to `out`.
    ///
    /// Mirrors `VBufStorage_fieldNode_t::getTextInRange` /
    /// `VBufStorage_textFieldNode_t::getTextInRange`. Markup
    /// generation (`useMarkup` in the C++) is not implemented yet --
    /// it requires the `generateMarkup*` family which is its own
    /// follow-up commit.
    pub fn get_text_in_range(
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
        match &n.kind {
            FieldNodeKind::Text(data) => {
                let s = start_offset as usize;
                let e = end_offset as usize;
                out.extend_from_slice(&data.text[s..e]);
            }
            // Control / Reference: walk children with offsets.
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
                        self.get_text_in_range(
                            ckey, sub_start, sub_end, out,
                        );
                    }
                    child_start += child_length;
                    child = child_node.next;
                }
            }
        }
    }

    /// Return `(start, end)` for the given node within the buffer's
    /// rendered text. Mirrors
    /// `VBufStorage_buffer_t::getFieldNodeOffsets`. Returns `None`
    /// when the key is stale.
    pub fn field_node_offsets(&self, key: NodeKey) -> Option<(i32, i32)> {
        if !self.contains(key) {
            return None;
        }
        let start = self.calculate_offset_in_tree(key);
        Some((start, start + self.nodes[key].length))
    }

    /// `true` when the given `offset` falls within the node's range
    /// in the buffer. Mirrors
    /// `VBufStorage_buffer_t::isFieldNodeAtOffset`.
    pub fn is_field_node_at_offset(
        &self,
        key: NodeKey,
        offset: i32,
    ) -> bool {
        if offset < 0 || offset >= self.text_length() {
            return false;
        }
        match self.field_node_offsets(key) {
            Some((start, end)) => offset >= start && offset < end,
            None => false,
        }
    }

    /// `true` when the buffer has rendered content (i.e. has a
    /// root). Mirrors `VBufStorage_buffer_t::hasContent`.
    pub fn has_content(&self) -> bool {
        self.root.is_some()
    }

    /// Buffer-level wrapper that finds the text-field node at the
    /// given `offset`, returning it with its `(start, end)` range in
    /// the buffer's rendered text. Mirrors
    /// `VBufStorage_buffer_t::locateTextFieldNodeAtOffset`
    /// (`nvdaHelper/vbufBase/storage.cpp:865`), the public buffer
    /// method backing `VBufRemote_locateTextFieldNodeAtOffset`.
    ///
    /// This is the buffer-entry counterpart to the recursive
    /// [`Buffer::locate_text_field_node_at_offset`] subtree helper --
    /// the same `buffer_*` / bare naming split used by
    /// [`Buffer::buffer_get_text_in_range`] vs
    /// [`Buffer::get_text_in_range`].
    ///
    /// Returns `None` for an empty buffer or an out-of-range `offset`
    /// (matching the C++ `offset < 0 || offset >= getTextLength()`
    /// guard). Where the C++ would `nhAssert` on an internal failure
    /// to locate a node under an in-range offset, the Rust returns
    /// `None`.
    pub fn buffer_locate_text_field_node_at_offset(
        &self,
        offset: i32,
    ) -> Option<LocateTextFieldResult> {
        let root = self.root?;
        if offset < 0 || offset >= self.text_length() {
            return None;
        }
        let (node, rel_within_text) =
            self.locate_text_field_node_at_offset(root, offset)?;
        let start = offset - rel_within_text;
        let end = start + self.nodes[node].length;
        Some(LocateTextFieldResult { node, start, end })
    }

    /// Find the deepest control field that contains `offset`. The
    /// result is the immediate parent of the text-field at that
    /// offset, plus that parent's `(start, end)` range and its
    /// `(docHandle, ID)` identifier.
    ///
    /// Mirrors `VBufStorage_buffer_t::locateControlFieldNodeAtOffset`.
    /// Returns `None` if the buffer is empty, the offset is out of
    /// range, the text field has no parent (i.e. it's the root --
    /// pathological), or the parent is a text variant (also
    /// pathological).
    pub fn locate_control_field_node_at_offset(
        &self,
        offset: i32,
    ) -> Option<LocateControlFieldResult> {
        let root = self.root?;
        if offset < 0 || offset >= self.text_length() {
            return None;
        }
        let (text_key, rel_within_text) =
            self.locate_text_field_node_at_offset(root, offset)?;
        let text_global_start = offset - rel_within_text;
        let parent = self.nodes[text_key].parent?;

        // Sum lengths of preceding siblings of the text node to
        // compute the text node's offset within its parent.
        let mut text_offset_in_parent = 0;
        let mut prev = self.nodes[text_key].previous;
        while let Some(p) = prev {
            text_offset_in_parent += self.nodes[p].length;
            prev = self.nodes[p].previous;
        }
        let parent_start = text_global_start - text_offset_in_parent;
        let parent_end = parent_start + self.nodes[parent].length;
        let identifier = match &self.nodes[parent].kind {
            FieldNodeKind::Control(d) => d.identifier,
            FieldNodeKind::Reference(d) => d.identifier,
            FieldNodeKind::Text(_) => return None,
        };
        Some(LocateControlFieldResult {
            node: parent,
            start: parent_start,
            end: parent_end,
            doc_handle: identifier.doc_handle,
            id: identifier.id,
        })
    }

    /// Find a field node whose attributes match `regexp`, searching
    /// from `offset` in the given `direction`. Mirrors
    /// `VBufStorage_buffer_t::findNodeByAttributes`
    /// (`nvdaHelper/vbufBase/storage.cpp:973`).
    ///
    /// `offset` is the character offset to search from; `-1` means
    /// "start at the root" (search the whole buffer). Any other
    /// negative value, or an offset at/beyond the buffer's length,
    /// yields `None`.
    ///
    /// `attribs` is a whitespace-separated list of attribute names
    /// (exactly the `" ".join(reqAttrs)` string built by
    /// `source/virtualBuffers/__init__.py::_prepareForFindByAttributes`).
    /// For each candidate node, a `name:value;name:value;...` string
    /// is assembled (see [`Buffer::match_attributes`]) and tested
    /// against `regexp`.
    ///
    /// # Regex dialect
    ///
    /// The C++ original compiles `regexp` as a `std::wregex`
    /// (ECMAScript grammar, case-sensitive) and applies it with
    /// `std::regex_match`, i.e. the *whole* candidate string must
    /// match. This port compiles `regexp` once with the standard
    /// `regex` crate, wrapping it as `\A(?:<regexp>)\z` so that
    /// `Regex::is_match` reproduces the fully-anchored `regex_match`
    /// semantics (the `(?:...)` guards top-level alternation, which
    /// the production patterns always contain). Matching is
    /// case-sensitive, mirroring the C++ default.
    ///
    /// The production patterns (from `_prepareForFindByAttributes`)
    /// use only alternation, non-capturing groups, character classes,
    /// quantifiers, and `\b` word boundaries -- no backreferences or
    /// lookaround -- so the standard `regex` crate is sufficient
    /// (`fancy-regex` is not required). The regex is compiled once per
    /// call, before the traversal, exactly as the C++ does; a compile
    /// error (or non-UTF-16 `regexp`) yields `None`, mirroring the C++
    /// `catch(...) { return NULL; }`.
    pub fn find_node_by_attributes(
        &self,
        offset: i32,
        direction: FindDirection,
        attribs: &[u16],
        regexp: &[u16],
    ) -> Option<FindNodeResult> {
        let root = self.root?;
        if offset >= self.nodes[root].length {
            // Empty buffer (length 0, offset >= 0) or offset past the
            // end -> no result. Note offset == -1 always passes here.
            return None;
        }

        // Determine the starting node and the running buffer-start
        // offset. `bufferEnd` from the C++ is recomputed inside every
        // direction branch before use, so it isn't tracked here.
        let start_node;
        let mut buffer_start;
        if offset == -1 {
            start_node = root;
            buffer_start = 0;
        } else if offset >= 0 {
            // locate the text field node at `offset`; its global start
            // is `offset - rel`.
            let (text_key, rel) =
                self.locate_text_field_node_at_offset(root, offset)?;
            buffer_start = offset - rel;
            start_node = text_key;
        } else {
            // offset < -1 is invalid.
            return None;
        }

        // Split the attribs string at whitespace (mirrors the C++
        // `istream_iterator<wstring>` copy into a vector).
        let attribs_list = split_whitespace_utf16(attribs);

        // Compile the regex (cached by pattern). `\A(?:...)\z` anchors
        // the whole candidate string, reproducing `std::regex_match`.
        // The cloned `Ref` is dropped before the miss branch takes a
        // `borrow_mut`, so there is no RefCell double-borrow.
        let cached = self.regex_cache.borrow().get(regexp).cloned();
        let regex = match cached {
            Some(r) => r,
            None => {
                let pattern = String::from_utf16(regexp).ok()?;
                let r = Regex::new(&format!(r"\A(?:{pattern})\z")).ok()?;
                let mut cache = self.regex_cache.borrow_mut();
                if cache.len() >= REGEX_CACHE_CAP {
                    cache.clear();
                }
                cache.insert(regexp.to_vec(), r.clone());
                r
            }
        };

        match direction {
            FindDirection::Forward => {
                let mut cursor = self.next_node_in_tree(
                    start_node,
                    TreeDirection::Forward,
                    None,
                );
                while let Some((node, rel)) = cursor {
                    buffer_start += rel;
                    let length = self.nodes[node].length;
                    let buffer_end = buffer_start + length;
                    let n = &self.nodes[node];
                    if length > 0
                        && !n.is_hidden
                        && self.match_attributes(node, &attribs_list, &regex)
                    {
                        return Some(FindNodeResult {
                            node,
                            start: buffer_start,
                            end: buffer_end,
                        });
                    }
                    cursor = self.next_node_in_tree(
                        node,
                        TreeDirection::Forward,
                        None,
                    );
                }
                None
            }
            FindDirection::Back => {
                // Skip the first containing-parent match (the node the
                // offset starts in, or a parent that strictly contains
                // the offset), so "previous" doesn't return the node
                // the caller is already inside.
                let mut skipped_first_match = false;
                let mut cursor = self.next_node_in_tree(
                    start_node,
                    TreeDirection::Back,
                    None,
                );
                while let Some((node, rel)) = cursor {
                    buffer_start += rel;
                    let length = self.nodes[node].length;
                    let buffer_end = buffer_start + length;
                    let n = &self.nodes[node];
                    if length > 0
                        && !n.is_hidden
                        && self.match_attributes(node, &attribs_list, &regex)
                    {
                        if buffer_start == offset
                            || (!skipped_first_match
                                && buffer_start < offset
                                && buffer_end > offset)
                        {
                            skipped_first_match = true;
                        } else {
                            return Some(FindNodeResult {
                                node,
                                start: buffer_start,
                                end: buffer_end,
                            });
                        }
                    }
                    cursor = self.next_node_in_tree(
                        node,
                        TreeDirection::Back,
                        None,
                    );
                }
                None
            }
            FindDirection::Up => {
                // do { walk to first sibling, then step to parent } while
                // the parent exists and is hidden or doesn't match.
                let mut node = start_node;
                loop {
                    // Walk to the first sibling, decrementing
                    // buffer_start by each sibling's length as we pass
                    // it (mirrors the C++ comma-operator update, which
                    // subtracts the *new* previous node's length).
                    while let Some(prev) = self.nodes[node].previous {
                        node = prev;
                        buffer_start -= self.nodes[node].length;
                    }
                    // Step up to the parent. No parent -> no enclosing
                    // match; return None.
                    let parent = self.nodes[node].parent?;
                    node = parent;
                    let buffer_end = buffer_start + self.nodes[node].length;
                    let n = &self.nodes[node];
                    if !n.is_hidden
                        && self.match_attributes(node, &attribs_list, &regex)
                    {
                        return Some(FindNodeResult {
                            node,
                            start: buffer_start,
                            end: buffer_end,
                        });
                    }
                    // else: loop again, walking up from this parent.
                }
            }
        }
    }

    /// Build the `name:value;name:value;...` candidate string for
    /// `key` and test it against `regex`. Mirrors
    /// `VBufStorage_fieldNode_t::matchAttributes`
    /// (`nvdaHelper/vbufBase/storage.cpp:151`).
    ///
    /// For each attribute name in `attribs`, the escaped name, a
    /// `:`, the escaped attribute value (empty when the node lacks
    /// the attribute), and a `;` are appended. A name beginning with
    /// `parent::` (and only when the node has a parent) is looked up
    /// on the parent node instead, with the prefix stripped. Values
    /// are truncated to 100 UTF-16 code units before escaping, exactly
    /// as the C++ does (`regexAttribValueLimit`).
    fn match_attributes(
        &self,
        key: NodeKey,
        attribs: &[Vec<u16>],
        regex: &Regex,
    ) -> bool {
        // The max source length (in UTF-16 code units) of an attribute
        // value included in the candidate string. The C++ truncates
        // large values (e.g. `name`) because `regex_match` can throw
        // on very large inputs; since matches only test non-emptiness
        // of such values, truncation is safe.
        const VALUE_LIMIT: usize = 100;
        let node = &self.nodes[key];
        let mut candidate: Vec<u16> = Vec::new();
        for name in attribs {
            push_escaped_attribute(&mut candidate, name, 0);
            candidate.push(b':' as u16);
            // A name may redirect to the parent via a `parent::`
            // prefix at index 0 (e.g. "parent::IAccessible2::role").
            // The redirect only applies when the node actually has a
            // parent; otherwise the full name is looked up on the node
            // itself (and typically won't exist), matching the C++.
            let value = match node.parent {
                Some(parent) if name.starts_with(&PARENT_PREFIX) => self
                    .nodes[parent]
                    .get_attribute(&name[PARENT_PREFIX.len()..]),
                _ => node.get_attribute(name),
            };
            if let Some(val) = value {
                push_escaped_attribute(&mut candidate, val, VALUE_LIMIT);
            }
            candidate.push(b';' as u16);
        }
        // Convert to UTF-8 for the regex. The candidate is built from
        // BMP-heavy attribute text; `from_utf16_lossy` maps any stray
        // unpaired surrogate to U+FFFD, which cannot spuriously match
        // the ASCII-structured production patterns.
        let candidate = String::from_utf16_lossy(&candidate);
        regex.is_match(&candidate)
    }

    /// Return the identifier of a control field (or reference)
    /// node, or `None` if the key is stale or the node is a text
    /// variant. Mirrors
    /// `VBufStorage_buffer_t::getIdentifierFromControlFieldNode`.
    pub fn identifier_of_control_field_node(
        &self,
        key: NodeKey,
    ) -> Option<ControlFieldIdentifier> {
        let n = self.nodes.get(key)?;
        match &n.kind {
            FieldNodeKind::Control(d) => Some(d.identifier),
            FieldNodeKind::Reference(d) => Some(d.identifier),
            FieldNodeKind::Text(_) => None,
        }
    }

    /// Compute the offset of `key` from the start of the tree by
    /// summing the lengths of every preceding sibling (recursively
    /// up the parent chain). Mirrors
    /// `VBufStorage_fieldNode_t::calculateOffsetInTree`.
    pub fn calculate_offset_in_tree(&self, key: NodeKey) -> i32 {
        if !self.contains(key) {
            return 0;
        }
        let mut offset = 0;
        let mut cur = self.nodes[key].previous;
        while let Some(prev) = cur {
            offset += self.nodes[prev].length;
            cur = self.nodes[prev].previous;
        }
        if let Some(parent) = self.nodes[key].parent {
            offset += self.calculate_offset_in_tree(parent);
        }
        offset
    }

    /// Locate the descendant text-field node that holds the given
    /// `offset` within the subtree rooted at `key`. Returns
    /// `Some((text_key, relative_offset))` where `relative_offset`
    /// is the byte offset within the text node, or `None` when
    /// `offset` falls outside the subtree.
    ///
    /// Mirrors `VBufStorage_fieldNode_t::locateTextFieldNodeAtOffset`
    /// / `VBufStorage_textFieldNode_t::locateTextFieldNodeAtOffset`.
    pub fn locate_text_field_node_at_offset(
        &self,
        key: NodeKey,
        offset: i32,
    ) -> Option<(NodeKey, i32)> {
        if !self.contains(key) {
            return None;
        }
        let n = &self.nodes[key];
        match &n.kind {
            FieldNodeKind::Text(_) => {
                if offset < 0 || offset >= n.length {
                    None
                } else {
                    Some((key, offset))
                }
            }
            _ => {
                let mut acc = 0;
                let mut child = n.first_child;
                while let Some(ckey) = child {
                    let child_length = self.nodes[ckey].length;
                    if offset < acc + child_length {
                        return self.locate_text_field_node_at_offset(
                            ckey,
                            offset - acc,
                        );
                    }
                    acc += child_length;
                    child = self.nodes[ckey].next;
                }
                None
            }
        }
    }

    /// `true` when the node has rendered content beyond pure
    /// whitespace + private-use characters. Mirrors
    /// `nodeHasUsefulContent` in `nvdaHelper/vbufBase/utils.cpp`:
    /// length 0 -> false, length > 3 -> true (cheap fast path),
    /// otherwise scan the rendered text and return true if any
    /// non-space non-private character is found.
    pub fn node_has_useful_content(&self, key: NodeKey) -> bool {
        let n = match self.nodes.get(key) {
            Some(n) => n,
            None => return false,
        };
        let length = n.length;
        if length == 0 {
            return false;
        }
        if length > 3 {
            return true;
        }
        let mut buf: Vec<u16> = Vec::new();
        self.get_text_in_range(key, 0, length, &mut buf);
        buf.iter().any(|&c| !is_whitespace_w(c) && !is_private_character(c))
    }

    /// `true` when the node's rendered text content equals `s`.
    /// Mirrors `nodeContentMatchesString` in
    /// `nvdaHelper/vbufBase/utils.cpp`.
    pub fn node_content_matches_string(
        &self,
        key: NodeKey,
        s: &[u16],
    ) -> bool {
        let length = match self.nodes.get(key) {
            Some(n) => n.length as usize,
            None => return false,
        };
        if length != s.len() {
            return false;
        }
        let mut buf: Vec<u16> = Vec::new();
        self.get_text_in_range(key, 0, length as i32, &mut buf);
        buf == s
    }

    /// Step from `key` to the next node in tree order, optionally
    /// stopping when reaching `limit_node`. Returns the next node's
    /// key and its offset relative to `key`'s start.
    ///
    /// Mirrors `VBufStorage_fieldNode_t::nextNodeInTree`. The C++
    /// returns the relative *start* offset; here that's the second
    /// tuple element.
    pub fn next_node_in_tree(
        &self,
        key: NodeKey,
        direction: TreeDirection,
        limit_node: Option<NodeKey>,
    ) -> Option<(NodeKey, i32)> {
        if !self.contains(key) {
            return None;
        }
        let length = self.nodes[key].length;
        match direction {
            TreeDirection::Forward => {
                if let Some(child) = self.nodes[key].first_child {
                    return Some((child, 0));
                }
                // Walk up until we find an ancestor with a `next`.
                let mut cur = Some(key);
                while let Some(c) = cur {
                    if let Some(next) = self.nodes[c].next {
                        if Some(next) == limit_node {
                            return None;
                        }
                        return Some((next, length));
                    }
                    let parent = self.nodes[c].parent;
                    if parent == limit_node {
                        return None;
                    }
                    cur = parent;
                }
                None
            }
            TreeDirection::Back => {
                if let Some(prev) = self.nodes[key].previous {
                    if Some(prev) == limit_node {
                        return None;
                    }
                    // Drill into prev's last-child chain (without
                    // crossing the limit).
                    let mut cur = prev;
                    loop {
                        let last = self.nodes[cur].last_child;
                        match last {
                            Some(lk) if Some(lk) != limit_node => cur = lk,
                            _ => break,
                        }
                    }
                    let rel = -self.nodes[cur].length;
                    return Some((cur, rel));
                }
                if let Some(parent) = self.nodes[key].parent {
                    if Some(parent) == limit_node {
                        return None;
                    }
                    // Parent's relative offset from `key` is 0 (the
                    // C++ leaves relativeOffset at 0 in this branch).
                    return Some((parent, 0));
                }
                None
            }
            TreeDirection::SymmetricalBack => {
                if let Some(last) = self.nodes[key].last_child {
                    let last_len = self.nodes[last].length;
                    return Some((last, length - last_len));
                }
                let mut cur = Some(key);
                while let Some(c) = cur {
                    if let Some(prev) = self.nodes[c].previous {
                        if Some(prev) == limit_node {
                            return None;
                        }
                        let rel = -self.nodes[prev].length;
                        return Some((prev, rel));
                    }
                    let parent = self.nodes[c].parent;
                    if parent == limit_node {
                        return None;
                    }
                    cur = parent;
                }
                None
            }
        }
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

/// Translate an optional `NodeKey` link via the `remap` table. Keys
/// not in the remap (i.e. pointing outside the moved subtree) are
/// returned as `None`.
fn translate_link(
    key: Option<NodeKey>,
    remap: &BTreeMap<NodeKey, NodeKey>,
) -> Option<NodeKey> {
    key.and_then(|k| remap.get(&k).copied())
}

/// Walk back from `start` via `previous` pointers in `buffer` until
/// a control-field-like node (Control or Reference variant) is
/// encountered. Returns `None` when the chain ends without finding
/// one. Used by `reuse_existing_node_in_render`.
fn walk_back_to_control_field(
    buffer: &Buffer,
    start: Option<NodeKey>,
) -> Option<NodeKey> {
    let mut cur = start;
    while let Some(k) = cur {
        match &buffer.nodes[k].kind {
            FieldNodeKind::Control(_) | FieldNodeKind::Reference(_) => {
                return Some(k);
            }
            FieldNodeKind::Text(_) => {
                cur = buffer.nodes[k].previous;
            }
        }
    }
    None
}

/// The `parent::` redirect prefix used by
/// [`Buffer::match_attributes`], as UTF-16 code units.
const PARENT_PREFIX: [u16; 8] = [
    b'p' as u16,
    b'a' as u16,
    b'r' as u16,
    b'e' as u16,
    b'n' as u16,
    b't' as u16,
    b':' as u16,
    b':' as u16,
];

/// Split a UTF-16 string on whitespace runs, dropping empty tokens.
/// Mirrors the C++ `istream_iterator<wstring>` extraction used to
/// split the attribs string in `findNodeByAttributes`.
fn split_whitespace_utf16(s: &[u16]) -> Vec<Vec<u16>> {
    let mut out: Vec<Vec<u16>> = Vec::new();
    let mut cur: Vec<u16> = Vec::new();
    for &c in s {
        if is_whitespace_w(c) {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
        } else {
            cur.push(c);
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Append `text` to `out`, backslash-escaping each `:`, `;`, and `\`.
/// When `max_len > 0`, at most `max_len` source code units are copied
/// (the escape backslashes don't count toward the limit). Mirrors
/// `outputEscapedAttribute` in `nvdaHelper/vbufBase/storage.cpp:130`.
fn push_escaped_attribute(out: &mut Vec<u16>, text: &[u16], max_len: usize) {
    const COLON: u16 = b':' as u16;
    const SEMI: u16 = b';' as u16;
    const BACKSLASH: u16 = b'\\' as u16;
    let mut count = 0usize;
    for &c in text {
        if c == COLON || c == SEMI || c == BACKSLASH {
            out.push(BACKSLASH);
        }
        out.push(c);
        count += 1;
        if max_len > 0 && count == max_len {
            break;
        }
    }
}

/// Borrow the underlying text of a text-field node, or `None` for
/// non-text variants.
fn node_text_slice(n: &Node) -> Option<&[u16]> {
    match &n.kind {
        FieldNodeKind::Text(data) => Some(&data.text),
        _ => None,
    }
}

/// `iswspace`-equivalent for the BMP characters vbuf encounters.
/// Char's `is_whitespace` covers space, tab, newline, NBSP, and the
/// rest of `iswspace`'s set.
fn is_whitespace_w(c: u16) -> bool {
    char::from_u32(c as u32)
        .map(|ch| ch.is_whitespace())
        .unwrap_or(false)
}

/// `isPrivateCharacter` from `nvdaHelper/vbufBase/utils.h`: BMP
/// private-use area `U+E000..=U+F8FF` plus the zero-width space
/// `U+200B`. The C++ predicate excludes these from "useful content".
fn is_private_character(c: u16) -> bool {
    (0xe000..=0xf8ff).contains(&c) || c == 0x200b
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

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    fn collect_text(b: &Buffer, key: NodeKey, start: i32, end: i32) -> String {
        let mut out: Vec<u16> = Vec::new();
        b.get_text_in_range(key, start, end, &mut out);
        String::from_utf16(&out).expect("valid utf16")
    }

    #[test]
    fn get_text_in_range_concatenates_children() {
        let mut b = Buffer::new();
        let root = b
            .add_control_field_node(None, None, cf(1, 1), true)
            .unwrap();
        let _t1 = b.add_text_field_node(Some(root), None, w("hello ")).unwrap();
        let _t2 = b
            .add_text_field_node(
                Some(root),
                Some(_t1),
                w("world"),
            )
            .unwrap();
        // Full range covers both children.
        assert_eq!(collect_text(&b, root, 0, 11), "hello world");
        // Partial range crosses the boundary.
        assert_eq!(collect_text(&b, root, 4, 8), "o wo");
        // Single-child slice.
        assert_eq!(collect_text(&b, root, 6, 11), "world");
    }

    #[test]
    fn calculate_offset_walks_predecessors_and_parents() {
        let mut b = Buffer::new();
        let root = b
            .add_control_field_node(None, None, cf(1, 1), true)
            .unwrap();
        let t1 = b.add_text_field_node(Some(root), None, w("abc")).unwrap();
        let t2 = b.add_text_field_node(Some(root), Some(t1), w("de")).unwrap();
        let t3 = b.add_text_field_node(Some(root), Some(t2), w("fgh")).unwrap();
        assert_eq!(b.calculate_offset_in_tree(t1), 0);
        assert_eq!(b.calculate_offset_in_tree(t2), 3);
        assert_eq!(b.calculate_offset_in_tree(t3), 5);
    }

    #[test]
    fn locate_text_field_node_at_offset_finds_correct_leaf() {
        let mut b = Buffer::new();
        let root = b
            .add_control_field_node(None, None, cf(1, 1), true)
            .unwrap();
        let t1 = b.add_text_field_node(Some(root), None, w("abc")).unwrap();
        let t2 = b.add_text_field_node(Some(root), Some(t1), w("de")).unwrap();
        // offset 0 -> t1 (rel 0)
        assert_eq!(
            b.locate_text_field_node_at_offset(root, 0),
            Some((t1, 0))
        );
        // offset 2 -> t1 (rel 2)
        assert_eq!(
            b.locate_text_field_node_at_offset(root, 2),
            Some((t1, 2))
        );
        // offset 3 -> t2 (rel 0) -- first char of t2
        assert_eq!(
            b.locate_text_field_node_at_offset(root, 3),
            Some((t2, 0))
        );
        // offset 4 -> t2 (rel 1)
        assert_eq!(
            b.locate_text_field_node_at_offset(root, 4),
            Some((t2, 1))
        );
        // offset >= total length -> None
        assert_eq!(b.locate_text_field_node_at_offset(root, 5), None);
    }

    #[test]
    fn buffer_locate_text_field_node_at_offset_returns_node_and_range() {
        let mut b = Buffer::new();
        let root = b
            .add_control_field_node(None, None, cf(1, 1), true)
            .unwrap();
        // Nest t2 under an inner control field so the text node's
        // buffer start differs from its offset within its parent.
        let t1 = b.add_text_field_node(Some(root), None, w("abc")).unwrap();
        let inner = b
            .add_control_field_node(Some(root), Some(t1), cf(1, 2), false)
            .unwrap();
        let t2 = b.add_text_field_node(Some(inner), None, w("de")).unwrap();
        // Empty buffer -> None.
        assert_eq!(Buffer::new().buffer_locate_text_field_node_at_offset(0), None);
        // offset 0 -> t1 spanning [0, 3).
        assert_eq!(
            b.buffer_locate_text_field_node_at_offset(0),
            Some(LocateTextFieldResult { node: t1, start: 0, end: 3 })
        );
        // offset 2 still inside t1 -> same [0, 3).
        assert_eq!(
            b.buffer_locate_text_field_node_at_offset(2),
            Some(LocateTextFieldResult { node: t1, start: 0, end: 3 })
        );
        // offset 3 -> t2 spanning [3, 5) (buffer-absolute, not
        // parent-relative).
        assert_eq!(
            b.buffer_locate_text_field_node_at_offset(3),
            Some(LocateTextFieldResult { node: t2, start: 3, end: 5 })
        );
        assert_eq!(
            b.buffer_locate_text_field_node_at_offset(4),
            Some(LocateTextFieldResult { node: t2, start: 3, end: 5 })
        );
        // Out-of-range offsets -> None.
        assert_eq!(b.buffer_locate_text_field_node_at_offset(5), None);
        assert_eq!(b.buffer_locate_text_field_node_at_offset(-1), None);
    }

    #[test]
    fn next_node_in_tree_forward() {
        let mut b = Buffer::new();
        let root = b
            .add_control_field_node(None, None, cf(1, 1), true)
            .unwrap();
        let mid = b
            .add_control_field_node(Some(root), None, cf(1, 2), false)
            .unwrap();
        let t1 = b.add_text_field_node(Some(mid), None, w("ab")).unwrap();
        let t2 = b.add_text_field_node(Some(root), Some(mid), w("cd")).unwrap();
        // Forward from root -> first child (mid).
        assert_eq!(
            b.next_node_in_tree(root, TreeDirection::Forward, None),
            Some((mid, 0))
        );
        // Forward from mid -> first child (t1).
        assert_eq!(
            b.next_node_in_tree(mid, TreeDirection::Forward, None),
            Some((t1, 0))
        );
        // Forward from t1 -> sibling-of-ancestor t2; relative offset
        // is t1.length = 2.
        assert_eq!(
            b.next_node_in_tree(t1, TreeDirection::Forward, None),
            Some((t2, 2))
        );
        // Forward from t2 -> nothing left.
        assert_eq!(
            b.next_node_in_tree(t2, TreeDirection::Forward, None),
            None
        );
    }

    #[test]
    fn next_node_in_tree_back() {
        let mut b = Buffer::new();
        let root = b
            .add_control_field_node(None, None, cf(1, 1), true)
            .unwrap();
        let mid = b
            .add_control_field_node(Some(root), None, cf(1, 2), false)
            .unwrap();
        let _t1 = b.add_text_field_node(Some(mid), None, w("ab")).unwrap();
        let t2 = b.add_text_field_node(Some(root), Some(mid), w("cd")).unwrap();
        // Back from t2 -> previous sibling's last descendant (t1) with
        // negative offset of -t1.length.
        let r = b.next_node_in_tree(t2, TreeDirection::Back, None);
        assert!(matches!(r, Some((_, off)) if off == -2));
        // Back from root -> nothing (no previous, no parent).
        assert_eq!(
            b.next_node_in_tree(root, TreeDirection::Back, None),
            None
        );
    }

    #[test]
    fn get_attributes_string_concatenates_pairs() {
        let mut b = Buffer::new();
        let key = b.add_control_field_node(None, None, cf(1, 1), true).unwrap();
        let n = b.get_mut(key).unwrap();
        n.add_attribute(&w("name"), &w("value"));
        n.add_attribute(&w("display"), &w("block"));
        let s = b.get(key).unwrap().get_attributes_string();
        // BTreeMap iterates by sorted key: "display" < "name".
        let actual = String::from_utf16(&s).unwrap();
        assert_eq!(actual, "display:block;name:value;");
    }

    #[test]
    fn node_has_useful_content_short_text() {
        let mut b = Buffer::new();
        let root = b.add_control_field_node(None, None, cf(1, 1), true).unwrap();
        // length-0 root -> false
        assert!(!b.node_has_useful_content(root));
        // pure-whitespace short text -> false
        let t1 = b.add_text_field_node(Some(root), None, w("   ")).unwrap();
        assert!(!b.node_has_useful_content(t1));
        // private-use character -> false
        let t2 = b
            .add_text_field_node(
                Some(root),
                Some(t1),
                vec![0xe000_u16, 0x200b_u16],
            )
            .unwrap();
        assert!(!b.node_has_useful_content(t2));
        // a real letter mixed with whitespace -> true
        let t3 = b.add_text_field_node(Some(root), Some(t2), w(" a ")).unwrap();
        assert!(b.node_has_useful_content(t3));
    }

    #[test]
    fn node_has_useful_content_long_skips_scan() {
        let mut b = Buffer::new();
        let root = b.add_control_field_node(None, None, cf(1, 1), true).unwrap();
        // length > 3 returns true even when the text is purely
        // whitespace -- mirrors the C++ short-circuit.
        let _t = b.add_text_field_node(Some(root), None, w("    ")).unwrap();
        assert!(b.node_has_useful_content(root));
    }

    #[test]
    fn node_content_matches_string_compares_text() {
        let mut b = Buffer::new();
        let root = b.add_control_field_node(None, None, cf(1, 1), true).unwrap();
        let _t = b.add_text_field_node(Some(root), None, w("hello world")).unwrap();
        assert!(b.node_content_matches_string(root, &w("hello world")));
        assert!(!b.node_content_matches_string(root, &w("HELLO WORLD")));
        assert!(!b.node_content_matches_string(root, &w("hello")));
        // length mismatch short-circuits.
        assert!(!b.node_content_matches_string(root, &w("hello world!")));
    }

    #[test]
    fn selection_defaults_to_empty() {
        let b = Buffer::new();
        assert_eq!(b.selection_offsets(), (0, 0));
        assert_eq!(b.text_length(), 0);
    }

    #[test]
    fn selection_set_then_get() {
        let mut b = Buffer::new();
        let root = b.add_control_field_node(None, None, cf(1, 1), true).unwrap();
        let _t = b.add_text_field_node(Some(root), None, w("hello world")).unwrap();
        assert_eq!(b.text_length(), 11);
        assert!(b.set_selection_offsets(2, 7));
        assert_eq!(b.selection_offsets(), (2, 7));
    }

    #[test]
    fn selection_clamped_against_text_length() {
        let mut b = Buffer::new();
        let root = b.add_control_field_node(None, None, cf(1, 1), true).unwrap();
        let _t = b.add_text_field_node(Some(root), None, w("hello")).unwrap();
        // Set selection past the end of text.
        assert!(b.set_selection_offsets(3, 100));
        // get() clamps to current text length (5).
        assert_eq!(b.selection_offsets(), (3, 5));
    }

    #[test]
    fn selection_rejects_invalid_offsets() {
        let mut b = Buffer::new();
        let root = b.add_control_field_node(None, None, cf(1, 1), true).unwrap();
        let _t = b.add_text_field_node(Some(root), None, w("hello")).unwrap();
        // Invalid: negative start.
        assert!(!b.set_selection_offsets(-1, 3));
        // Invalid: end < start.
        assert!(!b.set_selection_offsets(5, 3));
        // Selection unchanged from default.
        assert_eq!(b.selection_offsets(), (0, 0));
    }

    #[test]
    fn clear_resets_selection() {
        let mut b = Buffer::new();
        let root = b.add_control_field_node(None, None, cf(1, 1), true).unwrap();
        let _t = b.add_text_field_node(Some(root), None, w("hello")).unwrap();
        b.set_selection_offsets(1, 4);
        b.clear();
        assert_eq!(b.selection_offsets(), (0, 0));
    }

    #[test]
    fn buffer_get_text_in_range_validates_and_walks_root() {
        let mut b = Buffer::new();
        // Empty buffer -> false.
        let mut out: Vec<u16> = Vec::new();
        assert!(!b.buffer_get_text_in_range(0, 5, &mut out, false));

        let root = b.add_control_field_node(None, None, cf(1, 1), true).unwrap();
        let _t = b.add_text_field_node(Some(root), None, w("hello world")).unwrap();
        // In-range walk.
        assert!(b.buffer_get_text_in_range(0, 11, &mut out, false));
        assert_eq!(String::from_utf16(&out).unwrap(), "hello world");
        // Inverted range -> false (no mutation of out beyond what we
        // already wrote).
        let mut out2: Vec<u16> = Vec::new();
        assert!(!b.buffer_get_text_in_range(5, 5, &mut out2, false));
        assert!(out2.is_empty());
        // Out-of-range -> false.
        let mut out3: Vec<u16> = Vec::new();
        assert!(!b.buffer_get_text_in_range(0, 100, &mut out3, false));
        assert!(out3.is_empty());
    }

    #[test]
    fn line_offsets_single_text_node_no_breaks() {
        let mut b = Buffer::new();
        let root = b.add_control_field_node(None, None, cf(1, 1), true).unwrap();
        let _t = b.add_text_field_node(Some(root), None, w("hello world")).unwrap();
        // No CR/LF, no maxLineLength -> the whole text is one line.
        assert_eq!(b.line_offsets(0, 0, true), Some((0, 11)));
        assert_eq!(b.line_offsets(5, 0, true), Some((0, 11)));
    }

    #[test]
    fn line_offsets_hard_break_lf() {
        let mut b = Buffer::new();
        let root = b.add_control_field_node(None, None, cf(1, 1), true).unwrap();
        let _t = b
            .add_text_field_node(Some(root), None, w("first\nsecond"))
            .unwrap();
        // Position 0 is in the first line; line ends just past '\n'.
        assert_eq!(b.line_offsets(0, 0, true), Some((0, 6)));
        // Position 6 is in the second line.
        assert_eq!(b.line_offsets(6, 0, true), Some((6, 12)));
    }

    #[test]
    fn line_offsets_max_line_length_wraps_on_whitespace() {
        let mut b = Buffer::new();
        let root = b.add_control_field_node(None, None, cf(1, 1), true).unwrap();
        // 21-character line; max length 10 should wrap on whitespace.
        let _t = b
            .add_text_field_node(
                Some(root),
                None,
                w("aaaa bbbb cccc dddd ee"),
            )
            .unwrap();
        // Without wrapping the line is 0..22 (includes the full text).
        // With max 10 the first wrap candidate <= 10 is the boundary
        // after "aaaa " at position 5.
        let (start, end) = b.line_offsets(0, 10, true).unwrap();
        assert_eq!(start, 0);
        assert!(end <= 10);
        assert!(end >= 5);
    }

    #[test]
    fn line_offsets_offset_at_block_boundary_is_in_range() {
        let mut b = Buffer::new();
        let root = b.add_control_field_node(None, None, cf(1, 1), true).unwrap();
        let _t = b.add_text_field_node(Some(root), None, w("hello")).unwrap();
        // offset == length is out of range.
        assert_eq!(b.line_offsets(5, 0, true), None);
    }

    #[test]
    fn line_offsets_empty_buffer_returns_none() {
        let b = Buffer::new();
        assert_eq!(b.line_offsets(0, 0, true), None);
    }

    #[test]
    fn field_node_offsets_returns_start_end() {
        let mut b = Buffer::new();
        let root = b.add_control_field_node(None, None, cf(1, 1), true).unwrap();
        let t1 = b.add_text_field_node(Some(root), None, w("abc")).unwrap();
        let t2 = b.add_text_field_node(Some(root), Some(t1), w("de")).unwrap();
        assert_eq!(b.field_node_offsets(root), Some((0, 5)));
        assert_eq!(b.field_node_offsets(t1), Some((0, 3)));
        assert_eq!(b.field_node_offsets(t2), Some((3, 5)));
    }

    #[test]
    fn is_field_node_at_offset_checks_range() {
        let mut b = Buffer::new();
        let root = b.add_control_field_node(None, None, cf(1, 1), true).unwrap();
        let t1 = b.add_text_field_node(Some(root), None, w("abc")).unwrap();
        let t2 = b.add_text_field_node(Some(root), Some(t1), w("de")).unwrap();
        assert!(b.is_field_node_at_offset(t1, 0));
        assert!(b.is_field_node_at_offset(t1, 2));
        assert!(!b.is_field_node_at_offset(t1, 3)); // end-exclusive
        assert!(b.is_field_node_at_offset(t2, 3));
        assert!(b.is_field_node_at_offset(t2, 4));
        assert!(!b.is_field_node_at_offset(t2, 5)); // past end
        // Whole-buffer offset out of range -> false.
        assert!(!b.is_field_node_at_offset(root, 100));
    }

    fn allow_reuse(b: &Buffer, key: NodeKey) -> bool {
        match &b.get(key).unwrap().kind {
            FieldNodeKind::Control(d) => d.allow_reuse_in_ancestor_update,
            _ => true,
        }
    }

    #[test]
    fn invalidate_subtree_basic_enqueue() {
        let mut b = Buffer::new();
        let root = b
            .add_control_field_node(None, None, cf(1, 1), true)
            .unwrap();
        assert!(b.invalidate_subtree(root));
        assert!(!b.pending_invalid_subtrees_empty());
    }

    #[test]
    fn invalidate_subtree_no_op_for_already_invalid() {
        let mut b = Buffer::new();
        let root = b
            .add_control_field_node(None, None, cf(1, 1), true)
            .unwrap();
        assert!(b.invalidate_subtree(root));
        assert!(b.invalidate_subtree(root));
        // Still just one entry.
        assert_eq!(b.pending_invalid.len(), 1);
    }

    #[test]
    fn invalidate_subtree_descendant_of_invalid_marks_nonreusable() {
        let mut b = Buffer::new();
        let root = b
            .add_control_field_node(None, None, cf(1, 1), true)
            .unwrap();
        let child = b
            .add_control_field_node(Some(root), None, cf(1, 2), false)
            .unwrap();
        let grandchild = b
            .add_control_field_node(Some(child), None, cf(1, 3), false)
            .unwrap();
        b.invalidate_subtree(root);
        // Now invalidate grandchild -- it's already covered by root,
        // but child + grandchild should be marked non-reusable.
        b.invalidate_subtree(grandchild);
        assert!(!allow_reuse(&b, child));
        assert!(!allow_reuse(&b, grandchild));
        // Pending list still has just the root.
        assert_eq!(b.pending_invalid.len(), 1);
    }

    #[test]
    fn invalidate_subtree_ancestor_subsumes_descendants() {
        let mut b = Buffer::new();
        let root = b
            .add_control_field_node(None, None, cf(1, 1), true)
            .unwrap();
        let child = b
            .add_control_field_node(Some(root), None, cf(1, 2), false)
            .unwrap();
        b.invalidate_subtree(child);
        b.invalidate_subtree(root);
        // The earlier child invalidation is subsumed; only root
        // remains in pending. child should be marked non-reusable.
        assert_eq!(b.pending_invalid, vec![root]);
        assert!(!allow_reuse(&b, child));
    }

    #[test]
    fn take_pending_into_working_swaps_lists() {
        let mut b = Buffer::new();
        let root = b
            .add_control_field_node(None, None, cf(1, 1), true)
            .unwrap();
        let child = b
            .add_control_field_node(Some(root), None, cf(1, 2), false)
            .unwrap();
        b.invalidate_subtree(child);
        b.invalidate_subtree(root);
        // pending now has just `root` (child was subsumed).
        let snapshot = b.take_pending_into_working();
        assert_eq!(snapshot, vec![root]);
        // pending is empty, working has the snapshot.
        assert!(b.pending_invalid_subtrees_empty());
        assert!(!b.working_invalid_empty());
    }

    #[test]
    fn remove_from_working_takes_responsibility() {
        let mut b = Buffer::new();
        let root = b
            .add_control_field_node(None, None, cf(1, 1), true)
            .unwrap();
        b.invalidate_subtree(root);
        b.take_pending_into_working();
        assert!(b.remove_from_working(root));
        assert!(b.working_invalid_empty());
        assert!(!b.remove_from_working(root)); // already removed
    }

    #[test]
    fn move_subtree_from_simple_root() {
        let mut dest = Buffer::new();
        let dest_root = dest
            .add_control_field_node(None, None, cf(1, 1), true)
            .unwrap();

        // Build a small tree in source.
        let mut source = Buffer::new();
        let s_root = source
            .add_control_field_node(None, None, cf(2, 1), true)
            .unwrap();
        let _s_t1 = source
            .add_text_field_node(Some(s_root), None, w("hello"))
            .unwrap();
        // Move source's root under dest_root.
        let new_key = dest
            .move_subtree_from(&mut source, s_root, Some(dest_root), None)
            .expect("move");
        // Source is now empty.
        assert_eq!(source.root(), None);
        assert!(!source.contains(s_root));
        // Dest has the moved subtree as a child of dest_root.
        assert_eq!(dest.get(dest_root).unwrap().first_child, Some(new_key));
        assert_eq!(dest.get(new_key).unwrap().parent, Some(dest_root));
        // Identifier (2,1) is now in dest, gone from source.
        assert_eq!(
            dest.get_control_field_node_with_identifier(2, 1),
            Some(new_key)
        );
        assert_eq!(
            source.get_control_field_node_with_identifier(2, 1),
            None
        );
        // Length propagated -- "hello" is 5 chars.
        assert_eq!(dest.get(new_key).unwrap().length, 5);
        // dest_root's length is also 5 now.
        assert_eq!(dest.get(dest_root).unwrap().length, 5);
    }

    #[test]
    fn move_subtree_from_preserves_descendant_links() {
        let mut dest = Buffer::new();
        let dest_root = dest
            .add_control_field_node(None, None, cf(1, 1), true)
            .unwrap();
        let mut source = Buffer::new();
        let s_root = source
            .add_control_field_node(None, None, cf(2, 1), true)
            .unwrap();
        let s_mid = source
            .add_control_field_node(Some(s_root), None, cf(2, 2), false)
            .unwrap();
        let s_leaf = source
            .add_text_field_node(Some(s_mid), None, w("ab"))
            .unwrap();

        let new_root = dest
            .move_subtree_from(&mut source, s_root, Some(dest_root), None)
            .unwrap();

        // Walk from the new root and verify the moved subtree.
        let new_mid = dest.get(new_root).unwrap().first_child.unwrap();
        let new_leaf = dest.get(new_mid).unwrap().first_child.unwrap();
        // Old keys are gone from source (they belonged to source's
        // arena originally; slotmap keys are arena-scoped).
        assert!(!source.contains(s_root));
        assert!(!source.contains(s_mid));
        assert!(!source.contains(s_leaf));
        // New keys are connected correctly.
        assert_eq!(dest.get(new_mid).unwrap().parent, Some(new_root));
        assert_eq!(dest.get(new_leaf).unwrap().parent, Some(new_mid));
        // Length: leaf=2, mid=2 (carries leaf's length), new_root=2,
        // dest_root=2 (after bump_ancestor_lengths).
        assert_eq!(dest.get(new_leaf).unwrap().length, 2);
        assert_eq!(dest.get(new_mid).unwrap().length, 2);
        assert_eq!(dest.get(new_root).unwrap().length, 2);
        assert_eq!(dest.get(dest_root).unwrap().length, 2);
        // Text is reachable via dest's tree.
        let mut buf: Vec<u16> = Vec::new();
        dest.get_text_in_range(dest_root, 0, 2, &mut buf);
        assert_eq!(String::from_utf16(&buf).unwrap(), "ab");
    }

    #[test]
    fn move_subtree_from_mid_tree_unlinks_neighbours() {
        let mut dest = Buffer::new();
        let dest_root = dest
            .add_control_field_node(None, None, cf(1, 1), true)
            .unwrap();
        // Source has: S_root -> [S_a, S_b, S_c] where S_b has its
        // own subtree.
        let mut source = Buffer::new();
        let s_root = source
            .add_control_field_node(None, None, cf(2, 1), true)
            .unwrap();
        let s_a = source.add_text_field_node(Some(s_root), None, w("AAA")).unwrap();
        let s_b = source
            .add_control_field_node(Some(s_root), Some(s_a), cf(2, 2), false)
            .unwrap();
        let _s_b_leaf = source
            .add_text_field_node(Some(s_b), None, w("BB"))
            .unwrap();
        let s_c = source.add_text_field_node(Some(s_root), Some(s_b), w("C")).unwrap();
        // s_root.length should be 3+2+1 = 6.
        assert_eq!(source.get(s_root).unwrap().length, 6);

        // Move s_b's subtree from source to dest.
        let new_b = dest
            .move_subtree_from(&mut source, s_b, Some(dest_root), None)
            .unwrap();

        // Source's s_root should now have just s_a and s_c, length
        // = 3+1 = 4.
        assert_eq!(source.get(s_root).unwrap().length, 4);
        assert_eq!(source.get(s_a).unwrap().next, Some(s_c));
        assert_eq!(source.get(s_c).unwrap().previous, Some(s_a));
        assert_eq!(source.get(s_root).unwrap().first_child, Some(s_a));
        assert_eq!(source.get(s_root).unwrap().last_child, Some(s_c));

        // Dest has the moved subtree; its leaf is "BB".
        let mut buf: Vec<u16> = Vec::new();
        dest.get_text_in_range(new_b, 0, 2, &mut buf);
        assert_eq!(String::from_utf16(&buf).unwrap(), "BB");
    }

    #[test]
    fn replace_subtrees_simple_replacement() {
        let mut main = Buffer::new();
        let main_root = main
            .add_control_field_node(None, None, cf(1, 1), true)
            .unwrap();
        let main_target = main
            .add_control_field_node(Some(main_root), None, cf(1, 2), false)
            .unwrap();
        let _main_old_text = main
            .add_text_field_node(Some(main_target), None, w("OLD"))
            .unwrap();
        let _main_sibling = main
            .add_text_field_node(Some(main_root), Some(main_target), w("Z"))
            .unwrap();
        // Build a replacement temp buffer.
        let mut temp = Buffer::new();
        let temp_root = temp
            .add_control_field_node(None, None, cf(1, 2), false)
            .unwrap();
        let _temp_text = temp
            .add_text_field_node(Some(temp_root), None, w("NEW!"))
            .unwrap();

        let map = vec![(main_target, temp)];
        assert!(main.replace_subtrees(map));

        // main_target's identifier (1,2) is still present; the
        // returned key is the new control field replacing it.
        let new_target = main
            .get_control_field_node_with_identifier(1, 2)
            .expect("identifier preserved");
        let mut buf: Vec<u16> = Vec::new();
        main.get_text_in_range(
            new_target,
            0,
            main.get(new_target).unwrap().length,
            &mut buf,
        );
        assert_eq!(String::from_utf16(&buf).unwrap(), "NEW!");
        // The sibling text "Z" is still there.
        let total_len = main.text_length();
        let mut whole: Vec<u16> = Vec::new();
        main.get_text_in_range(main_root, 0, total_len, &mut whole);
        assert_eq!(String::from_utf16(&whole).unwrap(), "NEW!Z");
    }

    #[test]
    fn replace_subtrees_empty_temp_just_removes_target() {
        let mut main = Buffer::new();
        let main_root = main
            .add_control_field_node(None, None, cf(1, 1), true)
            .unwrap();
        let main_target = main
            .add_control_field_node(Some(main_root), None, cf(1, 2), false)
            .unwrap();
        let _t = main
            .add_text_field_node(Some(main_target), None, w("OLD"))
            .unwrap();
        let temp = Buffer::new(); // empty
        let map = vec![(main_target, temp)];
        assert!(main.replace_subtrees(map));
        assert_eq!(main.get_control_field_node_with_identifier(1, 2), None);
        assert_eq!(main.text_length(), 0);
    }

    #[test]
    fn replace_subtrees_resolves_references() {
        // Set up main with a "reusable" subtree that the temp will
        // reference rather than re-rendering. The temp buffer
        // starts as {root, Reference(reusable)} and after
        // resolution should contain {root, reusable_subtree_moved}.
        let mut main = Buffer::new();
        let main_root = main
            .add_control_field_node(None, None, cf(1, 1), true)
            .unwrap();
        // Target subtree we're going to replace.
        let main_target = main
            .add_control_field_node(Some(main_root), None, cf(1, 2), false)
            .unwrap();
        // Reusable subtree that the temp will reference. Lives
        // elsewhere in main.
        let reusable = main
            .add_control_field_node(Some(main_root), Some(main_target), cf(1, 3), false)
            .unwrap();
        let _reusable_text = main
            .add_text_field_node(Some(reusable), None, w("REUSED"))
            .unwrap();

        // Build the temp buffer with a reference to (1, 3).
        let mut temp = Buffer::new();
        let temp_root = temp
            .add_control_field_node(None, None, cf(1, 2), false)
            .unwrap();
        let _temp_pre = temp
            .add_text_field_node(Some(temp_root), None, w("pre"))
            .unwrap();
        // Reference the reusable node. The reference node carries
        // the same identifier (1, 3) as the actual node.
        let _ref = temp
            .add_reference_node(
                Some(temp_root),
                Some(_temp_pre),
                ControlFieldIdentifier {
                    doc_handle: 1,
                    id: 3,
                },
                reusable,
            )
            .unwrap();
        let _temp_post = temp
            .add_text_field_node(Some(temp_root), Some(_ref), w("post"))
            .unwrap();

        let map = vec![(main_target, temp)];
        assert!(main.replace_subtrees(map));

        // After replacement: main_root should have one child (the
        // new (1,2) tree). The (1,3) identifier resolves to the
        // moved reusable subtree, now living under (1,2).
        let new_target = main
            .get_control_field_node_with_identifier(1, 2)
            .expect("identifier preserved");
        assert_eq!(main.get(main_root).unwrap().first_child, Some(new_target));
        // The reusable subtree's text "REUSED" is reachable.
        let mut buf: Vec<u16> = Vec::new();
        let total = main.get(new_target).unwrap().length;
        main.get_text_in_range(new_target, 0, total, &mut buf);
        let text = String::from_utf16(&buf).unwrap();
        assert_eq!(text, "preREUSEDpost");
        // (1,3) identifier still resolves -- it's now under (1,2).
        let resolved_reusable = main
            .get_control_field_node_with_identifier(1, 3)
            .expect("(1,3) preserved");
        assert_eq!(
            main.get(resolved_reusable).unwrap().parent,
            Some(new_target)
        );
    }

    #[test]
    fn replace_subtrees_handles_identifier_collision() {
        // If the temp buffer carries an identifier that already
        // exists elsewhere in main (and not under the target being
        // replaced), the existing entry must be removed before the
        // move so by_identifier ends up consistent.
        let mut main = Buffer::new();
        let main_root = main
            .add_control_field_node(None, None, cf(1, 1), true)
            .unwrap();
        let main_target = main
            .add_control_field_node(Some(main_root), None, cf(1, 2), false)
            .unwrap();
        // Identifier (1,3) lives elsewhere in main, not under
        // main_target.
        let _other = main
            .add_control_field_node(Some(main_root), Some(main_target), cf(1, 3), false)
            .unwrap();

        // Temp brings a fresh (1,2) and reuses identifier (1,3).
        let mut temp = Buffer::new();
        let temp_root = temp
            .add_control_field_node(None, None, cf(1, 2), false)
            .unwrap();
        let _temp_inner = temp
            .add_control_field_node(Some(temp_root), None, cf(1, 3), false)
            .unwrap();

        let map = vec![(main_target, temp)];
        assert!(main.replace_subtrees(map));
        // (1,3) lookup now resolves to the temp's inner node, not
        // main's pre-existing one (which got removed pre-move).
        let new_inner = main
            .get_control_field_node_with_identifier(1, 3)
            .expect("identifier preserved");
        assert_eq!(
            main.get(new_inner).unwrap().parent,
            main.get_control_field_node_with_identifier(1, 2),
        );
    }

    #[test]
    fn move_subtree_from_invalid_anchor_returns_none() {
        let mut dest = Buffer::new();
        let dest_root = dest
            .add_control_field_node(None, None, cf(1, 1), true)
            .unwrap();
        let mut source = Buffer::new();
        let s_root = source
            .add_control_field_node(None, None, cf(2, 1), true)
            .unwrap();
        // Try moving as a *new* root when one already exists.
        assert_eq!(
            dest.move_subtree_from(&mut source, s_root, None, None),
            None
        );
        // Source should still have its content; dest unchanged.
        assert_eq!(source.root(), Some(s_root));
        assert_eq!(dest.get(dest_root).unwrap().first_child, None);
    }

    #[test]
    fn clear_working_drains_list() {
        let mut b = Buffer::new();
        let root = b
            .add_control_field_node(None, None, cf(1, 1), true)
            .unwrap();
        b.invalidate_subtree(root);
        b.take_pending_into_working();
        b.clear_working();
        assert!(b.working_invalid_empty());
    }

    #[test]
    fn invalidate_subtree_walks_up_requires_parent_update() {
        let mut b = Buffer::new();
        let root = b
            .add_control_field_node(None, None, cf(1, 1), true)
            .unwrap();
        let child = b
            .add_control_field_node(Some(root), None, cf(1, 2), false)
            .unwrap();
        // Set requires_parent_update on child.
        if let FieldNodeKind::Control(d) = &mut b.get_mut(child).unwrap().kind
        {
            d.requires_parent_update = true;
        }
        b.invalidate_subtree(child);
        // child should be marked non-reusable; root should be the
        // pending entry.
        assert!(!allow_reuse(&b, child));
        assert_eq!(b.pending_invalid, vec![root]);
    }

    #[test]
    fn has_content_reflects_root_presence() {
        let mut b = Buffer::new();
        assert!(!b.has_content());
        let _root = b.add_control_field_node(None, None, cf(1, 1), true).unwrap();
        assert!(b.has_content());
        b.clear();
        assert!(!b.has_content());
    }

    #[test]
    fn next_node_in_tree_symmetrical_back() {
        let mut b = Buffer::new();
        let root = b
            .add_control_field_node(None, None, cf(1, 1), true)
            .unwrap();
        let _t1 = b.add_text_field_node(Some(root), None, w("ab")).unwrap();
        let t2 = b.add_text_field_node(Some(root), Some(_t1), w("cd")).unwrap();
        // SymmetricalBack from root -> last child (t2). Relative
        // offset = root.length - t2.length = 4 - 2 = 2.
        assert_eq!(
            b.next_node_in_tree(root, TreeDirection::SymmetricalBack, None),
            Some((t2, 2))
        );
    }

    #[test]
    fn locate_control_field_at_offset_returns_text_parent() {
        // root (1,1)
        //   inner (1,2)
        //     "abc"
        //     "de"
        //   "fg"
        let mut b = Buffer::new();
        let root = b
            .add_control_field_node(None, None, cf(1, 1), true)
            .unwrap();
        let inner = b
            .add_control_field_node(Some(root), None, cf(1, 2), false)
            .unwrap();
        let _t_abc = b
            .add_text_field_node(Some(inner), None, w("abc"))
            .unwrap();
        let _t_de = b
            .add_text_field_node(Some(inner), Some(_t_abc), w("de"))
            .unwrap();
        let _t_fg = b
            .add_text_field_node(Some(root), Some(inner), w("fg"))
            .unwrap();

        // offset 0 -> first char of "abc", parent is inner (start 0,
        // end 5 = inner.length).
        let r = b.locate_control_field_node_at_offset(0).unwrap();
        assert_eq!(r.node, inner);
        assert_eq!((r.start, r.end), (0, 5));
        assert_eq!((r.doc_handle, r.id), (1, 2));

        // offset 4 -> last char of "de", still inner.
        let r = b.locate_control_field_node_at_offset(4).unwrap();
        assert_eq!(r.node, inner);
        assert_eq!((r.start, r.end), (0, 5));

        // offset 5 -> first char of "fg" (sibling of inner),
        // parent is root, start 0, end 7 (root.length).
        let r = b.locate_control_field_node_at_offset(5).unwrap();
        assert_eq!(r.node, root);
        assert_eq!((r.start, r.end), (0, 7));
        assert_eq!((r.doc_handle, r.id), (1, 1));

        // Out of range.
        assert_eq!(b.locate_control_field_node_at_offset(7), None);
        assert_eq!(b.locate_control_field_node_at_offset(-1), None);
    }

    #[test]
    fn locate_control_field_at_offset_with_deeper_inner_offset() {
        // root (1,1)
        //   "ab"  // 2 chars
        //   inner (1,2)
        //     "cdef"  // 4 chars
        // root.length = 6.  inner.start = 2, inner.end = 6.
        let mut b = Buffer::new();
        let root = b
            .add_control_field_node(None, None, cf(1, 1), true)
            .unwrap();
        let _t_ab = b.add_text_field_node(Some(root), None, w("ab")).unwrap();
        let inner = b
            .add_control_field_node(Some(root), Some(_t_ab), cf(1, 2), false)
            .unwrap();
        let _t_cdef = b
            .add_text_field_node(Some(inner), None, w("cdef"))
            .unwrap();
        let r = b.locate_control_field_node_at_offset(3).unwrap();
        assert_eq!(r.node, inner);
        assert_eq!((r.start, r.end), (2, 6));
    }

    #[test]
    fn locate_control_field_at_offset_empty_buffer_returns_none() {
        let b = Buffer::new();
        assert_eq!(b.locate_control_field_node_at_offset(0), None);
    }

    #[test]
    fn identifier_of_control_field_node_returns_pair() {
        let mut b = Buffer::new();
        let root = b
            .add_control_field_node(None, None, cf(7, 42), true)
            .unwrap();
        let t = b.add_text_field_node(Some(root), None, w("x")).unwrap();
        assert_eq!(
            b.identifier_of_control_field_node(root),
            Some(ControlFieldIdentifier {
                doc_handle: 7,
                id: 42,
            })
        );
        // Text field has no identifier.
        assert_eq!(b.identifier_of_control_field_node(t), None);
    }

    #[test]
    fn identifier_of_control_field_node_after_clear_returns_none() {
        // Once the arena is cleared, the old key's generation no
        // longer matches, so slotmap returns `None` on lookup.
        let mut b = Buffer::new();
        let root = b
            .add_control_field_node(None, None, cf(1, 1), true)
            .unwrap();
        b.clear();
        assert_eq!(b.identifier_of_control_field_node(root), None);
    }

    // ---- findNodeByAttributes ----------------------------------------

    fn u(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    fn set_attr(b: &mut Buffer, key: NodeKey, name: &str, value: &str) {
        b.get_mut(key).unwrap().add_attribute(&u(name), &u(value));
    }

    /// Three siblings under a block root:
    ///   c1 role=heading level=1 > "Title"   [0,5)
    ///   c2 role=paragraph        > "Body"    [5,9)
    ///   c3 role=heading level=2  > "Sub"     [9,12)
    /// Returns the buffer and `[c1, c2, c3]`.
    fn build_headings() -> (Buffer, [NodeKey; 3]) {
        let mut b = Buffer::new();
        let root = b
            .add_control_field_node(None, None, cf(1, 1), true)
            .unwrap();
        let c1 = b
            .add_control_field_node(Some(root), None, cf(1, 2), false)
            .unwrap();
        set_attr(&mut b, c1, "role", "heading");
        set_attr(&mut b, c1, "level", "1");
        b.add_text_field_node(Some(c1), None, u("Title")).unwrap();
        let c2 = b
            .add_control_field_node(Some(root), Some(c1), cf(1, 3), false)
            .unwrap();
        set_attr(&mut b, c2, "role", "paragraph");
        b.add_text_field_node(Some(c2), None, u("Body")).unwrap();
        let c3 = b
            .add_control_field_node(Some(root), Some(c2), cf(1, 4), false)
            .unwrap();
        set_attr(&mut b, c3, "role", "heading");
        set_attr(&mut b, c3, "level", "2");
        b.add_text_field_node(Some(c3), None, u("Sub")).unwrap();
        (b, [c1, c2, c3])
    }

    // The exact `(attribs, regexp)` a heading quick-nav search produces
    // via `_prepareForFindByAttributes({"role": ["heading"]})`.
    const HEADING_ATTRIBS: &str = "role";
    const HEADING_REGEXP: &str = "role:(?:heading;)";

    #[test]
    fn find_forward_skips_start_node_and_finds_next_heading() {
        let (b, [_c1, _c2, c3]) = build_headings();
        // Start inside c1's text (offset 0). Forward begins at the
        // node *after* the start node, so c1 (the heading we're in) is
        // skipped and the next heading (c3) is returned.
        let r = b
            .find_node_by_attributes(
                0,
                FindDirection::Forward,
                &u(HEADING_ATTRIBS),
                &u(HEADING_REGEXP),
            )
            .expect("forward heading");
        assert_eq!(r.node, c3);
        assert_eq!((r.start, r.end), (9, 12));
    }

    #[test]
    fn find_forward_from_minus_one_includes_first_heading() {
        let (b, [c1, ..]) = build_headings();
        // offset -1 starts at the root, so the very first heading (c1)
        // is a valid result -- unlike starting at offset 0.
        let r = b
            .find_node_by_attributes(
                -1,
                FindDirection::Forward,
                &u(HEADING_ATTRIBS),
                &u(HEADING_REGEXP),
            )
            .expect("forward-from-root heading");
        assert_eq!(r.node, c1);
        assert_eq!((r.start, r.end), (0, 5));
    }

    #[test]
    fn find_back_skips_containing_node_and_finds_previous_heading() {
        let (b, [c1, _c2, _c3]) = build_headings();
        // Start inside c3's text (offset 10, c3 spans [9,12)). Back
        // skips c3 (the heading strictly containing the offset) and
        // returns the previous heading c1.
        let r = b
            .find_node_by_attributes(
                10,
                FindDirection::Back,
                &u(HEADING_ATTRIBS),
                &u(HEADING_REGEXP),
            )
            .expect("back heading");
        assert_eq!(r.node, c1);
        assert_eq!((r.start, r.end), (0, 5));
    }

    #[test]
    fn find_no_match_returns_none() {
        let (b, _) = build_headings();
        assert!(b
            .find_node_by_attributes(
                -1,
                FindDirection::Forward,
                &u("role"),
                &u("role:(?:banner;)"),
            )
            .is_none());
    }

    #[test]
    fn find_empty_buffer_returns_none() {
        let b = Buffer::new();
        assert!(b
            .find_node_by_attributes(
                -1,
                FindDirection::Forward,
                &u(HEADING_ATTRIBS),
                &u(HEADING_REGEXP),
            )
            .is_none());
    }

    #[test]
    fn find_offset_past_end_returns_none() {
        let (b, _) = build_headings();
        // root length is 12; offset == length and beyond both fail.
        for offset in [12, 13, 100] {
            assert!(b
                .find_node_by_attributes(
                    offset,
                    FindDirection::Forward,
                    &u(HEADING_ATTRIBS),
                    &u(HEADING_REGEXP),
                )
                .is_none());
        }
    }

    #[test]
    fn find_invalid_offset_returns_none() {
        let (b, _) = build_headings();
        // offset < -1 is invalid.
        assert!(b
            .find_node_by_attributes(
                -2,
                FindDirection::Forward,
                &u(HEADING_ATTRIBS),
                &u(HEADING_REGEXP),
            )
            .is_none());
    }

    #[test]
    fn find_bad_regexp_returns_none() {
        let (b, _) = build_headings();
        // An unbalanced group fails to compile -> None (mirrors the
        // C++ catch that returns NULL).
        assert!(b
            .find_node_by_attributes(
                -1,
                FindDirection::Forward,
                &u("role"),
                &u("role:(?:heading;"),
            )
            .is_none());
    }

    #[test]
    fn find_skips_hidden_nodes() {
        let (mut b, [_c1, _c2, c3]) = build_headings();
        // Hide the second heading; a forward search from the root now
        // must not return it.
        b.get_mut(c3).unwrap().is_hidden = true;
        // From offset 0 (inside c1) there is no other visible heading.
        assert!(b
            .find_node_by_attributes(
                0,
                FindDirection::Forward,
                &u(HEADING_ATTRIBS),
                &u(HEADING_REGEXP),
            )
            .is_none());
    }

    /// Nested layout to exercise the "up" direction:
    ///   c_region role=main
    ///     "aa"                       [0,2)
    ///     c_h role=heading > "bb"    [2,4)
    /// Root carries role=document.
    fn build_nested() -> (Buffer, NodeKey, NodeKey) {
        let mut b = Buffer::new();
        let root = b
            .add_control_field_node(None, None, cf(1, 1), true)
            .unwrap();
        set_attr(&mut b, root, "role", "document");
        let c_region = b
            .add_control_field_node(Some(root), None, cf(1, 2), false)
            .unwrap();
        set_attr(&mut b, c_region, "role", "main");
        // "aa" is the region's first child; c_h follows it.
        let t_aa =
            b.add_text_field_node(Some(c_region), None, u("aa")).unwrap();
        let c_h = b
            .add_control_field_node(
                Some(c_region),
                Some(t_aa),
                cf(1, 3),
                false,
            )
            .unwrap();
        set_attr(&mut b, c_h, "role", "heading");
        b.add_text_field_node(Some(c_h), None, u("bb")).unwrap();
        (b, c_region, c_h)
    }

    #[test]
    fn find_up_returns_nearest_matching_ancestor() {
        let (b, _c_region, c_h) = build_nested();
        // From offset 2 (start of c_h's "bb"), searching up for a
        // heading returns the immediate parent c_h.
        let r = b
            .find_node_by_attributes(
                2,
                FindDirection::Up,
                &u("role"),
                &u("role:(?:heading;)"),
            )
            .expect("up heading");
        assert_eq!(r.node, c_h);
        assert_eq!((r.start, r.end), (2, 4));
    }

    #[test]
    fn find_up_walks_past_non_matching_ancestor_and_adjusts_start() {
        let (b, c_region, _c_h) = build_nested();
        // From offset 2, searching up for role=main skips c_h (heading)
        // and reaches c_region, whose start offset is 0 -- verifying
        // the buffer_start decrement across the preceding "aa" sibling.
        let r = b
            .find_node_by_attributes(
                2,
                FindDirection::Up,
                &u("role"),
                &u("role:(?:main;)"),
            )
            .expect("up region");
        assert_eq!(r.node, c_region);
        assert_eq!((r.start, r.end), (0, 4));
    }

    #[test]
    fn find_up_no_ancestor_match_returns_none() {
        let (b, _c_region, _c_h) = build_nested();
        assert!(b
            .find_node_by_attributes(
                2,
                FindDirection::Up,
                &u("role"),
                &u("role:(?:banner;)"),
            )
            .is_none());
    }

    #[test]
    fn find_up_from_minus_one_returns_none() {
        // offset -1 starts at the root, which has no parent, so "up"
        // immediately runs out of ancestors.
        let (b, _) = build_headings();
        assert!(b
            .find_node_by_attributes(
                -1,
                FindDirection::Up,
                &u(HEADING_ATTRIBS),
                &u(HEADING_REGEXP),
            )
            .is_none());
    }

    /// Two landmark siblings, to exercise the word-match and
    /// not-empty regex dialects produced by
    /// `_prepareForFindByAttributes`:
    ///   c1 landmark=mainland          (no name)   [0,2)
    ///   c2 landmark="banner main area" name=Skip  [2,4)
    fn build_landmarks() -> (Buffer, NodeKey, NodeKey) {
        let mut b = Buffer::new();
        let root = b
            .add_control_field_node(None, None, cf(1, 1), true)
            .unwrap();
        let c1 = b
            .add_control_field_node(Some(root), None, cf(1, 2), false)
            .unwrap();
        set_attr(&mut b, c1, "landmark", "mainland");
        b.add_text_field_node(Some(c1), None, u("aa")).unwrap();
        let c2 = b
            .add_control_field_node(Some(root), Some(c1), cf(1, 3), false)
            .unwrap();
        set_attr(&mut b, c2, "landmark", "banner main area");
        set_attr(&mut b, c2, "name", "Skip");
        b.add_text_field_node(Some(c2), None, u("bb")).unwrap();
        (b, c1, c2)
    }

    #[test]
    fn find_word_match_pattern_respects_word_boundaries() {
        let (b, _c1, c2) = build_landmarks();
        // Word-match for "main": `_prepareForFindByAttributes(
        //   {"landmark": [VBufStorage_findMatch_word("main")]})`.
        // Only c2 ("banner main area") matches; c1 ("mainland") does
        // not, because "main" is not a whole word there.
        let regexp = r"landmark:(?:\\;|[^;])*\b(?:main)\b(?:\\;|[^;])*;";
        let r = b
            .find_node_by_attributes(
                -1,
                FindDirection::Forward,
                &u("landmark"),
                &u(regexp),
            )
            .expect("word match");
        assert_eq!(r.node, c2);
    }

    #[test]
    fn find_not_empty_pattern_requires_a_value() {
        let (b, _c1, c2) = build_landmarks();
        // not-empty for "name": `_prepareForFindByAttributes(
        //   {"name": [VBufStorage_findMatch_notEmpty]})`. c1 has no
        // name (candidate "name:;") so is skipped; c2 has name=Skip.
        let regexp = r"name:(?:\\;|[^;])+;";
        let r = b
            .find_node_by_attributes(
                -1,
                FindDirection::Forward,
                &u("name"),
                &u(regexp),
            )
            .expect("not-empty match");
        assert_eq!(r.node, c2);
    }

    #[test]
    fn find_match_any_value_pattern_and_escaping() {
        // "match any (or no) value" pattern for an attribute, plus a
        // value that contains an escaped semicolon, to check the
        // escaping in the candidate string lines up with the
        // `(?:\\;|[^;])*` the Python emits.
        let mut b = Buffer::new();
        let root = b
            .add_control_field_node(None, None, cf(1, 1), true)
            .unwrap();
        let c1 = b
            .add_control_field_node(Some(root), None, cf(1, 2), false)
            .unwrap();
        // A value with a raw ';' -- match_attributes escapes it to
        // "\;", which the `\\;` alternative in the pattern matches.
        set_attr(&mut b, c1, "data", "a;b");
        b.add_text_field_node(Some(c1), None, u("x")).unwrap();
        let regexp = r"data:(?:\\;|[^;])*;";
        let r = b
            .find_node_by_attributes(
                -1,
                FindDirection::Forward,
                &u("data"),
                &u(regexp),
            )
            .expect("any-value match with escaped semicolon");
        assert_eq!(r.node, c1);
    }

    #[test]
    fn find_multi_attrib_and_parent_prefix() {
        // Two attributes (space-separated) plus a `parent::` redirect.
        // Layout: parent c_row has role=row; child c_cell has
        // role=cell. Search cells whose parent is a row.
        let mut b = Buffer::new();
        let root = b
            .add_control_field_node(None, None, cf(1, 1), true)
            .unwrap();
        let c_row = b
            .add_control_field_node(Some(root), None, cf(1, 2), false)
            .unwrap();
        set_attr(&mut b, c_row, "role", "row");
        let c_cell = b
            .add_control_field_node(Some(c_row), None, cf(1, 3), false)
            .unwrap();
        set_attr(&mut b, c_cell, "role", "cell");
        b.add_text_field_node(Some(c_cell), None, u("data")).unwrap();
        // attribs: "role parent::role" (names are unescaped in the
        // attribs list). The `:` in the "parent::role" name is escaped
        // both in the candidate string (by match_attributes) and in
        // the regexp (by `_prepareForFindByAttributes`'s `escape`,
        // which maps ':' -> `\\:`). The candidate for c_cell is
        // "role:cell;parent\:\:role:row;".
        let attribs = "role parent::role";
        let regexp = r"role:(?:cell;)parent\\:\\:role:(?:row;)";
        let r = b
            .find_node_by_attributes(
                -1,
                FindDirection::Forward,
                &u(attribs),
                &u(regexp),
            )
            .expect("parent-prefix match");
        assert_eq!(r.node, c_cell);
    }
}
