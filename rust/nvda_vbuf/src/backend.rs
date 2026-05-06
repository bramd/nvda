//! Backend-level orchestration for the vbuf storage layer.
//!
//! Phase 6c work-in-progress. The C++ `VBufBackend_t` (defined in
//! `nvdaHelper/vbufBase/backend.cpp`) is a polymorphic class that
//! inherits from `VBufStorage_buffer_t` and adds: render-thread
//! machinery (`SetTimer`, `WH_CALLWNDPROC` hook, WinEvent destroy
//! hook, `execInThread`), a polymorphic `render` virtual that
//! subclasses override, and the `update()` orchestration that
//! drains pending invalidations into temp buffers via `render` and
//! then merges them with `replaceSubtrees`.
//!
//! The Rust port currently provides:
//!
//! * The [`Renderer`] trait -- the polymorphic surface a backend
//!   implements. Used by [`update`] for both initial and partial
//!   re-renders.
//! * The [`update`] orchestration function. Operates on a
//!   borrowed `Buffer`; the caller (a future `VBufBackend` Rust
//!   trait or a C++ adapter shim) supplies the buffer and a
//!   renderer.
//!
//! Still TODO:
//! * `VBufBackend` trait -- groups buffer + renderer + render-
//!   thread state behind one polymorphic interface.
//! * Win32 render-thread machinery (timer, hooks, `execInThread`).

use crate::storage::{Buffer, FieldNodeKind, NodeKey};

/// Context passed to a [`Renderer::render`] call.
///
/// Initial renders write directly into the main buffer. Partial
/// re-renders write into a fresh temp buffer; the renderer is also
/// given access to the main buffer so it can call
/// [`Buffer::reuse_existing_node_in_render`] to short-circuit
/// re-rendering of unchanged subtrees.
pub enum RenderContext<'a> {
    /// First render of an empty buffer. The renderer writes
    /// directly into `buffer`, which is the main backend buffer
    /// (it has no content to displace).
    Initial { buffer: &'a mut Buffer },
    /// Re-render of an invalidated subtree. The renderer builds
    /// the new subtree in `temp`; afterwards [`update`] calls
    /// [`Buffer::replace_subtrees`] to splice it into `main`.
    /// `main` is borrowed exclusively for the duration of this
    /// render call -- it's needed for the cross-buffer
    /// reference-reuse query.
    Update {
        temp: &'a mut Buffer,
        main: &'a mut Buffer,
        old_node: NodeKey,
    },
}

