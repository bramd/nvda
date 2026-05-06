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
	void* nvda_ia2_from_identifier(int doc_handle, int id);
}

static IAccessible2* IAccessible2FromIdentifier(int docHandle, int ID) {
	return static_cast<IAccessible2*>(
		nvda_ia2_from_identifier(docHandle, ID));
}
#endif

#ifdef NVDA_HAS_RUST_HELPERS
extern "C" {
	typedef void (*nvda_ia2_toolkit_name_callback)(
		void* ctx,
		const wchar_t* ptr,
		size_t len);

	bool nvda_ia2_get_toolkit_name(
		void* pacc,
		void* ctx,
		nvda_ia2_toolkit_name_callback cb);
}

namespace {
	void toolkitName_cb(void* ctx, const wchar_t* ptr, size_t len) {
		try {
			static_cast<std::wstring*>(ctx)->assign(ptr, len);
		} catch (const std::bad_alloc&) {
			// Suppressed to prevent UB from a C++ exception crossing the
			// extern "C" frame back into Rust.
		}
	}
}

void GeckoVBufBackend_t::versionSpecificInit(IAccessible2* pacc) {
	nvda_ia2_get_toolkit_name(pacc, &this->toolkitName, toolkitName_cb);
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
	const bool isChrome = this->toolkitName.compare(L"Chrome") == 0;
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


#ifdef NVDA_HAS_RUST_HELPERS
extern "C" {
	int nvda_ia2_is_root_doc_alive(void* backend, void* root_doc_acc);
}

bool GeckoVBufBackend_t::isRootDocAlive() {
	const bool alive = nvda_ia2_is_root_doc_alive(this, this->rootDocAcc.p) != 0;
	if (!alive) {
		this->rootDocAcc = nullptr;
	}
	return alive;
}
#endif

void CALLBACK GeckoVBufBackend_t::renderThread_winEventProcHook(HWINEVENTHOOK hookID, DWORD eventID, HWND hwnd, long objectID, long childID, DWORD threadID, DWORD time) {
	switch(eventID) {
		case EVENT_OBJECT_FOCUS:
		case IA2_EVENT_DOCUMENT_LOAD_COMPLETE:
		case EVENT_SYSTEM_ALERT:
		case IA2_EVENT_TEXT_UPDATED:
		case IA2_EVENT_TEXT_INSERTED:
		case IA2_EVENT_TEXT_REMOVED:
		case EVENT_OBJECT_REORDER:
		case EVENT_OBJECT_NAMECHANGE:
		case EVENT_OBJECT_VALUECHANGE:
		case EVENT_OBJECT_DESCRIPTIONCHANGE:
		case EVENT_OBJECT_STATECHANGE:
		case EVENT_OBJECT_SELECTIONADD:
		case EVENT_OBJECT_SELECTIONREMOVE:
		case EVENT_OBJECT_SELECTIONWITHIN:
		case IA2_EVENT_OBJECT_ATTRIBUTE_CHANGED:
		case IA2_EVENT_TEXT_ATTRIBUTE_CHANGED:
		case EVENT_OBJECT_HIDE:
		break;
		default:
		return;
	}
	if(childID>=0||objectID!=OBJID_CLIENT)
		return;
	LOG_DEBUG(L"winEvent for window "<<hwnd);
	if(!hwnd) {
		LOG_DEBUG(L"Invalid window");
		return;
	}
	int docHandle=HandleToUlong(hwnd);
	int ID=childID;
	VBufBackend_t* backend=NULL;
	for(VBufBackendSet_t::iterator i=runningBackends.begin();i!=runningBackends.end();++i) {
		HWND rootWindow=(HWND)UlongToHandle(((*i)->rootDocHandle));
		if(rootWindow==hwnd||IsChild(rootWindow,hwnd))
			backend=(*i);
		else
			continue;
		LOG_DEBUG(L"found active backend for this window at "<<backend);

		//For focus, documentLoadComplete and alert events, force any nodes already marked as invalid  to be updated right now,
		if(
			eventID == EVENT_OBJECT_FOCUS
			|| eventID == IA2_EVENT_DOCUMENT_LOAD_COMPLETE
			|| eventID==EVENT_SYSTEM_ALERT
		) {
			backend->forceUpdate();
			continue;
		}

		//Ignore state change events on the root node (document) as it can cause rerendering when the document goes busy
		if(eventID==EVENT_OBJECT_STATECHANGE&&hwnd==(HWND)UlongToHandle(backend->rootDocHandle)&&childID==backend->rootID)
			return;

		VBufStorage_controlFieldNode_t* node=backend->getControlFieldNodeWithIdentifier(docHandle,ID);
		if(!node)
			continue;

		auto* geckoBackend = static_cast<GeckoVBufBackend_t*>(backend);
		if (!geckoBackend->isRootDocAlive()) {
			// The root doc is dead, but NVDA hasn't realised yet and so hasn't killed
			// this buffer. That means this id is now in a different document! Trying to
			// render this could cause a broken tree. At this point, we may as well
			// clear the buffer.
			LOG_DEBUG(L"Root doc is dead. Clearing buffer.");
			backend->clearBuffer();
			continue;
		}

		if (eventID == EVENT_OBJECT_HIDE) {
			// When an accessible is moved, events are fired as if the accessible were
			// removed and then inserted. The insertion events are fired as if it were
			// a new subtree; i.e. only one insertion for the root of the subtree.
			// This means that if new descendants are inserted at the same time as the
			// root is moved, we don't get specific events for those insertions.
			// Because of that, we mustn't reuse the subtree. Otherwise, we wouldn't
			// walk inside it and thus wouldn't know about the new descendants.
			node->alwaysRerenderDescendants = true;
			// We'll get a text removed event for the parent, so no need to invalidate
			// this node.
			continue;
		}
		backend->invalidateSubtree(node);
	}
}

void GeckoVBufBackend_t::renderThread_initialize() {
	registerWinEventHook(renderThread_winEventProcHook);
	VBufBackend_t::renderThread_initialize();
	this->rootDocAcc = IAccessible2FromIdentifier(this->rootDocHandle, this->rootID);
}

void GeckoVBufBackend_t::renderThread_terminate() {
	unregisterWinEventHook(renderThread_winEventProcHook);
	VBufBackend_t::renderThread_terminate();
	// The backend holds a reference to the root accessible of the document.
	// This must be specifically released here, in the UI thread where it was created.
	// See https://issues.chromium.org/issues/41487612
	if (this->rootDocAcc) {
		this->rootDocAcc.Release();
	}
}

void GeckoVBufBackend_t::render(VBufStorage_buffer_t* buffer, int docHandle, int ID, VBufStorage_controlFieldNode_t* oldNode) {
	IAccessible2* pacc=IAccessible2FromIdentifier(docHandle,ID);
	if(!pacc) {
		LOG_DEBUGWARNING(L"Could not get IAccessible2, returning");
		return;
	}
	if (!oldNode) {
		// This is the root node.
		this->versionSpecificInit(pacc);
	}
	if(!this->fillVBuf(pacc, buffer, nullptr, nullptr)) {
		if(oldNode) {
			LOG_DEBUGWARNING(L"No content rendered in update");
		} else {
			LOG_DEBUGWARNING(L"No initial content rendered");
		}
	}
	pacc->Release();
}

GeckoVBufBackend_t::GeckoVBufBackend_t(int docHandle, int ID): VBufBackend_t(docHandle,ID) {
}

GeckoVBufBackend_t::~GeckoVBufBackend_t() {
	// The backend holds a reference to the root accessible of the document.
	// This must be specifically released in the UI thread where it was created.
	// See https://issues.chromium.org/issues/41487612
	// In most cases this will be released in renderThread_terminate.
	// However in the unlikely case terminate can't run,
	// we must detach and leak the COM pointer here.
	// Otherwise it would be automatically deleted along with the backend which would cause a crash,
	// as the COM object would be released from within an RPC worker thread.
	nhAssert(!rootDocAcc);
	if (this->rootDocAcc) {
		this->rootDocAcc.Detach();
	}
}

VBufBackend_t* GeckoVBufBackend_t_createInstance(int docHandle, int ID) {
	VBufBackend_t* backend=new GeckoVBufBackend_t(docHandle,ID);
	return backend;
}
