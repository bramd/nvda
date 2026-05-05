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

// Forward declarations of the Rust shims (linked from nvda_ia2.lib on x86_64).
// Non-x86_64 builds do not link nvda_ia2 -- those builds use the C++ fallback
// at the bottom of this file (guarded by `#ifndef _M_X64`).
#ifdef _M_X64
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
#else
// Non-x86_64 fallback: keep the original C++ implementations because cargo
// only produces a host-triple staticlib. This is the same code as before
// this PR, kept verbatim. Multi-arch cargo builds are a future exercise.

bool fetchIA2Attributes(IAccessible2* pacc2, map<wstring, wstring>& attribsMap) {
	BSTR attribs = NULL;
	pacc2->get_attributes(&attribs);
	if (!attribs) {
		return false;
	}
	IA2AttribsToMap(attribs, attribsMap);
	SysFreeString(attribs);
	return true;
}

void IA2AttribsToMap(const wstring &attribsString, map<wstring, wstring> &attribsMap) {
	wstring str, key;
	bool inEscape = false;

	for (wstring::const_iterator it = attribsString.begin(); it != attribsString.end(); ++it) {
		if (inEscape) {
			str.push_back(*it);
			inEscape = false;
		} else if (*it == L'\\') {
			inEscape = true;
		} else if (*it == L':') {
			// We're about to move on to the value, so save the key and clear str.
			key = str;
			str.clear();
		} else if (*it == L';') {
			// We're about to move on to a new attribute.
			// Add this key/value pair to the map.
			if (!key.empty())
				attribsMap[key] = str;
				key.clear();
			str.clear();
		} else {
			str.push_back(*it);
		}
	}
	// If there was no trailing semi-colon, we need to handle the last attribute.
	if (!key.empty())
		attribsMap[key] = str;
	// Truncate the value of "src" if it contains base64 data
	map<wstring,wstring>::const_iterator attribsMapIt;
	if ((attribsMapIt = attribsMap.find(L"src")) != attribsMap.end()) {
		str = attribsMapIt->second;
		const wstring prefix = L"data:";
		if (str.substr(0, prefix.length()) == prefix) {
			const wstring needle = L"base64,";
			wstring::size_type pos = str.find(needle);
			if (pos != wstring::npos) {
				str.replace(pos + needle.length(), wstring::npos, L"<truncated>");
				attribsMap[L"src"] = str;
			}
		}
	}
}

namespace {
	class HtHyperlinkGetter : public HyperlinkGetter {
		public:
		HtHyperlinkGetter(CComPtr<IAccessibleHypertext> hypertext)
			: hypertext(hypertext) {}
		CComPtr<IAccessibleHyperlink> next() override;

		private:
		CComPtr<IAccessibleHypertext> hypertext;
		long index = 0;
	};

	class Ht2HyperlinkGetter : public HyperlinkGetter {
		public:
		Ht2HyperlinkGetter(CComPtr<IAccessibleHypertext2> hypertext)
			: hypertext(hypertext), count(-1) {}
		~Ht2HyperlinkGetter() override {
			if (this->rawLinks) {
				CoTaskMemFree(this->rawLinks);
			}
		}
		CComPtr<IAccessibleHyperlink> next() override;

		private:
		CComPtr<IAccessibleHypertext2> hypertext;
		IAccessibleHyperlink** rawLinks = nullptr;
		long count;
		long index = 0;
		void maybeFetch();
	};

	CComPtr<IAccessibleHyperlink> HtHyperlinkGetter::next() {
		CComPtr<IAccessibleHyperlink> link;
		// hyperlink will fail or return null if the index is too big.
		HRESULT res = this->hypertext->get_hyperlink(this->index, &link);
		++this->index;
		if (FAILED(res) || !link) {
			return nullptr;
		}
		return link;
	}

	void Ht2HyperlinkGetter::maybeFetch() {
		if (this->count >= 0) {
			return;
		}
		if (FAILED(hypertext->get_hyperlinks(&this->rawLinks, &this->count))) {
			this->count = 0;
		}
	}

	CComPtr<IAccessibleHyperlink> Ht2HyperlinkGetter::next() {
		this->maybeFetch();
		if (this->index >= this->count) {
			return nullptr;
		}
		// Ensure we don't AddRef this pointer.
		CComPtr<IAccessibleHyperlink> link;
		link.Attach(this->rawLinks[this->index]);
		++this->index;
		return link;
	}
}

std::unique_ptr<HyperlinkGetter> makeHyperlinkGetter(IAccessible2* acc) {
	// Try IAccessibleHypertext2 first.
	CComQIPtr<IAccessibleHypertext2> ht2 = acc;
	if (ht2) {
		return std::make_unique<Ht2HyperlinkGetter>(ht2);
	}
	// Fall back to IAccessibleHypertext.
	CComQIPtr<IAccessibleHypertext> ht = acc;
	if (ht) {
		return std::make_unique<HtHyperlinkGetter>(ht);
	}
	// Neither interface is supported.
	return nullptr;
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
