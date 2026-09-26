use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

#[cfg(target_os = "linux")]
use object::read::elf::{ElfFile64, FileHeader, ProgramHeader};
#[cfg(target_os = "linux")]
use object::Endianness;
use paco_driver::{LinkChoice, resolve_target};
use paco_link::LinkMode;
use paco_test_harness::{TargetRun, run_for_target};

const PURE: &str = "fn main() {\n    print(1.5);\n    print(42);\n}\n";
const COS: &str = "extern \"C\" {\n    fn cos(x: float) -> float;\n}\n\nfn main() {\n    unsafe {\n        print(cos(0.0));\n        print(cos(3.0) < 0.0)\n    }\n}\n";

fn scratch(name: &str, source: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::Builder::new().prefix(&format!("paco-toolchain-{name}")).tempdir().unwrap();
    let input = dir.path().join("input.paco");
    fs::write(&input, source).unwrap();
    (dir, input)
}

fn paco(args: &[&str], input: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_paco")).arg("build").args(args).arg(input).output().unwrap()
}

fn build(args: &[&str], input: &Path) -> PathBuf {
    let output = paco(args, input);
    assert!(output.status.success(), "{args:?}: {}", String::from_utf8_lossy(&output.stderr));
    input.with_extension(std::env::consts::EXE_EXTENSION)
}

fn paco_run(input: &Path) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_paco")).arg("run").arg(input).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout).unwrap()
}

/// Whether the ELF asks for a dynamic loader, and its `DT_NEEDED` entries.
#[cfg(target_os = "linux")]
fn dynamic_info(binary: &Path) -> (bool, Vec<String>, u16) {
    let data = fs::read(binary).unwrap();
    let elf = ElfFile64::<Endianness>::parse(&*data).unwrap();
    let endian = elf.endian();
    let header = elf.elf_header();
    let interp = header
        .program_headers(endian, &*data)
        .unwrap()
        .iter()
        .any(|segment| segment.p_type(endian) == object::elf::PT_INTERP);
    let mut needed = Vec::new();
    let sections = header.sections(endian, &*data).unwrap();
    if let Some((entries, index)) = sections.dynamic(endian, &*data).unwrap() {
        let strings = sections.strings(endian, &*data, index).unwrap();
        for entry in entries {
            if entry.d_tag.get(endian) == object::elf::DT_NEEDED {
                needed.push(String::from_utf8_lossy(strings.get(entry.d_val.get(endian) as u32).unwrap()).into_owned());
            }
        }
    }
    (interp, needed, header.e_machine(endian))
}

fn run_output(binary: &Path, target: &str, sysroot: Option<&Path>) -> Option<String> {
    match run_for_target(binary, target, sysroot) {
        TargetRun::Ran(output) => {
            assert!(output.status.success(), "{} exited with {:?}", binary.display(), output.status);
            Some(String::from_utf8(output.stdout).unwrap())
        }
        TargetRun::Skipped(message) => {
            eprintln!("{message}");
            None
        }
    }
}

fn host_arch() -> &'static str {
    std::env::consts::ARCH
}

fn foreign_arch() -> &'static str {
    if host_arch() == "aarch64" { "x86_64" } else { "aarch64" }
}

fn runtime_built_for(target: &str) -> bool {
    let built = paco_link::toolchain::Toolchain::locate().component(target, "libpaco_runtime.a").is_ok();
    if !built {
        eprintln!("skipped: the {target} runtime is not built; run scripts/dist.sh");
    }
    built
}

