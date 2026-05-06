/*
This file is a part of the NVDA project.
URL: http://www.nvda-project.org/
Copyright 2026 NV Access Limited
    This program is free software: you can redistribute it and/or modify
    it under the terms of the GNU General Public License version 2.0, as published by
    the Free Software Foundation.
    This program is distributed in the hope that it will be useful,
    but WITHOUT ANY WARRANTY; without even the implied warranty of
    MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.
This license can be found at:
http://www.gnu.org/licenses/old-licenses/gpl-2.0.html
*/

/*
Flat C ABI shim over the C++ VBufStorage_* / VBufBackend_t classes,
exposed so the Rust crates (currently nvda_ia2; future nvda_vbuf) can
call into vbufBase without binding the C++ ABI directly.

Surface chosen to cover gecko_ia2.cpp's vbuf usage. As more vbuf
backends move to Rust the surface may grow.

Conventions:
* Opaque pointers (`void*`) on the boundary; Rust-side bindings wrap
  them in newtypes. NULL is a valid input only where documented.
* Bools are int (0 / non-zero) for portability.
* Strings are passed as (ptr, len) pairs of UTF-16; OUT strings come
  back through the `vbuf_string_callback` typedef so the Rust caller
  controls allocation. Empty strings are invoked with `len == 0`.
* All operations are synchronous and run on the caller's thread; the
  vbufBase invariants for thread affinity (render thread vs. main
  thread) are the caller's responsibility, matching the existing C++
  contract.
*/

#include <string>
#include <vbufBase/storage.h>
#include <vbufBase/backend.h>
#include <vbufBase/utils.h>

/* --- string OUT callback ------------------------------------------- */

typedef void (*vbuf_string_callback)(
	void* ctx,
	const wchar_t* ptr,
	size_t len);

/* --- buffer-level operations --------------------------------------- */

extern "C" void* vbuf_buffer_add_control_field_node(
	void* buffer, void* parent, void* previous,
	int doc_handle, int id, int is_block
) {
	return static_cast<VBufStorage_buffer_t*>(buffer)->addControlFieldNode(
		static_cast<VBufStorage_controlFieldNode_t*>(parent),
		static_cast<VBufStorage_fieldNode_t*>(previous),
		doc_handle, id, is_block != 0);
}

extern "C" void* vbuf_buffer_add_text_field_node(
	void* buffer, void* parent, void* previous,
	const wchar_t* text_ptr, size_t text_len
) {
	std::wstring text(text_ptr, text_len);
	return static_cast<VBufStorage_buffer_t*>(buffer)->addTextFieldNode(
		static_cast<VBufStorage_controlFieldNode_t*>(parent),
		static_cast<VBufStorage_fieldNode_t*>(previous),
		text);
}

extern "C" void* vbuf_buffer_add_reference_node(
	void* buffer, void* parent, void* previous, void* node
) {
	return static_cast<VBufStorage_buffer_t*>(buffer)->addReferenceNodeToBuffer(
		static_cast<VBufStorage_controlFieldNode_t*>(parent),
		static_cast<VBufStorage_fieldNode_t*>(previous),
		static_cast<VBufStorage_controlFieldNode_t*>(node));
}

extern "C" void* vbuf_buffer_get_control_field_node_with_identifier(
	void* buffer, int doc_handle, int id
) {
	return static_cast<VBufStorage_buffer_t*>(buffer)
		->getControlFieldNodeWithIdentifier(doc_handle, id);
}

extern "C" int vbuf_buffer_is_descendant_node(
	void* buffer, void* parent, void* descendant
) {
	return static_cast<VBufStorage_buffer_t*>(buffer)->isDescendantNode(
		static_cast<VBufStorage_fieldNode_t*>(parent),
		static_cast<VBufStorage_fieldNode_t*>(descendant)) ? 1 : 0;
}

extern "C" int vbuf_buffer_is_node_in_buffer(void* buffer, void* node) {
	return static_cast<VBufStorage_buffer_t*>(buffer)->isNodeInBuffer(
		static_cast<VBufStorage_fieldNode_t*>(node)) ? 1 : 0;
}

/* --- field-node operations ----------------------------------------- */

extern "C" int vbuf_node_add_attribute(
	void* node,
	const wchar_t* name_ptr, size_t name_len,
	const wchar_t* value_ptr, size_t value_len
) {
	std::wstring name(name_ptr, name_len);
	std::wstring value(value_ptr, value_len);
	return static_cast<VBufStorage_fieldNode_t*>(node)
		->addAttribute(name, value) ? 1 : 0;
}

