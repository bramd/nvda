# Design: port `GeckoVBufBackend_t::fillVBuf` to Rust

**Status:** design (2026-05-06). No code yet.

This is the Phase 4 follow-up to `2026-05-05-rust-gecko-ia2-roadmap.md`.
It scopes the 854-line recursive renderer in
`nvdaHelper/vbufBackends/gecko_ia2/gecko_ia2.cpp:408-1262` into
Rust-portable chunks.

## Motivation

`fillVBuf` is the bulk of remaining gecko_ia2 C++ (854 / ~1300 lines
left). Every smaller helper has been ported, so the building blocks
are ready: IAccessible2 / IAccessible2_2 / IAccessibleText /
IAccessibleHypertext / IAccessibleHyperlink / IAccessibleTableCell
bindings, the vbuf C-shim, and Rust-side `IA2AttribsToMap`,
`getRoleLongRoleString`, `getLabelInfo`, `getChildCount`,
`getAccDescription`, `getSelectedItem`, `getTextBoxInComboBox`,
`fetchIA2Attributes`, `IAccessible2FromIdentifier`,
`_extendDetailsRolesAttribute`, `fillVBufAriaDetails`,
`fillVBufAriaError`, `getTableIDFromCell`,
`fillTableCellInfo_IATable2`, the relation walker, and the toolkit
name lookup.

Missing dependencies that have to land before the bulk port:

* **`IAccessibleAction` binding** — fillVBuf reads `actionCount` and the
  default action's localized name to set the `defaultAction` attribute
  for clickable nodes.
* **`IAccessibleTable2` binding** — for the `paccTable2` parameter that
  threads through the recursion. Only `get_cellAt`,
  `get_modelChange`, `get_summary` (?) and `get_isColumnSelected` /
  `get_isRowSelected` are touched. Audit needed.
* **`reuseExistingNodeInRender` vbuf C-shim** — fillVBuf calls
  `this->reuseExistingNodeInRender` for cross-buffer reuse during
  partial re-renders. Add to `nvdaHelper/vbufBase/c_shim.cpp`.
* **`hasXmlRoleAttribContainingValue` / `hasAriaHiddenAttribute`** —
  pure logic over the IA2 attribute map. The C++ map disappears once
  fillVBuf is Rust-side, so move these into Rust.
* **`nodeHasUsefulContent` / `nodeContentMatchesString` /
  `getNameForURL` / `getLabelIDCached` / `appendStringToTextNode` /
  any other fillVBuf-local helpers** — TBD during exploration; most
  are short enough to inline at port time.

## Carve-up

The function divides cleanly into seven sequential blocks. Port them
as separate commits in this order; each block depends on state from
the previous one.

### Block 1 — entry, identity, IA2 attributes (~80 lines)

Lines 408-505.

* `docHandle` from `pacc->get_windowHandle`; `ID` from
  `get_uniqueID`; bail if either fails.
* Buffer dedup: `getControlFieldNodeWithIdentifier`. If the node
  already exists, return null (loop guard).
* Cross-buffer reuse: `reuseExistingNodeInRender` + add reference.
* Add the new control field node to the buffer.
* `fetchIA2Attributes` and copy each `IAccessible2::attribute_*` onto
  the node.
* Role normalization: `getRoleLongRoleString`, treegrid override,
  equation-with-img-tag override.

**Carve point:** this block alone is small enough to be its own commit
once the dependencies above land. It takes the same input shape as the
full fillVBuf (so the port can stub the rest with a recursive call back
into the C++ original until the later blocks land).

### Block 2 — name, value, description, locale, states (~120 lines)

Lines 506-625 (approx).

* `accName(varChild)`, `accValue(varChild)`, `accDescription` (Rust
  helper exists), `get_locale`, `get_states` (incl. IA2 states from
  `get_states` on IAccessible2 -- already wrapped).
* Hidden / offscreen / labeled-by detection.
* Role-driven name/value/description handling for several role
  classes: most of this is straight-line conditional logic that
  writes attributes onto the new control field node.

This is the thickest "fan-out" section. Do not subdivide further —
the conditionals reference each other and would be awkward to split.

### Block 3 — IA2 text segmentation (~250 lines)

Lines 626-1010 (approx — needs measuring).

The IAccessibleText loop. For each text segment between embedded
object characters, reads attribute runs via
`IAccessibleText::get_attributes`, walks the hyperlink children
(`IAccessibleHypertext2::get_hyperlinks` — already wrapped), and
recursively calls `fillVBuf` on each child. This is the most
complex single section but also the most self-contained: it exits
with `previousNode` pointing at whatever was last appended.

