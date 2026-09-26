mod autodiff;
mod backend_parity;
mod bench_alloc;
mod bench_autodiff;
mod build;
mod check_borrowing;
mod check_data_types;
mod check_ownership;
mod check_slices;
mod check_std;
mod cli;
mod comptime_commands;
mod comptime_reentry;
mod concurrency_codegen_differential;
mod concurrency_front_end;
mod conformance;
mod cross_file_modules;
mod deref_and_tuples;
mod derive_deliverable;
mod derive_display;
mod derive_expansion;
mod derive_serialize;
mod differential;
mod differential_corpus;
mod dist_stdlib_root;
mod ffi_extern_unsafe;
mod fix_command;
mod fixed_width_numeric_types;
mod generic_aggregate_codegen_differential;
mod generic_enum_variant_layout_inference;
mod http_server_example;
mod iterator_protocol;
mod iterator_protocol_compiled;
mod leak_check;
mod matrix_shapes;
mod module_fetch;
mod numerics;
mod operator_overloading;
mod paco_test_command;
mod reduced_floats;
mod run_cache;
mod run_command;
mod run_core_programs;
mod run_data_types;
mod run_pattern_matching;
mod run_semantics;
mod run_latency;
mod slice_bounds_check;
mod slices_differential;
mod std_test_assertions;
mod struct_return_value_codegen;
mod test_assertions;
mod test_attribute_placement;
mod test_file_exclusion;
mod toolchain;
mod unsafe_ffi_codegen;

/// Whether this paco has the LLVM backend (`paco build --release`); built
/// without the `llvm` feature, tests skip their LLVM legs.
fn llvm_backend() -> bool {
    cfg!(feature = "llvm")
}

/// The `paco build` flag sets that exercise both backends, without the LLVM
/// one when paco is built without it.
fn backend_flag_sets() -> Vec<&'static [&'static str]> {
    let mut sets: Vec<&'static [&'static str]> = vec![&["--backend", "cranelift"]];
    if llvm_backend() {
        sets.push(&["--release", "--backend", "llvm"]);
    }
    sets
}

/// The backends `paco build --backend` accepts in this build of paco.
fn backends() -> &'static [&'static str] {
    if llvm_backend() { &["llvm", "cranelift"] } else { &["cranelift"] }
}
