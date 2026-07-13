//! Criterion micro-benchmarks comparing the Rust virtual-buffer storage
//! (`nvda_vbuf::storage::Buffer`) against the original C++
//! `VBufStorage_buffer_t` on identical synthetic workloads, in one process.
//!
//! The two implementations are driven from a single deterministic op list
//! (see [`BuildOp`] / [`gen_workload`]) so their trees are structurally
//! identical: same `(docHandle, ID)` identities, same text, same
//! attributes. Every criterion group runs both engines side by side per
//! `(op, size, shape)` so regressions and asymmetries are directly visible.
//!
//! Run with: `cargo bench -p vbuf_bench`

use std::os::raw::c_void;

use criterion::{
	black_box, criterion_group, criterion_main, BatchSize, BenchmarkId,
	Criterion,
};

use nvda_vbuf::storage::{
	Buffer, ControlFieldIdentifier, FindDirection, NodeKey,
};

// ---------------------------------------------------------------------
// C++ shim FFI (defined in cpp/bench_shim.cpp, compiled by build.rs).
// ---------------------------------------------------------------------

// The static lib name matches build.rs's `cc::Build::compile("vbuf_bench_cpp")`.
// The `#[link]` makes the bench binary pull it in explicitly; build.rs emits
// the matching `rustc-link-search` for its output dir.
#[link(name = "vbuf_bench_cpp", kind = "static")]
extern "C" {
	fn vbench_buffer_create() -> *mut c_void;
	fn vbench_buffer_destroy(buf: *mut c_void);
	fn vbench_add_control(
		buf: *mut c_void,
		parent: *mut c_void,
		previous: *mut c_void,
		doc_handle: i32,
		id: i32,
		is_block: i32,
	) -> *mut c_void;
	fn vbench_add_text(
		buf: *mut c_void,
		parent: *mut c_void,
		previous: *mut c_void,
		text: *const u16,
		len: usize,
	) -> *mut c_void;
	fn vbench_node_add_attribute(
		node: *mut c_void,
		name: *const u16,
		nlen: usize,
		val: *const u16,
		vlen: usize,
	);
	fn vbench_get_text_length(buf: *mut c_void) -> i32;
	fn vbench_get_text_in_range(
		buf: *mut c_void,
		start: i32,
		end: i32,
		use_markup: i32,
	) -> i32;
	fn vbench_find_node_by_attributes(
		buf: *mut c_void,
		offset: i32,
		direction: i32,
		attribs: *const u16,
		alen: usize,
		regexp: *const u16,
		rlen: usize,
	) -> i32;
	fn vbench_locate_text_field_at_offset(
		buf: *mut c_void,
		offset: i32,
	) -> i32;
	fn vbench_get_line_offsets(
		buf: *mut c_void,
		offset: i32,
		max_line_length: i32,
		use_screen_layout: i32,
		start: *mut i32,
		end: *mut i32,
	) -> i32;
	fn vbench_replace_subtrees_one(
		main_buf: *mut c_void,
		old_node: *mut c_void,
		temp_buf: *mut c_void,
	) -> i32;
}

/// Owns a C++ `VBufStorage_buffer_t*`, destroying it on drop so criterion's
/// `iter`/`iter_batched` teardown frees it outside the timed region.
struct CppBuffer(*mut c_void);
impl CppBuffer {
	fn new() -> Self {
		CppBuffer(unsafe { vbench_buffer_create() })
	}
	fn ptr(&self) -> *mut c_void {
		self.0
	}
}
impl Drop for CppBuffer {
	fn drop(&mut self) {
		if !self.0.is_null() {
			unsafe { vbench_buffer_destroy(self.0) };
		}
	}
}

fn w(s: &str) -> Vec<u16> {
	s.encode_utf16().collect()
}

// ---------------------------------------------------------------------
// Deterministic RNG (xorshift64) -- identical sequence every run.
// ---------------------------------------------------------------------

struct XorShift(u64);
impl XorShift {
	fn new(seed: u64) -> Self {
		XorShift(seed | 1)
	}
	fn next_u64(&mut self) -> u64 {
		let mut x = self.0;
		x ^= x << 13;
		x ^= x >> 7;
		x ^= x << 17;
		self.0 = x;
		x
	}
	/// Uniform-ish value in `[0, n)`.
	fn below(&mut self, n: u32) -> u32 {
		(self.next_u64() % n as u64) as u32
	}
}

// ---------------------------------------------------------------------
// Workload representation: a flat op list replayed identically against
// both engines. Each op appends one node; a node's index is its position
// in the op list. `parent` / `previous` reference earlier node indices.
// ---------------------------------------------------------------------

