//! `stdlib_root()` resolves `PACO_STD`, then the distribution's
//! `lib/paco/stdlib` next to the running executable (`paco-link`'s
//! `Toolchain::locate()`), then a development checkout. This builds a
//! minimal "unpacked distribution" (`bin/paco` + `lib/paco/stdlib/`) in a
//! temporary directory, runs the copied `paco` with `PACO_STD` unset and no
//! other environment, and proves it found *that* std — not the real
//! checkout's, which is still on disk but must not be reachable through the
//! distribution lookup — by importing a module that exists only in the fake
//! distribution.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn real_std_core() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../stdlib/core")
}

/// `paco` (the launcher, `src/launcher.rs`) shells out to `paco-compile`
/// (`src/main.rs`) for every non-cached command, so a fake distribution
/// needs both binaries side by side, as `scripts/dist.sh` ships them.
/// Returns the path to the copied launcher.
fn copy_bins(bin_dir: &Path) -> PathBuf {
    let mut launcher = None;
    for (name, source) in [("paco", env!("CARGO_BIN_EXE_paco")), ("paco-compile", env!("CARGO_BIN_EXE_paco-compile"))] {
        let dest = bin_dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
        fs::copy(source, &dest).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&dest).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&dest, perms).unwrap();
        }
        if name == "paco" {
            launcher = Some(dest);
        }
    }
    launcher.unwrap()
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).unwrap();
        }
    }
}

#[test]
fn resolves_std_from_an_unpacked_distribution_with_no_checkout_and_no_env() {
    let dist = tempfile::Builder::new().prefix("paco-dist-stdlib").tempdir().unwrap();
    let bin_dir = dist.path().join("bin");
    let std_dir = dist.path().join("lib/paco/stdlib");
    fs::create_dir_all(&bin_dir).unwrap();

    // The real prelude, so ordinary programs still type-check, plus a
    // module that exists ONLY in this fake distribution.
    copy_dir(&real_std_core(), &std_dir.join("core"));
    fs::write(std_dir.join("marker.paco"), "module marker;\n\npub const FOUND: i64 = 1;\n").unwrap();

    let paco_copy = copy_bins(&bin_dir);

    let workdir = tempfile::Builder::new().prefix("paco-dist-stdlib-work").tempdir().unwrap();
    let input = workdir.path().join("input.paco");
    fs::write(&input, "use stdlib::marker;\n\nfn main() {\n    print(1)\n}\n").unwrap();

    let output = Command::new(&paco_copy)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .current_dir(workdir.path())
        .arg("check")
        .arg(&input)
        .output()
        .unwrap();
    assert!(output.status.success(), "stdout: {}\nstderr: {}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
}

/// The same fake distribution, but without `stdlib::marker` — proves a program
/// that only needs the prelude has no reason to fall through to a checkout
/// either.
#[test]
fn a_prelude_only_program_needs_nothing_but_the_distribution_core() {
    let dist = tempfile::Builder::new().prefix("paco-dist-stdlib-core-only").tempdir().unwrap();
    let bin_dir = dist.path().join("bin");
    let std_dir = dist.path().join("lib/paco/stdlib");
    fs::create_dir_all(&bin_dir).unwrap();
    copy_dir(&real_std_core(), &std_dir.join("core"));

    let paco_copy = copy_bins(&bin_dir);

    let workdir = tempfile::Builder::new().prefix("paco-dist-stdlib-core-only-work").tempdir().unwrap();
    let input = workdir.path().join("input.paco");
    fs::write(&input, "fn main() {\n    print(1)\n}\n").unwrap();

    let output = Command::new(&paco_copy)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .current_dir(workdir.path())
        .arg("check")
        .arg(&input)
        .output()
        .unwrap();
    assert!(output.status.success(), "stdout: {}\nstderr: {}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
}