The `HyperlinkGetter` from `ia2utils.cpp` is already in Rust. This
block plugs into it.

### Block 4 — table state plumbing (~80 lines)

Lines ~750-850.

* QI to `IAccessibleTableCell` and `IAccessibleTable2`.
* Set `table-id`, `table-rownumber`, etc. via the existing
  `fill_table_cell_info` Rust helper.
* Maintain the `tableID` and `paccTable2` recursion state.
* Presentational row number propagation.

Depends on the new `IAccessibleTable2` binding from the prereq list.

### Block 5 — non-text children walk (~120 lines)

Lines ~1010-1130.

When the node has no `IAccessibleText` (or `paccText` returned no
text), fall back to `AccessibleChildren` (already used by the Rust
text helper) and recursively `fillVBuf` each child. Handles
container-specific recursion variations (combo boxes call
`getSelectedItem` instead).

### Block 6 — graphic / progressbar / link / cell content fallbacks (~70 lines)

Lines ~1130-1205.

The "if the node still has no content" tail: graphic name derivation
from URL, empty-cell space rendering, separator and interactive
forced-content.

### Block 7 — name-as-attribute, description-is-content, aria-details / aria-errormessage (~50 lines)

Lines ~1205-1250.

* If the name wasn't rendered as content, set it as an attribute and
  detect labelled-by-content.
* `descriptionIsContent` flag.
* Call `fillVBufAriaDetails` and `fillVBufAriaError` (both already
  Rust).

## Migration shape

`fillVBuf` in Rust is a free function that takes everything explicitly:

```rust
pub fn fill_vbuf(
    pacc: &IAccessible2,
    buffer: VbufBuffer,
    parent: Option<VbufControlFieldNode>,
    previous: Option<VbufFieldNode>,
    paccTable2: Option<&IAccessibleTable2>,
    table_id: i32,
    parent_presentational_row_number: Option<&[u16]>,
    ignore_interactive_unlabelled_graphics: bool,
    ctx: &FillVBufCtx,
) -> Option<VbufFieldNode>;

pub struct FillVBufCtx<'a> {
    pub backend: VbufBackend,                // for reuseExistingNodeInRender
    pub root_id: i32,                        // was this->rootID
    pub is_chrome: bool,                     // was this->toolkitName == "Chrome"
    // anything else fillVBuf looked up via `this->`
}
```

The C++ side reduces to a one-line shim:

```cpp
VBufStorage_fieldNode_t* GeckoVBufBackend_t::fillVBuf(...) {
    return static_cast<VBufStorage_fieldNode_t*>(
        nvda_ia2_fill_vbuf(/* unfold args */));
}
```

`reuseExistingNodeInRender` becomes a vbuf C-shim function so Rust
doesn't need a virtual-call back into the C++ class. Once Phase 5
lands, the whole `GeckoVBufBackend_t` class is replaced by a small
C++ adapter and this shim disappears.

## Ordering rationale

* Prereqs first (IAccessibleAction, IAccessibleTable2,
  `vbuf_backend_reuse_existing_node`, the two attribute helpers).
  These are mechanical and unblock several blocks at once.
* Block 1 next: it sets the function up; the rest of fillVBuf can
  remain in C++ behind a partial shim that calls the full C++
  recursive call for the other blocks.
* Then Blocks 2-7 in source order. Each block is its own commit.

## Open questions

* **Recursion across the FFI boundary:** while the port is in
  progress, the Rust port of Block 1 will need to call back into the
  partially-still-C++ fillVBuf. Does that introduce too much friction
  to be worth it, or do we land all of fillVBuf in one larger PR?
  Tentative: stage Block 1 + 2 in one commit, the rest one-block-per-
  commit, all on the same branch.
* **Class state migration:** `this->rootDocAcc`, `this->rootID`,
  `this->pendingInvalidSubtreesList` are touched outside fillVBuf
  (in `render` and `isRootDocAlive`). Phase 5 (render thread) will
  move them. For Phase 4 we just thread the read-only ones through
  `FillVBufCtx`.
* **`paccTable2` lifetime:** the recursion currently passes a raw
  `IAccessibleTable2*`. Rust would prefer a borrowed reference. Need
  to confirm there's no AddRef/Release manipulation we're missing —
  the C++ owns the AddRef'd handle higher up the stack.
