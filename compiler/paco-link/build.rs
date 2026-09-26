use std::path::Path;
use std::process::Command;

fn cargo_build(runtime: &Path, target: &str, target_dir: &Path, features: &[&str]) {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut command = Command::new(cargo);
    command
        .args(["build", "--release", "--quiet", "-p", "paco-runtime-ffi", "--target", target, "--manifest-path"])
        .arg(runtime.join("Cargo.toml"))
        .arg("--target-dir")
        .arg(target_dir)
        .env_remove("CARGO_TARGET_DIR")
        .env_remove("CARGO_BUILD_TARGET_DIR")
        .env_remove("RUSTFLAGS")
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .env_remove("RUSTC_WRAPPER")
        .env_remove("RUSTC_WORKSPACE_WRAPPER");
    if target.ends_with("-apple-darwin") {
        command.env("MACOSX_DEPLOYMENT_TARGET", "11.0");
    }
    if !target.ends_with("-msvc") {
        command.env(format!("CFLAGS_{}", target.replace('-', "_")), "-U_FORTIFY_SOURCE -D_FORTIFY_SOURCE=0");
        let cc_variable = format!("CC_{}", target.replace('-', "_"));
        if std::env::var_os(&cc_variable).is_none() {
            command.env(cc_variable, "cc");
        }
    }
    if !features.is_empty() {
        command.arg("--features").arg(features.join(","));
    }
    let status = command.status().expect("failed to run cargo for paco-runtime-ffi");
    assert!(status.success(), "building paco-runtime-ffi for `{target}` failed");
    if target.ends_with("-apple-darwin") {
        let archive = target_dir.join(target).join("release/libpaco_runtime_ffi.a");
        let status = Command::new("strip").arg("-S").arg(&archive).status().expect("failed to run strip");
        assert!(status.success(), "stripping the debug info of `{}` failed", archive.display());
    }
}

/// The native libraries Rust's standard library needs on `target`, from
/// `rustc --print native-static-libs` (its exact list, e.g. the `windows`
/// crate's generated import library, moves with the toolchain and its
/// locked dependencies, so it is read here rather than hardcoded).
fn windows_native_libs(runtime: &Path, target: &str, target_dir: &Path) -> String {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = Command::new(cargo)
        .args(["rustc", "--release", "--quiet", "-p", "paco-runtime-ffi", "--crate-type", "staticlib", "--target", target])
        .arg("--manifest-path")
        .arg(runtime.join("Cargo.toml"))
        .arg("--target-dir")
        .arg(target_dir)
        .args(["--", "--print", "native-static-libs"])
        .env_remove("CARGO_TARGET_DIR")
        .env_remove("RUSTFLAGS")
        .output()
        .expect("failed to run cargo rustc for native-static-libs");
    assert!(output.status.success(), "reading native-static-libs for `{target}` failed: {}", String::from_utf8_lossy(&output.stderr));
    let stderr = String::from_utf8_lossy(&output.stderr);
    let line = stderr.lines().find_map(|line| line.split_once("native-static-libs: ")).expect("rustc prints native-static-libs").1;
    line.split_whitespace()
        .map(|flag| flag.trim_start_matches("-l").trim_start_matches("/defaultlib:").trim_end_matches(".lib"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Directories cargo would add to the linker's search path for crate-owned
/// import libraries (`windows-sys`' prebuilt `windows.<version>.lib` and
/// similar): every `<crate>-<version>/lib` under the Cargo registry source
/// checkout whose crate name starts with `windows_` and ends with `_msvc`.
/// `--print native-static-libs` names these libraries but not where they
/// live (that comes from each crate's own `cargo:rustc-link-search`,
/// applied only when cargo performs the final link itself), so this walks
/// the same source cargo already fetched instead of hardcoding a version.
fn windows_registry_lib_dirs() -> String {
    let home = std::env::var_os("CARGO_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(|home| std::path::PathBuf::from(home).join(".cargo")))
        .unwrap_or_else(|| std::path::PathBuf::from(".cargo"));
    let mut dirs = Vec::new();
    let Ok(registries) = std::fs::read_dir(home.join("registry/src")) else { return String::new() };
    for registry in registries.flatten() {
        let Ok(crates) = std::fs::read_dir(registry.path()) else { continue };
        for entry in crates.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with("windows_") && name.contains("_msvc-") {
                let lib = entry.path().join("lib");
                if lib.is_dir() {
                    dirs.push(lib.display().to_string());
                }
            }
        }
    }
    dirs.join(";")
}

fn main() {
    let runtime = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../runtime").canonicalize().expect("runtime/ exists");
    let runtime = match runtime.to_str().and_then(|path| path.strip_prefix(r"\\?\")) {
        Some(plain) => std::path::PathBuf::from(plain),
        None => runtime,
    };
    for path in [
        "Cargo.toml",
        "Cargo.lock",
        "paco-runtime/Cargo.toml",
        "paco-runtime/src",
        "paco-runtime-ffi/Cargo.toml",
        "paco-runtime-ffi/src",
    ] {
        println!("cargo:rerun-if-changed={}", runtime.join(path).display());
    }
    let host = std::env::var("TARGET").expect("cargo sets TARGET");
    let target_dir = runtime.join("target");
    cargo_build(&runtime, &host, &target_dir, &[]);
    if host.ends_with("-unknown-linux-gnu") {
        cargo_build(&runtime, &host.replace("-gnu", "-musl"), &target_dir, &[]);
    }
    cargo_build(&runtime, &host, &target_dir.join("system-alloc"), &["system-alloc"]);
    let windows_native_libs = if host.ends_with("-pc-windows-msvc") { windows_native_libs(&runtime, &host, &target_dir) } else { String::new() };
    println!("cargo:rustc-env=PACO_WINDOWS_NATIVE_LIBS={windows_native_libs}");
    let windows_lib_dirs = if host.ends_with("-pc-windows-msvc") { windows_registry_lib_dirs() } else { String::new() };
    println!("cargo:rustc-env=PACO_WINDOWS_LIB_DIRS={windows_lib_dirs}");
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let sysroot = Command::new(rustc).args(["--print", "sysroot"]).output().expect("rustc runs");
    println!("cargo:rustc-env=PACO_RUST_SYSROOT={}", String::from_utf8_lossy(&sysroot.stdout).trim());
    println!("cargo:rustc-env=PACO_HOST_TRIPLE={host}");
    println!("cargo:rustc-env=PACO_RUNTIME_DIR={}", runtime.display());
}
