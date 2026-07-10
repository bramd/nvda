/*
This file is a part of the NVDA project.
URL: http://www.nvda-project.org/
Copyright 2006-2023 NVDA contributors.
    This program is free software: you can redistribute it and/or modify
    it under the terms of the GNU General Public License version 2.0, as published by
    the Free Software Foundation.
    This program is distributed in the hope that it will be useful,
    but WITHOUT ANY WARRANTY; without even the implied warranty of
    MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.
This license can be found at:
http://www.gnu.org/licenses/old-licenses/gpl-2.0.html
*/

#ifndef VIRTUALBUFFER_BACKENDS_EXAMPLE_H
#define VIRTUALBUFFER_BACKENDS_EXAMPLE_H

#include <vbufBase/backend.h>

class GeckoVBufBackend_t: public VBufBackend_t {
	private:

	void versionSpecificInit(IAccessible2* pacc);

	/* Per-instance Rust state. Allocated by
	 * nvda_ia2_gecko_backend_create() in the constructor and freed
	 * by nvda_ia2_gecko_backend_destroy() in the destructor. Holds
	 * the cached toolkit name (and other Phase 5 state as it
	 * migrates Rust-side).
	 */
	void* rustState;

	bool isRootDocAlive();

	protected:

	static void CALLBACK renderThread_winEventProcHook(HWINEVENTHOOK hookID, DWORD eventID, HWND hwnd, long objectID, long childID, DWORD threadID, DWORD time);

	virtual void renderThread_initialize();

	virtual void renderThread_terminate();

	/* Phase 6e (Stage D): gecko homes its live tree in a Rust
	 * storage::Buffer (embedded in GeckoBackendState), so it overrides
	 * update() to run the Rust drain/render/merge orchestration against
	 * that buffer instead of the inherited C++ VBufStorage_buffer_t. The
	 * base render-thread machinery (timer, forceUpdate,
	 * renderThread_initialize) reaches this through the now-virtual
	 * update().
	 */
	virtual void update();

	/* Vestigial after Stage D: update() is overridden and does all the
	 * rendering, so render() is never on the live path. It stays a
	 * concrete stub only because the base declares it pure-virtual and
	 * the class must remain instantiable. Its former body lives in the
	 * Rust renderer (fill_vbuf) driven by update().
	 */
	virtual void render(VBufStorage_buffer_t* buffer, int docHandle, int ID, VBufStorage_controlFieldNode_t* oldNode=NULL);

	virtual ~GeckoVBufBackend_t();

	public:

	GeckoVBufBackend_t(int docHandle, int ID);

	/* Phase 6e (Stage D): advertises this backend's embedded Rust
	 * storage::Buffer so vbufRemote's read RPCs route through the
	 * nvda_vbuf_* u64-key ABI instead of the legacy C++ storage virtuals.
	 * Returns the address of state.buffer via
	 * nvda_ia2_gecko_backend_get_buffer(rustState).
	 */
	virtual void* getRustStorageBuffer();

};

#endif
