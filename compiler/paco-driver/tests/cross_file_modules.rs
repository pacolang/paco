use std::{fs, path::PathBuf, process::Command};

use clap::Parser;
use paco_driver::{Cli, run};

fn temp_dir(name: &str) -> PathBuf {
    let mut dir = std::env::temp_dir();
    dir.push(format!("paco_cross_file_{}_{}_{}", name, std::process::id(), monotonic_suffix()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn monotonic_suffix() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
}

#[test]
fn a_qualified_function_call_resolves_and_runs() {
    let dir = temp_dir("qualified_call_run");
    fs::write(dir.join("a.paco"), "pub fn double(x: i64) -> i64 { x * 2 }\n").unwrap();
    fs::write(
        dir.join("b.paco"),
        "use a;\n\nfn main() {\n    print(a::double(3))\n}\n",
    )
    .unwrap();

    let cli = Cli::try_parse_from(["paco", "run", dir.join("b.paco").to_str().unwrap()]).unwrap();
    let output = run(cli).unwrap_or_else(|error| panic!("`paco run` failed: {error}"));
    assert_eq!(output.stdout, "6\n");

    let _ = fs::remove_dir_all(&dir);
}

#[cfg_attr(not(feature = "llvm"), ignore = "needs the LLVM backend (the `llvm` feature)")]
#[test]
fn a_qualified_function_call_matches_run_and_release_build() {
    let dir = temp_dir("qualified_call_build");
    fs::write(dir.join("a.paco"), "pub fn double(x: i64) -> i64 { x * 2 }\n").unwrap();
    let entry = dir.join("b.paco");
    fs::write(&entry, "use a;\n\nfn main() {\n    print(a::double(3))\n}\n").unwrap();

    let run_cli = Cli::try_parse_from(["paco", "run", entry.to_str().unwrap()]).unwrap();
    let run_output = run(run_cli).unwrap_or_else(|error| panic!("`paco run` failed: {error}"));

    let build_cli = Cli::try_parse_from(["paco", "build", "--release", "--backend", "llvm", entry.to_str().unwrap()]).unwrap();
    run(build_cli).unwrap_or_else(|error| panic!("`paco build` failed: {error}"));

    let binary_path = entry.with_extension(std::env::consts::EXE_EXTENSION);
    let compiled = Command::new(&binary_path).output().expect("compiled binary should run");
    let compiled_stdout = String::from_utf8_lossy(&compiled.stdout).to_string();

    assert_eq!(compiled_stdout, run_output.stdout);
    assert!(compiled.status.success());

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn importing_a_struct_from_another_file_type_checks() {
    let dir = temp_dir("qualified_struct");
    fs::write(dir.join("a.paco"), "pub struct Point { x: i64, y: i64 }\n").unwrap();
    let entry = dir.join("b.paco");
    fs::write(&entry, "use a;\n\nfn main() {\n    let p = a::Point { x: 1, y: 2 };\n}\n").unwrap();

    let cli = Cli::try_parse_from(["paco", "check", entry.to_str().unwrap()]).unwrap();
    let output = run(cli).unwrap_or_else(|error| panic!("`paco check` failed: {error}"));
    assert!(output.stderr.is_empty(), "{}", output.stderr);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn importing_a_private_item_is_a_visibility_error_not_a_generic_not_found() {
    let dir = temp_dir("private_item");
    fs::write(dir.join("a.paco"), "fn helper() -> i64 { 1 }\n").unwrap();
    let entry = dir.join("b.paco");
    fs::write(&entry, "use a;\n\nfn main() {\n    print(a::helper())\n}\n").unwrap();

    let cli = Cli::try_parse_from(["paco", "check", entry.to_str().unwrap()]).unwrap();
    let error = run(cli).unwrap_err();
    assert!(error.contains("PACO-E0333"), "{error}");
    assert!(error.contains("helper"), "{error}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_missing_use_target_is_a_clear_compile_error_not_a_panic() {
    let dir = temp_dir("missing_use");
    let entry = dir.join("main.paco");
    fs::write(&entry, "use does_not_exist;\n\nfn main() {\n    print(1)\n}\n").unwrap();

    let cli = Cli::try_parse_from(["paco", "check", entry.to_str().unwrap()]).unwrap();
    let error = run(cli).unwrap_err();
    assert!(error.contains("does_not_exist"), "{error}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_transitive_import_resolves() {
    let dir = temp_dir("transitive");
    fs::write(dir.join("a.paco"), "pub fn helper() -> i64 { 42 }\n").unwrap();
    fs::write(
        dir.join("b.paco"),
        "use a;\n\npub fn forward() -> i64 { a::helper() }\n",
    )
    .unwrap();
    let entry = dir.join("c.paco");
    fs::write(&entry, "use b;\n\nfn main() {\n    print(b::forward())\n}\n").unwrap();

    let cli = Cli::try_parse_from(["paco", "run", entry.to_str().unwrap()]).unwrap();
    let output = run(cli).unwrap_or_else(|error| panic!("`paco run` failed: {error}"));
    assert_eq!(output.stdout, "42\n");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_circular_use_is_rejected_with_a_clear_error_not_a_hang() {
    let dir = temp_dir("cycle");
    fs::write(dir.join("a.paco"), "use b;\n\npub fn a_fn() -> i64 { 1 }\n").unwrap();
    fs::write(dir.join("b.paco"), "use a;\n\npub fn b_fn() -> i64 { 1 }\n").unwrap();
    let entry = dir.join("main.paco");
    fs::write(&entry, "use a;\n\nfn main() {\n    print(1)\n}\n").unwrap();

    let cli = Cli::try_parse_from(["paco", "check", entry.to_str().unwrap()]).unwrap();
    let error = run(cli).unwrap_err();
    assert!(error.to_lowercase().contains("circular"), "{error}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_pub_item_from_the_same_file_remains_usable_without_use() {
    let dir = temp_dir("same_file");
    let entry = dir.join("main.paco");
    fs::write(
        &entry,
        "pub fn helper() -> i64 { 7 }\n\nfn main() {\n    print(helper())\n}\n",
    )
    .unwrap();

    let cli = Cli::try_parse_from(["paco", "run", entry.to_str().unwrap()]).unwrap();
    let output = run(cli).unwrap_or_else(|error| panic!("`paco run` failed: {error}"));
    assert_eq!(output.stdout, "7\n");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_prelude_injected_function_resolves_and_runs_with_no_use() {
    let std = tempfile::tempdir().unwrap();
    let source_std = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../stdlib");
    copy_dir(&source_std, std.path());
    fs::write(std.path().join("core/prelude_probe.paco"), "module core;\n\npub fn prelude_probe() -> i64 {\n    99\n}\n").unwrap();

    let dir = tempfile::tempdir().unwrap();
    let entry = dir.path().join("main.paco");
    fs::write(&entry, "fn main() {\n    print(prelude_probe());\n}\n").unwrap();

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_paco"))
        .arg("run")
        .arg(&entry)
        .env("PACO_STD", std.path())
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(String::from_utf8_lossy(&output.stdout), "99\n");
}

fn copy_dir(from: &std::path::Path, to: &std::path::Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}
