/*
This file is a part of the NVDA project.
URL: http://www.nvda-project.org/
Copyright 2007-2023 NV Access Limited, Mozilla Corporation
    This program is free software: you can redistribute it and/or modify
    it under the terms of the GNU General Public License version 2.0, as published by
    the Free Software Foundation.
    This program is distributed in the hope that it will be useful,
    but WITHOUT ANY WARRANTY; without even the implied warranty of
    MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.
This license can be found at:
http://www.gnu.org/licenses/old-licenses/gpl-2.0.html
*/

#include <memory>
#include <numeric>
#include <functional>
#include <vector>
#include <map>
#include <optional>
#include <windows.h>
#include <set>
#include <string>
#include <sstream>
#include <atlcomcli.h>
#include <ia2.h>
#include <common/ia2utils.h>
#include <remote/nvdaHelperRemote.h>
#include <vbufBase/backend.h>
#include <vbufBase/storage.h>
#include <common/log.h>
#include <vbufBase/utils.h>
#include <remote/textFromIAccessible.h>
#include "gecko_ia2.h"

using namespace std;

bool hasXmlRoleAttribContainingValue(const map<wstring,wstring>& attribsMap, const wstring roleName) {
	const auto attribsMapIt = attribsMap.find(L"xml-roles");
	return attribsMapIt != attribsMap.end() && attribsMapIt->second.find(roleName) != wstring::npos;
}

#ifdef NVDA_HAS_RUST_HELPERS
extern "C" {
	typedef void (*nvda_ia2_relation_target_callback)(
		void* ctx,
		void* iaccessible2_ptr);

	bool nvda_ia2_get_relation_targets_of_type(
		void* pacc2_2,
		const wchar_t* relation_ptr,
		size_t relation_len,
		int max_targets,
		bool is_chrome,
		void* ctx,
		nvda_ia2_relation_target_callback cb);
}

namespace {
	void relationTarget_cb(void* ctx, void* p) {
		auto* vec = static_cast<std::vector<CComQIPtr<IAccessible2>>*>(ctx);
		// p is either null (QI to IAccessible2 failed on the Rust side --
		// preserve the C++ original's behavior of pushing a null entry)
		// or an AddRef'd IAccessible2*. Attach to a CComPtr so the
		// CComQIPtr's QI sees the existing AddRef'd pointer; the
		// CComQIPtr move-constructs from CComPtr without an extra AddRef.
		if (!p) {
			vec->emplace_back();
			return;
		}
		CComPtr<IAccessible2> acc;
		acc.Attach(static_cast<IAccessible2*>(p));
		vec->emplace_back(acc);
	}
}

std::vector<CComQIPtr<IAccessible2>> GeckoVBufBackend_t::getRelationElementsOfType(
	LPCOLESTR ia2TargetRelation,
	IAccessible2_2* element,
	const std::optional<std::size_t> numRelations
) {
	constexpr long FETCH_ALL = 0l;  // See docs of relationTargetsOfType.
	const long maxTargets = static_cast<long>(
		min<size_t>(
			numRelations.value_or(FETCH_ALL),
			static_cast<size_t>((numeric_limits<long>::max)())
		)
	);
	const bool isChrome = this->toolkitName.compare(L"Chrome") == 0;
	std::vector<CComQIPtr<IAccessible2>> result;
	const size_t relationLen = wcslen(ia2TargetRelation);
	nvda_ia2_get_relation_targets_of_type(
		element,
		ia2TargetRelation, relationLen,
		maxTargets,
		isChrome,
		&result,
		relationTarget_cb);
	return result;
}
#endif

const wchar_t EMBEDDED_OBJ_CHAR = 0xFFFC;
// Always render a space for "empty" / metadata only
// text leaf nodes so the user can access them.
constexpr const wchar_t EMPTY_TEXT_NODE[]{L" "};

#ifdef NVDA_HAS_RUST_HELPERS
extern "C" {
	void* nvda_ia2_from_identifier(int doc_handle, int id);
}

static IAccessible2* IAccessible2FromIdentifier(int docHandle, int ID) {
	return static_cast<IAccessible2*>(
		nvda_ia2_from_identifier(docHandle, ID));
}
#endif

#ifdef NVDA_HAS_RUST_HELPERS
extern "C" {
	int nvda_ia2_get_table_id_from_cell(void* cell);
	void nvda_ia2_fill_table_cell_info(void* node, void* cell);
}

inline int getTableIDFromCell(IAccessibleTableCell* tableCell) {
	return nvda_ia2_get_table_id_from_cell(tableCell);
}