enum BuildOp {
	Control {
		parent: Option<usize>,
		previous: Option<usize>,
		doc_handle: i32,
		id: i32,
		is_block: bool,
		attrs: Vec<(Vec<u16>, Vec<u16>)>,
	},
	Text {
		parent: Option<usize>,
		previous: Option<usize>,
		text: Vec<u16>,
	},
}

struct Workload {
	ops: Vec<BuildOp>,
	/// Index of a mid-tree control node used as the `replace_subtrees`
	/// target (a control that has a parent and children).
	replace_target: usize,
}

const WORDS: &[&str] = &[
	"the", "quick", "brown", "fox", "jumps", "over", "lazy", "dog",
	"accessible", "screen", "reader", "virtual", "buffer", "document",
	"heading", "paragraph", "navigation", "content", "region", "landmark",
	"button", "link", "list", "item", "table", "row", "cell", "focus",
];

fn make_words(rng: &mut XorShift, count: usize) -> Vec<u16> {
	let mut s = String::new();
	for i in 0..count {
		if i > 0 {
			s.push(' ');
		}
		s.push_str(WORDS[rng.below(WORDS.len() as u32) as usize]);
	}
	w(&s)
}

/// Push a control op, returning its node index.
fn push_control(
	ops: &mut Vec<BuildOp>,
	parent: Option<usize>,
	previous: Option<usize>,
	doc_handle: i32,
	id: i32,
	is_block: bool,
	attrs: Vec<(Vec<u16>, Vec<u16>)>,
) -> usize {
	let idx = ops.len();
	ops.push(BuildOp::Control {
		parent,
		previous,
		doc_handle,
		id,
		is_block,
		attrs,
	});
	idx
}

/// Push a text op, returning its node index.
fn push_text(
	ops: &mut Vec<BuildOp>,
	parent: Option<usize>,
	previous: Option<usize>,
	text: Vec<u16>,
) -> usize {
	let idx = ops.len();
	ops.push(BuildOp::Text {
		parent,
		previous,
		text,
	});
	idx
}

const DOC: i32 = 1;

/// Root with many block control children, each holding one text node.
fn gen_wide_shallow(target: usize, rng: &mut XorShift) -> Workload {
	let mut ops = Vec::new();
	let mut next_id = 1;
	let root = push_control(
		&mut ops,
		None,
		None,
		DOC,
		next_id,
		true,
		vec![(w("role"), w("document"))],
	);
	next_id += 1;

	let children = target.saturating_sub(1) / 2;
	let mut prev_child: Option<usize> = None;
	let mut replace_target = root;
	for i in 0..children {
		let is_heading = i % 10 == 0;
		let mut attrs = vec![(w("class"), w(&format!("c{}", i % 5)))];
		if is_heading {
			attrs.push((w("role"), w("heading")));
			attrs.push((w("level"), w(&format!("{}", (i % 6) + 1))));
		} else {
			attrs.push((w("role"), w("section")));
		}
		let child = push_control(
			&mut ops,
			Some(root),
			prev_child,
			DOC,
			next_id,
			true,
			attrs,
		);
		next_id += 1;
		push_text(&mut ops, Some(child), None, make_words(rng, 3 + (i % 6)));
		prev_child = Some(child);
		if i == children / 2 {
			replace_target = child;
		}
	}
	Workload {
		ops,
		replace_target,
	}
}

/// A long nested control spine (each level a block control with a text run),
/// widened with extra leaf control+text pairs so the node budget is met
/// without an unsafely deep recursion. Depth is capped for stack safety.
fn gen_deep_nested(target: usize, rng: &mut XorShift, depth_cap: usize) -> Workload {
	let mut ops = Vec::new();
	let mut next_id = 1;
	let root = push_control(
		&mut ops,
		None,
		None,
		DOC,
		next_id,
		true,
		vec![(w("role"), w("document"))],
	);
	next_id += 1;

	// Each spine level contributes a control + a text = 2 nodes.
	let depth = ((target.saturating_sub(1)) / 2).min(depth_cap).max(1);
	// Remaining budget filled as leaf (control+text) pairs across levels.
	let spine_nodes = 1 + depth * 2;
	let remaining = target.saturating_sub(spine_nodes);
	let leaf_pairs = remaining / 2;

	let mut cur = root;
	let mut replace_target = root;
	for level in 0..depth {
		let is_heading = level % 15 == 0;
		let mut attrs = vec![(w("class"), w("grp"))];
		if is_heading {
			attrs.push((w("role"), w("heading")));
			attrs.push((w("level"), w(&format!("{}", (level % 6) + 1))));
		} else {
			attrs.push((w("role"), w("group")));
		}
		let node = push_control(
			&mut ops,
			Some(cur),
			None,
			DOC,
			next_id,
			true,
			attrs,
		);
		next_id += 1;
		// First child of this spine level: a text run.
		let mut prev = push_text(
			&mut ops,
			Some(node),
			None,
			make_words(rng, 2 + (level % 4)),
		);
		// Spread the leaf pairs evenly over the levels.
		let leaves_here = leaf_pairs / depth
			+ if level < leaf_pairs % depth { 1 } else { 0 };
		for _ in 0..leaves_here {
			let leaf = push_control(
				&mut ops,
				Some(node),
				Some(prev),
				DOC,
				next_id,
				false,
				vec![(w("role"), w("link")), (w("class"), w("lnk"))],
			);
			next_id += 1;
			push_text(&mut ops, Some(leaf), None, make_words(rng, 1 + (next_id as usize % 3)));
			prev = leaf;
		}
		if level == depth / 2 {
			replace_target = node;
		}
		cur = node;
	}
	Workload {
		ops,
		replace_target,
	}
}

