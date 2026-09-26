//! Builds every compiled conformance program with AddressSanitizer and runs
//! it: any leak, double free or use-after-free fails the test. A panic
//! (in `main` or in a task) ends its stack without running destructors, so
//! only memory errors are checked for a program that panics.

use std::fs;
use std::path::Path;
use std::process::Command;

use paco_test_harness::{GoldenStatus, TestKind, discover_golden_tests};

fn address_sanitizer_available(scratch: &Path) -> bool {
    if std::env::var("PACO_SKIP_LEAK_CHECK").is_ok_and(|value| !value.is_empty()) {
        eprintln!("skipping: PACO_SKIP_LEAK_CHECK is set (CI runs this check on Linux x86_64)");
        return false;
    }
    let source = scratch.join("probe.c");
    fs::write(&source, "int main(void) { return 0; }\n").unwrap();
    Command::new("cc")
        .arg("-fsanitize=address")
        .arg(&source)
        .arg("-o")
        .arg(scratch.join("probe"))
        .status()
        .is_ok_and(|status| status.success())
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

#[test]
fn compiled_conformance_programs_free_everything_exactly_once() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let scratch = std::env::temp_dir().join(format!("paco-leak-check-{}", std::process::id()));
    let _ = fs::remove_dir_all(&scratch);
    fs::create_dir_all(&scratch).unwrap();
    if !address_sanitizer_available(&scratch) {
        eprintln!("skipping: the C toolchain has no AddressSanitizer");
        return;
    }

    let conformance = manifest_dir.join("../../tests/conformance");
    let mut failures = Vec::new();
    let mut checked = 0;
    let only = std::env::var("PACO_CONFORMANCE_FILTER").ok();
    for test in discover_golden_tests(&conformance, 2).unwrap() {
        if only.as_deref().is_some_and(|only| !test.path.display().to_string().contains(only)) {
            continue;
        }
        if !test.build || test.kind != TestKind::Run || matches!(test.status, GoldenStatus::Skipped { .. }) {
            continue;
        }
        let legs: &[(&str, &[&str])] = if crate::llvm_backend() { &[("cranelift", &[]), ("llvm", &["--release"])] } else { &[("cranelift", &[])] };
        for &(leg, flags) in legs {
            let work = scratch.join(format!("{}-{leg}", test.path.file_name().unwrap().to_string_lossy()));
            copy_dir(&test.path, &work);
            let input = work.join("input.paco");
            let build = Command::new(env!("CARGO_BIN_EXE_paco"))
                .arg("build")
                .args(flags)
                .arg(&input)
                .env("PACO_SANITIZE", "address")
                .output()
                .unwrap();
            if !build.status.success() {
                let stderr = String::from_utf8_lossy(&build.stderr);
                if !crate::llvm_backend() && stderr.contains(crate::conformance::LLVM_MISSING) {
                    eprintln!("skipping {} ({leg}): needs the LLVM backend", test.path.display());
                    continue;
                }
                if stderr.contains(crate::conformance::LIBRARY_MISSING) {
                    eprintln!("skipping {} ({leg}): a linked library is missing on this host", test.path.display());
                    continue;
                }
                failures.push(format!("{} ({leg}): build failed: {stderr}", test.path.display()));
                continue;
            }
            let expected = test.expected_run(!flags.is_empty());
            let panics = expected.exit != 0 || expected.stderr.as_deref().is_some_and(|stderr| stderr.contains("panic at "));
            let detect_leaks = if panics || !cfg!(target_os = "linux") { "detect_leaks=0" } else { "detect_leaks=1" };
            let run = Command::new(input.with_extension(std::env::consts::EXE_EXTENSION))
                .current_dir(manifest_dir)
                .env("ASAN_OPTIONS", detect_leaks)
                .output()
                .unwrap();
            let stderr = String::from_utf8_lossy(&run.stderr);
            if run.status.code() != Some(expected.exit) || stderr.contains("Sanitizer") {
                failures.push(format!("{} ({leg}): {}\n{stderr}", test.path.display(), run.status));
            } else if paco_test_harness::strip_dir(&String::from_utf8_lossy(&run.stdout), &work) != expected.stdout {
                failures.push(format!("{} ({leg}): stdout differs", test.path.display()));
            }
            checked += 1;
        }
    }
    let _ = fs::remove_dir_all(&scratch);
    assert!(checked > 0, "no compiled conformance program was found");
    assert!(failures.is_empty(), "{} of {checked} programs failed:\n{}", failures.len(), failures.join("\n"));
}

