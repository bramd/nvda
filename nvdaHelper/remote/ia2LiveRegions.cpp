/*
This file is a part of the NVDA project.
URL: http://www.nvda-project.org/
Copyright 2006-2021 NV Access Limited, Google LLC, Leonard de Ruijter
    This program is free software: you can redistribute it and/or modify
    it under the terms of the GNU General Public License version 2.0, as published by
    the Free Software Foundation.
    This program is distributed in the hope that it will be useful,
    but WITHOUT ANY WARRANTY; without even the implied warranty of
    MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.
This license can be found at:
http://www.gnu.org/licenses/old-licenses/gpl-2.0.html
*/

#include <string>
#include <sstream>
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <atlcomcli.h>
#include <remote/nvdaControllerInternal.h>
#include <common/ia2utils.h>
#include "nvdaHelperRemote.h"
#include "textFromIAccessible.h"

using namespace std;

#ifdef NVDA_HAS_RUST_HELPERS
extern "C" {
	typedef void (*ReportLiveRegionCallback)(
		void* ctx,
		const wchar_t* text_ptr,     size_t text_len,
		const wchar_t* polite_ptr,   size_t polite_len);

	bool nvda_ia2_handle_live_region_event(
		void* pacc2,
		void* hwnd,
		unsigned int event_kind,
		int acc_state,
		void* ctx,
		ReportLiveRegionCallback report_cb);
}

namespace {
	void report_live(void* /*ctx*/,
	                 const wchar_t* text_ptr,   size_t text_len,
	                 const wchar_t* polite_ptr, size_t polite_len) {
		try {
			std::wstring text(text_ptr, text_len);
			std::wstring polite(polite_ptr, polite_len);
			nvdaControllerInternal_reportLiveRegion(text.c_str(), polite.c_str());
		} catch (const std::bad_alloc&) {
			// Suppressed to prevent UB from a C++ exception crossing the
			// extern "C" frame back into Rust.
		}
	}
}

void CALLBACK winEventProcHook(HWINEVENTHOOK hookID, DWORD eventID, HWND hwnd, long objectID, long childID, DWORD threadID, DWORD time) {
	HWND fgHwnd = GetForegroundWindow();
	if (!IsWindowVisible(hwnd) || (hwnd != fgHwnd && !IsChild(fgHwnd, hwnd))) return;

	unsigned int eventKind;
	switch (eventID) {
		case EVENT_OBJECT_NAMECHANGE:        eventKind = 0; break;
		case EVENT_OBJECT_DESCRIPTIONCHANGE: eventKind = 1; break;
		case EVENT_OBJECT_SHOW:              eventKind = 2; break;
		case IA2_EVENT_TEXT_INSERTED:        eventKind = 3; break;
		case IA2_EVENT_TEXT_UPDATED:         eventKind = 4; break;
		default: return;
	}

	CComPtr<IAccessible> pacc;
	CComVariant varChild;
	if (AccessibleObjectFromEvent(hwnd, objectID, childID, &pacc, &varChild) != S_OK) {
		return;
	}

	CComVariant varState;
	pacc->get_accState(varChild, &varState);
	if (varState.vt == VT_I4 && (varState.lVal & STATE_SYSTEM_INVISIBLE)) {
		return;
	}
	int accState = (varState.vt == VT_I4) ? varState.lVal : 0;

	CComQIPtr<IServiceProvider> pserv(pacc);
	if (!pserv) return;
	CComPtr<IAccessible2> pacc2;
	pserv->QueryService(IID_IAccessible, IID_IAccessible2, (void**)(&pacc2));
	if (!pacc2) return;

	nvda_ia2_handle_live_region_event(
		pacc2, hwnd, eventKind, accState,
		nullptr, report_live);
}

#endif

void ia2LiveRegions_inProcess_initialize() {
	registerWinEventHook(winEventProcHook);
}

void ia2LiveRegions_inProcess_terminate() {
	unregisterWinEventHook(winEventProcHook);
}
