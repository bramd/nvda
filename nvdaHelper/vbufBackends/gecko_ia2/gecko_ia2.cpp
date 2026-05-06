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
	void nvda_ia2_gecko_backend_render(
		void* state,
		void* backend,
		void* buffer,
		int doc_handle,
		int id,
		bool is_root_call,
		int root_id);
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

#ifdef NVDA_HAS_RUST_HELPERS
extern "C" {
	void* nvda_ia2_fill_vbuf(
		void* pacc,
		void* buffer,
		void* parent_node,
		void* previous_node,
		void* pacc_table2,
		int table_id,
		const wchar_t* parent_pres_row_num_ptr,
		size_t parent_pres_row_num_len,
		bool ignore_interactive_unlabelled_graphics,
		void* backend,
		int root_id,
		bool is_chrome);
}

VBufStorage_fieldNode_t* GeckoVBufBackend_t::fillVBuf(
	IAccessible2* pacc,
	VBufStorage_buffer_t* buffer,
	VBufStorage_controlFieldNode_t* parentNode,
	VBufStorage_fieldNode_t* previousNode,
	IAccessibleTable2* paccTable2,
	long tableID,
	const wchar_t* parentPresentationalRowNumber,
	bool ignoreInteractiveUnlabelledGraphics
) {
	nhAssert(buffer); //buffer can't be NULL
	nhAssert(!parentNode||buffer->isNodeInBuffer(parentNode));
	nhAssert(!previousNode||buffer->isNodeInBuffer(previousNode));
	const bool isChrome =
		nvda_ia2_gecko_backend_is_chrome(this->rustState) != 0;
	const size_t presRowLen = parentPresentationalRowNumber
		? wcslen(parentPresentationalRowNumber)
		: 0;
	return static_cast<VBufStorage_fieldNode_t*>(nvda_ia2_fill_vbuf(
		pacc,
		buffer,
		parentNode,
		previousNode,
		paccTable2,
		static_cast<int>(tableID),
		parentPresentationalRowNumber,
		presRowLen,
		ignoreInteractiveUnlabelledGraphics,
		this,
		this->rootID,
		isChrome));
}
#endif


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
	// The backend holds a reference to the root accessible of the document.
	// This must be specifically released here, in the UI thread where it was created.
	// See https://issues.chromium.org/issues/41487612
	nvda_ia2_gecko_backend_render_thread_terminate(this->rustState);
}

void GeckoVBufBackend_t::render(VBufStorage_buffer_t* buffer, int docHandle, int ID, VBufStorage_controlFieldNode_t* oldNode) {
	nvda_ia2_gecko_backend_render(
		this->rustState,
		this,
		buffer,
		docHandle,
		ID,
		oldNode == nullptr,
		this->rootID);
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
