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
    pub status: GoldenStatus,
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
        status,
    })
}