#[test]
fn completes_linux_targets_by_link_mode() {
    let host = paco_link::host_triple();
    let check = |requested: Option<&str>, link: Option<LinkChoice>, has_extern: bool, mode: LinkMode, triple: &str| {
        assert_eq!(resolve_target(requested, link, has_extern), (mode, triple.to_string()), "{requested:?} {link:?}");
    };
    if cfg!(target_os = "linux") {
        let host_base = host.strip_suffix("-gnu").unwrap();
        check(None, None, false, LinkMode::Static, &format!("{host_base}-musl"));
        check(None, None, true, LinkMode::Dynamic, &format!("{host_base}-gnu"));
        check(None, Some(LinkChoice::Dynamic), false, LinkMode::Dynamic, &format!("{host_base}-gnu"));
    } else {
        check(None, None, false, LinkMode::Static, host);
        check(None, None, true, LinkMode::Dynamic, host);
    }
    check(Some("aarch64-unknown-linux"), None, false, LinkMode::Static, "aarch64-unknown-linux-musl");
    check(Some("aarch64-unknown-linux"), None, true, LinkMode::Dynamic, "aarch64-unknown-linux-gnu");
    check(Some("x86_64-unknown-linux-gnu"), None, false, LinkMode::Dynamic, "x86_64-unknown-linux-gnu");
    check(Some("x86_64-unknown-linux-musl"), None, true, LinkMode::Static, "x86_64-unknown-linux-musl");
    check(Some("bogus-unknown-nowhere"), None, false, LinkMode::Static, "bogus-unknown-nowhere");
}

#[cfg(target_os = "linux")]
#[test]
fn a_pure_program_builds_static_with_no_loader_and_no_shared_libraries() {
    let (_dir, input) = scratch("static", PURE);
    let expected = paco_run(&input);
    for flags in crate::backend_flag_sets() {
        let binary = build(flags, &input);
        let (interp, needed, _) = dynamic_info(&binary);
        assert!(!interp && needed.is_empty(), "{flags:?}: interp={interp} needed={needed:?}");
        assert_eq!(run_output(&binary, host_arch(), None).unwrap(), expected);
    }
}

#[cfg(target_os = "linux")]
#[test]
fn link_dynamic_on_a_pure_program_links_glibc() {
    let (_dir, input) = scratch("dynamic-pure", PURE);
    let binary = build(&["--link", "dynamic"], &input);
    let (interp, needed, _) = dynamic_info(&binary);
    assert!(interp && needed.contains(&"libc.so.6".to_string()), "{needed:?}");
    assert_eq!(run_output(&binary, host_arch(), None).unwrap(), paco_run(&input));
}

#[cfg(target_os = "linux")]
#[test]
fn a_program_with_an_extern_block_links_dynamically_and_matches_paco_run() {
    let (_dir, input) = scratch("extern-cos", COS);
    let expected = paco_run(&input);
    for flags in crate::backend_flag_sets() {
        let binary = build(flags, &input);
        let (interp, needed, _) = dynamic_info(&binary);
        assert!(interp && needed.contains(&"libc.so.6".to_string()), "{needed:?}");
        assert_eq!(run_output(&binary, host_arch(), None).unwrap(), expected, "{flags:?}");
    }
}

#[test]
fn static_mode_rejects_extern_blocks_at_the_first_block() {
    let (_dir, input) = scratch("static-extern", COS);
    let musl = format!("{}-unknown-linux-musl", host_arch());
    for flags in [&["--link", "static"][..], &["--target", musl.as_str()][..]] {
        let output = paco(flags, &input);
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("PACO-E0804"), "{flags:?}: {stderr}");
        assert!(stderr.contains("input.paco:1:"), "{flags:?}: {stderr}");
        assert!(!input.with_extension(std::env::consts::EXE_EXTENSION).exists());
    }
}

#[cfg(target_os = "linux")]
#[test]
fn builds_without_any_c_toolchain_or_cargo_on_path() {
    let empty = tempfile::tempdir().unwrap();
    for (name, source, flags) in [("no-cc-static", PURE, &[][..]), ("no-cc-dynamic", COS, &["--link", "dynamic"][..])] {
        let (_dir, input) = scratch(name, source);
        let output = Command::new(env!("CARGO_BIN_EXE_paco"))
            .env("PATH", empty.path())
            .arg("build")
            .args(flags)
            .arg(&input)
            .output()
            .unwrap();
        assert!(output.status.success(), "{name}: {}", String::from_utf8_lossy(&output.stderr));
        assert_eq!(run_output(&input.with_extension(std::env::consts::EXE_EXTENSION), host_arch(), None).unwrap(), paco_run(&input));
    }
}

