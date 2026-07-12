//! Backend-level orchestration for the vbuf storage layer.
//!
//! The C++ `VBufBackend_t` (`nvdaHelper/vbufBase/backend.cpp`) is a
//! polymorphic class that inherits from `VBufStorage_buffer_t` and adds
//! render-thread machinery (`SetTimer`, `WH_CALLWNDPROC` hook, WinEvent
//! destroy hook, `execInThread`), a polymorphic `render` virtual that
//! subclasses override, and the `update()` orchestration that drains
//! pending invalidations into temp buffers via `render` and then merges
//! them with `replaceSubtrees`.
//!
//! The Rust port keeps the render-thread machinery in C++ (see
//! `docs/plans/2026-07-11-rust-vbuf-6e-design.md`) and moves only the
//! storage side of `update()` here, as [`run_raw_update`]. Every Rust
//! backend (gecko_ia2 today, adobeAcrobat next) shares that one
//! orchestration; only the per-backend *render* closure — which resolves
//! an accessible for a `(docHandle, ID)` and fills a target [`Buffer`] —
//! differs.
//!
//! [`run_raw_update`] is deliberately raw-pointer based: a backend's
//! renderer needs a `main` buffer handle even during the *initial* render
//! (where the render target and `main` alias the same buffer, so that
//! cross-buffer reference-reuse queries work uniformly), which safe
//! `&mut` borrows cannot express. The render closure carries the
//! `unsafe` obligation to touch only the buffers it is handed.

use crate::storage::{Buffer, NodeKey};

