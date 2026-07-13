/*
This file is a part of the NVDA project.
URL: http://www.nvda-project.org/
Copyright 2011 NV Access Inc
    This program is free software: you can redistribute it and/or modify
    it under the terms of the GNU General Public License version 2.0, as published by
    the Free Software Foundation.
    This program is distributed in the hope that it will be useful,
    but WITHOUT ANY WARRANTY; without even the implied warranty of
    MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.
This license can be found at:
http://www.gnu.org/licenses/old-licenses/gpl-2.0.html
*/

#ifndef VIRTUALBUFFER_BACKENDS_WEBKIT_H
#define VIRTUALBUFFER_BACKENDS_WEBKIT_H

#include <vbufBase/backend.h>

class WebKitVBufBackend_t: public VBufBackend_t {
	private:

	/* Per-instance Rust state (WebKitBackendState). Allocated by
	 * nvda_ia2_webkit_backend_create() in the constructor and freed by
	 * nvda_ia2_webkit_backend_destroy() in the destructor. Homes the live
	 * tree in a Rust storage::Buffer; the render logic (fillVBuf) lives in
	 * the nvda_ia2 crate (webkit_fill_vbuf.rs).
	 */
	void* rustState = nullptr;

	protected:

	static void CALLBACK renderThread_winEventProcHook(HWINEVENTHOOK hookID, DWORD eventID, HWND hwnd, long objectID, long childID, DWORD threadID, DWORD time);

	virtual void renderThread_initialize();

	virtual void renderThread_terminate();

	/* Vestigial after the Rust flip: update() is overridden and does all
	 * the rendering against the Rust buffer (via the nvda_ia2 webkit
	 * fill_vbuf renderer), so this C++ render() is never on the live path.
	 * Kept as an empty stub only to satisfy the base's pure-virtual
	 * render().
	 */
	virtual void render(VBufStorage_buffer_t* buffer, int docHandle, int ID, VBufStorage_controlFieldNode_t* oldNode=NULL);

	/* This backend homes its live tree in a Rust storage::Buffer (in
	 * WebKitBackendState), so it overrides update() to run the shared Rust
	 * drain/render/merge orchestration against that buffer instead of the
	 * inherited C++ VBufStorage_buffer_t.
	 */
	virtual void update();

	virtual ~WebKitVBufBackend_t();

	public:

	WebKitVBufBackend_t(int docHandle, int ID);

	/* Advertises this backend's embedded Rust storage::Buffer so
	 * vbufRemote's read RPCs route through the nvda_vbuf_* u64-key ABI
	 * instead of the legacy C++ storage virtuals.
	 */
	virtual void* getRustStorageBuffer();

};

#endif