/// Headings / paragraphs / links / lists at a few control levels, with a
/// realistic distribution of a couple of attributes per control node and
/// realistic word-length text runs.
fn gen_realistic_mixed(target: usize, rng: &mut XorShift) -> Workload {
	let mut ops = Vec::new();
	let mut next_id = 1;
	let root = push_control(
		&mut ops,
		None,
		None,
		DOC,
		next_id,
		true,
		vec![(w("role"), w("document"))],
	);
	next_id += 1;

	let mut prev_top: Option<usize> = None;
	let mut replace_target = root;
	let mut recorded = false;

	while ops.len() < target {
		let roll = rng.below(100);
		if roll < 12 {
			// Heading.
			let level = (rng.below(6) + 1) as i32;
			let hd = push_control(
				&mut ops,
				Some(root),
				prev_top,
				DOC,
				next_id,
				true,
				vec![
					(w("role"), w("heading")),
					(w("level"), w(&format!("{}", level))),
					(w("class"), w("hdr")),
				],
			);
			next_id += 1;
			let n = 3 + rng.below(6) as usize;
			push_text(&mut ops, Some(hd), None, make_words(rng, n));
			prev_top = Some(hd);
		} else if roll < 62 {
			// Paragraph, possibly with an inline link run.
			let para = push_control(
				&mut ops,
				Some(root),
				prev_top,
				DOC,
				next_id,
				true,
				vec![(w("role"), w("paragraph")), (w("class"), w("p"))],
			);
			next_id += 1;
			let n = 8 + rng.below(20) as usize;
			let t = push_text(&mut ops, Some(para), None, make_words(rng, n));
			if rng.below(100) < 40 {
				let link = push_control(
					&mut ops,
					Some(para),
					Some(t),
					DOC,
					next_id,
					false,
					vec![(w("role"), w("link")), (w("class"), w("lnk"))],
				);
				next_id += 1;
				let n = 1 + rng.below(4) as usize;
				push_text(&mut ops, Some(link), None, make_words(rng, n));
			}
			prev_top = Some(para);
			if !recorded && ops.len() >= target / 2 {
				replace_target = para;
				recorded = true;
			}
		} else if roll < 82 {
			// List of a few items, each with text and an optional link.
			let list = push_control(
				&mut ops,
				Some(root),
				prev_top,
				DOC,
				next_id,
				true,
				vec![(w("role"), w("list")), (w("class"), w("ul"))],
			);
			next_id += 1;
			let items = 2 + rng.below(5) as usize;
			let mut prev_item: Option<usize> = None;
			for _ in 0..items {
				let item = push_control(
					&mut ops,
					Some(list),
					prev_item,
					DOC,
					next_id,
					true,
					vec![(w("role"), w("listitem"))],
				);
				next_id += 1;
				let n = 2 + rng.below(6) as usize;
				push_text(&mut ops, Some(item), None, make_words(rng, n));
				prev_item = Some(item);
			}
			prev_top = Some(list);
		} else {
			// Standalone link.
			let link = push_control(
				&mut ops,
				Some(root),
				prev_top,
				DOC,
				next_id,
				false,
				vec![(w("role"), w("link")), (w("class"), w("lnk"))],
			);
			next_id += 1;
			let n = 1 + rng.below(4) as usize;
			push_text(&mut ops, Some(link), None, make_words(rng, n));
			prev_top = Some(link);
		}
	}
	Workload {
		ops,
		replace_target,
	}
}

