/*
This file is a part of the NVDA project.
URL: http://www.nvda-project.org/
Copyright 2026 NV Access Limited
    This program is free software: you can redistribute it and/or modify
    it under the terms of the GNU General Public License version 2.0, as published by
    the Free Software Foundation.
    This program is distributed in the hope that it will be useful,
    but WITHOUT ANY WARRANTY; without even the implied warranty of
    MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.
This license can be found at:
http://www.gnu.org/licenses/old-licenses/gpl-2.0.html
*/

/*
Flat C ABI shim over the C++ VBufBackend_t render-thread machinery,
exposed so the Rust backend crates can arm / query that machinery without
binding the C++ ABI directly.

Every backend now homes its live tree in a Rust storage::Buffer, so this
shim no longer touches the C++ VBufStorage_* storage classes at all: it
only reaches VBufBackend_t members (rootDocHandle / rootID, the update
timer, and getRustStorageBuffer). The tree itself is created, read, and
mutated entirely on the Rust side.

Conventions:
* Opaque `void*` on the boundary; Rust-side bindings wrap them in newtypes.
* All operations are synchronous and run on the caller's thread; the
  vbufBase thread-affinity invariants (render thread vs. main thread) are
  the caller's responsibility, matching the existing C++ contract.
*/

#include <vbufBase/backend.h>

/* --- VBufBackend_t operations -------------------------------------- */

extern "C" int vbuf_backend_get_root_doc_handle(void* backend) {
	return static_cast<VBufBackend_t*>(backend)->rootDocHandle;
}

extern "C" int vbuf_backend_get_root_id(void* backend) {
	return static_cast<VBufBackend_t*>(backend)->rootID;
}

extern "C" void vbuf_backend_force_update(void* backend) {
	static_cast<VBufBackend_t*>(backend)->forceUpdate();
}

/* Arms the render-thread timer so the backend re-renders any invalid
   nodes on the next update tick. Lets a Rust-side invalidation (a
   backend's WinEvent dispatch / change sink) request an update after
   invalidating its Rust storage::Buffer directly. */
extern "C" void vbuf_backend_request_update(void* backend) {
	static_cast<VBufBackend_t*>(backend)->requestUpdate();
}

/* Returns the backend's Rust storage::Buffer. Lets the backend crates
   reach the buffer from a bare VBufBackend_t* where they lack the
   per-backend state struct. See VBufBackend_t::getRustStorageBuffer. */
extern "C" void* vbuf_backend_get_rust_storage_buffer(void* backend) {
	return static_cast<VBufBackend_t*>(backend)->getRustStorageBuffer();
}
