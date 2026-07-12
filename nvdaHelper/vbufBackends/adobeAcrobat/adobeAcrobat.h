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

#ifndef VIRTUALBUFFER_BACKENDS_ADOBEACROBAT_H
#define VIRTUALBUFFER_BACKENDS_ADOBEACROBAT_H

#include <map>
#include <string>
#include <list>
#include <vbufBase/backend.h>
#include <AcrobatAccess.h>

class AdobeAcrobatVBufStorage_controlFieldNode_t;

typedef struct {
	int uniqueId;
	int type;
} TableHeaderInfo;

typedef struct TableInfo_t {
	long tableID{ 0 };
	int curRowNumber{ 0 };
	int curColumnNumber{ 0 };
	// Maps column numbers to remaining row spans.
	std::map<int, int> columnRowSpans;
	// Maps column numbers to table-columnheadercells attribute values.
	std::map<int, std::wstring> columnHeaders;
	// Maps row numbers to table-rowheadercells attribute values.
	std::map<int, std::wstring> rowHeaders;
	// Maps node id strings to TableHeaderInfo.
	std::map<std::wstring, TableHeaderInfo> headersInfo;
	// Lists nodes with explicit headers along with their Headers attribute string.
	std::list<std::pair<AdobeAcrobatVBufStorage_controlFieldNode_t*, std::wstring>> nodesWithExplicitHeaders;
} TableInfo;

class AdobeAcrobatVBufBackend_t: public VBufBackend_t {
	private:

	std::wstring* getPageNum(IPDDomNode* domNode);

	AdobeAcrobatVBufStorage_controlFieldNode_t* fillVBuf(int docHandle, IAccessible* pacc, VBufStorage_buffer_t* buffer,
		AdobeAcrobatVBufStorage_controlFieldNode_t* parentNode, VBufStorage_fieldNode_t* previousNode,
		AdobeAcrobatVBufStorage_controlFieldNode_t* oldNode,
		TableInfo* tableInfo = NULL, std::wstring* pageNum = NULL
	);

	bool isXFA = true;

	IPDDomDocPagination* docPagination = nullptr;

	/* Per-instance Rust state (AcrobatBackendState). Allocated by
	 * acrobat_backend_create() in the constructor and freed by
	 * acrobat_backend_destroy() in the destructor. Homes the live tree in
	 * a Rust storage::Buffer plus the isXFA / docPagination render state.
	 */
	void* rustState = nullptr;

	protected:

	static void CALLBACK renderThread_winEventProcHook(HWINEVENTHOOK hookID, DWORD eventID, HWND hwnd, long objectID, long childID, DWORD threadID, DWORD time);

	virtual void renderThread_initialize();

	virtual void renderThread_terminate();

	/* This backend homes its live tree in a Rust storage::Buffer (in
	 * AcrobatBackendState), so it overrides update() to run the shared
	 * Rust drain/render/merge orchestration against that buffer instead of
	 * the inherited C++ VBufStorage_buffer_t. The base render-thread
	 * machinery reaches this through the now-virtual update().
	 */
	virtual void update();

	/* Vestigial after the Rust flip: update() is overridden and does all
	 * the rendering against the Rust buffer, so this C++ render() (and the
	 * fillVBuf it drives) is no longer on the live path. Kept only to
	 * satisfy the base's pure-virtual render().
	 */
	virtual void render(VBufStorage_buffer_t* buffer, int docHandle, int ID, VBufStorage_controlFieldNode_t* oldNode);

	virtual ~AdobeAcrobatVBufBackend_t();

	public:

	AdobeAcrobatVBufBackend_t(int docHandle, int ID);

	/* Advertises this backend's embedded Rust storage::Buffer so
	 * vbufRemote's read RPCs route through the nvda_vbuf_* u64-key ABI
	 * instead of the legacy C++ storage virtuals. Returns the address of
	 * state.buffer via acrobat_backend_get_buffer(rustState).
	 */
	virtual void* getRustStorageBuffer();

};

#endif
