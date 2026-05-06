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

/// Polymorphic backend surface that ties together storage state,
/// renderer state, and identity. Mirrors the C++ `VBufBackend_t`
/// class; used by the upcoming Win32 render-thread machinery to
/// dispatch into per-backend code without knowing the concrete
/// type.
///
/// Implementors hold their own `Buffer` and renderer state. The
/// trait splits access via [`VBufBackend::for_update`] so the
/// caller can simultaneously borrow the buffer and the renderer
/// without violating Rust's aliasing rules.
pub trait VBufBackend {
    /// Read-only buffer access -- for size queries, identifier
    /// lookup, etc. without taking the update borrow.
    fn buffer(&self) -> &Buffer;
    /// Mutable buffer access -- for invalidation, attribute
    /// updates, and other operations that don't need to render.
    fn buffer_mut(&mut self) -> &mut Buffer;
    /// `(docHandle, ID)` of the document root this backend
    /// renders for. Mirrors `VBufBackend_t::rootDocHandle` /
    /// `rootID`.
    fn root_doc_handle(&self) -> i32;
    fn root_id(&self) -> i32;
    /// Split-borrow accessor for [`update`] orchestration.
    /// Returns simultaneous borrows of the buffer and the
    /// renderer plus identity values; the caller threads them
    /// into [`update`].
    fn for_update(
        &mut self,
    ) -> (&mut Buffer, &mut dyn Renderer, i32, i32);
}

/// Run [`update`] on a backend by way of its [`VBufBackend::for_update`]
/// split-borrow. Equivalent to the C++ `VBufBackend_t::update` entry
/// point.
pub fn update_backend(backend: &mut dyn VBufBackend) {
    let (buffer, renderer, root_doc, root_id) = backend.for_update();
    update(buffer, renderer, root_doc, root_id);
}

/// Invalidate a subtree on the given backend and trigger an
/// update via [`update_backend`]. Mirrors the C++
/// `VBufBackend_t::invalidateSubtree` -> `requestUpdate` flow,
/// minus the timer (the Win32 timer wiring lands in a follow-up
/// commit; this function performs the update synchronously for
/// now).
pub fn invalidate_and_update<B: VBufBackend>(
    backend: &mut B,
    node: NodeKey,
) -> bool {
    if !backend.buffer_mut().invalidate_subtree(node) {
        return false;
    }
    update_backend(backend);
    true
}

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
pub fn update<R: Renderer + ?Sized>(
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

    /// A minimal backend that wraps the StubRenderer. Lets us
    /// exercise the VBufBackend trait + update_backend /
    /// invalidate_and_update wrappers without depending on a
    /// real backend implementation.
    struct StubBackend {
        buffer: Buffer,
        renderer: StubRenderer,
        root_doc_handle: i32,
        root_id: i32,
    }

    impl VBufBackend for StubBackend {
        fn buffer(&self) -> &Buffer {
            &self.buffer
        }
        fn buffer_mut(&mut self) -> &mut Buffer {
            &mut self.buffer
        }
        fn root_doc_handle(&self) -> i32 {
            self.root_doc_handle
        }
        fn root_id(&self) -> i32 {
            self.root_id
        }
        fn for_update(
            &mut self,
        ) -> (&mut Buffer, &mut dyn Renderer, i32, i32) {
            (
                &mut self.buffer,
                &mut self.renderer,
                self.root_doc_handle,
                self.root_id,
            )
        }
    }

    #[test]
    fn update_backend_dispatches_through_trait() {
        let mut backend = StubBackend {
            buffer: Buffer::new(),
            renderer: StubRenderer {
                calls: Vec::new(),
                text: "trait test",
            },
            root_doc_handle: 5,
            root_id: 9,
        };
        update_backend(&mut backend);
        assert_eq!(backend.renderer.calls, vec![(5, 9, false)]);
        // Buffer was populated through the trait dispatch.
        assert!(backend.buffer().has_content());
    }

    #[test]
    fn invalidate_and_update_triggers_rerender() {
        let mut backend = StubBackend {
            buffer: Buffer::new(),
            renderer: StubRenderer {
                calls: Vec::new(),
                text: "v1",
            },
            root_doc_handle: 1,
            root_id: 1,
        };
        update_backend(&mut backend);
        let root = backend
            .buffer()
            .get_control_field_node_with_identifier(1, 1)
            .unwrap();
        // Switch the renderer to a different text.
        backend.renderer.text = "v2";
        backend.renderer.calls.clear();
        assert!(invalidate_and_update(&mut backend, root));
        // The invalidation triggered an Update-context render.
        assert_eq!(backend.renderer.calls, vec![(1, 1, true)]);
        let new_root = backend
            .buffer()
            .get_control_field_node_with_identifier(1, 1)
            .unwrap();
        let mut buf: Vec<u16> = Vec::new();
        backend
            .buffer()
            .get_text_in_range(new_root, 0, 2, &mut buf);
        assert_eq!(String::from_utf16(&buf).unwrap(), "v2");
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
