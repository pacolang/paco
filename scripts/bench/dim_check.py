#!/usr/bin/env python3
"""Median wall time of `paco check` on a generated program with 2 000
dimension-generic calls (static `const` dimensions), the same program over
named run-time dimensions (`dim` parameters, witnesses, `assume_dims`), and
the same program with no dimensions.

    PACO_BIN  paco binary (default: compiler/target/release/paco)
    RUNS      runs per program (default: 10)
    CALLS     generic calls per program (default: 2000)
    NAMED     0 skips the named-dimension variant (for compilers without it)
"""
import os
import statistics
import subprocess
import sys
import tempfile
import time

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
PACO = os.environ.get("PACO_BIN", os.path.join(ROOT, "compiler/target/release/paco"))
RUNS = int(os.environ.get("RUNS", "10"))
CALLS = int(os.environ.get("CALLS", "2000"))
PER_FN = 100


NAMED_HEAD = """struct Grid<T: Numeric, const D: int...> {
    data: []T,

    pub fn zeros() -> Self {
        Grid { data: slice_of_zeros<T>(1) }
    }

    pub fn extent(&self, axis: i64) -> i64 {
        1
    }
}

fn widen<dim M, dim K>(a: &Grid<i64, M, K>, b: &Grid<i64, K, M * 2>) -> Grid<i64, M * 2 + K, K> {
    Grid<i64, M * 2 + K, K>::zeros()
}

fn same<dim M>(a: Grid<i64, M * 2, 4>) -> Grid<i64, 2 * M, 4> {
    a
}
"""


def named_program() -> str:
    out = [NAMED_HEAD]
    for f in range(CALLS // PER_FN):
        out.append(f"fn part{f}() -> i64 {{\n    let mut total = 0;")
        for i in range(PER_FN // 2):
            out.append(f"    let a{i} = Grid<i64, Dyn, Dyn>::zeros();")
            out.append(f"    let m{i} = a{i}.dim(0);")
            out.append(f"    let k{i} = a{i}.dim(1);")
            out.append(f"    let b{i}: Grid<i64, k{i}, m{i} * 2> = unsafe {{ Grid<i64, Dyn, Dyn>::zeros().assume_dims() }};")
            out.append(f"    let c{i}: Grid<i64, m{i} * 2 + k{i}, k{i}> = widen(&a{i}, &b{i});")
            out.append(f"    let d{i}: Grid<i64, m{i} + m{i}, 4> = same<m{i}>(unsafe {{ Grid<i64, Dyn, 4>::zeros().assume_dims() }});")
            out.append(f"    total = total + c{i}.data.len() + d{i}.data.len();")
        out.append("    total\n}\n")
    out.append("fn main() {\n    let mut total = 0;")
    for f in range(CALLS // PER_FN):
        out.append(f"    total = total + part{f}();")
    out.append("    print(total);\n}\n")
    return "\n".join(out)


def program(dims: bool) -> str:
    if dims:
        head = """struct Grid<T: Numeric, const D: int...> {
    data: []T,

    pub fn zeros() -> Self {
        Grid { data: slice_of_zeros<T>(1) }
    }
}

fn widen<const M: int, const K: int>(a: &Grid<i64, M, K>, b: &Grid<i64, K, M * 2>) -> Grid<i64, M * 2 + K, K> {
    Grid<i64, M * 2 + K, K>::zeros()
}

fn same<const M: int>(a: Grid<i64, M * 2, 4>) -> Grid<i64, 2 * M, 4> {
    a
}
"""
        grid = lambda *d: "Grid<i64, " + ", ".join(str(x) for x in d) + ">"
    else:
        head = """struct Grid<T: Numeric> {
    data: []T,

    pub fn zeros() -> Self {
        Grid { data: slice_of_zeros<T>(1) }
    }
}

fn widen(a: &Grid<i64>, b: &Grid<i64>) -> Grid<i64> {
    Grid<i64>::zeros()
}

fn same(a: Grid<i64>) -> Grid<i64> {
    a
}
"""
        grid = lambda *d: "Grid<i64>"
    out = [head]
    for f in range(CALLS // PER_FN):
        out.append(f"fn part{f}() -> i64 {{\n    let mut total = 0;")
        for i in range(PER_FN // 2):
            m, k = 1 + (i % 7), 2 + (i % 5)
            out.append(f"    let a{i}: {grid(m, k)} = {grid(m, k)}::zeros();")
            out.append(f"    let b{i}: {grid(k, m * 2)} = {grid(k, m * 2)}::zeros();")
            out.append(f"    let c{i}: {grid(m * 2 + k, k)} = widen(&a{i}, &b{i});")
            out.append(f"    let d{i}: {grid(m * 2, 4)} = same{'<' + str(m) + '>' if dims else ''}({grid(m * 2, 4)}::zeros());")
            out.append(f"    total = total + c{i}.data.len() + d{i}.data.len();")
        out.append("    total\n}\n")
    out.append("fn main() {\n    let mut total = 0;")
    for f in range(CALLS // PER_FN):
        out.append(f"    total = total + part{f}();")
    out.append("    print(total);\n}\n")
    return "\n".join(out)


def median_ms(path: str) -> float:
    first = subprocess.run([PACO, "check", path], capture_output=True, text=True)
    if first.returncode != 0:
        sys.exit(f"paco check failed on {path}:\n{first.stderr}")
    times = []
    for _ in range(RUNS):
        start = time.perf_counter()
        subprocess.run([PACO, "check", path], check=True, capture_output=True)
        times.append(time.perf_counter() - start)
    return statistics.median(times) * 1000


def main() -> int:
    if not os.access(PACO, os.X_OK):
        print(f"no paco binary at {PACO} (cargo build --release -p paco-driver)", file=sys.stderr)
        return 2
    with tempfile.TemporaryDirectory() as work:
        variants = [("dims", program(True)), ("nodims", program(False))]
        if os.environ.get("NAMED", "1") != "0":
            variants.append(("named", named_program()))
        for name, source in variants:
            path = os.path.join(work, f"{name}.paco")
            with open(path, "w") as f:
                f.write(source)
            print(f"{name}: median {median_ms(path):.1f} ms over {RUNS} runs ({CALLS} calls)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
