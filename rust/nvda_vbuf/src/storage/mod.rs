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

use std::collections::{BTreeMap, BTreeSet};

use slotmap::SlotMap;

/// The `(docHandle, ID)` pair that uniquely identifies a control
/// field node in a buffer. Mirrors
/// `VBufStorage_controlFieldNodeIdentifier_t`.
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
pub struct ControlFieldIdentifier {
    pub doc_handle: i32,
    pub id: i32,
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
/// `VBufStorage_buffer_t`.
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
}

impl Buffer {
    /// Construct an empty buffer. The C++ default constructor.
    pub fn new() -> Self {
        Self {
            nodes: SlotMap::with_key(),
            root: None,
            by_identifier: BTreeMap::new(),
            selection_start: 0,
            selection_length: 0,
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
}
