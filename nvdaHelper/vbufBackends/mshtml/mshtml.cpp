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
#include <oleacc.h>
#include <mshtml.h>
#include <vbufBase/backend.h>
#include <remote/dllmain.h>
#include <remote/nvdaControllerInternal.h>
#include <common/log.h>
#include "mshtml.h"

extern "C" {
	// Per-instance Rust state + C-ABI entry points (nvda_mshtml crate).
	void* mshtml_backend_create();
	void mshtml_backend_destroy(void* state);
	void* mshtml_backend_get_buffer(void* state);
	void mshtml_backend_clear_buffer(void* state);
	// Drives the Rust drain/render/merge over the embedded Buffer;
	// returns true when the caller should fire vbufChangeNotify.
	bool mshtml_backend_update(void* state, void* backend);
	// Phase B change sink: does a control node (docHandle, id) exist in
	// the Rust Buffer? (drives the change sink's getDeepest walk.)
	bool mshtml_backend_has_node(void* state, int docHandle, int ID);
	// Invalidate the subtree covering the dirty range [beginID, endID]
	// (either may be 0) + arm the update timer.
	void mshtml_backend_invalidate_range(void* state, void* backend, int docHandle, int beginID, int endID);
}

// Phase B: a single document-level dirty-range change sink. Registered on
// the root document's IMarkupContainer2; on a DOM mutation it maps the
// dirty range's endpoints to the deepest rendered nodes (querying the Rust
// buffer via mshtml_backend_has_node) and invalidates the covering subtree
// via mshtml_backend_invalidate_range. This replaces the per-node
// CHTMLChangeSink of the dead C++ storage path (node.cpp). The per-node
// property/focus CDispatchChangeSink is intentionally not ported in Phase B.
class MshtmlDocumentChangeSink : public IHTMLChangeSink {
	private:
	ULONG refCount;
	void* rustState;
	void* backend; // VBufBackend_t* for mshtml_backend_invalidate_range
	int rootDocHandle;
	IMarkupContainer2* pMarkupContainer2;
	DWORD cookie;
	IMarkupPointer* pBegin;
	IMarkupPointer* pEnd;

	// Walk an element up its ancestors, returning the unique number of the
	// deepest one that has a rendered node in the Rust buffer, or 0.
	int getDeepestNodeID(IHTMLElement* pEl) {
		bool needRelease = false;
		int found = 0;
		while (pEl) {
			IHTMLUniqueName* pUnique = NULL;
			pEl->QueryInterface(IID_IHTMLUniqueName, (void**)&pUnique);
			int id = 0;
			if (pUnique) {
				pUnique->get_uniqueNumber((long*)&id);
				pUnique->Release();
			}
			if (id != 0 && mshtml_backend_has_node(this->rustState, this->rootDocHandle, id)) {
				found = id;
				break;
			}
			IHTMLElement* parent = NULL;
			pEl->get_parentElement(&parent);
			if (needRelease) pEl->Release();
			pEl = parent;
			needRelease = true;
		}
		// Release the last walked element (the passed-in element is owned
		// by the caller, so it is only released here if we walked past it).
		if (needRelease && pEl) pEl->Release();
		return found;
	}

	public:
	MshtmlDocumentChangeSink(void* backend, void* rustState, int rootDocHandle, IMarkupContainer2* pContainer)
		: refCount(1), rustState(rustState), backend(backend), rootDocHandle(rootDocHandle),
		  pMarkupContainer2(pContainer), cookie(0), pBegin(NULL), pEnd(NULL) {
		this->pMarkupContainer2->AddRef();
		IMarkupServices2* pServices = NULL;
		if (pContainer->QueryInterface(IID_IMarkupServices2, (void**)&pServices) == S_OK) {
			pServices->CreateMarkupPointer(&this->pBegin);
			pServices->CreateMarkupPointer(&this->pEnd);
			pServices->Release();
		}
		incBackendLibRefCount();
	}

	~MshtmlDocumentChangeSink() {
		if (this->pBegin) this->pBegin->Release();
		if (this->pEnd) this->pEnd->Release();
		if (this->pMarkupContainer2) this->pMarkupContainer2->Release();
		decBackendLibRefCount();
	}

	bool registerForDirtyRange() {
		if (this->pMarkupContainer2->RegisterForDirtyRange(this, &this->cookie) != S_OK) {
			this->cookie = 0;
			return false;
		}
		return true;
	}

	void disconnect() {
		if (this->cookie != 0) {
			this->pMarkupContainer2->UnRegisterForDirtyRange(this->cookie);
			this->cookie = 0;
		}
	}

	HRESULT STDMETHODCALLTYPE QueryInterface(REFIID riid, void** ppv) {
		if (!ppv) return E_INVALIDARG;
		*ppv = NULL;
		if (riid == __uuidof(IHTMLChangeSink)) {
			*ppv = static_cast<IHTMLChangeSink*>(this);
		} else if (riid == __uuidof(IUnknown)) {
			*ppv = static_cast<IUnknown*>(this);
		} else {
			return E_NOINTERFACE;
		}
		this->AddRef();
		return S_OK;
	}

	ULONG STDMETHODCALLTYPE AddRef() {
		return ++this->refCount;
	}

	ULONG STDMETHODCALLTYPE Release() {
		nhAssert(this->refCount > 0);
		this->refCount--;
		if (this->refCount == 0) {
			delete this;
			return 0;
		}
		return this->refCount;
	}

