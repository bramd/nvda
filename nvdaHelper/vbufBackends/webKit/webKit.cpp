/*
This file is a part of the NVDA project.
URL: http://www.nvda-project.org/
Copyright 2011-2016 NV Access Limited
    This program is free software: you can redistribute it and/or modify
    it under the terms of the GNU General Public License version 2.0, as published by
    the Free Software Foundation.
    This program is distributed in the hope that it will be useful,
    but WITHOUT ANY WARRANTY; without even the implied warranty of
    MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.
This license can be found at:
http://www.gnu.org/licenses/old-licenses/gpl-2.0.html
*/

#include <windows.h>
#include <remote/nvdaHelperRemote.h>
#include <remote/nvdaControllerInternal.h>
#include <common/log.h>
#include <vbufBase/backend.h>
#include "webKit.h"

using namespace std;

extern "C" {
	// Per-instance Rust state + C-ABI entry points (nvda_ia2 crate,
	// webkit_backend_state.rs). The live tree lives in an embedded Rust
	// storage::Buffer; the render logic is webkit_fill_vbuf.rs.
	void* nvda_ia2_webkit_backend_create();
	void nvda_ia2_webkit_backend_destroy(void* state);
	void* nvda_ia2_webkit_backend_get_buffer(void* state);
	void nvda_ia2_webkit_backend_clear_buffer(void* state);
	// Drives the Rust drain/render/merge over the embedded Buffer;
	// returns true when the caller should fire vbufChangeNotify.
	bool nvda_ia2_webkit_backend_update(void* state, void* backend);
	// WinEvent hook: outer event filter + per-backend dispatch (invalidate
	// the affected subtree + arm the render timer).
	bool nvda_ia2_webkit_backend_win_event_is_relevant(unsigned int event_id);
	void nvda_ia2_webkit_backend_dispatch_win_event(void* state, void* backend, int doc_handle, int child_id);
}

void CALLBACK WebKitVBufBackend_t::renderThread_winEventProcHook(HWINEVENTHOOK hookID, DWORD eventID, HWND hwnd, long objectID, long childID, DWORD threadID, DWORD time) {
	if (!nvda_ia2_webkit_backend_win_event_is_relevant(eventID)) {
		return;
	}
	const int docHandle = HandleToUlong(hwnd);
	for (auto* backend : runningBackends) {
		HWND rootWindow = (HWND)UlongToHandle(backend->rootDocHandle);
		if (rootWindow != hwnd && !IsChild(rootWindow, hwnd))
			continue;
		auto* webKitBackend = static_cast<WebKitVBufBackend_t*>(backend);
		// The Rust dispatch flips the sign of childID for the buffer
		// lookup (WebKit stores positive unique IDs but fires events with
		// negative ones), invalidates the subtree, and arms the timer.
		nvda_ia2_webkit_backend_dispatch_win_event(
			webKitBackend->rustState, backend, docHandle, childID);
		break;
	}
}

void WebKitVBufBackend_t::renderThread_initialize() {
	registerWinEventHook(renderThread_winEventProcHook);
	VBufBackend_t::renderThread_initialize();
}

void WebKitVBufBackend_t::renderThread_terminate() {
	unregisterWinEventHook(renderThread_winEventProcHook);
	VBufBackend_t::renderThread_terminate();
	// WebKit's live tree is the Rust storage::Buffer, not the (always-empty)
	// C++ storage the base call just cleared; empty the Rust buffer too.
	nvda_ia2_webkit_backend_clear_buffer(this->rustState);
}

void WebKitVBufBackend_t::update() {
	// Drive the Rust drain/render/merge orchestration over the embedded
	// storage::Buffer. The lock is held across the whole Rust update (so no
	// vbufRemote reader thread materializes a &Buffer while the render
	// thread holds a &mut Buffer); the change-notify fires OUTSIDE the lock,
	// and only when the orchestration reports it took the re-render branch
	// (the base update() skips vbufChangeNotify on the initial render, which
	// nvda_ia2_webkit_backend_update preserves by returning false).
	this->lock.acquire();
	const bool shouldNotify = nvda_ia2_webkit_backend_update(this->rustState, this);
	this->lock.release();
	if (shouldNotify) {
		nvdaControllerInternal_vbufChangeNotify(this->rootDocHandle, this->rootID);
	}
}

void* WebKitVBufBackend_t::getRustStorageBuffer() {
	return nvda_ia2_webkit_backend_get_buffer(this->rustState);
}

void WebKitVBufBackend_t::render(VBufStorage_buffer_t* buffer, int docHandle, int ID, VBufStorage_controlFieldNode_t* oldNode) {
	// Vestigial after the Rust flip: update() is overridden and performs all
	// rendering against the Rust storage::Buffer (via the nvda_ia2 webkit
	// fill_vbuf renderer), so render() is never reached. It stays a concrete
	// (empty) definition only to satisfy the base's pure-virtual render() and
	// keep the class instantiable.
}

WebKitVBufBackend_t::WebKitVBufBackend_t(int docHandle, int ID): VBufBackend_t(docHandle,ID), rustState(nvda_ia2_webkit_backend_create()) {
}

WebKitVBufBackend_t::~WebKitVBufBackend_t() {
	// Frees the WebKitBackendState (its Drop releases the live Buffer).
	nvda_ia2_webkit_backend_destroy(this->rustState);
	this->rustState = nullptr;
}

VBufBackend_t* WebKitVBufBackend_t_createInstance(int docHandle, int ID) {
	VBufBackend_t* backend=new WebKitVBufBackend_t(docHandle,ID);
	return backend;
}
