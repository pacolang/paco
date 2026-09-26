use std::path::PathBuf;
use std::process::Command;

use cranelift_module::default_libcall_names;
use cranelift_object::{ObjectBuilder, ObjectModule};
use paco_diag::Reporter;
use paco_mir::{Profile, TypeLayouts, TypeRegistry};
use paco_span::SourceMap;
use paco_syntax::ast::Item;
use paco_syntax::{lex::lex, parse::parse_module};
use paco_types::infer_module;

fn temp_path(name: &str, extension: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "paco_link_{name}_{}_{}.{extension}",
        std::process::id(),
        monotonic_suffix()
    ));
    path
}

fn monotonic_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

fn run_and_get_exit_code(binary: &PathBuf) -> i32 {
    let status = Command::new(binary).status().expect("compiled binary should run");
    status.code().expect("process should exit normally, not via a signal")
}

fn compile_program_object(source: &str) -> PathBuf {
    let mut sources = SourceMap::new();
    let file = sources.add_file("main.paco", source);
    let mut reporter = Reporter::new();
    let tokens = lex(sources.source(file).unwrap(), file, &mut reporter);
    let module = parse_module(&tokens, &mut reporter).unwrap();
    assert!(!reporter.has_errors(), "{}", reporter.emit_to_string(&sources));

    let typed = infer_module(&module, &mut reporter).expect("module should type-check");
    let drops =
        paco_borrow::analyze_module(&module, &mut reporter).expect("module should borrow-check");
    let registry = TypeRegistry::from_module(&module);
    let layouts = TypeLayouts::from_module(&module);

    let bodies: Vec<(String, paco_mir::Body)> = module
        .items
        .iter()
        .flat_map(|item| match item {
            Item::Fn(function) => {
                let (body, outlined) =
                    paco_mir::lower_function(function, &typed, &registry, &drops, Profile::Debug);
                let name = if function.name == "main" {
                    paco_mir::ENTRY_SYMBOL.to_string()
                } else {
                    function.name.clone()
                };
                let mut result = vec![(name, body)];
                result.extend(outlined);
                result
            }
            _ => Vec::new(),
        })
        .collect();

    let isa = paco_codegen_cranelift::host_isa().expect("host isa");
    let object_builder = ObjectBuilder::new(isa, "program", default_libcall_names()).unwrap();
    let mut module_out = ObjectModule::new(object_builder);
    paco_codegen_cranelift::declare_and_define(&mut module_out, &bodies, &[], &layouts).unwrap();

    let product = module_out.finish();
    let bytes = product.emit().unwrap();
    let object_path = temp_path("program", "o");
    std::fs::write(&object_path, bytes).unwrap();
    object_path
}

#[test]
fn compiles_and_links_a_program_whose_exit_code_matches_its_main_return_value() {
    let object_path = compile_program_object("fn main() -> i64 { 7 }");
    let exe_path = temp_path("hello", "exe");
    link(&[object_path], &exe_path).expect("linking should succeed");

    assert_eq!(run_and_get_exit_code(&exe_path), 7);
    assert_native_link(&exe_path);
}

#[cfg(target_os = "macos")]
fn assert_native_link(exe_path: &std::path::Path) {
    let otool = Command::new("otool").arg("-L").arg(exe_path).output().expect("otool should run");
    let report = String::from_utf8_lossy(&otool.stdout);
    let libraries: Vec<&str> = report.lines().skip(1).map(str::trim).collect();
    assert!(!libraries.is_empty(), "{report}");
    assert!(
        libraries.iter().all(|library| library.starts_with("/usr/lib/libSystem.B.dylib")),
        "expected only libSystem, otool said: {report}"
    );
}

#[cfg(windows)]
fn assert_native_link(exe_path: &std::path::Path) {
    let binary = std::fs::read(exe_path).unwrap();
    assert!(binary.starts_with(b"MZ"), "expected a PE executable");
}

#[cfg(target_os = "linux")]
fn assert_native_link(exe_path: &std::path::Path) {
    let ldd_output = Command::new("ldd")
        .arg(exe_path)
        .output()
        .expect("ldd should run");
    let ldd_report = String::from_utf8_lossy(&ldd_output.stdout);
    assert!(
        ldd_report.contains("not a dynamic executable") || !ldd_output.status.success(),
        "expected a statically linked binary, ldd said: {ldd_report}"
    );
}

pub fn link(objects: &[PathBuf], output: &std::path::Path) -> Result<(), String> {
    paco_link::link_program(&paco_link::LinkRequest {
        objects,
        output,
        mode: paco_link::LinkMode::Static,
        extra_libs: &[],
        target: &native_target(),
        sysroot: None,
        debug: false,
    })
}

/// The host's default target: static musl on Linux, the host triple
/// elsewhere.
pub fn native_target() -> String {
    paco_link::host_triple().replace("-linux-gnu", "-linux-musl")
}