inline void GeckoVBufBackend_t::fillTableCellInfo_IATable2(VBufStorage_controlFieldNode_t* node, IAccessibleTableCell* paccTableCell) {
	nvda_ia2_fill_table_cell_info(node, paccTableCell);
}
#endif

#ifdef NVDA_HAS_RUST_HELPERS
extern "C" {
	typedef void (*nvda_ia2_toolkit_name_callback)(
		void* ctx,
		const wchar_t* ptr,
		size_t len);

	bool nvda_ia2_get_toolkit_name(
		void* pacc,
		void* ctx,
		nvda_ia2_toolkit_name_callback cb);
}

namespace {
	void toolkitName_cb(void* ctx, const wchar_t* ptr, size_t len) {
		try {
			static_cast<std::wstring*>(ctx)->assign(ptr, len);
		} catch (const std::bad_alloc&) {
			// Suppressed to prevent UB from a C++ exception crossing the
			// extern "C" frame back into Rust.
		}
	}
}

void GeckoVBufBackend_t::versionSpecificInit(IAccessible2* pacc) {
	nvda_ia2_get_toolkit_name(pacc, &this->toolkitName, toolkitName_cb);
}
#endif


class LabelInfo {
public:
	bool isVisible;
	optional<int> ID;
};

using OptionalLabelInfo = optional< LabelInfo >;
#ifdef NVDA_HAS_RUST_HELPERS
extern "C" {
	typedef void (*nvda_ia2_label_info_callback)(
		void* ctx,
		bool is_visible,
		bool has_id,
		int id);

	bool nvda_ia2_get_label_info(
		void* pacc2,
		void* ctx,
		nvda_ia2_label_info_callback cb);
}

namespace {
	struct LabelInfoCtx {
		LabelInfo info;
	};

	void labelInfo_cb(void* ctx, bool is_visible, bool has_id, int id) {
		auto* c = static_cast<LabelInfoCtx*>(ctx);
		c->info.isVisible = is_visible;
		if (has_id) {
			c->info.ID = id;
		}
	}
}

OptionalLabelInfo GeckoVBufBackend_t::getLabelInfo(IAccessible2* pacc2) {
	LabelInfoCtx ctx{};
	const bool present =
		nvda_ia2_get_label_info(pacc2, &ctx, labelInfo_cb);
	if (!present) {
		return OptionalLabelInfo();
	}
	return ctx.info;
}
#endif

#ifdef NVDA_HAS_RUST_HELPERS
extern "C" {
	int nvda_ia2_get_child_count(void* pacc, bool is_aria_hidden);
}

long getChildCount(const bool isAriaHidden, IAccessible2 * const pacc){
	return static_cast<long>(nvda_ia2_get_child_count(pacc, isAriaHidden));
}
#endif

bool hasAriaHiddenAttribute(const map<wstring,wstring>& IA2AttribsMap){
	const auto IA2AttribsMapIt = IA2AttribsMap.find(L"hidden");
	return (IA2AttribsMapIt != IA2AttribsMap.end() && IA2AttribsMapIt->second == L"true");
}

#ifdef NVDA_HAS_RUST_HELPERS
extern "C" {
	typedef void (*nvda_ia2_acc_description_callback)(
		void* ctx,
		const wchar_t* description_ptr,
		size_t description_len);

	bool nvda_ia2_get_acc_description(
		void* pacc,
		int childid,
		void* ctx,
		nvda_ia2_acc_description_callback cb);
}

namespace {
	void accDescription_cb(void* ctx, const wchar_t* p, size_t n) {
		auto* out = static_cast<std::wstring*>(ctx);
		// (p, n) borrows from a BSTR inside the Rust shim and may not
		// outlive the call; copy the contents into the std::wstring.
		out->assign(p, n);
	}
}

std::optional<wstring> getAccDescription(IAccessible2* pacc, VARIANT childID) {
	const int childId = (childID.vt == VT_I4) ? static_cast<int>(childID.lVal) : 0;
	std::wstring buf;
	const bool present = nvda_ia2_get_acc_description(
		pacc, childId, &buf, accDescription_cb);
	if (!present) {
		return std::optional<std::wstring>();
	}
	return buf;
}
#endif

/**
 * Get the selected item or the first item if no item is selected.
 */
#ifdef NVDA_HAS_RUST_HELPERS
extern "C" {
	void* nvda_ia2_get_selected_item(void* pacc2);
}