/// Drain `main`'s pending invalidations, render each invalidated subtree
/// into a temp buffer via `render`, and atomically merge the results
/// back into `main` via [`Buffer::replace_subtrees`]. If `main` has no
/// content, performs an initial render straight into it.
///
/// `render(target, main, doc_handle, id, old_node)` renders the subtree
/// rooted at `(doc_handle, id)` into `*target`, may query `*main` for
/// reference-reuse, and returns `true` when it produced a subtree to
/// merge. Returning `false` skips this subtree — the existing content is
/// left untouched — mirroring the C++ `continue` taken when the
/// accessible cannot be resolved. On the initial render `target == main`
/// (the same buffer; it has no content to displace) and `old_node` is
/// `None`; on a re-render `old_node` is the `main` node being replaced
/// (the base `VBufBackend_t::render`'s `oldNode` argument), which a
/// backend may consult for inherited state.
///
/// Returns `true` when the re-render branch ran and `false` on the
/// initial render, reproducing `VBufBackend_t::update`'s notify
/// contract: the C++ fires `nvdaControllerInternal_vbufChangeNotify`
/// only after a re-render, never after the initial render. The notify
/// call itself is Win32 integration and stays with the C++ caller; this
/// return value tells it whether to fire.
///
/// Mirrors `VBufBackend_t::update` from `backend.cpp:188-226`.
///
/// # Safety
///
/// * `main` must be a valid `*mut Buffer`, exclusively owned for the
///   duration of the call (the caller holds the backend lock).
/// * `render` must touch only the buffer pointers it is handed, and must
///   tolerate `target == main` on the initial render.
pub unsafe fn run_raw_update(
    main: *mut Buffer,
    root_doc_handle: i32,
    root_id: i32,
    mut render: impl FnMut(*mut Buffer, *mut Buffer, i32, i32, Option<NodeKey>) -> bool,
) -> bool {
    // Initial render: an empty buffer is rendered straight into `main`
    // (it has no content to displace). The base `update()`'s `else`
    // branch does not fire vbufChangeNotify -- return `false`.
    if !unsafe { (*main).has_content() } {
        render(main, main, root_doc_handle, root_id, None);
        return false;
    }

    // Re-render path: drain pending invalidations into the working list,
    // re-render each into its own temp buffer (the renderer queries
    // `main` for reuse), then atomically merge.
    let working_keys = unsafe { (*main).take_pending_into_working() };
    let mut map: Vec<(NodeKey, Buffer)> = Vec::new();
    for key in working_keys {
        // Identifier of the invalidated subtree root, looked up from
        // `main`. Skip stale keys (a prior cascade may have removed one).
        let identifier =
            match unsafe { (*main).identifier_of_control_field_node(key) } {
                Some(i) => i,
                None => continue,
            };
        let mut temp = Buffer::new();
        let temp_ptr: *mut Buffer = &mut temp as *mut Buffer;
        // `temp` is moved into `map` only after `render` returns, so
        // `temp_ptr` is never used past the move; the arena is
        // heap-allocated, so the move is address-stable for stored nodes.
        if render(temp_ptr, main, identifier.doc_handle, identifier.id, Some(key)) {
            map.push((key, temp));
        }
    }
    unsafe { (*main).clear_working() };
    unsafe { (*main).replace_subtrees(map) };
    // Re-render branch taken: mirror the base `update()`'s `hasContent`
    // arm, which fires vbufChangeNotify.
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::ControlFieldIdentifier;

    fn cf(doc_handle: i32, id: i32) -> ControlFieldIdentifier {
        ControlFieldIdentifier { doc_handle, id }
    }

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    /// Render a fixed `control > text` subtree into `*buffer`. Stands in
    /// for a real backend's `fill_vbuf` in the orchestration tests.
    fn render_text(buffer: *mut Buffer, doc_handle: i32, id: i32, text: &str) {
        let buffer = unsafe { &mut *buffer };
        let root = buffer
            .add_control_field_node(None, None, cf(doc_handle, id), true)
            .expect("add root");
        buffer
            .add_text_field_node(Some(root), None, w(text))
            .expect("add text");
    }

    #[test]
    fn initial_render_writes_into_main() {
        let mut main = Buffer::new();
        let main_ptr: *mut Buffer = &mut main;
        let mut calls: Vec<(i32, i32, bool)> = Vec::new();
        let notify = unsafe {
            run_raw_update(main_ptr, 7, 42, |target, m, dh, id, _old| {
                // On the initial render the target and main alias.
                calls.push((dh, id, target == m));
                render_text(target, dh, id, "hello");
                true
            })
        };
        // Initial render never asks the caller to notify.
        assert!(!notify);
        assert_eq!(calls, vec![(7, 42, true)]);
        let root = main
            .get_control_field_node_with_identifier(7, 42)
            .expect("root present");
        assert_eq!(main.text_length(), 5);
        let mut buf: Vec<u16> = Vec::new();
        main.get_text_in_range(root, 0, 5, &mut buf);
        assert_eq!(String::from_utf16(&buf).unwrap(), "hello");
    }

    #[test]
    fn replays_pending_invalidations() {
        // Prime main with an initial render, then invalidate the root and
        // run update again with a renderer that emits different text.
        let mut main = Buffer::new();
        unsafe {
            let p: *mut Buffer = &mut main;
            run_raw_update(p, 1, 1, |t, _m, dh, id, _old| {
                render_text(t, dh, id, "FIRST");
                true
            });
        }
        let root = main
            .get_control_field_node_with_identifier(1, 1)
            .unwrap();
        main.invalidate_subtree(root);

        let mut calls: Vec<(i32, i32)> = Vec::new();
        let notify = unsafe {
            let p: *mut Buffer = &mut main;
            run_raw_update(p, 1, 1, |t, _m, dh, id, _old| {
                calls.push((dh, id));
                render_text(t, dh, id, "SECOND");
                true
            })
        };
        // Re-render branch: the caller should notify.
        assert!(notify);
        assert_eq!(calls, vec![(1, 1)]);
        let new_root = main
            .get_control_field_node_with_identifier(1, 1)
            .unwrap();
        let mut buf: Vec<u16> = Vec::new();
        main.get_text_in_range(new_root, 0, 6, &mut buf);
        assert_eq!(String::from_utf16(&buf).unwrap(), "SECOND");
    }

    #[test]
    fn no_pending_is_a_noop_when_content_exists() {
        let mut main = Buffer::new();
        unsafe {
            let p: *mut Buffer = &mut main;
            run_raw_update(p, 1, 1, |t, _m, dh, id, _old| {
                render_text(t, dh, id, "hello");
                true
            });
        }
        // No invalidation queued; the renderer must not be called again.
        let mut count = 0;
        let notify = unsafe {
            let p: *mut Buffer = &mut main;
            run_raw_update(p, 1, 1, |_t, _m, _dh, _id, _old| {
                count += 1;
                true
            })
        };
        assert_eq!(count, 0);
        // Still the re-render branch (content exists), so notify is true.
        assert!(notify);
    }

    #[test]
    fn render_returning_false_leaves_subtree_untouched() {
        let mut main = Buffer::new();
        unsafe {
            let p: *mut Buffer = &mut main;
            run_raw_update(p, 1, 1, |t, _m, dh, id, _old| {
                render_text(t, dh, id, "ORIGINAL");
                true
            });
        }
        let root = main
            .get_control_field_node_with_identifier(1, 1)
            .unwrap();
        main.invalidate_subtree(root);
        // Renderer reports failure (e.g. accessible could not be
        // resolved): the invalidation is consumed but the subtree is not
        // replaced.
        unsafe {
            let p: *mut Buffer = &mut main;
            run_raw_update(p, 1, 1, |_t, _m, _dh, _id, _old| false);
        }
        let same_root = main
            .get_control_field_node_with_identifier(1, 1)
            .unwrap();
        let mut buf: Vec<u16> = Vec::new();
        main.get_text_in_range(same_root, 0, 8, &mut buf);
        assert_eq!(String::from_utf16(&buf).unwrap(), "ORIGINAL");
    }
}