#[test]
fn dynamic_cross_builds_report_a_missing_glibc_and_missing_extern_libraries() {
    let target = format!("{}-unknown-linux", foreign_arch());
    let sysroot = tempfile::tempdir().unwrap();
    let sysroot_arg = sysroot.path().to_str().unwrap();
    if !runtime_built_for(&format!("{target}-gnu")) {
        return;
    }
    let (_dir, input) = scratch("no-glibc", PURE);
    let output = paco(&["--target", &target, "--link", "dynamic", "--sysroot", sysroot_arg], &input);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(stderr.contains("PACO-E0802") && stderr.contains("libc.so.6") && stderr.contains("--sysroot"), "{stderr}");
    assert!(stderr.contains(&format!("{sysroot_arg}/usr/lib/{}-linux-gnu", foreign_arch())), "{stderr}");
    assert!(!input.with_extension(std::env::consts::EXE_EXTENSION).exists());

    fs::create_dir_all(sysroot.path().join("lib")).unwrap();
    fs::write(sysroot.path().join("lib/libc.so.6"), "").unwrap();
    let (_dir, input) = scratch("no-blas", "use stdlib::blas;\n\nfn main() {\n    print(1)\n}\n");
    let output = Command::new(env!("CARGO_BIN_EXE_paco"))
        .env("PACO_SYSROOT", sysroot.path())
        .args(["build", "--target", &target])
        .arg(&input)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(stderr.contains("PACO-E0803") && stderr.contains("libblas") && stderr.contains("--sysroot"), "{stderr}");
    assert!(!input.with_extension(std::env::consts::EXE_EXTENSION).exists());
}

/// A Debian `libc6`/`libgcc-s1` root for the other architecture, from
/// `scripts/fetch-sysroot.sh`.
#[cfg(target_os = "linux")]
fn foreign_sysroot() -> Option<PathBuf> {
    let dir = std::env::var_os("PACO_TEST_SYSROOT")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache/paco/sysroot")))?
        .join(format!("{}-linux-gnu", foreign_arch()));
    if dir.join(format!("usr/lib/{}-linux-gnu/libc.so.6", foreign_arch())).exists() {
        Some(dir)
    } else {
        eprintln!("skipped: no {} system root; run scripts/fetch-sysroot.sh {}", foreign_arch(), foreign_arch());
        None
    }
}

#[cfg(target_os = "linux")]
#[test]
fn cross_builds_a_program_with_an_extern_block_against_a_sysroot() {
    let target = format!("{}-unknown-linux-gnu", foreign_arch());
    let Some(sysroot) = foreign_sysroot() else { return };
    if !runtime_built_for(&target) {
        return;
    }
    let (_dir, input) = scratch("cross-cos", COS);
    let binary = build(&["--target", &format!("{}-unknown-linux", foreign_arch()), "--sysroot", sysroot.to_str().unwrap()], &input);
    let (interp, needed, _) = dynamic_info(&binary);
    assert!(interp && needed.contains(&"libm.so.6".to_string()), "{needed:?}");
    if let Some(stdout) = run_output(&binary, &target, Some(&sysroot)) {
        assert_eq!(stdout, paco_run(&input));
    }
}

#[cfg(target_os = "linux")]
#[test]
fn a_static_binary_runs_alone_in_an_empty_root() {
    let (dir, input) = scratch("chroot", PURE);
    let binary = build(&[], &input);
    let root = dir.path().join("root");
    fs::create_dir(&root).unwrap();
    fs::copy(&binary, root.join("program")).unwrap();
    let output = Command::new("unshare").args(["--user", "--map-root-user", "chroot"]).arg(&root).arg("/program").output();
    match output {
        Ok(output) if output.status.success() => {
            assert_eq!(String::from_utf8_lossy(&output.stdout), "1.5\n42\n");
        }
        Ok(output) => eprintln!("skipped: user namespaces unavailable: {}", String::from_utf8_lossy(&output.stderr)),
        Err(error) => eprintln!("skipped: unshare unavailable: {error}"),
    }
}

