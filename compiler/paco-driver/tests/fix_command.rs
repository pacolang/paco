//! `extract-domain-libraries` task 4.3: `paco fix` end to end, against the
//! real `paco` binary (`module_fetch.rs`'s own conventions: a throwaway
//! local git repository standing in for `github.com/pacolang/numerics`,
//! pre-fetched into the package cache the way a prior `paco get` would
//! have left it — `paco fix` itself only edits `paco.mod`/sources; it
//! does not fetch).

use std::{fs, path::PathBuf, process::Command};

use paco_driver::{manifest, pkg_cache::{self, RefKind}};

fn temp_dir(name: &str) -> PathBuf {
    let mut dir = std::env::temp_dir();
    let suffix = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    dir.push(format!("paco_fix_{}_{}_{}", name, std::process::id(), suffix));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn git(dir: &std::path::Path, args: &[&str]) {
    let output = Command::new("git").arg("-C").arg(dir).args(args).output().expect("git should be installed for this test");
    assert!(output.status.success(), "git {args:?} failed: {}", String::from_utf8_lossy(&output.stderr));
}

/// A throwaway local git repository standing in for
/// `github.com/pacolang/numerics`, tagged `v0.1.0` — the fixed
/// placeholder version `paco fix` writes into `paco.mod` while no real
/// tag has been cut yet (`extract-domain-libraries` design.md's "latest
/// tag compatible with the running compiler" note).
const NUMERICS_TAG: &str = "v0.1.0";

const FAKE_TENSOR_MODULE: &str = r#"
pub struct ShapeError {
    pub axis: i64,
    pub expected: i64,
    pub found: i64,
}

pub fn element_count(shape: &[]i64) -> i64 {
    let mut count = 1;
    for i in 0..shape.len() {
        count = count * shape[i]
    }
    count
}

pub struct Tensor<T: Numeric, const D: int...> {
    data: []T,

    pub fn zeros() -> Self {
        let dims = D;
        Tensor { data: slice_of_zeros<T>(element_count(&dims)) }
    }

    pub fn len(&self) -> i64 {
        self.data.len()
    }
}

methods<T: Numeric + Add, const D: int...> Tensor<T, D...> {
    pub fn checked_add(&self, other: &Self) -> Result<Self, ShapeError> {
        let mut out = slice_of_zeros<T>(self.data.len());
        for i in 0..self.data.len() {
            out[i] = self.data[i] + other.data[i]
        }
        Result::Ok(Tensor { data: out })
    }

    // Still present, matching the old library's semantics
    // (`checked_add`'s own alias) -- so `paco fix` can type-check the
    // pre-migration source, which still calls `.add`, before rewriting it.
    pub fn add(&self, other: &Self) -> Result<Self, ShapeError> {
        self.checked_add(other)
    }
}
"#;

const FAKE_MATH_MODULE: &str = "pub fn value() -> i64 { 9 }\n";

fn throwaway_numerics_repo(name: &str) -> PathBuf {
    let dir = temp_dir(name);
    git(&dir, &["init", "--quiet"]);
    git(&dir, &["config", "user.email", "test@example.com"]);
    git(&dir, &["config", "user.name", "Test"]);
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("src/tensor.paco"), FAKE_TENSOR_MODULE).unwrap();
    fs::write(dir.join("src/math.paco"), FAKE_MATH_MODULE).unwrap();
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "--quiet", "-m", "init"]);
    git(&dir, &["tag", NUMERICS_TAG]);
    dir
}

fn paco() -> Command {
    Command::new(env!("CARGO_BIN_EXE_paco"))
}

/// Pre-fetches the throwaway repo into `cache_root` under `NUMERICS_TAG`
/// and writes a matching (but not-yet-declared-in-`paco.mod`) lock entry
/// — `resolve_domain_use_path` looks lock entries up by dependency path
/// and tag alone, so this is harmless until `paco fix` adds the same
/// entry to `paco.mod`, exactly matching what a real prior `paco get`
/// followed by `paco fix` would leave behind.
fn prefetch_numerics(repo: &std::path::Path, cache_root: &std::path::Path, program_dir: &std::path::Path) {
    let fetched = pkg_cache::fetch_into_cache(cache_root, "github.com/pacolang/numerics", NUMERICS_TAG, RefKind::TagOrBranch, repo.to_str().unwrap(), None).unwrap();
    manifest::Lockfile {
        packages: vec![manifest::LockedPackage {
            path: "github.com/pacolang/numerics".to_string(),
            tag: Some(NUMERICS_TAG.to_string()),
            commit: fetched.commit,
            ..Default::default()
        }],
    }
    .write(&program_dir.join("paco.lock"))
    .unwrap();
}

