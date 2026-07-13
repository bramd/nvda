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

#ifndef VIRTUALBUFFER_BACKENDS_LOTUSNOTESRICHTEXT_H
#define VIRTUALBUFFER_BACKENDS_LOTUSNOTESRICHTEXT_H

#include <vbufBase/backend.h>

class lotusNotesRichTextVBufBackend_t: public VBufBackend_t {
	private:

	/* Per-instance Rust state (LotusNotesBackendState). Allocated by
	 * nvda_lotus_notes_backend_create() in the constructor and freed by
	 * nvda_lotus_notes_backend_destroy() in the destructor. Homes the live
	 * tree in a Rust storage::Buffer; the render logic (renderControlContent
	 * + root enumeration) lives in the nvda_lotus_notes crate.
	 */
	void* rustState = nullptr;

	protected:

	static void CALLBACK renderThread_winEventProcHook(HWINEVENTHOOK hookID, DWORD eventID, HWND hwnd, long objectID, long childID, DWORD threadID, DWORD time);

	virtual void renderThread_initialize();

	virtual void renderThread_terminate();

	/* This backend homes its live tree in a Rust storage::Buffer (in
	 * LotusNotesBackendState), so it overrides update() to run the shared
	 * Rust drain/render/merge orchestration against that buffer instead of
	 * the inherited C++ VBufStorage_buffer_t.
	 */
	virtual void update();

	virtual ~lotusNotesRichTextVBufBackend_t();

	public:

	lotusNotesRichTextVBufBackend_t(int docHandle, int ID);

	/* Advertises this backend's embedded Rust storage::Buffer so
	 * vbufRemote's read RPCs route through the nvda_vbuf_* u64-key ABI
	 * instead of the legacy C++ storage virtuals.
	 */
	virtual void* getRustStorageBuffer();

};

#endif
