"""Benchmark: a representative UIA hot path (walk a container's children,
batch-read a few properties each) done in comtypes-in-Python vs. windows-rs
via PyO3 (the `uia_bench` module).

Both sides walk the *same* deterministic test window (built by the Rust
module on a message-pumping thread), read the *same* properties, and compute
the *same* checksum — so the binding mechanism (comtypes dynamic dispatch vs.
windows-rs vtable calls) is the only variable.

Three sub-paths, because *where the time goes* is the real question:

* live  — GetCurrentPropertyValue per property (uncached). Each read marshals
  to the provider; this is dominated by UIA infrastructure, not the binding.
* cached-walk — one FindAllBuildCache fetch, then local cached reads. NVDA's
  real pattern.
* cached-read — build the cache once (outside timing), then time *only* the
  local cached reads: isolates the pure binding overhead.

Run:  uv run python rust/uia_bench/bench_uia.py
"""

import time

import uia_bench
import comtypes.client

comtypes.client.GetModule("UIAutomationCore.dll")
from comtypes.gen import UIAutomationClient as UIA  # noqa: E402

PROPS = {
	"Name": UIA.UIA_NamePropertyId,
	"ControlType": UIA.UIA_ControlTypePropertyId,
	"ClassName": UIA.UIA_ClassNamePropertyId,
	"AutomationId": UIA.UIA_AutomationIdPropertyId,
	"IsEnabled": UIA.UIA_IsEnabledPropertyId,
}
PROP_IDS = list(PROPS.values())
MASK = (1 << 64) - 1

client = comtypes.client.CreateObject(UIA.CUIAutomation, interface=UIA.IUIAutomation)


def _cksum(v) -> int:
	if isinstance(v, bool):
		return 1 if v else 0
	if isinstance(v, int):
		return v
	if isinstance(v, str):
		return len(v)
	return 0


def py_walk_live(hwnd: int) -> int:
	root = client.ElementFromHandle(hwnd)
	children = root.FindAll(UIA.TreeScope_Children, client.CreateTrueCondition())
	total = 0
	for i in range(children.Length):
		child = children.GetElement(i)
		for pid in PROP_IDS:
			total += _cksum(child.GetCurrentPropertyValue(pid))
	return total & MASK


def _build_py_cache(hwnd: int):
	root = client.ElementFromHandle(hwnd)
	cache = client.CreateCacheRequest()
	for pid in PROP_IDS:
		cache.AddProperty(pid)
	return root.FindAllBuildCache(UIA.TreeScope_Children, client.CreateTrueCondition(), cache)


def py_walk_cached(hwnd: int) -> int:
	children = _build_py_cache(hwnd)
	total = 0
	for i in range(children.Length):
		child = children.GetElement(i)
		for pid in PROP_IDS:
			total += _cksum(child.GetCachedPropertyValue(pid))
	return total & MASK


def py_read_cached(children) -> int:
	total = 0
	for i in range(children.Length):
		child = children.GetElement(i)
		for pid in PROP_IDS:
			total += _cksum(child.GetCachedPropertyValue(pid))
	return total & MASK


def bench(fn, iters: int) -> float:
	for _ in range(5):
		fn()
	t0 = time.perf_counter()
	for _ in range(iters):
		fn()
	return (time.perf_counter() - t0) / iters


def row(label: str, t: float, n: int) -> str:
	reads = n * len(PROP_IDS)
	return f"  {label:26} {t * 1e6:9.1f} us/walk  {t / reads * 1e9:8.0f} ns/read"


def run(n: int, iters: int) -> None:
	hwnd = uia_bench.make_test_window(n)
	time.sleep(0.3)

	# Fairness: every path must produce the same checksum.
	sums = {
		"py_live": py_walk_live(hwnd),
		"rs_live": uia_bench.rust_walk(hwnd, PROP_IDS),
		"py_cached": py_walk_cached(hwnd),
		"rs_cached": uia_bench.rust_walk_cached(hwnd, PROP_IDS),
	}
	ok = "all MATCH" if len(set(sums.values())) == 1 else f"*** MISMATCH {sums} ***"
	print(
		f"\nN={n} children, {len(PROP_IDS)} props ({n * len(PROP_IDS)} reads/walk), "
		f"{iters} iters — checksums {ok}",
	)

	print(" live (uncached — each read marshals to the provider):")
	print(row("comtypes", bench(lambda: py_walk_live(hwnd), iters), n))
	print(row("windows-rs (PyO3)", bench(lambda: uia_bench.rust_walk(hwnd, PROP_IDS), iters), n))

	print(" cached-walk (one fetch + local reads — NVDA's pattern):")
	t_pyc = bench(lambda: py_walk_cached(hwnd), iters)
	t_rsc = bench(lambda: uia_bench.rust_walk_cached(hwnd, PROP_IDS), iters)
	print(row("comtypes", t_pyc, n))
	print(row("windows-rs (PyO3)", t_rsc, n))
	print(f"   => comtypes / windows-rs: {t_pyc / t_rsc:5.1f}x")

	print(" cached-read only (isolates the binding — local reads):")
	py_children = _build_py_cache(hwnd)
	uia_bench.build_cache(hwnd, PROP_IDS)
	t_pyr = bench(lambda: py_read_cached(py_children), iters)
	t_rsr = bench(lambda: uia_bench.read_cached(PROP_IDS), iters)
	print(row("comtypes", t_pyr, n))
	print(row("windows-rs (PyO3)", t_rsr, n))
	print(f"   => comtypes / windows-rs: {t_pyr / t_rsr:5.1f}x")


if __name__ == "__main__":
	print("comtypes", comtypes.__version__)
	for n in (20, 200):
		run(n, iters=300)