/// LeakSanitizer runs only on Linux; macOS's AddressSanitizer has no leak
/// detection, so there the check above covers double frees and
/// use-after-free alone.
#[cfg(target_os = "linux")]
#[test]
fn leak_sanitizer_sees_allocations_made_through_the_runtime_allocator() {
    let scratch = std::env::temp_dir().join(format!("paco-leak-probe-{}", std::process::id()));
    let _ = fs::remove_dir_all(&scratch);
    fs::create_dir_all(&scratch).unwrap();
    if !address_sanitizer_available(&scratch) {
        eprintln!("skipping: the C toolchain has no AddressSanitizer");
        return;
    }
    let input = scratch.join("input.paco");
    fs::write(
        &input,
        "extern \"C\" {\n    fn paco_alloc(size: i64) -> *const i64;\n}\n\nfn main() {\n    unsafe {\n        paco_alloc(64);\n    }\n    print(1)\n}\n",
    )
    .unwrap();
    let build = Command::new(env!("CARGO_BIN_EXE_paco")).arg("build").arg(&input).env("PACO_SANITIZE", "address").output().unwrap();
    assert!(build.status.success(), "{}", String::from_utf8_lossy(&build.stderr));
    let run = Command::new(input.with_extension(std::env::consts::EXE_EXTENSION)).env("ASAN_OPTIONS", "detect_leaks=1").output().unwrap();
    let stderr = String::from_utf8_lossy(&run.stderr);
    let _ = fs::remove_dir_all(&scratch);
    assert!(!run.status.success(), "the leak went unreported");
    assert!(stderr.contains("LeakSanitizer") && stderr.contains("64 byte(s) leaked"), "{stderr}");
}

/// LeakSanitizer runs only on Linux; macOS's AddressSanitizer has no leak
/// detection.
#[cfg(target_os = "linux")]
#[test]
fn leak_sanitizer_reports_a_gradient_tape_that_is_never_freed() {
    let scratch = std::env::temp_dir().join(format!("paco-leak-tape-{}", std::process::id()));
    let _ = fs::remove_dir_all(&scratch);
    fs::create_dir_all(&scratch).unwrap();
    if !address_sanitizer_available(&scratch) {
        eprintln!("skipping: the C toolchain has no AddressSanitizer");
        return;
    }
    let input = scratch.join("input.paco");
    let case = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/conformance/autodiff/mutation_accumulator/input.paco");
    fs::copy(case, &input).unwrap();
    let build = Command::new(env!("CARGO_BIN_EXE_paco")).arg("build").arg(&input).env("PACO_SANITIZE", "address").output().unwrap();
    assert!(build.status.success(), "{}", String::from_utf8_lossy(&build.stderr));
    let binary = input.with_extension(std::env::consts::EXE_EXTENSION);
    let clean = Command::new(&binary).env("ASAN_OPTIONS", "detect_leaks=1").output().unwrap();
    let leaked = Command::new(&binary).env("ASAN_OPTIONS", "detect_leaks=1").env("PACO_AD_LEAK_TAPE", "1").output().unwrap();
    let _ = fs::remove_dir_all(&scratch);
    assert!(clean.status.success(), "{}", String::from_utf8_lossy(&clean.stderr));
    let stderr = String::from_utf8_lossy(&leaked.stderr);
    assert!(!leaked.status.success() && stderr.contains("LeakSanitizer"), "the leaked tape went unreported: {stderr}");
}