#[test]
fn fix_rewrites_an_aliased_import_and_adds_the_dependency() {
    let repo = throwaway_numerics_repo("aliased_repo");
    let cache_root = temp_dir("aliased_cache");
    let program_dir = temp_dir("aliased_program");
    prefetch_numerics(&repo, &cache_root, &program_dir);
    fs::write(program_dir.join("paco.mod"), "module = \"example.com/myprogram\"\n").unwrap();
    fs::write(program_dir.join("main.paco"), "use stdlib::math as m;\n\nfn main() {\n    print(m::value())\n}\n").unwrap();

    let fix = paco().arg("fix").arg(program_dir.join("paco.mod")).env("PACO_PKG_CACHE", &cache_root).output().unwrap();
    assert!(fix.status.success(), "paco fix failed: {}", String::from_utf8_lossy(&fix.stderr));

    let fixed = fs::read_to_string(program_dir.join("main.paco")).unwrap();
    assert_eq!(fixed, "use github.com/pacolang/numerics/math as m;\n\nfn main() {\n    print(m::value())\n}\n");

    let manifest_text = fs::read_to_string(program_dir.join("paco.mod")).unwrap();
    let parsed = manifest::Manifest::parse(&manifest_text).unwrap();
    assert_eq!(parsed.dependencies, vec![("github.com/pacolang/numerics".to_string(), manifest::DependencySource::Tag(NUMERICS_TAG.to_string()))]);

    // `paco check` (type-checking only, no codegen): an *aliased*
    // `Domain`-kind import (`as m`) hits a pre-existing codegen panic
    // (`paco-codegen-cranelift`, "undeclared function `j::value`") that
    // reproduces on plain `use a.b/c as j;` with no `extract-domain-
    // libraries` code involved at all -- confirmed independently and out
    // of this task's paths (`compiler/paco-driver`, not
    // `paco-codegen-cranelift`) to fix. `paco check` still proves the
    // rewritten import resolves and type-checks cleanly, which is what
    // this task owns; see the final report for the codegen finding.
    let check = paco().arg("check").arg(program_dir.join("main.paco")).env("PACO_PKG_CACHE", &cache_root).output().unwrap();
    assert!(check.status.success(), "paco check failed after `paco fix`: {}", String::from_utf8_lossy(&check.stderr));
    assert!(check.stderr.is_empty(), "expected no warnings after `paco fix`: {}", String::from_utf8_lossy(&check.stderr));

    let _ = fs::remove_dir_all(&repo);
    let _ = fs::remove_dir_all(&cache_root);
    let _ = fs::remove_dir_all(&program_dir);
}

#[test]
fn fix_rewrites_a_numerics_import_the_qualifier_and_the_add_call() {
    let repo = throwaway_numerics_repo("numerics_repo");
    let cache_root = temp_dir("numerics_cache");
    let program_dir = temp_dir("numerics_program");
    prefetch_numerics(&repo, &cache_root, &program_dir);
    fs::write(program_dir.join("paco.mod"), "module = \"example.com/myprogram\"\n").unwrap();
    fs::write(
        program_dir.join("main.paco"),
        "use stdlib::numerics;\n\nfn main() {\n    let a = numerics::Tensor<f32, 4>::zeros();\n    let b = numerics::Tensor<f32, 4>::zeros();\n    match a.add(&b) {\n        Result::Ok(c) => print(c.len()),\n        Result::Err(e) => print(e.axis),\n    }\n}\n",
    )
    .unwrap();

    let fix = paco().arg("fix").arg(program_dir.join("paco.mod")).env("PACO_PKG_CACHE", &cache_root).output().unwrap();
    assert!(fix.status.success(), "paco fix failed: {}", String::from_utf8_lossy(&fix.stderr));

    let fixed = fs::read_to_string(program_dir.join("main.paco")).unwrap();
    assert!(fixed.starts_with("use github.com/pacolang/numerics/tensor;\n"), "{fixed}");
    assert!(fixed.contains("tensor::Tensor<f32, 4>::zeros()"), "{fixed}");
    assert!(!fixed.contains("numerics::"), "{fixed}");
    assert!(fixed.contains("a.checked_add(&b)"), "{fixed}");
    assert!(!fixed.contains(".add(&b)"), "{fixed}");

    let run = paco().arg("run").arg(program_dir.join("main.paco")).env("PACO_PKG_CACHE", &cache_root).output().unwrap();
    assert!(run.status.success(), "paco run failed after `paco fix`: {}", String::from_utf8_lossy(&run.stderr));
    assert_eq!(String::from_utf8_lossy(&run.stdout), "4\n");
    assert!(run.stderr.is_empty(), "expected no warnings after `paco fix`: {}", String::from_utf8_lossy(&run.stderr));

    let _ = fs::remove_dir_all(&repo);
    let _ = fs::remove_dir_all(&cache_root);
    let _ = fs::remove_dir_all(&program_dir);
}

