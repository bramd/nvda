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

#include <map>
#include <cwchar>
#include <remote/vbufRemote.h>
#include <vbufBase/backend.h>
#include "dllmain.h"
#include <common/log.h>

using namespace std;

// Phase 6e: read externs over the Rust nvda_vbuf storage.
//
// For a gecko_ia2 backend, backend->getRustStorageBuffer() returns the address of the backend's embedded Rust
// storage::Buffer; the RPC node handle (VBufRemote_nodeHandle_t, an unsigned hyper == u64) then carries a Rust slotmap
// key verbatim rather than a narrowed VBufStorage_fieldNode_t*. Each read RPC below branches on that accessor: non-null
// routes through these nvda_vbuf_* functions (u64 keys, 0 == none), null takes the unchanged legacy C++ virtual call.
// The buffer is passed as an opaque void*; the direction wire value (0 forward, 1 back, 2 up) is identical between
// VBufStorage_findDirection_t and the Rust FindDirection encoding. OUT params are only written on success.
extern "C" {
	// OUT-string delivery for get-text-in-range: invoked once, with a UTF-16 (ptr,len) range valid only for the call.
	typedef void(*NvdaVbufStringCallback)(void* ctx, const wchar_t* ptr, size_t len);

	int nvda_vbuf_buffer_field_node_offsets(const void* buffer, unsigned long long key, int* outStart, int* outEnd);
	int nvda_vbuf_buffer_is_field_node_at_offset(const void* buffer, unsigned long long key, int offset);
	unsigned long long nvda_vbuf_buffer_locate_text_field_node_at_offset(const void* buffer, int offset, int* outStart, int* outEnd);
	unsigned long long nvda_vbuf_buffer_locate_control_field_node_at_offset(const void* buffer, int offset, int* outStart, int* outEnd, int* outDocHandle, int* outID);
	unsigned long long nvda_vbuf_buffer_get_control_field_node_with_identifier(const void* buffer, int docHandle, int id);
	int nvda_vbuf_node_identifier(const void* buffer, unsigned long long key, int* outDocHandle, int* outID);
	unsigned long long nvda_vbuf_buffer_find_node_by_attributes(const void* buffer, int offset, int direction, const wchar_t* attribsPtr, size_t attribsLen, const wchar_t* regexpPtr, size_t regexpLen, int* outStart, int* outEnd);
	int nvda_vbuf_buffer_get_selection_offsets(const void* buffer, int* outStart, int* outEnd);
	int nvda_vbuf_buffer_set_selection_offsets(void* buffer, int startOffset, int endOffset);
	int nvda_vbuf_buffer_text_length(const void* buffer);
	int nvda_vbuf_buffer_get_text_in_range(const void* buffer, int startOffset, int endOffset, int useMarkup, void* ctx, NvdaVbufStringCallback cb);
	int nvda_vbuf_buffer_line_offsets(const void* buffer, int offset, int maxLineLength, int useScreenLayout, int* outStart, int* outEnd);
}

// getTextInRange OUT-string shim: allocate a BSTR for the delivered text. A zero-length result leaves the BSTR null,
// so the RPC preserves the C++ contract of returning false (with no allocation) for an empty range.
static void vbufRemote_getTextInRange_stringCallback(void* ctx, const wchar_t* ptr, size_t len) {
	if (len == 0) return;
	*(BSTR*)ctx = SysAllocStringLen(ptr, (UINT)len);
}

const map<wstring,VBufBackend_create_proc> VBufBackendFactoryMap {
	{L"adobeAcrobat",AdobeAcrobatVBufBackend_t_createInstance},
	{L"gecko_ia2",GeckoVBufBackend_t_createInstance},
	{L"mshtml",MshtmlVBufBackend_t_createInstance},
	{L"lotusNotesRichText",lotusNotesRichTextVBufBackend_t_createInstance},
	{L"webKit",WebKitVBufBackend_t_createInstance}
};