// ---------------------------------------------------------------------
// MSHTML-representative workload.
// ---------------------------------------------------------------------
//
// Same web-page structure as realistic_mixed, but with the per-control-node
// ATTRIBUTE DENSITY a real MSHTML (Trident) render emits: IHTMLDOMNode::
// nodeName, a numeric IAccessible::role, one or more IAccessible::state_N
// bits, language, and formatState (~6-9 attributes vs realistic_mixed's
// 2-3). All the vbuf backends -- gecko, acrobat, mshtml -- render into the
// SAME nvda_vbuf::storage::Buffer, so the only storage cost that is
// MSHTML-specific is this attribute count, which stresses the per-node
// attribute map. Headings also carry a "role"="heading" marker so the
// quick-nav find op stays comparable across shapes. (Text-node attributes
// -- MSHTML emits language/formatState there too -- are omitted:
// BuildOp::Text carries no attributes and control nodes dominate the cost.)

// MSAA states (oleacc.h) MSHTML commonly emits as IAccessible::state_N.
const ST_FOCUSABLE: i32 = 0x0010_0000;
const ST_LINKED: i32 = 0x0040_0000;
const ST_READONLY: i32 = 0x40;
// MSAA roles (oleacc.h).
const ROLE_TEXT: i32 = 42;
const ROLE_LINK: i32 = 30;
const ROLE_LIST: i32 = 33;
const ROLE_LISTITEM: i32 = 34;
const ROLE_TABLE: i32 = 24;
const ROLE_ROW: i32 = 28;
const ROLE_CELL: i32 = 29;
// A representative formatState bit (STRONG).
const FS_STRONG: u32 = 8;

fn mshtml_attrs(
	node_name: &str,
	role: i32,
	states: &[i32],
	lang: &str,
	format_state: u32,
	extra: &[(&str, &str)],
) -> Vec<(Vec<u16>, Vec<u16>)> {
	let mut a = vec![
		(w("IHTMLDOMNode::nodeName"), w(node_name)),
		(w("IAccessible::role"), w(&format!("{role}"))),
		(w("language"), w(lang)),
		(w("formatState"), w(&format!("{format_state}"))),
	];
	for &st in states {
		a.push((w(&format!("IAccessible::state_{st}")), w("1")));
	}
	for &(k, v) in extra {
		a.push((w(k), w(v)));
	}
	a
}

