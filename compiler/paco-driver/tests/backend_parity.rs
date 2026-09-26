use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn scratch(name: &str, source: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("paco-backend-parity-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let input = dir.join("input.paco");
    fs::write(&input, source).unwrap();
    input
}

fn paco_build(input: &Path, flags: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_paco")).arg("build").args(flags).arg(input).output().unwrap()
}

fn build_and_run(input: &Path, flags: &[&str]) -> Output {
    let build = paco_build(input, flags);
    assert!(build.status.success(), "{flags:?}: {}", String::from_utf8_lossy(&build.stderr));
    Command::new(input.with_extension(std::env::consts::EXE_EXTENSION)).output().unwrap()
}

const OVERFLOW: &str = "fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n\nfn mul(a: i64, b: i64) -> i64 {\n    a * b\n}\n\nfn main() {\n    let a: i32 = 2147483647;\n    let b: i32 = 1;\n    print(add(a, b));\n    let big: i64 = 4611686018427387904;\n    print(mul(big, 4));\n    let small: u8 = 250;\n    let step: u8 = 10;\n    print(small + step);\n}\n";

#[test]
fn release_overflow_wraps_identically_on_both_backends() {
    let input = scratch("overflow", OVERFLOW);
    for &backend in crate::backends() {
        let run = build_and_run(&input, &["--release", "--backend", backend]);
        assert!(run.status.success(), "{backend}: {:?}", run.status);
        assert_eq!(String::from_utf8_lossy(&run.stdout), "-2147483648\n0\n4\n", "{backend}");
    }
}

#[test]
fn debug_overflow_panics_on_both_backends() {
    let input = scratch("overflow-debug", OVERFLOW);
    for &backend in crate::backends() {
        let run = build_and_run(&input, &["--backend", backend]);
        assert_eq!(run.status.code(), Some(101), "{backend} should panic on overflow");
        assert!(run.stdout.is_empty(), "{backend}: {}", String::from_utf8_lossy(&run.stdout));
        let stderr = String::from_utf8_lossy(&run.stderr);
        assert!(stderr.contains("input.paco:2:5: attempt to add with overflow"), "{backend}: {stderr}");
    }
}

#[test]
fn an_unsupported_target_is_reported_without_writing_a_binary() {
    let input = scratch("bogus-target", "fn main() {\n    print(1);\n}\n");
    for &backend in crate::backends() {
        let build = paco_build(&input, &["--release", "--backend", backend, "--target", "bogus-unknown-nowhere"]);
        assert!(!build.status.success());
        let stderr = String::from_utf8_lossy(&build.stderr);
        assert!(stderr.contains("unsupported target triple `bogus-unknown-nowhere`"), "{backend}: {stderr}");
        assert!(!input.with_extension(std::env::consts::EXE_EXTENSION).exists(), "{backend} left a binary behind");
        assert!(!input.with_extension("o").exists(), "{backend} left an object file behind");
    }
}

#[test]
fn an_explicit_host_triple_builds_a_running_binary() {
    let input = scratch("host-target", "fn main() {\n    print(42);\n}\n");
    let backend = crate::backends()[0];
    let run = build_and_run(&input, &["--release", "--backend", backend, "--target", paco_link::host_triple()]);
    assert_eq!(String::from_utf8_lossy(&run.stdout), "42\n");
}

fn foreign_arch() -> &'static str {
    if std::env::consts::ARCH == "aarch64" { "x86_64" } else { "aarch64" }
}

fn foreign_runtime_built() -> bool {
    let target = format!("{}-unknown-linux-musl", foreign_arch());
    let built = paco_link::toolchain::Toolchain::locate().component(&target, "libpaco_runtime.a").is_ok();
    if !built {
        eprintln!("skipped: the {target} runtime is not built; run scripts/dist.sh");
    }
    built
}

/// Named for its first target; builds for whichever architecture is not
/// the host's, needing nothing beyond the Paco toolchain.
#[test]
fn cross_compiles_for_aarch64_when_its_toolchain_is_installed() {
    if !foreign_runtime_built() {
        return;
    }
    let input = scratch("aarch64", "fn main() {\n    let mut total: i64 = 0;\n    let mut i: i64 = 1;\n    while i <= 10 {\n        total = total + i;\n        i = i + 1;\n    }\n    print(total);\n}\n");
    let target = format!("{}-unknown-linux", foreign_arch());
    let machine = if foreign_arch() == "aarch64" { 183u16 } else { 62 };
    let release = ["--release", "--backend", crate::backends()[0], "--target", target.as_str()];
    for flags in [&["--target", target.as_str()][..], &release[..]] {
        let build = paco_build(&input, flags);
        assert!(build.status.success(), "{flags:?}: {}", String::from_utf8_lossy(&build.stderr));
        let binary = fs::read(input.with_extension(std::env::consts::EXE_EXTENSION)).unwrap();
        assert_eq!(u16::from_le_bytes([binary[18], binary[19]]), machine, "{flags:?}");
        match paco_test_harness::run_for_target(&input.with_extension(std::env::consts::EXE_EXTENSION), &target, None) {
            paco_test_harness::TargetRun::Ran(run) => assert_eq!(String::from_utf8_lossy(&run.stdout), "55\n"),
            paco_test_harness::TargetRun::Skipped(message) => eprintln!("{message}"),
        }
    }
}

/// Every compiled conformance program, cross-compiled statically for the
/// other architecture on both backends and profiles, must print what it
/// prints on the host. Programs with `extern` blocks need that
/// architecture's C libraries and are left to the dynamic legs.
#[test]
fn aarch64_conformance_matches_the_host_under_qemu() {
    if !foreign_runtime_built() {
        return;
    }
    let target = format!("{}-unknown-linux", foreign_arch());
    let conformance = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/conformance");
    let mut failures = Vec::new();
    for test in paco_test_harness::discover_golden_tests(&conformance, 2).unwrap() {
        if !test.build || test.kind != paco_test_harness::TestKind::Run {
            continue;
        }
        for &backend in crate::backends() {
            let flags = ["--backend", backend];
            for release in [false, true] {
                let expected = test.expected_run(release).stdout;
                let name = test.path.file_name().unwrap().to_string_lossy();
                let input = scratch(&format!("cross-{name}-{}-{release}", flags[1]), "");
                copy_dir(&test.path, input.parent().unwrap());
                let mut args = vec!["--target", target.as_str(), "--link", "static", flags[0], flags[1]];
                if release {
                    args.push("--release");
                }
                let build = paco_build(&input, &args);
                let stderr = String::from_utf8_lossy(&build.stderr);
                if stderr.contains("PACO-E0804")
                    || (!crate::llvm_backend() && stderr.contains(crate::conformance::LLVM_MISSING))
                {
                    continue;
                }
                if !build.status.success() {
                    failures.push(format!("{} {args:?}: {stderr}", test.path.display()));
                    continue;
                }
                let run = match paco_test_harness::run_for_target(&input.with_extension(std::env::consts::EXE_EXTENSION), &target, None) {
                    paco_test_harness::TargetRun::Ran(run) => run,
                    paco_test_harness::TargetRun::Skipped(message) => {
                        eprintln!("{message}");
                        return;
                    }
                };
                if paco_test_harness::strip_dir(&String::from_utf8_lossy(&run.stdout), input.parent().unwrap()) != expected {
                    failures.push(format!("{} {args:?}: stdout differs", test.path.display()));
                }
                let _ = fs::remove_dir_all(input.parent().unwrap());
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

fn copy_dir(from: &Path, to: &Path) {
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
