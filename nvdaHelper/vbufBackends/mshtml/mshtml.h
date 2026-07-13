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

#ifndef VIRTUALBUFFER_BACKENDS_MSHTML_H
#define VIRTUALBUFFER_BACKENDS_MSHTML_H

#include <vbufBase/storage.h>
#include <vbufBase/backend.h>

void incBackendLibRefCount();
void decBackendLibRefCount();

// gets the window message registered by MSHTML which is used to fetch the MSHTML object model from its window.
UINT getHTMLWindowMessage();

class MshtmlDocumentChangeSink;

class MshtmlVBufBackend_t: public VBufBackend_t {
	private:

	/* Per-instance Rust state (MshtmlBackendState). Allocated by
	 * mshtml_backend_create() in the constructor and freed by
	 * mshtml_backend_destroy() in the destructor. Homes the live tree in
	 * a Rust storage::Buffer; the render logic lives in the nvda_mshtml
	 * crate.
	 */
	void* rustState = nullptr;

	/* Phase B: the single document-level dirty-range change sink,
	 * registered on the first render and torn down on
	 * renderThread_terminate / destruction. Drives DOM-mutation re-renders
	 * against the Rust buffer.
	 */
	MshtmlDocumentChangeSink* docChangeSink = nullptr;

	void registerDocumentSink();
	void unregisterDocumentSink();

	protected:

	/* Vestigial after the Rust flip: update() is overridden and does all
	 * the rendering against the Rust buffer (via the nvda_mshtml fill_vbuf
	 * renderer + the document change sink), so this C++ render() is never
	 * on the live path. Kept as an empty stub only to satisfy the base's
	 * pure-virtual render().
	 */
	virtual void render(VBufStorage_buffer_t* buffer, int docHandle, int ID, VBufStorage_controlFieldNode_t* oldNode=NULL);

	/* This backend homes its live tree in a Rust storage::Buffer (in
	 * MshtmlBackendState), so it overrides update() to run the shared
	 * Rust drain/render/merge orchestration against that buffer instead
	 * of the inherited C++ VBufStorage_buffer_t.
	 */
	virtual void update();

	/* Phase B: tear down the change sink + empty the Rust buffer when the
	 * render thread terminates (document going away). */
	virtual void renderThread_terminate();

	virtual ~MshtmlVBufBackend_t();

	public:

	MshtmlVBufBackend_t(int docHandle, int ID);

	/* Advertises this backend's embedded Rust storage::Buffer so
	 * vbufRemote's read RPCs route through the nvda_vbuf_* u64-key ABI
	 * instead of the legacy C++ storage virtuals.
	 */
	virtual void* getRustStorageBuffer();

};

#endif
