use std::path::Path;
use std::process::Command;

/// The native libraries each link line supplies: the static line adds
/// musl's `libc.a` and `libunwind.a`; the dynamic line adds `libc.so.6`,
/// `libm.so.6` and `libgcc_s.so.1`, and glibc 2.34+ serves `libutil`,
/// `librt`, `libpthread` and `libdl` from `libc.so.6`; on macOS the system
/// driver links `libSystem` (which serves `libc` and `libm`) and
/// `libiconv`; on Windows the COFF line names every import library itself.
fn supplied(target: &str) -> Vec<&'static str> {
    if target.ends_with("-apple-darwin") {
        vec!["System", "c", "m", "iconv"]
    } else if target.ends_with("-windows-msvc") {
        paco_link::windows_system_libs()
    } else if target.ends_with("-musl") {
        vec!["unwind", "c"]
    } else {
        vec!["gcc_s", "util", "rt", "pthread", "m", "dl", "c"]
    }
}

#[test]
fn the_link_lines_supply_every_native_library_rust_std_needs() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let toolchain = paco_link::toolchain::Toolchain::locate();
    let targets: &[&str] = if cfg!(target_os = "linux") {
        &["x86_64-unknown-linux-musl", "x86_64-unknown-linux-gnu", "aarch64-unknown-linux-musl", "aarch64-unknown-linux-gnu"]
    } else {
        &[paco_link::host_triple()]
    };
    for &target in targets {
        if toolchain.component(target, paco_link::toolchain::RUNTIME_ARCHIVE).is_err() {
            eprintln!("skipped: the {target} runtime is not built; run scripts/dist.sh");
            continue;
        }
        let mut command = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
        if cfg!(target_os = "linux") {
            let env = Command::new(root.join("scripts/runtime-env.sh")).arg(target).output().unwrap();
            assert!(env.status.success(), "{}", String::from_utf8_lossy(&env.stderr));
            for setting in String::from_utf8(env.stdout).unwrap().lines() {
                let (key, value) = setting.split_once('=').unwrap();
                command.env(key, value);
            }
        }
        let output = command
            .args(["rustc", "--release", "-p", "paco-runtime-ffi", "--crate-type", "staticlib", "--target", target])
            .arg("--manifest-path")
            .arg(root.join("runtime/Cargo.toml"))
            .args(["--", "--print", "native-static-libs"])
            .env_remove("CARGO_TARGET_DIR")
            .env_remove("RUSTFLAGS")
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "{target}: {stderr}");
        let line = stderr.lines().find_map(|line| line.split_once("native-static-libs: ")).unwrap().1;
        let libs: Vec<&str> = line
            .split_whitespace()
            .map(|flag| flag.trim_start_matches("-l").trim_start_matches("/defaultlib:").trim_end_matches(".lib"))
            .collect();
        assert!(libs.contains(&if target.ends_with("-msvc") { "msvcrt" } else { "c" }), "{target}: {line}");
        for lib in &libs {
            assert!(supplied(target).contains(lib), "{target} needs `{lib}`, which the link line does not supply: {line}");
        }
    }
}