fn gen_mshtml_document(target: usize, rng: &mut XorShift) -> Workload {
	let mut ops = Vec::new();
	let mut next_id = 1;
	let root = push_control(
		&mut ops,
		None,
		None,
		DOC,
		next_id,
		true,
		mshtml_attrs("BODY", ROLE_TEXT, &[ST_READONLY], "en", 0, &[]),
	);
	next_id += 1;

	let mut prev_top: Option<usize> = None;
	let mut replace_target = root;
	let mut recorded = false;

	while ops.len() < target {
		let roll = rng.below(100);
		if roll < 12 {
			// Heading.
			let level = (rng.below(6) + 1) as i32;
			let hd = push_control(
				&mut ops,
				Some(root),
				prev_top,
				DOC,
				next_id,
				true,
				mshtml_attrs(
					&format!("H{level}"),
					ROLE_TEXT,
					&[ST_READONLY],
					"en",
					0,
					&[("role", "heading"), ("level", "2")],
				),
			);
			next_id += 1;
			let n = 3 + rng.below(6) as usize;
			push_text(&mut ops, Some(hd), None, make_words(rng, n));
			prev_top = Some(hd);
		} else if roll < 62 {
			// Paragraph with optional inline strong + link runs.
			let para = push_control(
				&mut ops,
				Some(root),
				prev_top,
				DOC,
				next_id,
				true,
				mshtml_attrs("P", ROLE_TEXT, &[ST_READONLY], "en", 0, &[]),
			);
			next_id += 1;
			let n = 8 + rng.below(20) as usize;
			let t = push_text(&mut ops, Some(para), None, make_words(rng, n));
			let mut prev_inline = Some(t);
			if rng.below(100) < 35 {
				let strong = push_control(
					&mut ops,
					Some(para),
					prev_inline,
					DOC,
					next_id,
					false,
					mshtml_attrs(
						"STRONG",
						ROLE_TEXT,
						&[ST_READONLY],
						"en",
						FS_STRONG,
						&[],
					),
				);
				next_id += 1;
				push_text(&mut ops, Some(strong), None, make_words(rng, 2));
				prev_inline = Some(strong);
			}
			if rng.below(100) < 40 {
				let link = push_control(
					&mut ops,
					Some(para),
					prev_inline,
					DOC,
					next_id,
					false,
					mshtml_attrs(
						"A",
						ROLE_LINK,
						&[ST_FOCUSABLE, ST_LINKED],
						"en",
						0,
						&[("role", "link")],
					),
				);
				next_id += 1;
				let n = 1 + rng.below(4) as usize;
				push_text(&mut ops, Some(link), None, make_words(rng, n));
			}
			prev_top = Some(para);
			if !recorded && ops.len() >= target / 2 {
				replace_target = para;
				recorded = true;
			}
		} else if roll < 78 {
			// Unordered list.
			let list = push_control(
				&mut ops,
				Some(root),
				prev_top,
				DOC,
				next_id,
				true,
				mshtml_attrs("UL", ROLE_LIST, &[ST_READONLY], "en", 0, &[]),
			);
			next_id += 1;
			let items = 2 + rng.below(5) as usize;
			let mut prev_item: Option<usize> = None;
			for _ in 0..items {
				let item = push_control(
					&mut ops,
					Some(list),
					prev_item,
					DOC,
					next_id,
					true,
					mshtml_attrs(
						"LI",
						ROLE_LISTITEM,
						&[ST_READONLY],
						"en",
						0,
						&[],
					),
				);
				next_id += 1;
				let n = 2 + rng.below(6) as usize;
				push_text(&mut ops, Some(item), None, make_words(rng, n));
				prev_item = Some(item);
			}
			prev_top = Some(list);
		} else if roll < 90 {
			// Small table: header row + data rows, cells carrying the
			// table-* attributes MSHTML emits.
			let table = push_control(
				&mut ops,
				Some(root),
				prev_top,
				DOC,
				next_id,
				true,
				mshtml_attrs(
					"TABLE",
					ROLE_TABLE,
					&[ST_READONLY],
					"en",
					0,
					&[("table-id", "1")],
				),
			);
			next_id += 1;
			let cols = 2 + rng.below(3) as usize;
			let table_rows = 2 + rng.below(3) as usize;
			let mut prev_row: Option<usize> = None;
			for r in 0..table_rows {
				let row = push_control(
					&mut ops,
					Some(table),
					prev_row,
					DOC,
					next_id,
					true,
					mshtml_attrs("TR", ROLE_ROW, &[ST_READONLY], "en", 0, &[]),
				);
				next_id += 1;
				let mut prev_cell: Option<usize> = None;
				for c in 0..cols {
					let tag = if r == 0 { "TH" } else { "TD" };
					let cell = push_control(
						&mut ops,
						Some(row),
						prev_cell,
						DOC,
						next_id,
						true,
						mshtml_attrs(
							tag,
							ROLE_CELL,
							&[ST_READONLY],
							"en",
							0,
							&[
								("table-id", "1"),
								(
									"table-rownumber",
									&format!("{}", r + 1),
								),
								(
									"table-columnnumber",
									&format!("{}", c + 1),
								),
							],
						),
					);
					next_id += 1;
					push_text(&mut ops, Some(cell), None, make_words(rng, 2));
					prev_cell = Some(cell);
				}
				prev_row = Some(row);
			}
			prev_top = Some(table);
		} else {
			// Standalone link.
			let link = push_control(
				&mut ops,
				Some(root),
				prev_top,
				DOC,
				next_id,
				false,
				mshtml_attrs(
					"A",
					ROLE_LINK,
					&[ST_FOCUSABLE, ST_LINKED],
					"en",
					0,
					&[("role", "link")],
				),
			);
			next_id += 1;
			let n = 1 + rng.below(4) as usize;
			push_text(&mut ops, Some(link), None, make_words(rng, n));
			prev_top = Some(link);
		}
	}
	Workload {
		ops,
		replace_target,
	}
}

// ---------------------------------------------------------------------
// Replay: build a tree from the op list against each engine.
// ---------------------------------------------------------------------

fn build_rust(ops: &[BuildOp]) -> (Buffer, Vec<NodeKey>) {
	let mut buf = Buffer::new();
	let mut handles: Vec<NodeKey> = Vec::with_capacity(ops.len());
	for op in ops {
		match op {
			BuildOp::Control {
				parent,
				previous,
				doc_handle,
				id,
				is_block,
				attrs,
			} => {
				let key = buf
					.add_control_field_node(
						parent.map(|i| handles[i]),
						previous.map(|i| handles[i]),
						ControlFieldIdentifier {
							doc_handle: *doc_handle,
							id: *id,
						},
						*is_block,
					)
					.expect("rust add_control_field_node failed");
				if !attrs.is_empty() {
					let node = buf.get_mut(key).unwrap();
					for (name, value) in attrs {
						node.add_attribute(name, value);
					}
				}
				handles.push(key);
			}
			BuildOp::Text {
				parent,
				previous,
				text,
			} => {
				let key = buf
					.add_text_field_node(
						parent.map(|i| handles[i]),
						previous.map(|i| handles[i]),
						text.clone(),
					)
					.expect("rust add_text_field_node failed");
				handles.push(key);
			}
		}
	}
	(buf, handles)
}

