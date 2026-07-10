/*
This file is a part of the NVDA project.
URL: http://www.nvda-project.org/
Copyright 2007-2023 NV Access Limited, Mozilla Corporation
    This program is free software: you can redistribute it and/or modify
    it under the terms of the GNU General Public License version 2.0, as published by
    the Free Software Foundation.
    This program is distributed in the hope that it will be useful,
    but WITHOUT ANY WARRANTY; without even the implied warranty of
    MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.
This license can be found at:
http://www.gnu.org/licenses/old-licenses/gpl-2.0.html
*/

#include <memory>
#include <numeric>
#include <functional>
#include <vector>
#include <map>
#include <optional>
#include <windows.h>
#include <set>
#include <string>
#include <sstream>
#include <atlcomcli.h>
#include <ia2.h>
#include <common/ia2utils.h>
#include <remote/nvdaHelperRemote.h>
#include <remote/nvdaControllerInternal.h>
#include <vbufBase/backend.h>
#include <vbufBase/storage.h>
#include <common/log.h>
#include <vbufBase/utils.h>
#include <remote/textFromIAccessible.h>
#include "gecko_ia2.h"

using namespace std;

#ifdef NVDA_HAS_RUST_HELPERS
extern "C" {
	void* nvda_ia2_gecko_backend_create();
	void nvda_ia2_gecko_backend_destroy(void* state);
	int nvda_ia2_gecko_backend_is_chrome(void* state);
	void nvda_ia2_gecko_backend_version_specific_init(
		void* state,
		void* pacc);
	// Phase 6e (Stage D) orchestration over the embedded Rust
	// storage::Buffer. update runs the drain/render/merge and returns
	// whether the caller should fire vbufChangeNotify (mirrors the base
	// update()'s notify-only-on-re-render condition). get_buffer backs
	// getRustStorageBuffer(); clear_buffer empties state.buffer on the
	// terminate path.
	bool nvda_ia2_gecko_backend_update(
		void* state,
		void* backend);
	void* nvda_ia2_gecko_backend_get_buffer(void* state);
	void nvda_ia2_gecko_backend_clear_buffer(void* state);
	void nvda_ia2_gecko_backend_render_thread_initialize(
		void* state,
		int doc_handle,
		int id);
	void nvda_ia2_gecko_backend_render_thread_terminate(void* state);
	int nvda_ia2_gecko_backend_is_root_doc_alive(
		void* state,
		void* backend);
	bool nvda_ia2_gecko_backend_win_event_is_relevant(
		unsigned int event_id,
		void* hwnd,
		int object_id,
		int child_id);
	int nvda_ia2_gecko_backend_dispatch_win_event(
		void* state,
		void* backend,
		unsigned int event_id,
		int doc_handle,
		int id);
}

void GeckoVBufBackend_t::versionSpecificInit(IAccessible2* pacc) {
	nvda_ia2_gecko_backend_version_specific_init(this->rustState, pacc);
}
#endif

// Phase 6e (Stage D): the former GeckoVBufBackend_t::fillVBuf shim (which
// delegated to the nvda_ia2_fill_vbuf extern) was removed at the flip.
// Nothing called it any more: the old render() reached it, and render()
// is now a vestigial stub -- the live path drives the Rust fill_vbuf from
// nvda_ia2_gecko_backend_update. See fill_vbuf.rs for the matching note.

bool GeckoVBufBackend_t::isRootDocAlive() {
	return nvda_ia2_gecko_backend_is_root_doc_alive(
		this->rustState, this) != 0;
}

void CALLBACK GeckoVBufBackend_t::renderThread_winEventProcHook(
	HWINEVENTHOOK hookID, DWORD eventID, HWND hwnd,
	long objectID, long childID, DWORD threadID, DWORD time
) {
	if (!nvda_ia2_gecko_backend_win_event_is_relevant(
			eventID, hwnd, objectID, childID))
	{
		return;
	}
	const int docHandle = HandleToUlong(hwnd);
	const int ID = childID;
	for (auto* backend : runningBackends) {
		HWND rootWindow = (HWND)UlongToHandle(backend->rootDocHandle);
		if (rootWindow != hwnd && !IsChild(rootWindow, hwnd))
			continue;
		auto* geckoBackend = static_cast<GeckoVBufBackend_t*>(backend);
		const int outcome = nvda_ia2_gecko_backend_dispatch_win_event(
			geckoBackend->rustState,
			backend,
			eventID,
			docHandle,
			ID);
		if (outcome != 0) {
			// WinEventOutcome::StopAll -- exit the whole hook.
			return;
		}
	}
}

