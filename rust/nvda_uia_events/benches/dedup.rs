//! Microbenchmarks for the UIA event-limiter's pure dedup core.
//!
//! This characterises the per-event work the rate-limiter does (coalescing
//! key construction + the insertion-ordered dedup queue) and serves as a
//! regression guard. It is Rust-only: unlike the vbuf storage benchmark
//! there is no fair C++ head-to-head here, because the shipping C++ dedup
//! is entangled with COM (its key comes from `IUIAutomationElement::
//! GetRuntimeId`) and this is an event-rate path, not a throughput hot loop
//! — the value of the port is safety/correctness, not raw speed. Run:
//!   cargo bench -p nvda_uia_events

use criterion::{
    black_box, criterion_group, criterion_main, BatchSize, BenchmarkId,
    Criterion, Throughput,
};
use nvda_uia_events::dedup::OrderedDedup;
use nvda_uia_events::event::EventKind;

/// Insert a fixed number of events whose keys cycle over `n_keys` distinct
/// values (so `n_keys` controls the coalescing ratio: fewer keys => more
/// duplicates collapsed), then drain. Models one flush window.
fn bench_insert_drain(c: &mut Criterion) {
    const N_EVENTS: usize = 2000;
    let mut group = c.benchmark_group("dedup_insert_drain");
    group.throughput(Throughput::Elements(N_EVENTS as u64));
    for n_keys in [N_EVENTS, N_EVENTS / 2, 100, 10, 1] {
        // RuntimeId-shaped keys (a couple of prefix ints + a cycling id),
        // pre-built so the loop measures the queue, not key generation.
        let keys: Vec<Vec<i32>> = (0..N_EVENTS)
            .map(|i| vec![1, 2, (i % n_keys) as i32])
            .collect();
        group.bench_with_input(
            BenchmarkId::from_parameter(n_keys),
            &keys,
            |b, keys| {
                b.iter_batched(
                    OrderedDedup::<u64>::new,
                    |mut q| {
                        for (i, key) in keys.iter().enumerate() {
                            // Fresh owned key per insert, as in production
                            // (each event computes its own key).
                            q.insert(key.clone(), i as u64);
                        }
                        black_box(q.drain().len())
                    },
                    BatchSize::SmallInput,
                );
            },
        );
    }
    group.finish();
}

/// Coalescing-key construction for the two non-trivial event kinds.
fn bench_coalescing_key(c: &mut Criterion) {
    let rid = [42, 7, 99, 123456];
    let mut group = c.benchmark_group("coalescing_key");
    group.bench_function("property", |b| {
        let ev = EventKind::PropertyChanged { property_id: 30003 };
        b.iter(|| black_box(ev.coalescing_key(black_box(&rid))));
    });
    group.bench_function("notification", |b| {
        let ev = EventKind::Notification {
            kind: 2,
            processing: 1,
            activity_id: Some("activity".encode_utf16().collect()),
        };
        b.iter(|| black_box(ev.coalescing_key(black_box(&rid))));
    });
    group.finish();
}

criterion_group!(benches, bench_insert_drain, bench_coalescing_key);
criterion_main!(benches);