fn build_cpp(ops: &[BuildOp]) -> (CppBuffer, Vec<*mut c_void>) {
	let buf = CppBuffer::new();
	let mut handles: Vec<*mut c_void> = Vec::with_capacity(ops.len());
	let null = std::ptr::null_mut();
	for op in ops {
		match op {
			BuildOp::Control {
				parent,
				previous,
				doc_handle,
				id,
				is_block,
				attrs,
			} => {
				let node = unsafe {
					vbench_add_control(
						buf.ptr(),
						parent.map(|i| handles[i]).unwrap_or(null),
						previous.map(|i| handles[i]).unwrap_or(null),
						*doc_handle,
						*id,
						*is_block as i32,
					)
				};
				assert!(!node.is_null(), "cpp add_control failed");
				for (name, value) in attrs {
					unsafe {
						vbench_node_add_attribute(
							node,
							name.as_ptr(),
							name.len(),
							value.as_ptr(),
							value.len(),
						);
					}
				}
				handles.push(node);
			}
			BuildOp::Text {
				parent,
				previous,
				text,
			} => {
				let node = unsafe {
					vbench_add_text(
						buf.ptr(),
						parent.map(|i| handles[i]).unwrap_or(null),
						previous.map(|i| handles[i]).unwrap_or(null),
						text.as_ptr(),
						text.len(),
					)
				};
				assert!(!node.is_null(), "cpp add_text failed");
				handles.push(node);
			}
		}
	}
	(buf, handles)
}

/// Build the small replacement subtree used by the `replace_subtrees`
/// benchmark, in a fresh Rust temp buffer. Uses high, collision-free
/// identifiers.
fn build_rust_temp() -> Buffer {
	let mut temp = Buffer::new();
	let root = temp
		.add_control_field_node(
			None,
			None,
			ControlFieldIdentifier {
				doc_handle: 999,
				id: 1,
			},
			true,
		)
		.unwrap();
	temp.get_mut(root).unwrap().add_attribute(&w("role"), &w("paragraph"));
	let t1 = temp
		.add_text_field_node(Some(root), None, w("replacement content one"))
		.unwrap();
	temp.add_text_field_node(Some(root), Some(t1), w("replacement content two"))
		.unwrap();
	temp
}

/// Build the same replacement subtree in a fresh C++ temp buffer.
fn build_cpp_temp() -> CppBuffer {
	let temp = CppBuffer::new();
	let null = std::ptr::null_mut();
	let role = w("role");
	let para = w("paragraph");
	let root = unsafe {
		vbench_add_control(temp.ptr(), null, null, 999, 1, 1)
	};
	unsafe {
		vbench_node_add_attribute(
			root,
			role.as_ptr(),
			role.len(),
			para.as_ptr(),
			para.len(),
		);
	}
	let t1txt = w("replacement content one");
	let t2txt = w("replacement content two");
	let t1 = unsafe {
		vbench_add_text(temp.ptr(), root, null, t1txt.as_ptr(), t1txt.len())
	};
	unsafe {
		vbench_add_text(temp.ptr(), root, t1, t2txt.as_ptr(), t2txt.len());
	}
	temp
}

// ---------------------------------------------------------------------
// A prepared case: a workload plus prebuilt buffers and query params for a
// single (size, shape).
// ---------------------------------------------------------------------

struct Case {
	label: String,
	workload: Workload,
	rust_buf: Buffer,
	cpp_buf: CppBuffer,
	text_len: i32,
	/// ~100 pseudo-random offsets across [0, text_len).
	offsets: Vec<i32>,
	/// attribs / regexp for a "find next heading" quick-nav search, built
	/// the way source/virtualBuffers/__init__.py::_prepareForFindByAttributes
	/// builds them for attribs = {"role": ["heading"]}.
	find_attribs: Vec<u16>,
	find_regexp: Vec<u16>,
}

const SIZES: &[(&str, usize)] = &[
	("small", 200),
	("medium", 2000),
	("large", 10000),
];

/// Depth caps per size keep the deep_nested recursion within the main
/// thread's ~1 MB Windows stack for both getTextInRange (recursive) and
/// calculateOffsetInTree (recursive).
fn deep_cap(target: usize) -> usize {
	match target {
		0..=500 => 100,
		501..=5000 => 700,
		_ => 1000,
	}
}