void GeckoVBufBackend_t::renderThread_initialize() {
	registerWinEventHook(renderThread_winEventProcHook);
	VBufBackend_t::renderThread_initialize();
	nvda_ia2_gecko_backend_render_thread_initialize(
		this->rustState, this->rootDocHandle, this->rootID);
}

void GeckoVBufBackend_t::renderThread_terminate() {
	unregisterWinEventHook(renderThread_winEventProcHook);
	VBufBackend_t::renderThread_terminate();
	// Gecko's live tree is the Rust storage::Buffer, not the (always-empty)
	// C++ storage the base call just cleared; empty the Rust buffer too
	// (Phase 6e, Decision 5).
	nvda_ia2_gecko_backend_clear_buffer(this->rustState);
	// The backend holds a reference to the root accessible of the document.
	// This must be specifically released here, in the UI thread where it was created.
	// See https://issues.chromium.org/issues/41487612
	nvda_ia2_gecko_backend_render_thread_terminate(this->rustState);
}

void GeckoVBufBackend_t::update() {
	// Phase 6e (Stage D): drive the Rust drain/render/merge orchestration
	// over the embedded storage::Buffer. The lock is held across the WHOLE
	// Rust update -- coarser than the base VBufBackend_t::update(), which
	// releases the lock while rendering temp subtrees -- so that no
	// vbufRemote reader thread can materialize a &Buffer over state.buffer
	// while the render thread holds a &mut Buffer (Decision 2/3). The
	// change-notify then fires OUTSIDE the lock, exactly as the base does,
	// and ONLY when the orchestration reports it took the re-render branch:
	// the base update() skips vbufChangeNotify on the initial render, and
	// nvda_ia2_gecko_backend_update returns false there to preserve that.
	this->lock.acquire();
	const bool shouldNotify =
		nvda_ia2_gecko_backend_update(this->rustState, this);
	this->lock.release();
	if (shouldNotify) {
		nvdaControllerInternal_vbufChangeNotify(this->rootDocHandle, this->rootID);
	}
}

void* GeckoVBufBackend_t::getRustStorageBuffer() {
	return nvda_ia2_gecko_backend_get_buffer(this->rustState);
}

void GeckoVBufBackend_t::render(VBufStorage_buffer_t* buffer, int docHandle, int ID, VBufStorage_controlFieldNode_t* oldNode) {
	// Vestigial after Stage D. update() is overridden and performs all
	// rendering against the Rust storage::Buffer, so render() is never
	// reached for a gecko backend: forceUpdate(), the render-thread timer
	// proc, and the initial renderThread_initialize all dispatch through
	// the now-virtual update(). This stays a concrete (empty) definition
	// only to satisfy the base's pure-virtual render() and keep the class
	// instantiable; its former body moved into the Rust renderer
	// (fill_vbuf), driven by nvda_ia2_gecko_backend_update.
}

GeckoVBufBackend_t::GeckoVBufBackend_t(int docHandle, int ID):
	VBufBackend_t(docHandle, ID),
	rustState(nvda_ia2_gecko_backend_create())
{
}

GeckoVBufBackend_t::~GeckoVBufBackend_t() {
	// The Rust state may still hold an AddRef'd reference to the
	// root IAccessible2. Releasing that pointer from the wrong
	// thread can crash (see https://issues.chromium.org/issues/41487612).
	// nvda_ia2_gecko_backend_destroy's Drop impl deliberately leaks
	// any non-null root_doc_acc by mem::forget'ing it -- mirrors the
	// CComPtr::Detach() fallback the previous C++ destructor used
	// when renderThread_terminate didn't run.
	nvda_ia2_gecko_backend_destroy(this->rustState);
	this->rustState = nullptr;
}

VBufBackend_t* GeckoVBufBackend_t_createInstance(int docHandle, int ID) {
	VBufBackend_t* backend=new GeckoVBufBackend_t(docHandle,ID);
	return backend;
}
