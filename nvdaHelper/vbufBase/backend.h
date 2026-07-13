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
#include <common/lock.h>

class VBufBackend_t;

typedef std::set<VBufBackend_t*> VBufBackendSet_t;

/**
 * Drives the render-thread machinery for a virtual buffer. Each backend homes
 * its live tree in a Rust storage::Buffer (reachable via getRustStorageBuffer);
 * this base owns the Win32 scheduling (update timer, destroy hooks,
 * runningBackends, the lock) and thread affinity.
 */
class VBufBackend_t {
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
 * @return the address of the backend's embedded Rust storage::Buffer.
 *
 * Every backend homes its live tree in a Rust storage::Buffer (embedded in its
 * per-backend state struct); vbufRemote's read RPCs route through it, treating
 * the RPC node handles as Rust slotmap keys (u64) passed to the nvda_vbuf_* C
 * ABI. Pure virtual so every backend must supply its buffer. Returned as void*
 * to keep backend.h free of Rust/FFI types; callers cast to the Rust Buffer
 * pointer.
 */
	virtual void* getRustStorageBuffer()=0;

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