fn build_cases() -> Vec<Case> {
	let mut cases = Vec::new();
	for &(size_name, target) in SIZES {
		// NB the per-(size,shape) seed below mixes in shape.len(); keep new
		// shape names a distinct length from the others ("mshtml" = 6) so
		// they don't collide and the existing baselines stay unchanged.
		for shape in
			["wide_shallow", "deep_nested", "realistic_mixed", "mshtml"]
		{
			// Fixed seed per (size, shape) -> deterministic across runs.
			let mut rng = XorShift::new(
				0x9E3779B97F4A7C15u64
					^ (target as u64)
					^ (shape.len() as u64) << 32,
			);
			let workload = match shape {
				"wide_shallow" => gen_wide_shallow(target, &mut rng),
				"deep_nested" => {
					gen_deep_nested(target, &mut rng, deep_cap(target))
				}
				"mshtml" => gen_mshtml_document(target, &mut rng),
				_ => gen_realistic_mixed(target, &mut rng),
			};

			let (rust_buf, _rk) = build_rust(&workload.ops);
			let (cpp_buf, _ck) = build_cpp(&workload.ops);

			let rlen = rust_buf.text_length();
			let clen = unsafe { vbench_get_text_length(cpp_buf.ptr()) };
			assert_eq!(
				rlen, clen,
				"text length mismatch for {size_name}/{shape}: rust={rlen} cpp={clen}"
			);

			// Deterministic offset set (same for both engines).
			let mut orng = XorShift::new(0xD1B54A32D192ED03u64 ^ rlen as u64);
			let offsets: Vec<i32> = (0..100)
				.map(|_| {
					if rlen > 0 {
						orng.below(rlen as u32) as i32
					} else {
						0
					}
				})
				.collect();

			// _prepareForFindByAttributes({"role": ["heading"]}) ->
			// reqAttrs = "role", regexp = "role:(?:heading;)".
			let find_attribs = w("role");
			let find_regexp = w("role:(?:heading;)");

			cases.push(Case {
				label: format!("{size_name}/{shape}"),
				workload,
				rust_buf,
				cpp_buf,
				text_len: rlen,
				offsets,
				find_attribs,
				find_regexp,
			});
		}
	}
	cases
}

// ---------------------------------------------------------------------
// Benchmark groups.
// ---------------------------------------------------------------------

fn bench_construct(c: &mut Criterion, cases: &[Case]) {
	let mut g = c.benchmark_group("construct");
	for case in cases {
		let ops = &case.workload.ops;
		g.bench_with_input(
			BenchmarkId::new("rust", &case.label),
			ops,
			|b, ops| {
				b.iter(|| {
					// Return the buffer so criterion drops it (frees the
					// arena) outside the timed region.
					let (buf, _h) = build_rust(black_box(ops));
					buf
				})
			},
		);
		g.bench_with_input(
			BenchmarkId::new("cpp", &case.label),
			ops,
			|b, ops| {
				b.iter(|| {
					let (buf, _h) = build_cpp(black_box(ops));
					buf
				})
			},
		);
	}
	g.finish();
}

fn bench_get_text_length(c: &mut Criterion, cases: &[Case]) {
	let mut g = c.benchmark_group("get_text_length");
	for case in cases {
		g.bench_with_input(
			BenchmarkId::new("rust", &case.label),
			case,
			|b, case| b.iter(|| black_box(case.rust_buf.text_length())),
		);
		g.bench_with_input(
			BenchmarkId::new("cpp", &case.label),
			case,
			|b, case| {
				b.iter(|| {
					black_box(unsafe {
						vbench_get_text_length(case.cpp_buf.ptr())
					})
				})
			},
		);
	}
	g.finish();
}

fn bench_get_text_in_range(c: &mut Criterion, cases: &[Case], markup: bool) {
	let name = if markup {
		"get_text_in_range_markup"
	} else {
		"get_text_in_range_plain"
	};
	let mut g = c.benchmark_group(name);
	for case in cases {
		let len = case.text_len;
		g.bench_with_input(
			BenchmarkId::new("rust", &case.label),
			case,
			|b, case| {
				b.iter(|| {
					let mut out: Vec<u16> = Vec::new();
					case.rust_buf.buffer_get_text_in_range(
						0, len, &mut out, markup,
					);
					black_box(out.len())
				})
			},
		);
		g.bench_with_input(
			BenchmarkId::new("cpp", &case.label),
			case,
			|b, case| {
				b.iter(|| {
					black_box(unsafe {
						vbench_get_text_in_range(
							case.cpp_buf.ptr(),
							0,
							len,
							markup as i32,
						)
					})
				})
			},
		);
	}
	g.finish();
}

fn bench_find_node_by_attributes(c: &mut Criterion, cases: &[Case]) {
	let mut g = c.benchmark_group("find_node_by_attributes");
	for case in cases {
		g.bench_with_input(
			BenchmarkId::new("rust", &case.label),
			case,
			|b, case| {
				b.iter(|| {
					black_box(case.rust_buf.find_node_by_attributes(
						0,
						FindDirection::Forward,
						&case.find_attribs,
						&case.find_regexp,
					))
				})
			},
		);
		g.bench_with_input(
			BenchmarkId::new("cpp", &case.label),
			case,
			|b, case| {
				b.iter(|| {
					black_box(unsafe {
						vbench_find_node_by_attributes(
							case.cpp_buf.ptr(),
							0,
							0, // forward
							case.find_attribs.as_ptr(),
							case.find_attribs.len(),
							case.find_regexp.as_ptr(),
							case.find_regexp.len(),
						)
					})
				})
			},
		);
	}
	g.finish();
}