CComPtr<IAccessible2> GeckoVBufBackend_t::getSelectedItem(
	IAccessible2* container, const map<wstring, wstring>& attribs
) {
	CComPtr<IAccessible2> result;
	auto* raw = static_cast<IAccessible2*>(
		nvda_ia2_get_selected_item(container));
	// raw is already AddRef'd by the Rust side or null;
	// Attach takes ownership without extra AddRef.
	result.Attach(raw);
	return result;
}
#endif

/**
 * Get the text box inside a combo box, if any.
 */
#ifdef NVDA_HAS_RUST_HELPERS
extern "C" {
	void* nvda_ia2_get_text_box_in_combo_box(void* combo_box);
}

CComPtr<IAccessible2> getTextBoxInComboBox(
	IAccessible2* comboBox
) {
	CComPtr<IAccessible2> result;
	auto* raw = static_cast<IAccessible2*>(
		nvda_ia2_get_text_box_in_combo_box(comboBox));
	// raw is already AddRef'd by the Rust side or null;
	// Attach takes ownership without extra AddRef.
	result.Attach(raw);
	return result;
}
#endif


#ifdef NVDA_HAS_RUST_HELPERS
extern "C" {
	typedef void (*nvda_ia2_role_string_callback)(
		void* ctx,
		const wchar_t* role_string_ptr,
		size_t role_string_len);

	int nvda_ia2_get_role_long_role_string(
		void* pacc,
		int childid,
		void* ctx,
		nvda_ia2_role_string_callback cb);
}

namespace {
	void roleString_cb(void* ctx, const wchar_t* p, size_t n) {
		auto* out = static_cast<CComBSTR*>(ctx);
		// Allocate a fresh BSTR -- the (p, n) range borrows from the
		// VARIANT inside the Rust shim and may not outlive the call.
		*out = CComBSTR(static_cast<int>(n), p);
	}
}

std::tuple<long, CComBSTR> getRoleLongRoleString(CComPtr<IAccessible2> pacc, CComVariant varChild) {
	CComBSTR roleString;
	const int childId = (varChild.vt == VT_I4) ? static_cast<int>(varChild.lVal) : 0;
	long role = static_cast<long>(
		nvda_ia2_get_role_long_role_string(
			pacc, childId, &roleString, roleString_cb));
	return std::make_tuple(role, roleString);
}
#endif


const vector<wstring>ATTRLIST_ROLES(1, L"IAccessible2::attribute_xml-roles");
const wregex REGEX_PRESENTATION_ROLE(L"IAccessible2\\\\:\\\\:attribute_xml-roles:.*\\bpresentation\\b.*;");


#ifdef NVDA_HAS_RUST_HELPERS
extern "C" {
	void nvda_ia2_extend_details_roles_attribute(
		void* node,
		const wchar_t* role_ptr,
		size_t role_len);
}

void _extendDetailsRolesAttribute(VBufStorage_controlFieldNode_t& node, const std::wstring& detailsRole)
{
	nvda_ia2_extend_details_roles_attribute(
		&node,
		detailsRole.data(),
		detailsRole.size());
}
#endif

#ifdef NVDA_HAS_RUST_HELPERS
extern "C" {
	void nvda_ia2_fill_vbuf_aria_details(
		int doc_handle,
		void* pacc,
		void* buffer,
		void* node,
		const wchar_t* node_role_ptr,
		size_t node_role_len,
		bool is_chrome);
}

void GeckoVBufBackend_t::fillVBufAriaDetails(
	int docHandle,
	CComPtr<IAccessible2> pacc,
	VBufStorage_buffer_t& buffer,
	VBufStorage_controlFieldNode_t& nodeBeingFilled,
	const std::wstring& nodeBeingFilledRole
){
	const bool isChrome = this->toolkitName.compare(L"Chrome") == 0;
	nvda_ia2_fill_vbuf_aria_details(
		docHandle,
		pacc.p,
		&buffer,
		&nodeBeingFilled,
		nodeBeingFilledRole.data(),
		nodeBeingFilledRole.size(),
		isChrome);
}
#endif


#ifdef NVDA_HAS_RUST_HELPERS
extern "C" {
	void nvda_ia2_fill_vbuf_aria_error(
		void* pacc,
		void* node,
		bool is_chrome);
}

void GeckoVBufBackend_t::fillVBufAriaError(
	CComPtr<IAccessible2> pacc,
	VBufStorage_controlFieldNode_t& nodeBeingFilled
){
	const bool isChrome = this->toolkitName.compare(L"Chrome") == 0;
	nvda_ia2_fill_vbuf_aria_error(pacc.p, &nodeBeingFilled, isChrome);
}
#endif


