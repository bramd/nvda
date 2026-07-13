#!/usr/bin/env python3
"""Regression guard for the vbuf storage benchmark (`vbuf_bench`).

Runs `cargo bench -p vbuf_bench`, reads Criterion's median estimates, and
compares the **Rust-vs-C++ ratio** (`rust_ns / cpp_ns`) per
`(op, size, shape)` against a committed baseline (`regression_baseline.json`).

Why the ratio and not absolute times: both engines are measured in the same
run, so the ratio cancels out machine speed and run-to-run noise that hits
both equally. A regression therefore means the **Rust** engine got slower
*relative to the unchanged C++ reference* -- which is exactly the "did our
storage rewrite regress?" signal, and it stays meaningful across machines.

Usage (run from anywhere; needs `cargo` on PATH -- no uv/venv needed):

    python regression_check.py --update        # run bench, (re)write the baseline
    python regression_check.py                 # run bench, compare, exit 1 on regression
    python regression_check.py --no-run        # compare using the existing criterion output
    python regression_check.py --threshold 20  # regression = ratio worse by >20% (default 20)
    python regression_check.py -- --sample-size 50   # pass extra args through to criterion

The baseline + this run should use comparable Criterion settings; the ratio
is fairly insensitive to sample size, but keep them in the same ballpark.
`get_text_length` is a ~1 ns floor marker -- its ratio is noisy; it is
reported but excluded from the pass/fail gate.
"""

import argparse
import json
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent  # rust/vbuf_bench
RUST_DIR = HERE.parent  # rust/
CRITERION = RUST_DIR / "target" / "criterion"
BASELINE = HERE / "regression_baseline.json"

# Default criterion settings: a balance of speed vs stability for a full
# (~192-benchmark) run. Override by passing args after `--`.
DEFAULT_BENCH_ARGS = [
	"--sample-size",
	"20",
	"--warm-up-time",
	"1.0",
	"--measurement-time",
	"1.5",
]

# Excluded from the pass/fail gate (still reported): O(1) floor markers whose
# sub-2 ns timings are dominated by measurement noise.
GATE_EXCLUDE_PREFIXES = ("get_text_length/",)


def run_bench(extra_args):
	cmd = ["cargo", "bench", "-p", "vbuf_bench", "--"] + extra_args
	print("running:", " ".join(cmd), flush=True)
	subprocess.run(cmd, cwd=RUST_DIR, check=True)


def collect():
	"""{"<group>/<size_shape>": {"rust": ns, "cpp": ns}} from criterion output."""
	out = {}
	for est in CRITERION.glob("*/*/*/new/estimates.json"):
		# criterion/<group>/<engine>/<value>/new/estimates.json
		group, engine, value = est.relative_to(CRITERION).parts[:3]
		if engine not in ("rust", "cpp"):
			continue
		median = json.loads(est.read_text())["median"]["point_estimate"]
		out.setdefault(f"{group}/{value}", {})[engine] = median
	return {k: v for k, v in out.items() if "rust" in v and "cpp" in v}


def main():
	ap = argparse.ArgumentParser(description=__doc__)
	ap.add_argument("--update", action="store_true", help="(re)write the baseline")
	ap.add_argument("--no-run", action="store_true", help="use existing criterion output")
	ap.add_argument("--threshold", type=float, default=20.0, help="regression %% on the ratio")
	ap.add_argument("bench_args", nargs="*", help="extra args passed to criterion (after --)")
	args = ap.parse_args()

	if not args.no_run:
		run_bench(args.bench_args or DEFAULT_BENCH_ARGS)

	cur = collect()
	if not cur:
		print("no criterion results found under", CRITERION, file=sys.stderr)
		return 2

	if args.update:
		BASELINE.write_text(json.dumps(cur, indent=1, sort_keys=True) + "\n")
		print(f"wrote {len(cur)} entries to {BASELINE.name}")
		return 0

	if not BASELINE.exists():
		print("no baseline; run with --update first", file=sys.stderr)
		return 2
	base = json.loads(BASELINE.read_text())

	rows = []
	regressions = []
	for key in sorted(set(base) | set(cur)):
		b, c = base.get(key), cur.get(key)
		if not b or not c:
			rows.append((key, None, None, None, "MISSING"))
			continue
		b_ratio = b["rust"] / b["cpp"]
		c_ratio = c["rust"] / c["cpp"]
		pct = (c_ratio - b_ratio) / b_ratio * 100.0
		gated = not key.startswith(GATE_EXCLUDE_PREFIXES)
		if pct > args.threshold and gated:
			flag = "REGRESSED"
			regressions.append((key, pct))
		elif pct < -args.threshold:
			flag = "faster"
		else:
			flag = ""
		rows.append((key, b_ratio, c_ratio, pct, flag))

	rows.sort(key=lambda r: (r[3] is None, -(r[3] or 0)))
	print(f"\n{'benchmark':44} {'base r/c':>9} {'now r/c':>9} {'delta%':>8}  flag")
	print("-" * 84)
	for key, br, cr, pct, flag in rows:
		if br is None:
			print(f"{key:44} {'-':>9} {'-':>9} {'-':>8}  {flag}")
		else:
			print(f"{key:44} {br:9.3f} {cr:9.3f} {pct:+8.1f}  {flag}")

	if regressions:
		print(f"\nFAIL: {len(regressions)} regression(s) > {args.threshold:.0f}% (rust/cpp ratio):")
		for key, pct in sorted(regressions, key=lambda x: -x[1]):
			print(f"  {key}: +{pct:.1f}%")
		return 1
	print(f"\nOK: no regressions > {args.threshold:.0f}% (rust/cpp ratio).")
	return 0


if __name__ == "__main__":
	sys.exit(main())
