/*
This file is a part of the NVDA project.
URL: http://www.nvda-project.org/
Copyright 2008-2013 NV Access Limited, Aleksey Sadovoy.
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
#include <remote/nvdaHelperRemote.h>
#include <remote/nvdaControllerInternal.h>
#include <vbufBase/backend.h>
#include <common/log.h>
#include "adobeAcrobat.h"

extern "C" {
	// Per-instance Rust state + C-ABI entry points (nvda_acrobat crate).
	void* acrobat_backend_create();
	void acrobat_backend_destroy(void* state);
	void* acrobat_backend_get_buffer(void* state);
	void acrobat_backend_clear_buffer(void* state);
	// Drives the Rust drain/render/merge over the embedded Buffer;
	// returns true when the caller should fire vbufChangeNotify.
	bool acrobat_backend_update(void* state, void* backend);
	// Looks up (docHandle, id) in the Rust Buffer and, if present,
	// invalidates its subtree + arms the update timer.
	void acrobat_backend_invalidate_node(void* state, void* backend, int docHandle, int ID);
}

void CALLBACK AdobeAcrobatVBufBackend_t::renderThread_winEventProcHook(HWINEVENTHOOK hookID, DWORD eventID, HWND hwnd, long objectID, long childID, DWORD threadID, DWORD time) {
	if (eventID != EVENT_OBJECT_STATECHANGE && eventID != EVENT_OBJECT_VALUECHANGE)
		return;
	if (eventID == EVENT_OBJECT_VALUECHANGE && objectID == OBJID_CLIENT && childID == CHILDID_SELF) {
		// This indicates that a new document or page replaces this one.
		// The client will ditch this buffer and create a new one, so there's no point rendering it here.
		return;
	}

	LOG_DEBUG(L"winEvent for window "<<hwnd);

	int docHandle=HandleToUlong(hwnd);
	int ID=(objectID>0)?objectID:childID;
	VBufBackend_t* backend=NULL;
	LOG_DEBUG(L"Searching for backend in collection of "<<runningBackends.size()<<L" running backends");
	for(VBufBackendSet_t::iterator i=runningBackends.begin();i!=runningBackends.end();++i) {
		HWND rootWindow=(HWND)UlongToHandle((*i)->rootDocHandle);
		LOG_DEBUG(L"Comparing backend's root window "<<rootWindow<<L" with window "<<hwnd);
		if(rootWindow==hwnd) {
			backend=(*i);
		}
	}
	if(!backend) {
		LOG_DEBUG(L"No matching backend found");
		return;
	}
	LOG_DEBUG(L"found active backend for this window at "<<backend);

	// The live tree is in the Rust storage::Buffer, so route the node
	// lookup + invalidation there. This hook only matches Acrobat document
	// windows, so the matched backend is always an AdobeAcrobatVBufBackend_t.
	auto* acrobatBackend = static_cast<AdobeAcrobatVBufBackend_t*>(backend);
	acrobat_backend_invalidate_node(acrobatBackend->rustState, backend, docHandle, ID);
}

void AdobeAcrobatVBufBackend_t::renderThread_initialize() {
	registerWinEventHook(renderThread_winEventProcHook);
	LOG_DEBUG(L"Registered win event callback");
	VBufBackend_t::renderThread_initialize();
}

void AdobeAcrobatVBufBackend_t::renderThread_terminate() {
	unregisterWinEventHook(renderThread_winEventProcHook);
	LOG_DEBUG(L"Unregistered winEvent hook");
	// The live tree + docPagination now live in the Rust state; empty the
	// Rust storage::Buffer (the docPagination interface is released when
	// the Rust state is dropped in the destructor).
	acrobat_backend_clear_buffer(this->rustState);
	VBufBackend_t::renderThread_terminate();
}

void AdobeAcrobatVBufBackend_t::update() {
	// Drive the Rust drain/render/merge orchestration over the embedded
	// storage::Buffer. The lock is held across the whole Rust update (so
	// no vbufRemote reader thread materializes a &Buffer while the render
	// thread holds a &mut Buffer); the change-notify fires OUTSIDE the
	// lock, and only when the orchestration reports it took the re-render
	// branch (the base update() skips vbufChangeNotify on the initial
	// render, which acrobat_backend_update preserves by returning false).
	this->lock.acquire();
	const bool shouldNotify = acrobat_backend_update(this->rustState, this);
	this->lock.release();
	if (shouldNotify) {
		nvdaControllerInternal_vbufChangeNotify(this->rootDocHandle, this->rootID);
	}
}

void* AdobeAcrobatVBufBackend_t::getRustStorageBuffer() {
	return acrobat_backend_get_buffer(this->rustState);
}

AdobeAcrobatVBufBackend_t::AdobeAcrobatVBufBackend_t(int docHandle, int ID)
	: VBufBackend_t(docHandle,ID)
	, rustState(acrobat_backend_create())
{
	LOG_DEBUG(L"AdobeAcrobat backend constructor");
}

AdobeAcrobatVBufBackend_t::~AdobeAcrobatVBufBackend_t() {
	LOG_DEBUG(L"AdobeAcrobat backend destructor");
	// Frees the AcrobatBackendState (its Drop releases the docPagination
	// interface + the live storage::Buffer).
	acrobat_backend_destroy(this->rustState);
	this->rustState = nullptr;
}

VBufBackend_t* AdobeAcrobatVBufBackend_t_createInstance(int docHandle, int ID) {
	VBufBackend_t* backend=new AdobeAcrobatVBufBackend_t(docHandle,ID);
	LOG_DEBUG(L"Created new backend at "<<backend);
	return backend;
}