#[test]
fn fix_leaves_a_generic_receivers_same_named_method_untouched() {
    let program_dir = temp_dir("generic_receiver");
    fs::write(program_dir.join("paco.mod"), "module = \"example.com/myprogram\"\n").unwrap();
    let source = "trait Addable {\n    fn add(&self, other: &Self) -> Self;\n}\n\nfn f<T: Addable>(a: T, b: T) -> T {\n    a.add(&b)\n}\n\nfn main() {}\n";
    fs::write(program_dir.join("main.paco"), source).unwrap();

    let fix = paco().arg("fix").arg(program_dir.join("paco.mod")).output().unwrap();
    assert!(fix.status.success(), "paco fix failed: {}", String::from_utf8_lossy(&fix.stderr));

    let fixed = fs::read_to_string(program_dir.join("main.paco")).unwrap();
    assert_eq!(fixed, source, "a generic receiver's own `.add` must be left exactly as written");

    let manifest_text = fs::read_to_string(program_dir.join("paco.mod")).unwrap();
    assert_eq!(manifest_text, "module = \"example.com/myprogram\"\n", "no moved import means no dependency should be added");

    let _ = fs::remove_dir_all(&program_dir);
}

#[test]
fn fix_is_idempotent() {
    let repo = throwaway_numerics_repo("idempotent_repo");
    let cache_root = temp_dir("idempotent_cache");
    let program_dir = temp_dir("idempotent_program");
    prefetch_numerics(&repo, &cache_root, &program_dir);
    fs::write(program_dir.join("paco.mod"), "module = \"example.com/myprogram\"\n").unwrap();
    fs::write(
        program_dir.join("main.paco"),
        "use stdlib::numerics;\n\nfn main() {\n    let a = numerics::Tensor<f32, 4>::zeros();\n    let b = numerics::Tensor<f32, 4>::zeros();\n    match a.add(&b) {\n        Result::Ok(c) => print(c.len()),\n        Result::Err(e) => print(e.axis),\n    }\n}\n",
    )
    .unwrap();

    let first = paco().arg("fix").arg(program_dir.join("paco.mod")).env("PACO_PKG_CACHE", &cache_root).output().unwrap();
    assert!(first.status.success(), "first `paco fix` failed: {}", String::from_utf8_lossy(&first.stderr));
    let fixed_once = fs::read_to_string(program_dir.join("main.paco")).unwrap();
    let manifest_once = fs::read_to_string(program_dir.join("paco.mod")).unwrap();

    let second = paco().arg("fix").arg(program_dir.join("paco.mod")).env("PACO_PKG_CACHE", &cache_root).output().unwrap();
    assert!(second.status.success(), "second `paco fix` failed: {}", String::from_utf8_lossy(&second.stderr));
    assert!(second.stdout.is_empty(), "a second `paco fix` should have nothing left to fix: {}", String::from_utf8_lossy(&second.stdout));

    let fixed_twice = fs::read_to_string(program_dir.join("main.paco")).unwrap();
    let manifest_twice = fs::read_to_string(program_dir.join("paco.mod")).unwrap();
    assert_eq!(fixed_once, fixed_twice);
    assert_eq!(manifest_once, manifest_twice);

    let _ = fs::remove_dir_all(&repo);
    let _ = fs::remove_dir_all(&cache_root);
    let _ = fs::remove_dir_all(&program_dir);
}
