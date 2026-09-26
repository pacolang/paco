#!/usr/bin/env python3
"""Median run time of a 10^6-iteration element-wise loop over a `Dyn`
grid with `checked_add` (a run-time shape comparison and a `Result` per
iteration) versus the same loop after one `with_dims` refinement, where `+`
carries no check. Both are built with `paco build --release`.

    PACO_BIN  paco binary (default: compiler/target/release/paco)
    RUNS      runs per program (default: 15)
"""
import os
import shutil
import statistics
import subprocess
import sys
import tempfile
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
PACO = os.environ.get("PACO_BIN", os.path.join(ROOT, "compiler/target/release/paco"))
RUNS = int(os.environ.get("RUNS", "15"))


def main() -> int:
    with tempfile.TemporaryDirectory() as work:
        binaries = {}
        for name in ("checked", "refined"):
            source = os.path.join(work, f"{name}.paco")
            shutil.copy(os.path.join(HERE, f"dims_loop_{name}.paco"), source)
            subprocess.run([PACO, "build", "--release", source], check=True)
            binaries[name] = os.path.join(work, name)
        outputs = {name: subprocess.run([path], check=True, capture_output=True, text=True).stdout for name, path in binaries.items()}
        if outputs["checked"] != outputs["refined"]:
            sys.exit(f"outputs differ: {outputs}")
        times = {name: [] for name in binaries}
        for run in range(RUNS):
            for name in (("checked", "refined") if run % 2 == 0 else ("refined", "checked")):
                start = time.perf_counter()
                subprocess.run([binaries[name]], check=True, capture_output=True)
                times[name].append((time.perf_counter() - start) * 1000)
        for name, samples in times.items():
            print(f"{name}: median {statistics.median(samples):.1f} ms over {RUNS} runs")
    return 0


if __name__ == "__main__":
    sys.exit(main())
