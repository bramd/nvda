/*
This file is a part of the NVDA project.
URL: http://www.nvda-project.org/
Copyright 2007-2021 NV Access Limited, Mozilla Corporation
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
#include <map>
#include "ia2utils.h"
#include <ia2.h>

using namespace std;

// Forward declarations of the Rust shims (linked from nvda_ia2.lib on
// every NVDA arch that produces nvdaHelperRemote.dll).
#ifdef NVDA_HAS_RUST_HELPERS
extern "C" {
	typedef void (*AttribCallback)(
		void* ctx,
		const wchar_t* key, size_t key_len,
		const wchar_t* val, size_t val_len);

	void nvda_ia2_attribs_to_map(
		const wchar_t* input, size_t input_len,
		void* ctx, AttribCallback cb);

	bool nvda_ia2_fetch_attributes(
		void* pacc2, void* ctx, AttribCallback cb);

	void* nvda_ia2_make_hyperlink_getter(void* pacc2);
	void* nvda_ia2_hyperlink_getter_next(void* handle);
	void  nvda_ia2_hyperlink_getter_free(void* handle);
}

namespace {
	void insert_into_map(
		void* ctx,
		const wchar_t* key, size_t key_len,
		const wchar_t* val, size_t val_len
	) {
		auto& m = *static_cast<std::map<std::wstring, std::wstring>*>(ctx);
		m.emplace(std::wstring(key, key_len), std::wstring(val, val_len));
	}
}

bool fetchIA2Attributes(IAccessible2* pacc2, std::map<std::wstring, std::wstring>& attribsMap) {
	return nvda_ia2_fetch_attributes(pacc2, &attribsMap, insert_into_map);
}

void IA2AttribsToMap(const std::wstring& attribsString, std::map<std::wstring, std::wstring>& attribsMap) {
	nvda_ia2_attribs_to_map(
		attribsString.c_str(),
		attribsString.size(),
		&attribsMap,
		insert_into_map);
}

namespace {
	class RustHyperlinkGetter : public HyperlinkGetter {
		public:
		explicit RustHyperlinkGetter(void* h) : handle(h) {}
		~RustHyperlinkGetter() override {
			if (handle) {
				nvda_ia2_hyperlink_getter_free(handle);
			}
		}
		// No copy, no assign.
		RustHyperlinkGetter(const RustHyperlinkGetter&) = delete;
		RustHyperlinkGetter& operator=(const RustHyperlinkGetter&) = delete;

		CComPtr<IAccessibleHyperlink> next() override {
			CComPtr<IAccessibleHyperlink> link;
			if (!handle) {
				return link;
			}
			auto* raw = static_cast<IAccessibleHyperlink*>(
				nvda_ia2_hyperlink_getter_next(handle));
			// raw is already AddRef'd by the Rust side or null;
			// Attach takes ownership without extra AddRef.
			link.Attach(raw);
			return link;
		}

		private:
		void* handle;
	};
}

std::unique_ptr<HyperlinkGetter> makeHyperlinkGetter(IAccessible2* acc) {
	void* handle = nvda_ia2_make_hyperlink_getter(acc);
	if (!handle) {
		return nullptr;
	}
	return std::make_unique<RustHyperlinkGetter>(handle);
}
#endif

std::pair<std::vector<CComVariant>, HRESULT>
getAccessibleChildren(IAccessible* pacc, long indexOfFirstChild, long maxChildCount) {
	try {
		std::vector<CComVariant> varChildren(maxChildCount);
		const auto res = AccessibleChildren(
			pacc,
			indexOfFirstChild,
			maxChildCount,
			varChildren.data(),
			&maxChildCount
		);
		if (res != S_OK) {
			return std::make_pair(
				std::vector<CComVariant>(0),
				res
			);
		}
		// shrink the vector in case less children were returned.
		// so that varChildren.size() will equal actual filled size
		varChildren.resize(maxChildCount);
		// no need to shrink to fit, make_pair will copy the vector, using only the first varChildren.size() elements.
		return std::make_pair(varChildren, S_OK);
	}
	catch (std::bad_array_new_length&) {
		return std::make_pair(
			std::vector<CComVariant>(0),
			S_FALSE
		);
	}
}
