# paco-codegen-llvm

The optimizing backend behind `paco build --release`. It lowers MIR to LLVM
IR through `inkwell`, runs LLVM's `default<O3>` pipeline and emits a native
object file for the host or for `--target <triple>`.

## Required LLVM

**LLVM 18.1** (`inkwell` feature `llvm18-1-prefer-dynamic`, `llvm-sys` 181),
linked dynamically against `libLLVM-18`. The build finds `llvm-config` (or
`llvm-config-18`) on `PATH`; otherwise set `LLVM_SYS_181_PREFIX` to the LLVM
install prefix.

- Debian/Ubuntu: `apt install llvm-18-dev`
- macOS: `brew install llvm@18` and `LLVM_SYS_181_PREFIX=$(brew --prefix llvm@18)`
- CI builds without LLVM (`--no-default-features`): the LLVM legs of the
  test suite are skipped there and run locally when LLVM is present.

The backend is behind this crate's `llvm` feature, which `paco-driver`
enables by default; building the driver with `--no-default-features` drops
the LLVM dependency and makes `paco build --release` report an error.

Cross-compiling (`--target`) additionally needs the target's Rust standard
library (`rustup target add <triple>`) to build the runtime, and a C cross
toolchain named `<arch>-linux-gnu-gcc` to link.