extern "C" {

VBufRemote_bufferHandle_t VBufRemote_createBuffer(handle_t bindingHandle, int docHandle, int ID, const wchar_t* backendName) {
	if(!backendName) {
		LOG_ERROR(L"backendName is NULL");
		return nullptr;
	}
	auto i=VBufBackendFactoryMap.find(backendName);
	if(i==VBufBackendFactoryMap.end()) {
		LOG_ERROR(L"Unknown backend: "<<backendName);
		return nullptr;
	}
	VBufBackend_create_proc createBackend=i->second;
	VBufBackend_t* backend=createBackend(docHandle,ID);
	if(backend==NULL) {
		return NULL;
	}
	backend->initialize();
	// Stop nvdaHelperRemote from being unloaded while a backend exists.
	HINSTANCE tempHandle=nullptr;
	if(!GetModuleHandleEx(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS,reinterpret_cast<LPCWSTR>(dllHandle),&tempHandle)) {
		LOG_ERROR(L"Could not keep nvdaHelperRemote loaded for backend!");
	}
	return (VBufRemote_bufferHandle_t)backend;
}

void VBufRemote_destroyBuffer(VBufRemote_bufferHandle_t* buffer) {
	#ifndef NDEBUG
	Beep(4000,80);
	#endif
	VBufBackend_t* backend=(VBufBackend_t*)*buffer;
	backend->terminate();
	backend->lock.acquire();
	backend->destroy();
	FreeLibrary(dllHandle);
	*buffer=NULL;
}

int VBufRemote_getFieldNodeOffsets(VBufRemote_bufferHandle_t buffer, VBufRemote_nodeHandle_t node, int *startOffset, int *endOffset) {
	VBufBackend_t* backend=(VBufBackend_t*)buffer;
	backend->lock.acquire();
	int res;
	if(void* rustBuffer=backend->getRustStorageBuffer()) {
		res=nvda_vbuf_buffer_field_node_offsets(rustBuffer,node,startOffset,endOffset);
	} else {
		VBufStorage_fieldNode_t* realNode=(VBufStorage_fieldNode_t*)node;
		res=backend->getFieldNodeOffsets(realNode,startOffset,endOffset);
	}
	backend->lock.release();
	return res;
}

int VBufRemote_isFieldNodeAtOffset(VBufRemote_bufferHandle_t buffer, VBufRemote_nodeHandle_t node, int offset) {
	VBufBackend_t* backend=(VBufBackend_t*)buffer;
	backend->lock.acquire();
	int res;
	if(void* rustBuffer=backend->getRustStorageBuffer()) {
		res=nvda_vbuf_buffer_is_field_node_at_offset(rustBuffer,node,offset);
	} else {
		VBufStorage_fieldNode_t* realNode=(VBufStorage_fieldNode_t*)node;
		res=backend->isFieldNodeAtOffset(realNode,offset);
	}
	backend->lock.release();
	return res;
}

int VBufRemote_locateTextFieldNodeAtOffset(VBufRemote_bufferHandle_t buffer, int offset, int *nodeStartOffset, int *nodeEndOffset, VBufRemote_nodeHandle_t* foundNode) {
	VBufBackend_t* backend=(VBufBackend_t*)buffer;
	backend->lock.acquire();
	if(void* rustBuffer=backend->getRustStorageBuffer())
		*foundNode=nvda_vbuf_buffer_locate_text_field_node_at_offset(rustBuffer,offset,nodeStartOffset,nodeEndOffset);
	else
		*foundNode=(VBufRemote_nodeHandle_t)(backend->locateTextFieldNodeAtOffset(offset,nodeStartOffset,nodeEndOffset));
	backend->lock.release();
	return (*foundNode)!=NULL;
}

int VBufRemote_locateControlFieldNodeAtOffset(VBufRemote_bufferHandle_t buffer, int offset, int *nodeStartOffset, int *nodeEndOffset, int *docHandle, int *ID, VBufRemote_nodeHandle_t* foundNode) {
	VBufBackend_t* backend=(VBufBackend_t*)buffer;
	backend->lock.acquire();
	if(void* rustBuffer=backend->getRustStorageBuffer())
		*foundNode=nvda_vbuf_buffer_locate_control_field_node_at_offset(rustBuffer,offset,nodeStartOffset,nodeEndOffset,docHandle,ID);
	else
		*foundNode=(VBufRemote_nodeHandle_t)(backend->locateControlFieldNodeAtOffset(offset,nodeStartOffset,nodeEndOffset,docHandle,ID));
	backend->lock.release();
	return (*foundNode)!=0;
}

int VBufRemote_getControlFieldNodeWithIdentifier(VBufRemote_bufferHandle_t buffer, int docHandle, int ID, VBufRemote_nodeHandle_t* foundNode) {
	VBufBackend_t* backend=(VBufBackend_t*)buffer;
	backend->lock.acquire();
	if(void* rustBuffer=backend->getRustStorageBuffer())
		*foundNode=nvda_vbuf_buffer_get_control_field_node_with_identifier(rustBuffer,docHandle,ID);
	else
		*foundNode=(VBufRemote_nodeHandle_t)(backend->getControlFieldNodeWithIdentifier(docHandle,ID));
	backend->lock.release();
	return (*foundNode)!=0;
}

int VBufRemote_getIdentifierFromControlFieldNode(VBufRemote_bufferHandle_t buffer, VBufRemote_nodeHandle_t node, int* docHandle, int* ID) {
	VBufBackend_t* backend=(VBufBackend_t*)buffer;
	backend->lock.acquire();
	int res;
	if(void* rustBuffer=backend->getRustStorageBuffer())
		res=nvda_vbuf_node_identifier(rustBuffer,node,docHandle,ID);
	else
		res=backend->getIdentifierFromControlFieldNode((VBufStorage_controlFieldNode_t*)node,docHandle,ID);
	backend->lock.release();
	return res;
}

int VBufRemote_findNodeByAttributes(VBufRemote_bufferHandle_t buffer, int offset, int direction, const wchar_t* attribs, const wchar_t* regexp, int *startOffset, int *endOffset, VBufRemote_nodeHandle_t* foundNode) {
	VBufBackend_t* backend=(VBufBackend_t*)buffer;
	backend->lock.acquire();
	if(void* rustBuffer=backend->getRustStorageBuffer())
		*foundNode=nvda_vbuf_buffer_find_node_by_attributes(rustBuffer,offset,direction,attribs,wcslen(attribs),regexp,wcslen(regexp),startOffset,endOffset);
	else
		*foundNode=(VBufRemote_nodeHandle_t)(backend->findNodeByAttributes(offset,(VBufStorage_findDirection_t)direction,attribs,regexp,startOffset,endOffset));
	backend->lock.release();
	return (*foundNode)!=0;
}

int VBufRemote_getSelectionOffsets(VBufRemote_bufferHandle_t buffer, int *startOffset, int *endOffset) {
	VBufBackend_t* backend=(VBufBackend_t*)buffer;
	backend->lock.acquire();
	int res;
	if(void* rustBuffer=backend->getRustStorageBuffer())
		res=nvda_vbuf_buffer_get_selection_offsets(rustBuffer,startOffset,endOffset);
	else
		res=backend->getSelectionOffsets(startOffset,endOffset);
	backend->lock.release();
	return res;
}

int VBufRemote_setSelectionOffsets(VBufRemote_bufferHandle_t buffer, int startOffset, int endOffset) {
	VBufBackend_t* backend=(VBufBackend_t*)buffer;
	backend->lock.acquire();
	int res;
	if(void* rustBuffer=backend->getRustStorageBuffer())
		res=nvda_vbuf_buffer_set_selection_offsets(rustBuffer,startOffset,endOffset);
	else
		res=backend->setSelectionOffsets(startOffset,endOffset);
	backend->lock.release();
	return res;
}

int VBufRemote_getTextLength(VBufRemote_bufferHandle_t buffer) {
	VBufBackend_t* backend=(VBufBackend_t*)buffer;
	backend->lock.acquire();
	int res;
	if(void* rustBuffer=backend->getRustStorageBuffer())
		res=nvda_vbuf_buffer_text_length(rustBuffer);
	else
		res=backend->getTextLength();
	backend->lock.release();
	return res;
}

int VBufRemote_getTextInRange(VBufRemote_bufferHandle_t buffer, int startOffset, int endOffset, wchar_t** text, boolean useMarkup) {
	VBufBackend_t* backend=(VBufBackend_t*)buffer;
	backend->lock.acquire();
	if(void* rustBuffer=backend->getRustStorageBuffer()) {
		BSTR result=nullptr;
		nvda_vbuf_buffer_get_text_in_range(rustBuffer,startOffset,endOffset,useMarkup!=false,&result,vbufRemote_getTextInRange_stringCallback);
		backend->lock.release();
		// An empty (or failed) range leaves result null: preserve the C++ empty-string-returns-false contract.
		if(result==nullptr) {
			return false;
		}
		*text=result;
		return true;
	}
	 wstring textString;
	 backend->getTextInRange(startOffset,endOffset,textString,useMarkup!=false);
	backend->lock.release();
	if(textString.empty()) {
		return false;
	}
	*text=SysAllocString(textString.c_str());
	return true;
}

int VBufRemote_getLineOffsets(VBufRemote_bufferHandle_t buffer, int offset, int maxLineLength, boolean useScreenLayout, int *startOffset, int *endOffset) {
	VBufBackend_t* backend=(VBufBackend_t*)buffer;
	backend->lock.acquire();
	int res;
	if(void* rustBuffer=backend->getRustStorageBuffer())
		res=nvda_vbuf_buffer_line_offsets(rustBuffer,offset,maxLineLength,useScreenLayout!=false,startOffset,endOffset);
	else
		res=backend->getLineOffsets(offset,maxLineLength,useScreenLayout!=false,startOffset,endOffset);
	backend->lock.release();
	return res;
}

//Special cleanup method for VBufRemote when client is lost
void __RPC_USER VBufRemote_bufferHandle_t_rundown(VBufRemote_bufferHandle_t buffer) {
	VBufRemote_destroyBuffer(&buffer);
}

}
