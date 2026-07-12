//! Compiles the original C++ vbuf storage into a static library the
//! benchmark links against, so the criterion harness can call
//! `VBufStorage_buffer_t` directly and compare it to the Rust port in one
//! process.
//!
//! `storage.cpp` is self-contained: it pulls in only std headers plus the
//! header-only `common/xml.h`, `common/log.h` (whose `LOG_*` macros become
//! no-ops at `LOGLEVEL=60`), and `utils.h`/`utils.cpp`. No backend / RPC /
//! COM dependencies. The flags mirror what scons uses for the production
//! nvdaHelper C++ (see `nvdaHelper/remote/sconscript` /
//! `sconstruct`): C++20, unicode, `NOMINMAX`, Win10 target, `NDEBUG`.
//! `/O2` is supplied by cc automatically for the (release) bench profile.

use std::path::PathBuf;

fn main() {
	let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
	// rust/vbuf_bench -> rust -> repo root.
	let repo_root = manifest
		.parent()
		.and_then(|p| p.parent())
		.expect("repo root")
		.to_path_buf();
	let nvda_helper = repo_root.join("nvdaHelper");
	let vbuf_base = nvda_helper.join("vbufBase");
	let include = repo_root.join("include");

	let storage_cpp = vbuf_base.join("storage.cpp");
	let utils_cpp = vbuf_base.join("utils.cpp");
	let shim_cpp = manifest.join("cpp").join("bench_shim.cpp");

	let mut build = cc::Build::new();
	build
		.cpp(true)
		.include(&nvda_helper)
		.include(&vbuf_base)
		.include(&include)
		// LOGLEVEL_NONE: every LOG_* macro compiles to nothing, so no
		// logMessage symbol is required (we also define a no-op one in
		// the shim, belt-and-braces).
		.define("LOGLEVEL", "60")
		.define("UNICODE", None)
		.define("_UNICODE", None)
		.define("NOMINMAX", None)
		.define("_WIN32_WINNT", "0x0A00")
		.define("NDEBUG", None)
		.flag_if_supported("/std:c++20")
		.flag_if_supported("/EHsc")
		.file(&storage_cpp)
		.file(&utils_cpp)
		.file(&shim_cpp);

	build.compile("vbuf_bench_cpp");

	println!("cargo:rerun-if-changed={}", shim_cpp.display());
	println!("cargo:rerun-if-changed={}", storage_cpp.display());
	println!("cargo:rerun-if-changed={}", utils_cpp.display());
	println!("cargo:rerun-if-changed={}", vbuf_base.join("storage.h").display());
	println!("cargo:rerun-if-changed={}", vbuf_base.join("utils.h").display());
}