/* Returns 1 and invokes `cb` once if the attribute is present, 0 if
   absent (cb not invoked). An attribute present with an empty value
   invokes `cb` with `len == 0`. */
extern "C" int vbuf_node_get_attribute(
	void* node,
	const wchar_t* name_ptr, size_t name_len,
	void* ctx, vbuf_string_callback cb
) {
	std::wstring name(name_ptr, name_len);
	auto result = static_cast<VBufStorage_fieldNode_t*>(node)
		->getAttribute(name);
	if (!result.has_value()) {
		return 0;
	}
	cb(ctx, result->c_str(), result->size());
	return 1;
}

/* The "name:value;..." semicolon-separated string of every attribute. */
extern "C" void vbuf_node_get_attributes_string(
	void* node, void* ctx, vbuf_string_callback cb
) {
	std::wstring s = static_cast<VBufStorage_fieldNode_t*>(node)
		->getAttributesString();
	cb(ctx, s.c_str(), s.size());
}

extern "C" int vbuf_node_get_length(void* node) {
	return static_cast<VBufStorage_fieldNode_t*>(node)->getLength();
}

extern "C" int vbuf_node_is_block(void* node) {
	return static_cast<VBufStorage_fieldNode_t*>(node)->isBlock ? 1 : 0;
}

extern "C" void vbuf_node_set_is_block(void* node, int value) {
	static_cast<VBufStorage_fieldNode_t*>(node)->isBlock = value != 0;
}

extern "C" void vbuf_node_set_is_hidden(void* node, int value) {
	static_cast<VBufStorage_fieldNode_t*>(node)->isHidden = value != 0;
}

extern "C" int vbuf_node_is_hidden(void* node) {
	return static_cast<VBufStorage_fieldNode_t*>(node)->isHidden ? 1 : 0;
}

/* --- public-field setters on control-field nodes ------------------- */

extern "C" void vbuf_node_set_always_rerender_descendants(
	void* node, int value
) {
	static_cast<VBufStorage_controlFieldNode_t*>(node)
		->alwaysRerenderDescendants = (value != 0);
}

extern "C" void vbuf_node_set_always_rerender_children(
	void* node, int value
) {
	static_cast<VBufStorage_controlFieldNode_t*>(node)
		->alwaysRerenderChildren = (value != 0);
}

extern "C" void vbuf_node_set_deny_reuse_if_previous_siblings_changed(
	void* node, int value
) {
	static_cast<VBufStorage_controlFieldNode_t*>(node)
		->denyReuseIfPreviousSiblingsChanged = (value != 0);
}

extern "C" void vbuf_node_set_requires_parent_update(
	void* node, int value
) {
	static_cast<VBufStorage_controlFieldNode_t*>(node)
		->requiresParentUpdate = (value != 0);
}

/* --- VBufBackend_t operations -------------------------------------- */

extern "C" int vbuf_backend_get_root_doc_handle(void* backend) {
	return static_cast<VBufBackend_t*>(backend)->rootDocHandle;
}

extern "C" int vbuf_backend_get_root_id(void* backend) {
	return static_cast<VBufBackend_t*>(backend)->rootID;
}

extern "C" void vbuf_backend_clear_buffer(void* backend) {
	// clearBuffer is on the VBufStorage_buffer_t base; backend IS-A buffer.
	static_cast<VBufStorage_buffer_t*>(backend)->clearBuffer();
}

extern "C" void vbuf_backend_force_update(void* backend) {
	static_cast<VBufBackend_t*>(backend)->forceUpdate();
}

extern "C" int vbuf_backend_invalidate_subtree(void* backend, void* node) {
	return static_cast<VBufBackend_t*>(backend)->invalidateSubtree(
		static_cast<VBufStorage_controlFieldNode_t*>(node)) ? 1 : 0;
}

extern "C" int vbuf_node_has_useful_content(void* node) {
	return nodeHasUsefulContent(
		static_cast<VBufStorage_fieldNode_t*>(node)) ? 1 : 0;
}

extern "C" int vbuf_node_content_matches_string(
	void* node,
	const wchar_t* str_ptr,
	size_t str_len
) {
	std::wstring s(str_ptr, str_len);
	return nodeContentMatchesString(
		static_cast<VBufStorage_fieldNode_t*>(node), s) ? 1 : 0;
}

extern "C" void* vbuf_backend_reuse_existing_node(
	void* backend,
	void* parent,
	void* previous,
	int doc_handle,
	int id
) {
	return static_cast<VBufBackend_t*>(backend)->reuseExistingNodeInRender(
		static_cast<VBufStorage_controlFieldNode_t*>(parent),
		static_cast<VBufStorage_fieldNode_t*>(previous),
		doc_handle,
		id);
}