	HRESULT STDMETHODCALLTYPE Notify() {
		if (this->cookie == 0) return E_FAIL;
		if (this->pMarkupContainer2->GetAndClearDirtyRange(this->cookie, this->pBegin, this->pEnd) != S_OK) {
			return E_FAIL;
		}
		IHTMLElement* pEl = NULL;
		this->pBegin->CurrentScope(&pEl);
		int beginID = getDeepestNodeID(pEl);
		if (pEl) {
			pEl->Release();
			pEl = NULL;
		}
		this->pEnd->CurrentScope(&pEl);
		int endID = getDeepestNodeID(pEl);
		if (pEl) pEl->Release();
		mshtml_backend_invalidate_range(this->rustState, this->backend, this->rootDocHandle, beginID, endID);
		return S_OK;
	}
};

void incBackendLibRefCount() {
	HMODULE h=NULL;
	BOOL res=GetModuleHandleEx(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS,(LPCTSTR)dllHandle,&h);
	nhAssert(res); //Result of upping backend lib ref count
	LOG_DEBUG(L"Increased  remote lib ref count");
}

void decBackendLibRefCount() {
	BOOL res=FreeLibrary(dllHandle);
	nhAssert(res); //Result of freeing backend lib
	LOG_DEBUG(L"Decreased remote lib ref count");
}

UINT getHTMLWindowMessage() {
	static UINT wm=RegisterWindowMessage(L"WM_HTML_GETOBJECT");
	return wm;
}

void MshtmlVBufBackend_t::render(VBufStorage_buffer_t* buffer, int docHandle, int ID, VBufStorage_controlFieldNode_t* oldNode) {
	// Vestigial after the Rust flip: update() is overridden and performs all
	// rendering against the Rust storage::Buffer (via the nvda_mshtml
	// fill_vbuf renderer + the document change sink), so render() is never
	// reached. It stays a concrete (empty) definition only to satisfy the
	// base's pure-virtual render() and keep the class instantiable.
}

void MshtmlVBufBackend_t::update() {
	// Drive the Rust drain/render/merge orchestration over the embedded
	// storage::Buffer (Phase A). The lock is held across the whole Rust
	// update (so no vbufRemote reader thread materializes a &Buffer while
	// the render thread holds a &mut Buffer); the change-notify fires
	// OUTSIDE the lock, and only when the orchestration reports it took
	// the re-render branch (the base update() skips vbufChangeNotify on
	// the initial render, which mshtml_backend_update preserves by
	// returning false).
	this->lock.acquire();
	const bool shouldNotify = mshtml_backend_update(this->rustState, this);
	this->lock.release();
	// Phase B: after the first render populates the buffer, register the
	// document dirty-range change sink so subsequent DOM mutations
	// invalidate + re-render. Registered once (guarded by docChangeSink).
	if (!this->docChangeSink) {
		this->registerDocumentSink();
	}
	if (shouldNotify) {
		nvdaControllerInternal_vbufChangeNotify(this->rootDocHandle, this->rootID);
	}
}

void* MshtmlVBufBackend_t::getRustStorageBuffer() {
	return mshtml_backend_get_buffer(this->rustState);
}

void MshtmlVBufBackend_t::registerDocumentSink() {
	if (this->docChangeSink) return;
	LRESULT res = SendMessage((HWND)UlongToHandle(this->rootDocHandle), getHTMLWindowMessage(), 0, 0);
	if (res == 0) return;
	IHTMLDocument2* pDoc2 = NULL;
	if (ObjectFromLresult(res, IID_IHTMLDocument2, 0, (void**)&pDoc2) != S_OK || !pDoc2) return;
	IMarkupContainer2* pContainer = NULL;
	pDoc2->QueryInterface(IID_IMarkupContainer2, (void**)&pContainer);
	pDoc2->Release();
	if (!pContainer) return;
	MshtmlDocumentChangeSink* sink = new MshtmlDocumentChangeSink(this, this->rustState, this->rootDocHandle, pContainer);
	pContainer->Release(); // the sink AddRef'd it
	if (!sink->registerForDirtyRange()) {
		sink->Release();
		return;
	}
	this->docChangeSink = sink;
	LOG_DEBUG(L"Registered document dirty-range change sink");
}

void MshtmlVBufBackend_t::unregisterDocumentSink() {
	if (this->docChangeSink) {
		this->docChangeSink->disconnect();
		this->docChangeSink->Release();
		this->docChangeSink = NULL;
	}
}

MshtmlVBufBackend_t::MshtmlVBufBackend_t(int docHandle, int ID): VBufBackend_t(docHandle,ID), rustState(mshtml_backend_create()) {
	LOG_DEBUG(L"Mshtml backend constructor");
}

void MshtmlVBufBackend_t::renderThread_terminate() {
	// Phase B: stop listening for DOM mutations before the document goes
	// away, and empty the Rust storage::Buffer.
	this->unregisterDocumentSink();
	mshtml_backend_clear_buffer(this->rustState);
	VBufBackend_t::renderThread_terminate();
}

MshtmlVBufBackend_t::~MshtmlVBufBackend_t() {
	LOG_DEBUG(L"Mshtml backend destructor");
	this->unregisterDocumentSink();
	// Frees the MshtmlBackendState (its Drop releases the live Buffer).
	mshtml_backend_destroy(this->rustState);
	this->rustState = nullptr;
}

VBufBackend_t* MshtmlVBufBackend_t_createInstance(int docHandle, int ID) {
	VBufBackend_t* backend=new MshtmlVBufBackend_t(docHandle,ID);
	LOG_DEBUG(L"Created new backend at "<<backend);
	return backend;
}
