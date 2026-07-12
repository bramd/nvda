//! Intentionally empty.
//!
//! `vbuf_bench` carries no library code; it exists purely as a home for the
//! `benches/storage.rs` criterion harness plus the `build.rs` that compiles
//! the C++ `VBufStorage_buffer_t` (from `nvdaHelper/vbufBase`) into a static
//! library the benchmark links against. Having a `lib` target ensures the
//! build-script's native library is linked into the bench binaries.