/// Implemented by per-backend code that knows how to convert an
/// IAccessible (or equivalent) into vbuf nodes. Mirrors the C++
/// `VBufBackend_t::render` pure virtual.
pub trait Renderer {
    /// Render the (`doc_handle`, `id`)-rooted subtree into the
    /// buffer described by `ctx`. The renderer is responsible for
    /// adding nodes via the buffer's add-node methods.
    fn render(&mut self, ctx: RenderContext<'_>, doc_handle: i32, id: i32);
}

/// Drain the buffer's pending invalidations, render each into a
/// temp buffer via `renderer`, and atomically merge the results
/// back into `main` via [`Buffer::replace_subtrees`]. If `main`
/// is empty, performs an initial render directly into it.
///
/// Mirrors `VBufBackend_t::update` from `backend.cpp:188-226`.
/// The C++ also notifies NVDA via
/// `nvdaControllerInternal_vbufChangeNotify` after a re-render
/// completes; that notify call belongs in the eventual Win32
/// integration layer rather than the storage-side update loop, so
/// it isn't here.
pub fn update<R: Renderer>(
    main: &mut Buffer,
    renderer: &mut R,
    root_doc_handle: i32,
    root_id: i32,
) {
    if !main.has_content() {
        // Initial render -- straight into main.
        renderer.render(
            RenderContext::Initial { buffer: main },
            root_doc_handle,
            root_id,
        );
        return;
    }

    // Re-render path: drain pending invalidations into working,
    // render each into a temp buffer, then atomically merge.
    let working_keys = main.take_pending_into_working();
    let mut map: Vec<(NodeKey, Buffer)> = Vec::new();
    for key in working_keys {
        // Identifier of the invalidated subtree's root, looked up
        // from `main`. Skip stale keys (could happen if a previous
        // mutation cascade-removed the node).
        let identifier = {
            let n = match main.get(key) {
                Some(n) => n,
                None => continue,
            };
            match &n.kind {
                FieldNodeKind::Control(d) => d.identifier,
                _ => continue,
            }
        };
        let mut temp = Buffer::new();
        renderer.render(
            RenderContext::Update {
                temp: &mut temp,
                main,
                old_node: key,
            },
            identifier.doc_handle,
            identifier.id,
        );
        map.push((key, temp));
    }
    main.clear_working();
    main.replace_subtrees(map);
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

    /// A minimal renderer that builds a fixed text subtree on
    /// demand. Records each call so tests can verify the
    /// orchestration without depending on a real backend.
    struct StubRenderer {
        calls: Vec<(i32, i32, bool)>,
        text: &'static str,
    }

    impl Renderer for StubRenderer {
        fn render(
            &mut self,
            ctx: RenderContext<'_>,
            doc_handle: i32,
            id: i32,
        ) {
            let is_update = matches!(ctx, RenderContext::Update { .. });
            self.calls.push((doc_handle, id, is_update));
            let buffer = match ctx {
                RenderContext::Initial { buffer } => buffer,
                RenderContext::Update { temp, .. } => temp,
            };
            let root = buffer
                .add_control_field_node(
                    None,
                    None,
                    cf(doc_handle, id),
                    true,
                )
                .expect("add root");
            buffer
                .add_text_field_node(
                    Some(root),
                    None,
                    w(self.text),
                )
                .expect("add text");
        }
    }

    #[test]
    fn update_initial_render_writes_into_main() {
        let mut main = Buffer::new();
        let mut renderer = StubRenderer {
            calls: Vec::new(),
            text: "hello",
        };
        update(&mut main, &mut renderer, 7, 42);
        assert_eq!(renderer.calls, vec![(7, 42, false)]);
        let root = main
            .get_control_field_node_with_identifier(7, 42)
            .expect("root present");
        assert_eq!(main.text_length(), 5);
        let mut buf: Vec<u16> = Vec::new();
        main.get_text_in_range(root, 0, 5, &mut buf);
        assert_eq!(String::from_utf16(&buf).unwrap(), "hello");
    }

    #[test]
    fn update_replays_pending_invalidations() {
        // Prime main with an initial render, then invalidate one
        // node and run update again. The second update should
        // re-render that node into a temp buffer and merge.
        let mut main = Buffer::new();
        let mut renderer = StubRenderer {
            calls: Vec::new(),
            text: "FIRST",
        };
        update(&mut main, &mut renderer, 1, 1);
        // The root is (1, 1); invalidate it.
        let root = main
            .get_control_field_node_with_identifier(1, 1)
            .unwrap();
        main.invalidate_subtree(root);

        // Switch to a renderer that emits different text.
        let mut renderer2 = StubRenderer {
            calls: Vec::new(),
            text: "SECOND",
        };
        update(&mut main, &mut renderer2, 1, 1);
        // The second render was an Update call (re-render path).
        assert_eq!(renderer2.calls, vec![(1, 1, true)]);
        let new_root = main
            .get_control_field_node_with_identifier(1, 1)
            .unwrap();
        let mut buf: Vec<u16> = Vec::new();
        main.get_text_in_range(new_root, 0, 6, &mut buf);
        assert_eq!(String::from_utf16(&buf).unwrap(), "SECOND");
    }

    #[test]
    fn update_with_no_pending_is_a_noop_when_content_exists() {
        let mut main = Buffer::new();
        let mut renderer = StubRenderer {
            calls: Vec::new(),
            text: "hello",
        };
        update(&mut main, &mut renderer, 1, 1);
        let calls_after_initial = renderer.calls.len();
        // No invalidation; update should not call render again.
        update(&mut main, &mut renderer, 1, 1);
        assert_eq!(renderer.calls.len(), calls_after_initial);
    }
}
