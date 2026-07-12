/*
Thin `extern "C"` shim over the C++ `VBufStorage_buffer_t` class, used ONLY
by the `vbuf_bench` criterion harness to drive the original storage on the
same synthetic workloads as the Rust `nvda_vbuf::storage::Buffer`.

This deliberately does NOT reuse `nvdaHelper/vbufBase/c_shim.cpp`: that shim
references `VBufBackend_t`, which drags in `backend.cpp` and its RPC / COM
dependencies. Here we only need the buffer's public storage API (see
`nvdaHelper/vbufBase/storage.h`).

Node handles cross the boundary as opaque `void*` (real
`VBufStorage_fieldNode_t*` / `_controlFieldNode_t*`). Strings are UTF-16
`(ptr, len)` pairs; on Windows MSVC `wchar_t` is 16-bit, matching Rust `u16`.
*/

#include <cstddef>
#include <map>
#include <string>

#include <vbufBase/storage.h>

// LOGLEVEL=60 (LOGLEVEL_NONE) makes every LOG_* macro in storage.cpp a
// no-op, so `logMessage` is never referenced. Define a no-op anyway so the
// benchmark links even if a stray reference sneaks in.
void logMessage(int, const wchar_t*) {}

extern "C" {

void* vbench_buffer_create() {
	return new VBufStorage_buffer_t();
}

void vbench_buffer_destroy(void* buf) {
	delete static_cast<VBufStorage_buffer_t*>(buf);
}

void* vbench_add_control(
	void* buf,
	void* parent,
	void* previous,
	int docHandle,
	int id,
	int isBlock
) {
	return static_cast<VBufStorage_buffer_t*>(buf)->addControlFieldNode(
		static_cast<VBufStorage_controlFieldNode_t*>(parent),
		static_cast<VBufStorage_fieldNode_t*>(previous),
		docHandle,
		id,
		isBlock != 0
	);
}

void* vbench_add_text(
	void* buf,
	void* parent,
	void* previous,
	const wchar_t* text,
	size_t len
) {
	std::wstring s(text, len);
	return static_cast<VBufStorage_buffer_t*>(buf)->addTextFieldNode(
		static_cast<VBufStorage_controlFieldNode_t*>(parent),
		static_cast<VBufStorage_fieldNode_t*>(previous),
		s
	);
}

void vbench_node_add_attribute(
	void* node,
	const wchar_t* name,
	size_t nlen,
	const wchar_t* val,
	size_t vlen
) {
	std::wstring n(name, nlen);
	std::wstring v(val, vlen);
	static_cast<VBufStorage_fieldNode_t*>(node)->addAttribute(n, v);
}

int vbench_get_text_length(void* buf) {
	return static_cast<VBufStorage_buffer_t*>(buf)->getTextLength();
}

// Builds the (optionally marked-up) text into a std::wstring and returns its
// length. We deliberately do not marshal the string back across the FFI --
// building the wstring is the work we want to measure, and returning the
// length keeps the boundary allocation-free and symmetric with the Rust
// side (which fills a Vec<u16> and reports its len()).
int vbench_get_text_in_range(void* buf, int start, int end, int useMarkup) {
	std::wstring out;
	static_cast<VBufStorage_buffer_t*>(buf)->getTextInRange(
		start, end, out, useMarkup != 0
	);
	return static_cast<int>(out.length());
}

// Returns 1 if a matching node was found, else 0.
int vbench_find_node_by_attributes(
	void* buf,
	int offset,
	int direction,
	const wchar_t* attribs,
	size_t alen,
	const wchar_t* regexp,
	size_t rlen
) {
	std::wstring a(attribs, alen);
	std::wstring r(regexp, rlen);
	int startOffset = 0;
	int endOffset = 0;
	VBufStorage_fieldNode_t* node =
		static_cast<VBufStorage_buffer_t*>(buf)->findNodeByAttributes(
			offset,
			static_cast<VBufStorage_findDirection_t>(direction),
			a,
			r,
			&startOffset,
			&endOffset
		);
	return node != NULL ? 1 : 0;
}

// Returns the located text field node's start offset, or -1 if none.
int vbench_locate_text_field_at_offset(void* buf, int offset) {
	int startOffset = 0;
	int endOffset = 0;
	VBufStorage_textFieldNode_t* node =
		static_cast<VBufStorage_buffer_t*>(buf)->locateTextFieldNodeAtOffset(
			offset, &startOffset, &endOffset
		);
	return node != NULL ? startOffset : -1;
}

// Returns 1 on success (and fills *start / *end), else 0.
int vbench_get_line_offsets(
	void* buf,
	int offset,
	int maxLineLength,
	int useScreenLayout,
	int* start,
	int* end
) {
	bool ok = static_cast<VBufStorage_buffer_t*>(buf)->getLineOffsets(
		offset, maxLineLength, useScreenLayout != 0, start, end
	);
	return ok ? 1 : 0;
}

// Replaces the subtree rooted at `oldNode` in `mainBuf` with the entire
// content of `tempBuf` (a one-entry replaceSubtrees map). NOTE:
// replaceSubtrees takes ownership of and `delete`s `tempBuf` internally, so
// the caller must NOT destroy `tempBuf` afterwards (mirrors the Rust
// `replace_subtrees(Vec<(NodeKey, Buffer)>)`, which consumes the temp
// Buffer). Returns 1 on success, else 0.
int vbench_replace_subtrees_one(void* mainBuf, void* oldNode, void* tempBuf) {
	std::map<VBufStorage_fieldNode_t*, VBufStorage_buffer_t*> m;
	m[static_cast<VBufStorage_fieldNode_t*>(oldNode)] =
		static_cast<VBufStorage_buffer_t*>(tempBuf);
	bool ok =
		static_cast<VBufStorage_buffer_t*>(mainBuf)->replaceSubtrees(m);
	return ok ? 1 : 0;
}

}  // extern "C"
