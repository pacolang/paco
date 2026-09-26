#!/usr/bin/env python3
"""Compile time and binary size of a program that calls one generic item
with 300 different extents: as `dim B` (run-time extents, one instance)
versus `const B: int` (literal extents, one instance each). Every build
uses an empty build cache.

    PACO_BIN  paco binary (default: compiler/target/release/paco)
    RUNS      builds per program and mode (default: 5)
    EXTENTS   distinct extents (default: 300)
"""
import os
import statistics
import subprocess
import sys
import tempfile
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
PACO = os.environ.get("PACO_BIN", os.path.join(ROOT, "compiler/target/release/paco"))
RUNS = int(os.environ.get("RUNS", "5"))
EXTENTS = int(os.environ.get("EXTENTS", "300"))

GRID = """struct Grid<T: Numeric, const D: int...> {
    data: []T,
    dims: []i64,

    #[instantiation_limit(1024)]
    pub fn zeros() -> Self {
        let dims = D;
        Grid { data: slice_of_zeros<T>(dims[0] * dims[1]), dims: dims }
    }

    pub fn with_rows(rows: i64) -> Self {
        let mut dims = slice_of_zeros<i64>(2);
        dims[0] = rows;
        dims[1] = 4;
        Grid { data: slice_of_zeros<T>(rows * 4), dims: dims }
    }

    #[instantiation_limit(1024)]
    pub fn extent(&self, axis: i64) -> i64 {
        self.dims[axis]
    }
}
"""


def program(kind: str) -> str:
    out = [GRID]
    if kind == "dim":
        out.append("fn rows<dim B>(g: &Grid<f32, B, 4>) -> i64 {\n    B + g.extent(1)\n}\n")
    else:
        out.append(f"#[instantiation_limit(1024)]\nfn rows<const B: int>(g: &Grid<f32, B, 4>) -> i64 {{\n    B + g.extent(1)\n}}\n")
    out.append("fn main() {\n    let mut total = 0;")
    for extent in range(1, EXTENTS + 1):
        if kind == "dim":
            out.append(f"    total = total + rows(&Grid<f32, Dyn, 4>::with_rows({extent}));")
        else:
            out.append(f"    total = total + rows(&Grid<f32, {extent}, 4>::zeros());")
    out.append("    print(total);\n}\n")
    return "\n".join(out)


def build(path: str, release: bool) -> tuple[float, int]:
    with tempfile.TemporaryDirectory() as cache:
        env = dict(os.environ, PACO_CACHE=cache)
        args = [PACO, "build"] + (["--release"] if release else []) + [path]
        start = time.perf_counter()
        subprocess.run(args, check=True, env=env, capture_output=True)
        elapsed = (time.perf_counter() - start) * 1000
    binary = path[: -len(".paco")]
    return elapsed, os.path.getsize(binary)


def main() -> int:
    with tempfile.TemporaryDirectory() as work:
        outputs = {}
        for kind in ("dim", "const"):
            path = os.path.join(work, f"{kind}.paco")
            with open(path, "w") as f:
                f.write(program(kind))
            for release in (False, True):
                samples = [build(path, release) for _ in range(RUNS)]
                mode = "release" if release else "debug"
                print(
                    f"{kind} {mode}: median {statistics.median(t for t, _ in samples):.0f} ms, "
                    f"binary {samples[-1][1]} bytes ({EXTENTS} extents)"
                )
            outputs[kind] = subprocess.run([path[: -len(".paco")]], check=True, capture_output=True, text=True).stdout
        if outputs["dim"] != outputs["const"]:
            sys.exit(f"outputs differ: {outputs}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
