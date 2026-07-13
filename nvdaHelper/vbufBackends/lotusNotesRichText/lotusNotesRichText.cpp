/*
This file is a part of the NVDA project.
URL: http://www.nvda-project.org/
Copyright 2006-2010 NVDA contributers.
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
#include "lotusNotesRichText.h"

using namespace std;

extern "C" {
	// Per-instance Rust state + C-ABI entry points (nvda_lotus_notes crate).
	// The live tree lives in an embedded Rust storage::Buffer; the render
	// logic is fill_vbuf.rs.
	void* nvda_lotus_notes_backend_create();
	void nvda_lotus_notes_backend_destroy(void* state);
	void* nvda_lotus_notes_backend_get_buffer(void* state);
	void nvda_lotus_notes_backend_clear_buffer(void* state);
	// Drives the Rust drain/render/merge over the embedded Buffer;
	// returns true when the caller should fire vbufChangeNotify.
	bool nvda_lotus_notes_backend_update(void* state, void* backend);
	// WinEvent hook: outer event filter + per-backend dispatch (invalidate
	// the affected subtree + arm the render timer).
	bool nvda_lotus_notes_backend_win_event_is_relevant(unsigned int event_id);
	void nvda_lotus_notes_backend_dispatch_win_event(void* state, void* backend, int doc_handle, int child_id);
}

void CALLBACK lotusNotesRichTextVBufBackend_t::renderThread_winEventProcHook(HWINEVENTHOOK hookID, DWORD eventID, HWND hwnd, long objectID, long childID, DWORD threadID, DWORD time) {
	if (!nvda_lotus_notes_backend_win_event_is_relevant(eventID)) {
		return;
	}
	const int docHandle = HandleToUlong(hwnd);
	for (auto* backend : runningBackends) {
		HWND rootWindow = (HWND)UlongToHandle(backend->rootDocHandle);
		if (rootWindow != hwnd)
			continue;
		auto* lotusBackend = static_cast<lotusNotesRichTextVBufBackend_t*>(backend);
		nvda_lotus_notes_backend_dispatch_win_event(
			lotusBackend->rustState, backend, docHandle, childID);
		break;
	}
}

void lotusNotesRichTextVBufBackend_t::renderThread_initialize() {
	registerWinEventHook(renderThread_winEventProcHook);
	VBufBackend_t::renderThread_initialize();
}

void lotusNotesRichTextVBufBackend_t::renderThread_terminate() {
	unregisterWinEventHook(renderThread_winEventProcHook);
	VBufBackend_t::renderThread_terminate();
	// The live tree is the Rust storage::Buffer, not the (always-empty) C++
	// storage the base call just cleared; empty the Rust buffer too.
	nvda_lotus_notes_backend_clear_buffer(this->rustState);
}

void lotusNotesRichTextVBufBackend_t::update() {
	// Drive the Rust drain/render/merge orchestration over the embedded
	// storage::Buffer. The lock is held across the whole Rust update (so no
	// vbufRemote reader thread materializes a &Buffer while the render
	// thread holds a &mut Buffer); the change-notify fires OUTSIDE the lock,
	// and only when the orchestration reports it took the re-render branch
	// (the base update() skips vbufChangeNotify on the initial render, which
	// nvda_lotus_notes_backend_update preserves by returning false).
	this->lock.acquire();
	const bool shouldNotify = nvda_lotus_notes_backend_update(this->rustState, this);
	this->lock.release();
	if (shouldNotify) {
		nvdaControllerInternal_vbufChangeNotify(this->rootDocHandle, this->rootID);
	}
}

void* lotusNotesRichTextVBufBackend_t::getRustStorageBuffer() {
	return nvda_lotus_notes_backend_get_buffer(this->rustState);
}

lotusNotesRichTextVBufBackend_t::lotusNotesRichTextVBufBackend_t(int docHandle, int ID): VBufBackend_t(docHandle,ID), rustState(nvda_lotus_notes_backend_create()) {
}

lotusNotesRichTextVBufBackend_t::~lotusNotesRichTextVBufBackend_t() {
	// Frees the LotusNotesBackendState (its Drop releases the live Buffer).
	nvda_lotus_notes_backend_destroy(this->rustState);
	this->rustState = nullptr;
}

VBufBackend_t* lotusNotesRichTextVBufBackend_t_createInstance(int docHandle, int ID) {
	VBufBackend_t* backend=new lotusNotesRichTextVBufBackend_t(docHandle,ID);
	return backend;
}
