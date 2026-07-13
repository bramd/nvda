/*
This file is a part of the NVDA project.
URL: http://www.nvda-project.org/
Copyright 2007-2016 NV Access Limited
    This program is free software: you can redistribute it and/or modify
    it under the terms of the GNU General Public License version 2.0, as published by
    the Free Software Foundation.
    This program is distributed in the hope that it will be useful,
    but WITHOUT ANY WARRANTY; without even the implied warranty of
    MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.
This license can be found at:
http://www.gnu.org/licenses/old-licenses/gpl-2.0.html
*/

#ifndef VIRTUALBUFFER_BACKEND_H
#define VIRTUALBUFFER_BACKEND_H

#include <set>
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include "storage.h"
#include <common/lock.h>

class VBufBackend_t;

typedef std::set<VBufBackend_t*> VBufBackendSet_t;

/**
 * Renders content in to a storage buffer for linea access.
 */
class VBufBackend_t  : public VBufStorage_buffer_t {
	private:

/**
 * A callback to handle windows being destroyed.
 */
static LRESULT CALLBACK destroy_callWndProcHook(int code, WPARAM wParam, LPARAM lParam);

/**
 * The ID of the current timer for this backend.
 */
	UINT_PTR renderThreadTimerID;

/**
 * A timer callback that will rerender invalid subtrees
 */
	static void CALLBACK renderThread_timerProc(HWND hwnd, UINT msg, UINT_PTR timerID, DWORD time);

/**
 * A winEvent callback that will watch for destroy of a backend's root window and clear the backend.
 */
	static void CALLBACK renderThread_winEventProcHook(HWINEVENTHOOK hookID, DWORD eventID, HWND hwnd, long objectID, long childID, DWORD threadID, DWORD time);

	protected:

/**
 * The set of currently running backends
 */
	static VBufBackendSet_t runningBackends;

/**
 * The thread ID of the rendering thread
 */
	const int renderThreadID;

/**
 * Cancels any pending request to update invalid nodes.
 */
	void cancelPendingUpdate();

/**
 * Sets up any code in the render thread
 */
	virtual void renderThread_initialize();

/**
 * Terminates any code in the render thread
 */
	virtual void renderThread_terminate();

/**
 * Renders content starting from the given doc handle and ID, in to the given buffer.
 * The buffer will always start off empty as even for subtree re-rendering, a temp buffer is provided.
 * @param buffer the buffer to render content in.
 * @param docHandle the doc handle to start from
 * @param ID the ID to start from.
 * @param oldNode an optional node that will be replaced by the rendered content (useful for retreaving cached data)
 */
	virtual void render(VBufStorage_buffer_t* buffer, int docHandle, int ID, VBufStorage_controlFieldNode_t* oldNode=NULL)=0;

/**
 * Updates the content of the buffer. Pure virtual: every backend homes its
 * live tree in a Rust storage::Buffer and provides its own drain/render/merge
 * orchestration, driven through the base render-thread machinery (timer proc,
 * renderThread_initialize, forceUpdate). The base no longer has a C++-storage
 * fallback implementation.
 */
	virtual void update()=0;

/**
 * Destructor, (protected as you must use the destroy method).
 */
	virtual ~VBufBackend_t();

	public:

/**
 * constructor
 * @param docHandle uniquely identifies the document or window containing the content to ve rendered
 * @param ID uniquely identifies where to start rendering from in the document or window
 * @param storageBuffer the storage buffer to render the content in
 */
	VBufBackend_t(int docHandle, int ID);

/**
 * Initializes the state of the backend and performs an initial rendering of content.
 */
	virtual void initialize();

/**
 * identifies the window or document where the backend starts rendering from
 */
	const int rootDocHandle;

/**
 * Represents the ID in the window or document where the backend starts rendering
 */
	const int rootID;

/**
 * Forces any invalidated nodes to be updated right now.
 */
	virtual void forceUpdate();

/**
 * Requests that the backend should update any invalid nodes when it can in the next little while.
 * Public so that a backend which invalidates its subtrees outside the C++ storage (e.g. the gecko_ia2 backend,
 * whose Rust-side WinEvent dispatch invalidates the Rust storage::Buffer under Phase 6e) can arm the render-thread
 * timer via the c_shim without breaking encapsulation of the render-thread machinery.
 */
	void requestUpdate();

/**
 * Clears the content of the backend and terminates any code used for rendering.
 */
	virtual void terminate();

/**
 * Destructs and deletes the backend. Must be used rather than delete as this will handle crossing CRT boundaries.
 */
	virtual void destroy();

 /**
 * Useful for cerializing access to the buffer
 */
	LockableObject lock;

/**
 * @return the backend's Rust storage::Buffer when this backend homes its live tree in Rust rather than in the C++
 * VBufStorage_buffer_t, or NULL when the backend uses C++ storage.
 *
 * Phase 6e contract: the gecko_ia2 backend renders into, and reads out of, a Rust storage::Buffer (embedded in its
 * GeckoBackendState) instead of the inherited C++ storage. vbufRemote's read RPCs branch on this accessor: a non-null
 * result means node handles for this buffer are Rust slotmap keys (u64) to be routed through the nvda_vbuf_* C ABI,
 * while NULL means the legacy path (a narrowed VBufStorage_fieldNode_t* through the C++ storage virtuals). The base
 * implementation returns NULL so every existing backend keeps the C++ storage with no change; only gecko_ia2 overrides
 * this. Returned as void* to keep backend.h free of Rust/FFI types; callers cast to the Rust Buffer pointer.
 */
	virtual void* getRustStorageBuffer() { return nullptr; }

};

/**
 * a function signature for the VBufBackend_create factory function all backend libraries must implement to create a backend.
 */
typedef VBufBackend_t*(*VBufBackend_create_proc)(int,int);

// The backend creation functions
VBufBackend_t* AdobeAcrobatVBufBackend_t_createInstance(int docHandle, int ID);
VBufBackend_t* GeckoVBufBackend_t_createInstance(int docHandle, int ID);
VBufBackend_t* lotusNotesRichTextVBufBackend_t_createInstance(int docHandle, int ID);
VBufBackend_t* MshtmlVBufBackend_t_createInstance(int docHandle, int ID);
VBufBackend_t* WebKitVBufBackend_t_createInstance(int docHandle, int ID);

#endif
