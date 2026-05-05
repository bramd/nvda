/*
This file is a part of the NVDA project.
Copyright 2006-2021 NV Access Limited
	This program is free software: you can redistribute it and/or modify
	it under the terms of the GNU General Public License version 2.0, as published by
	the Free Software Foundation.
	This program is distributed in the hope that it will be useful,
	but WITHOUT ANY WARRANTY; without even the implied warranty of
	MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.
This license can be found at:
http://www.gnu.org/licenses/old-licenses/gpl-2.0.html
*/

#include "textFromIAccessible.h"
#include <string>
#include <vector>
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <atlcomcli.h>
#include <ia2.h>
#include <common/ia2utils.h>

using namespace std;
auto constexpr OBJ_REPLACEMENT_CHAR = L'\xfffc';

#ifdef NVDA_HAS_RUST_HELPERS
extern "C" {
	typedef void (*AppendCharsCallback)(
		void* ctx,
		const wchar_t* ptr,
		size_t len);

	bool nvda_ia2_get_text_from_iaccessible(
		void* pacc2,
		bool use_new_text,
		bool recurse,
		bool include_top_level_text,
		void* ctx,
		AppendCharsCallback cb);
}

namespace {
	void append_chars(void* ctx, const wchar_t* ptr, size_t len) {
		try {
			static_cast<std::wstring*>(ctx)->append(ptr, len);
		} catch (const std::bad_alloc&) {
			// Suppressed to prevent UB from a C++ exception crossing the
			// extern "C" frame back into Rust. The caller receives a
			// partially-populated text buffer; the Rust shim still returns
			// its computed gotText boolean.
		}
	}
}

bool getTextFromIAccessible(
	wstring& textBuf,
	IAccessible2* pacc2,
	bool useNewText,
	bool recurse,
	bool includeTopLevelText
) {
	return nvda_ia2_get_text_from_iaccessible(
		pacc2, useNewText, recurse, includeTopLevelText,
		&textBuf, append_chars);
}
#endif