#ifdef NVDA_HAS_RUST_HELPERS
extern "C" {
	void* nvda_ia2_fill_vbuf(
		void* pacc,
		void* buffer,
		void* parent_node,
		void* previous_node,
		void* pacc_table2,
		int table_id,
		const wchar_t* parent_pres_row_num_ptr,
		size_t parent_pres_row_num_len,
		bool ignore_interactive_unlabelled_graphics,
		void* backend,
		int root_id,
		bool is_chrome);
}

VBufStorage_fieldNode_t* GeckoVBufBackend_t::fillVBuf(
	IAccessible2* pacc,
	VBufStorage_buffer_t* buffer,
	VBufStorage_controlFieldNode_t* parentNode,
	VBufStorage_fieldNode_t* previousNode,
	IAccessibleTable2* paccTable2,
	long tableID,
	const wchar_t* parentPresentationalRowNumber,
	bool ignoreInteractiveUnlabelledGraphics
) {
	nhAssert(buffer); //buffer can't be NULL
	nhAssert(!parentNode||buffer->isNodeInBuffer(parentNode));
	nhAssert(!previousNode||buffer->isNodeInBuffer(previousNode));
	const bool isChrome = this->toolkitName.compare(L"Chrome") == 0;
	const size_t presRowLen = parentPresentationalRowNumber
		? wcslen(parentPresentationalRowNumber)
		: 0;
	return static_cast<VBufStorage_fieldNode_t*>(nvda_ia2_fill_vbuf(
		pacc,
		buffer,
		parentNode,
		previousNode,
		paccTable2,
		static_cast<int>(tableID),
		parentPresentationalRowNumber,
		presRowLen,
		ignoreInteractiveUnlabelledGraphics,
		this,
		this->rootID,
		isChrome));
}
#endif


bool GeckoVBufBackend_t::isRootDocAlive() {
	if (!this->pendingInvalidSubtreesList.empty()) {
		// There is a pending update. We only want to check this once per update tick
		// to avoid unnecessary COM calls.
		return true;
	}
	AccessibleStates states;
	if (!this->rootDocAcc || FAILED(this->rootDocAcc->get_states(&states)) ||
			states & IA2_STATE_DEFUNCT) {
		this->rootDocAcc = nullptr;
		return false;
	}
	return true;
}

void CALLBACK GeckoVBufBackend_t::renderThread_winEventProcHook(HWINEVENTHOOK hookID, DWORD eventID, HWND hwnd, long objectID, long childID, DWORD threadID, DWORD time) {
	switch(eventID) {
		case EVENT_OBJECT_FOCUS:
		case IA2_EVENT_DOCUMENT_LOAD_COMPLETE:
		case EVENT_SYSTEM_ALERT:
		case IA2_EVENT_TEXT_UPDATED:
		case IA2_EVENT_TEXT_INSERTED:
		case IA2_EVENT_TEXT_REMOVED:
		case EVENT_OBJECT_REORDER:
		case EVENT_OBJECT_NAMECHANGE:
		case EVENT_OBJECT_VALUECHANGE:
		case EVENT_OBJECT_DESCRIPTIONCHANGE:
		case EVENT_OBJECT_STATECHANGE:
		case EVENT_OBJECT_SELECTIONADD:
		case EVENT_OBJECT_SELECTIONREMOVE:
		case EVENT_OBJECT_SELECTIONWITHIN:
		case IA2_EVENT_OBJECT_ATTRIBUTE_CHANGED:
		case IA2_EVENT_TEXT_ATTRIBUTE_CHANGED:
		case EVENT_OBJECT_HIDE:
		break;
		default:
		return;
	}
	if(childID>=0||objectID!=OBJID_CLIENT)
		return;
	LOG_DEBUG(L"winEvent for window "<<hwnd);
	if(!hwnd) {
		LOG_DEBUG(L"Invalid window");
		return;
	}
	int docHandle=HandleToUlong(hwnd);
	int ID=childID;
	VBufBackend_t* backend=NULL;
	for(VBufBackendSet_t::iterator i=runningBackends.begin();i!=runningBackends.end();++i) {
		HWND rootWindow=(HWND)UlongToHandle(((*i)->rootDocHandle));
		if(rootWindow==hwnd||IsChild(rootWindow,hwnd))
			backend=(*i);
		else
			continue;
		LOG_DEBUG(L"found active backend for this window at "<<backend);

		//For focus, documentLoadComplete and alert events, force any nodes already marked as invalid  to be updated right now,
		if(
			eventID == EVENT_OBJECT_FOCUS
			|| eventID == IA2_EVENT_DOCUMENT_LOAD_COMPLETE
			|| eventID==EVENT_SYSTEM_ALERT
		) {
			backend->forceUpdate();
			continue;
		}

		//Ignore state change events on the root node (document) as it can cause rerendering when the document goes busy
		if(eventID==EVENT_OBJECT_STATECHANGE&&hwnd==(HWND)UlongToHandle(backend->rootDocHandle)&&childID==backend->rootID)
			return;

		VBufStorage_controlFieldNode_t* node=backend->getControlFieldNodeWithIdentifier(docHandle,ID);
		if(!node)
			continue;

		auto* geckoBackend = static_cast<GeckoVBufBackend_t*>(backend);
		if (!geckoBackend->isRootDocAlive()) {
			// The root doc is dead, but NVDA hasn't realised yet and so hasn't killed
			// this buffer. That means this id is now in a different document! Trying to
			// render this could cause a broken tree. At this point, we may as well
			// clear the buffer.
			LOG_DEBUG(L"Root doc is dead. Clearing buffer.");
			backend->clearBuffer();
			continue;
		}

		if (eventID == EVENT_OBJECT_HIDE) {
			// When an accessible is moved, events are fired as if the accessible were
			// removed and then inserted. The insertion events are fired as if it were
			// a new subtree; i.e. only one insertion for the root of the subtree.
			// This means that if new descendants are inserted at the same time as the
			// root is moved, we don't get specific events for those insertions.
			// Because of that, we mustn't reuse the subtree. Otherwise, we wouldn't
			// walk inside it and thus wouldn't know about the new descendants.
			node->alwaysRerenderDescendants = true;
			// We'll get a text removed event for the parent, so no need to invalidate
			// this node.
			continue;
		}
		backend->invalidateSubtree(node);
	}
}

