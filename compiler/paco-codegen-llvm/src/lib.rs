//! LLVM optimizing codegen backend: lowers `paco-mir::Body` to LLVM IR,
//! optimizes it with LLVM's standard pipeline and emits an object file.

#[cfg(feature = "llvm")]
mod codegen;
#[cfg(feature = "llvm")]
mod glue;

#[cfg(feature = "llvm")]
pub use codegen::LlvmBackend;

pub const CRATE_NAME: &str = "paco-codegen-llvm";
