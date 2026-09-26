//! Golden test discovery for Paco compiler conformance tests.

use std::{
    error::Error,
    fs,
    path::{Path, PathBuf},
};

use walkdir::WalkDir;

pub type HarnessResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TestKind {
    Fail,
    Run,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GoldenStatus {
    Active,
    Skipped { feature_min: u32 },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GoldenTest {
    pub path: PathBuf,
    pub input: PathBuf,
    pub expected: PathBuf,
    pub kind: TestKind,
    pub feature_min: u32,
    pub build: bool,
    /// The exit status a `run` case's compiled program ends with.
    pub exit: i32,
    /// `exit` for release builds, when it differs.
    pub release_exit: i32,
    /// Also run the program with every `print(e)` in `main` evaluated at
    /// compile time, as `print(comptime { e })`, expecting the same output.
    pub comptime_differential: bool,
    pub status: GoldenStatus,
}

/// What a compiled `run` case must produce in one profile.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExpectedRun {
    pub stdout: String,
    pub stderr: Option<String>,
    pub exit: i32,
}

impl GoldenTest {
    /// `expected.stdout`/`expected.stderr`, overridden for release builds by
    /// `expected.release.stdout`/`expected.release.stderr` when present.
    pub fn expected_run(&self, release: bool) -> ExpectedRun {
        let read = |name: &str| fs::read_to_string(self.path.join(name)).ok().map(|text| text.replace("\r\n", "\n"));
        let (release_stdout, release_stderr) = if release {
            (read("expected.release.stdout"), read("expected.release.stderr"))
        } else {
            (None, None)
        };
        ExpectedRun {
            stdout: release_stdout.or_else(|| read("expected.stdout")).unwrap_or_default(),
            stderr: release_stderr.or_else(|| read("expected.stderr")),
            exit: if release { self.release_exit } else { self.exit },
        }
    }
}

pub fn discover_golden_tests(
    root: impl AsRef<Path>,
    current_feature_level: u32,
) -> HarnessResult<Vec<GoldenTest>> {
    let root = root.as_ref();
    if !root.exists() {
        return Ok(Vec::new());
    }

    let mut tests = Vec::new();
    for entry in WalkDir::new(root).into_iter().filter_map(Result::ok) {
        if !entry.file_type().is_file() || entry.file_name() != "flags.toml" {
            continue;
        }
        tests.push(read_golden_test(entry.path(), current_feature_level)?);
    }

    tests.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(tests)
}

fn read_golden_test(flags_path: &Path, current_feature_level: u32) -> HarnessResult<GoldenTest> {
    let dir = flags_path
        .parent()
        .ok_or_else(|| format!("flags file has no parent: {}", flags_path.display()))?;
    let flags = fs::read_to_string(flags_path)?;
    let flags: toml::Value = flags.parse()?;
    let kind = match flags.get("kind").and_then(toml::Value::as_str) {
        Some("fail") => TestKind::Fail,
        Some("run") => TestKind::Run,
        Some(other) => return Err(format!("unknown golden test kind `{other}`").into()),
        None => return Err("golden test flags.toml is missing `kind`".into()),
    };
    let feature_min = flags
        .get("feature_min")
        .and_then(toml::Value::as_integer)
        .unwrap_or(0)
        .try_into()?;

    let build = flags.get("build").and_then(toml::Value::as_bool).unwrap_or(false);
    let exit: i32 = flags.get("exit").and_then(toml::Value::as_integer).unwrap_or(0).try_into()?;
    let release_exit: i32 =
        flags.get("release_exit").and_then(toml::Value::as_integer).map_or(Ok(exit), i32::try_from)?;
    let expected = match kind {
        TestKind::Fail => dir.join("expected.stderr"),
        TestKind::Run => dir.join("expected.stdout"),
    };
    let status = if feature_min > current_feature_level {
        GoldenStatus::Skipped { feature_min }
    } else {
        GoldenStatus::Active
    };

    Ok(GoldenTest {
        path: dir.to_path_buf(),
        input: dir.join("input.paco"),
        expected,
        kind,
        feature_min,
        build,
        exit,
        release_exit,
        comptime_differential: flags.get("comptime_differential").and_then(toml::Value::as_bool).unwrap_or(false),
        status,
    })
}

/// Finite-difference checks of gradients in conformance programs: a central
/// difference `(f(x + h) - f(x - h)) / (2 h)` agrees with the gradient when
/// `|gradient - difference| <= tolerance * (1 + |difference|)`. Each entry is
/// `(width, h, tolerance)`; the reduced widths are compared after widening,
/// and a gradient in them is also checked against the rounded `f32` one to
/// within one unit in the last place.
pub const FINITE_DIFFERENCES: [(&str, f64, f64); 4] =
    [("f64", 0.000001, 0.000001), ("f32", 0.001, 0.001), ("f16", 0.1, 0.05), ("bf16", 0.25, 0.1)];

pub fn finite_difference_agrees(width: &str, gradient: f64, difference: f64) -> bool {
    let (_, _, tolerance) = FINITE_DIFFERENCES.iter().find(|(name, ..)| *name == width).copied().unwrap_or(FINITE_DIFFERENCES[0]);
    (gradient - difference).abs() <= tolerance * (1.0 + difference.abs())
}

/// `text` with every `dir` prefix of a path removed, whichever separator
/// follows it (`/`, or `\` on Windows).
pub fn strip_dir(text: &str, dir: &Path) -> String {
    let dir = dir.display();
    text.replace(&format!("{dir}/"), "").replace(&format!("{dir}\\"), "")
}

/// Checks that every `(leg, output)` pair produced the same output as the
/// first one, naming the first leg that disagrees.
pub fn differential(legs: &[(&str, String)]) -> Result<(), String> {
    let Some((reference, expected)) = legs.first() else {
        return Ok(());
    };
    match legs.iter().find(|(_, output)| output != expected) {
        Some((leg, output)) => Err(format!("`{leg}` disagrees with `{reference}`:\n{reference}: {expected:?}\n{leg}: {output:?}")),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::{differential, finite_difference_agrees};

    #[test]
    fn finite_differences_accept_within_the_tolerance_of_their_width() {
        let f = |x: f64| x * x * x;
        let h = 0.000001;
        let difference = (f(2.0 + h) - f(2.0 - h)) / (2.0 * h);
        assert!(finite_difference_agrees("f64", 12.0, difference));
        assert!(!finite_difference_agrees("f64", 12.1, difference));
        assert!(finite_difference_agrees("bf16", 12.5, 12.0));
    }

    #[test]
    fn differential_names_the_disagreeing_leg() {
        let agree = [("run", "1\n".to_string()), ("llvm", "1\n".to_string())];
        assert!(differential(&agree).is_ok());
        let mismatched = [("run", "1\n".to_string()), ("cranelift", "1\n".to_string()), ("llvm", "2\n".to_string())];
        let error = differential(&mismatched).unwrap_err();
        assert!(error.starts_with("`llvm` disagrees with `run`"), "{error}");
    }
}

#[derive(Debug)]
pub enum TargetRun {
    Ran(std::process::Output),
    Skipped(String),
}

/// Runs `binary`, built for `target`, natively when it targets the host's
/// OS and architecture, else, on a Linux host, a Linux binary under
/// `qemu-<arch>` from `PATH` (with `sysroot` as the dynamic loader prefix).
pub fn run_for_target(binary: &Path, target: &str, sysroot: Option<&Path>) -> TargetRun {
    run_for_target_with_path(binary, target, sysroot, &std::env::var_os("PATH").unwrap_or_default())
}

fn run_for_target_with_path(binary: &Path, target: &str, sysroot: Option<&Path>, path: &std::ffi::OsStr) -> TargetRun {
    let arch = target.split('-').next().unwrap_or(target);
    let target_os = if target.contains("-linux") {
        "linux"
    } else if target.contains("-apple-") {
        "macos"
    } else if target.contains("-windows") {
        "windows"
    } else {
        std::env::consts::OS
    };
    if target_os != std::env::consts::OS && !(target_os == "linux" && cfg!(target_os = "linux")) {
        return TargetRun::Skipped(format!("skipped: a {target} binary cannot run on a {} host", std::env::consts::OS));
    }
    let mut command = if arch == std::env::consts::ARCH {
        std::process::Command::new(binary)
    } else {
        let qemu = format!("qemu-{arch}");
        let Some(found) = std::env::split_paths(path).map(|dir| dir.join(&qemu)).find(|file| file.is_file()) else {
            return TargetRun::Skipped(format!("skipped: {qemu} not installed"));
        };
        let mut command = std::process::Command::new(found);
        command.arg(binary);
        if let Some(root) = sysroot {
            command.env("QEMU_LD_PREFIX", root);
        }
        command
    };
    let mut attempts = 0;
    loop {
        match command.output() {
            Err(error) if error.kind() == std::io::ErrorKind::ExecutableFileBusy && attempts < 50 => {
                attempts += 1;
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            result => {
                return TargetRun::Ran(result.unwrap_or_else(|error| panic!("failed to run {}: {error}", binary.display())));
            }
        }
    }
}

#[cfg(test)]
mod target_run_tests {
    use super::*;

    fn host_target() -> String {
        match std::env::consts::OS {
            "linux" => format!("{}-unknown-linux-musl", std::env::consts::ARCH),
            "macos" => format!("{}-apple-darwin", std::env::consts::ARCH),
            _ => format!("{}-pc-windows-msvc", std::env::consts::ARCH),
        }
    }

    #[test]
    fn runs_host_binaries_natively() {
        let program = if cfg!(windows) { "C:/Windows/System32/hostname.exe" } else { "/usr/bin/true" };
        let TargetRun::Ran(output) = run_for_target_with_path(Path::new(program), &host_target(), None, "".as_ref()) else {
            panic!("a host binary must run natively");
        };
        assert!(output.status.success());
    }

    #[cfg(target_os = "linux")]
    fn foreign_arch() -> &'static str {
        if std::env::consts::ARCH == "aarch64" { "x86_64" } else { "aarch64" }
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn skips_linux_binaries_on_other_hosts() {
        let target = format!("{}-unknown-linux-musl", std::env::consts::ARCH);
        let run = run_for_target_with_path(Path::new("/prog"), &target, None, "".as_ref());
        let TargetRun::Skipped(message) = run else { panic!("a Linux binary must not run on {}", std::env::consts::OS) };
        assert!(message.contains(&target), "{message}");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn runs_foreign_binaries_under_qemu_from_path() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let qemu = dir.path().join(format!("qemu-{}", foreign_arch()));
        fs::write(&qemu, "#!/bin/sh\necho \"$QEMU_LD_PREFIX $1\"\n").unwrap();
        fs::set_permissions(&qemu, fs::Permissions::from_mode(0o755)).unwrap();
        let target = format!("{}-unknown-linux-gnu", foreign_arch());
        let run = run_for_target_with_path(Path::new("/prog"), &target, Some(Path::new("/root")), dir.path().as_os_str());
        let TargetRun::Ran(output) = run else { panic!("qemu on PATH must be used") };
        assert_eq!(String::from_utf8_lossy(&output.stdout), "/root /prog\n");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn skips_foreign_binaries_without_qemu() {
        let dir = tempfile::tempdir().unwrap();
        let target = format!("{}-unknown-linux-musl", foreign_arch());
        let run = run_for_target_with_path(Path::new("/prog"), &target, None, dir.path().as_os_str());
        let TargetRun::Skipped(message) = run else { panic!("no qemu must skip") };
        assert_eq!(message, format!("skipped: qemu-{} not installed", foreign_arch()));
    }
}