void GeckoVBufBackend_t::renderThread_initialize() {
	registerWinEventHook(renderThread_winEventProcHook);
	VBufBackend_t::renderThread_initialize();
	this->rootDocAcc = IAccessible2FromIdentifier(this->rootDocHandle, this->rootID);
}

void GeckoVBufBackend_t::renderThread_terminate() {
	unregisterWinEventHook(renderThread_winEventProcHook);
	VBufBackend_t::renderThread_terminate();
	// The backend holds a reference to the root accessible of the document.
	// This must be specifically released here, in the UI thread where it was created.
	// See https://issues.chromium.org/issues/41487612
	if (this->rootDocAcc) {
		this->rootDocAcc.Release();
	}
}

void GeckoVBufBackend_t::render(VBufStorage_buffer_t* buffer, int docHandle, int ID, VBufStorage_controlFieldNode_t* oldNode) {
	IAccessible2* pacc=IAccessible2FromIdentifier(docHandle,ID);
	if(!pacc) {
		LOG_DEBUGWARNING(L"Could not get IAccessible2, returning");
		return;
	}
	if (!oldNode) {
		// This is the root node.
		this->versionSpecificInit(pacc);
	}
	if(!this->fillVBuf(pacc, buffer, nullptr, nullptr)) {
		if(oldNode) {
			LOG_DEBUGWARNING(L"No content rendered in update");
		} else {
			LOG_DEBUGWARNING(L"No initial content rendered");
		}
	}
	pacc->Release();
}

GeckoVBufBackend_t::GeckoVBufBackend_t(int docHandle, int ID): VBufBackend_t(docHandle,ID) {
}

GeckoVBufBackend_t::~GeckoVBufBackend_t() {
	// The backend holds a reference to the root accessible of the document.
	// This must be specifically released in the UI thread where it was created.
	// See https://issues.chromium.org/issues/41487612
	// In most cases this will be released in renderThread_terminate.
	// However in the unlikely case terminate can't run,
	// we must detach and leak the COM pointer here.
	// Otherwise it would be automatically deleted along with the backend which would cause a crash,
	// as the COM object would be released from within an RPC worker thread.
	nhAssert(!rootDocAcc);
	if (this->rootDocAcc) {
		this->rootDocAcc.Detach();
	}
}

VBufBackend_t* GeckoVBufBackend_t_createInstance(int docHandle, int ID) {
	VBufBackend_t* backend=new GeckoVBufBackend_t(docHandle,ID);
	return backend;
}