#[test]
fn stdout_is_flushed_before_each_stderr_write() {
    let source = "use stdlib::io;\n\nfn main() {\n    print(\"before\");\n    io::print_err(\"oops\");\n    print(\"after\")\n}\n";
    let (_dir, input) = scratch("interleave", source);
    for flags in crate::backend_flag_sets() {
        let binary = build(flags, &input);
        let output = Command::new("sh").arg("-c").arg("\"$0\" 2>&1").arg(&binary).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&output.stdout), "before\noops\nafter\n", "{flags:?}");
    }
}

/// The shared libraries a Mach-O binary loads, from `otool -L`.
#[cfg(target_os = "macos")]
fn loaded_libraries(binary: &Path) -> Vec<String> {
    let output = Command::new("otool").arg("-L").arg(binary).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout).unwrap().lines().skip(1).map(|line| line.trim().split(' ').next().unwrap().to_string()).collect()
}

#[cfg(target_os = "macos")]
#[test]
fn macos_programs_link_through_the_system_driver_against_libsystem_only() {
    for (name, source) in [("macos-pure", PURE), ("macos-extern", COS)] {
        let (_dir, input) = scratch(name, source);
        let expected = paco_run(&input);
        for flags in crate::backend_flag_sets() {
            let binary = build(flags, &input);
            assert_eq!(loaded_libraries(&binary), ["/usr/lib/libSystem.B.dylib"], "{name} {flags:?}");
            assert_eq!(run_output(&binary, host_arch(), None).unwrap(), expected, "{name} {flags:?}");
        }
    }
}

#[cfg(target_os = "macos")]
#[test]
fn macos_builds_need_no_c_compiler_on_path() {
    let empty = tempfile::tempdir().unwrap();
    for (name, source) in [("macos-no-cc", PURE), ("macos-no-cc-extern", COS)] {
        let (_dir, input) = scratch(name, source);
        let output = Command::new(env!("CARGO_BIN_EXE_paco")).env("PATH", empty.path()).arg("build").arg(&input).output().unwrap();
        assert!(output.status.success(), "{name}: {}", String::from_utf8_lossy(&output.stderr));
        assert_eq!(run_output(&input.with_extension(std::env::consts::EXE_EXTENSION), host_arch(), None).unwrap(), paco_run(&input));
    }
}

#[cfg(target_os = "macos")]
#[test]
fn macos_extern_libraries_resolve_from_the_sdk_or_report_the_searched_directories() {
    let (_dir, input) = scratch("macos-missing-lib", "use nosuchlib;\n\nfn main() {\n    print(1)\n}\n");
    fs::write(input.with_file_name("nosuchlib.paco"), "extern \"C\" {\n    fn nosuchlib_fn() -> i64;\n}\n").unwrap();
    let output = paco(&[], &input);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("PACO-E0803") && stderr.contains("libnosuchlib") && stderr.contains("/usr/lib`"), "{stderr}");
}

#[cfg(windows)]
#[test]
fn windows_programs_link_without_a_c_compiler_and_match_paco_run() {
    let empty = tempfile::tempdir().unwrap();
    for (name, source) in [("windows-pure", PURE), ("windows-extern", COS)] {
        let (_dir, input) = scratch(name, source);
        let expected = paco_run(&input);
        let output = Command::new(env!("CARGO_BIN_EXE_paco")).env("PATH", empty.path()).arg("build").arg(&input).output().unwrap();
        assert!(output.status.success(), "{name}: {}", String::from_utf8_lossy(&output.stderr));
        let binary = input.with_extension(std::env::consts::EXE_EXTENSION);
        assert_eq!(run_output(&binary, host_arch(), None).unwrap(), expected, "{name}");
    }
}