fn bench_locate_text_field(c: &mut Criterion, cases: &[Case]) {
	let mut g = c.benchmark_group("locate_text_field_at_offset");
	for case in cases {
		g.bench_with_input(
			BenchmarkId::new("rust", &case.label),
			case,
			|b, case| {
				b.iter(|| {
					let mut acc = 0i64;
					for &off in &case.offsets {
						if let Some(r) = case
							.rust_buf
							.buffer_locate_text_field_node_at_offset(off)
						{
							acc += r.start as i64;
						}
					}
					black_box(acc)
				})
			},
		);
		g.bench_with_input(
			BenchmarkId::new("cpp", &case.label),
			case,
			|b, case| {
				b.iter(|| {
					let mut acc = 0i64;
					for &off in &case.offsets {
						acc += unsafe {
							vbench_locate_text_field_at_offset(
								case.cpp_buf.ptr(),
								off,
							)
						} as i64;
					}
					black_box(acc)
				})
			},
		);
	}
	g.finish();
}

fn bench_get_line_offsets(c: &mut Criterion, cases: &[Case]) {
	const MAX_LINE: i32 = 100;
	let mut g = c.benchmark_group("get_line_offsets");
	for case in cases {
		g.bench_with_input(
			BenchmarkId::new("rust", &case.label),
			case,
			|b, case| {
				b.iter(|| {
					let mut acc = 0i64;
					for &off in &case.offsets {
						if let Some((s, e)) =
							case.rust_buf.line_offsets(off, MAX_LINE, true)
						{
							acc += (e - s) as i64;
						}
					}
					black_box(acc)
				})
			},
		);
		g.bench_with_input(
			BenchmarkId::new("cpp", &case.label),
			case,
			|b, case| {
				b.iter(|| {
					let mut acc = 0i64;
					let mut s = 0i32;
					let mut e = 0i32;
					for &off in &case.offsets {
						let ok = unsafe {
							vbench_get_line_offsets(
								case.cpp_buf.ptr(),
								off,
								MAX_LINE,
								1,
								&mut s,
								&mut e,
							)
						};
						if ok != 0 {
							acc += (e - s) as i64;
						}
					}
					black_box(acc)
				})
			},
		);
	}
	g.finish();
}

fn bench_replace_subtrees(c: &mut Criterion, cases: &[Case]) {
	let mut g = c.benchmark_group("replace_subtrees");
	for case in cases {
		let ops = &case.workload.ops;
		let target = case.workload.replace_target;

		g.bench_with_input(
			BenchmarkId::new("rust", &case.label),
			case,
			|b, _case| {
				b.iter_batched(
					|| {
						// Fresh main buffer + fresh temp each iteration
						// (the merge mutates both).
						let (main, handles) = build_rust(ops);
						let temp = build_rust_temp();
						(main, handles[target], temp)
					},
					|(mut main, target_key, temp)| {
						black_box(
							main.replace_subtrees(vec![(target_key, temp)]),
						);
						main
					},
					BatchSize::SmallInput,
				)
			},
		);

		g.bench_with_input(
			BenchmarkId::new("cpp", &case.label),
			case,
			|b, _case| {
				b.iter_batched(
					|| {
						let (main, handles) = build_cpp(ops);
						let temp = build_cpp_temp();
						(main, handles[target], temp)
					},
					|(main, target_node, temp)| {
						// replaceSubtrees consumes/deletes the temp buffer;
						// forget our wrapper so its Drop doesn't double-free.
						let temp_ptr = temp.ptr();
						std::mem::forget(temp);
						let r = unsafe {
							vbench_replace_subtrees_one(
								main.ptr(),
								target_node,
								temp_ptr,
							)
						};
						black_box(r);
						main
					},
					BatchSize::SmallInput,
				)
			},
		);
	}
	g.finish();
}

fn all_benches(c: &mut Criterion) {
	let cases = build_cases();
	bench_construct(c, &cases);
	bench_get_text_length(c, &cases);
	bench_get_text_in_range(c, &cases, false);
	bench_get_text_in_range(c, &cases, true);
	bench_find_node_by_attributes(c, &cases);
	bench_locate_text_field(c, &cases);
	bench_get_line_offsets(c, &cases);
	bench_replace_subtrees(c, &cases);
}

criterion_group!(benches, all_benches);
criterion_main!(benches);
