use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

use paco_driver::cache::{Cache, Policy, cache_dir};
use paco_driver::{build_cached, link_count};

fn env(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
    let vars: Vec<(String, String)> = vars.iter().map(|(name, value)| (name.to_string(), value.to_string())).collect();
    move |name| vars.iter().find(|(var, _)| var == name).map(|(_, value)| OsString::from(value))
}

#[test]
fn the_cache_directory_comes_from_paco_cache_then_xdg_then_home() {
    let all = [("PACO_CACHE", "/p"), ("XDG_CACHE_HOME", "/x"), ("HOME", "/h")];
    assert_eq!(cache_dir(env(&all)), Some(PathBuf::from("/p")));
    assert_eq!(cache_dir(env(&all[1..])), Some(PathBuf::from("/x/paco")));
    assert_eq!(cache_dir(env(&all[2..])), Some(PathBuf::from("/h/.cache/paco")));
    assert_eq!(cache_dir(env(&[("PACO_CACHE", ""), ("HOME", "/h")])), Some(PathBuf::from("/h/.cache/paco")));
    assert_eq!(cache_dir(env(&[])), None);
}

#[test]
fn an_unusable_cache_directory_falls_back_to_a_temporary_one() {
    let blocker = tempfile::NamedTempFile::new().unwrap();
    let cache = Cache::open(Some(blocker.path().join("cache")), Policy::default());
    assert!(cache.is_temporary());
    let root = cache.root().to_path_buf();
    assert!(root.is_dir());
    drop(cache);
    assert!(!root.exists());
}

struct Project {
    dir: tempfile::TempDir,
}

impl Project {
    fn new(files: &[(&str, &str)]) -> Self {
        let dir = tempfile::tempdir().unwrap();
        for (name, source) in files {
            fs::write(dir.path().join(name), source).unwrap();
        }
        Self { dir }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }
}

const MAIN: &str = "use helper;\n\nfn main() {\n    print(helper::value());\n}\n";

fn helper(value: i64) -> String {
    format!("pub fn value() -> i64 {{\n    {value}\n}}\n")
}

fn run(binary: &Path) -> String {
    String::from_utf8(Command::new(binary).output().unwrap().stdout).unwrap()
}

fn fresh_cache(policy: Policy) -> (tempfile::TempDir, Cache) {
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::open(Some(dir.path().join("cache")), policy);
    (dir, cache)
}

fn manifests(cache: &Cache) -> Vec<PathBuf> {
    fs::read_dir(cache.root().join("index")).map_or(Vec::new(), |dir| dir.flatten().map(|entry| entry.path()).collect())
}

#[test]
fn the_manifest_lists_every_module_and_prelude_file_the_build_read() {
    let project = Project::new(&[("main.paco", MAIN), ("helper.paco", &helper(7))]);
    let (_dir, cache) = fresh_cache(Policy::default());
    let entry = build_cached(&project.path("main.paco"), &cache, "test").unwrap();
    assert_eq!(run(&entry.binary), "7\n");

    let manifests = manifests(&cache);
    assert_eq!(manifests.len(), 1);
    let manifest = fs::read_to_string(&manifests[0]).unwrap();
    let canonical = |name: &str| fs::canonicalize(project.path(name)).unwrap().display().to_string();
    assert!(manifest.contains(&canonical("main.paco")), "{manifest}");
    assert!(manifest.contains(&canonical("helper.paco")), "{manifest}");
    let std_core_sep = format!("{}stdlib{}core{}", std::path::MAIN_SEPARATOR, std::path::MAIN_SEPARATOR, std::path::MAIN_SEPARATOR);
    assert!(manifest.lines().any(|line| line.contains(&std_core_sep) && line.ends_with(".paco")), "{manifest}");
    assert_eq!(fs::read_dir(cache.root().join("tmp")).unwrap().count(), 0, "no staged files are left behind");
}

#[test]
fn an_unchanged_program_is_not_recompiled() {
    let project = Project::new(&[("main.paco", MAIN), ("helper.paco", &helper(1))]);
    let (_dir, cache) = fresh_cache(Policy::default());
    let links = link_count();
    let first = build_cached(&project.path("main.paco"), &cache, "test").unwrap();
    let second = build_cached(&project.path("main.paco"), &cache, "test").unwrap();
    assert_eq!(link_count() - links, 1);
    assert_eq!(first.binary, second.binary);
    assert_eq!(run(&second.binary), "1\n");
}

#[test]
fn editing_an_imported_module_invalidates_the_entry() {
    let project = Project::new(&[("main.paco", MAIN), ("helper.paco", &helper(1))]);
    let (_dir, cache) = fresh_cache(Policy::default());
    let links = link_count();
    build_cached(&project.path("main.paco"), &cache, "test").unwrap();
    fs::write(project.path("helper.paco"), helper(2)).unwrap();
    let edited = build_cached(&project.path("main.paco"), &cache, "test").unwrap();
    assert_eq!(link_count() - links, 2);
    assert_eq!(run(&edited.binary), "2\n");
}

#[test]
fn a_different_compiler_version_does_not_reuse_entries() {
    let project = Project::new(&[("main.paco", MAIN), ("helper.paco", &helper(3))]);
    let (_dir, cache) = fresh_cache(Policy::default());
    let links = link_count();
    build_cached(&project.path("main.paco"), &cache, "paco 1").unwrap();
    build_cached(&project.path("main.paco"), &cache, "paco 2").unwrap();
    build_cached(&project.path("main.paco"), &cache, "paco 2").unwrap();
    assert_eq!(link_count() - links, 2);
}

#[test]
fn a_program_that_fails_checking_leaves_nothing_in_the_cache() {
    let project = Project::new(&[("main.paco", "fn main() {\n    let x: i64 = \"text\";\n}\n")]);
    let (_dir, cache) = fresh_cache(Policy::default());
    let error = build_cached(&project.path("main.paco"), &cache, "test").err().expect("a type error");
    assert!(error.contains("PACO-E0302"), "{error}");
    assert!(manifests(&cache).is_empty());
    assert_eq!(fs::read_dir(cache.root().join("tmp")).unwrap().count(), 0);
}

fn set_age(path: &Path, hours: u64) {
    let file = fs::File::options().append(true).open(path).unwrap();
    file.set_modified(SystemTime::now() - Duration::from_secs(hours * 60 * 60)).unwrap();
}

#[test]
fn pruning_removes_the_least_recently_used_entries_first() {
    let project = Project::new(&[
        ("one.paco", "fn main() {\n    print(1);\n}\n"),
        ("two.paco", "fn main() {\n    print(2);\n}\n"),
        ("three.paco", "fn main() {\n    print(3);\n}\n"),
    ]);
    // The policy fits `one` and `two` (measured directly, since a binary's
    // size varies with its embedded source file name and, on Windows,
    // COFF's page alignment) but not all three.
    let size = |binary: &std::path::Path| {
        let dwarf = fs::metadata(paco_driver::cache::debug_file(binary)).map_or(0, |metadata| metadata.len());
        fs::metadata(binary).unwrap().len() + dwarf
    };
    let two_entries = {
        let (_dir, cache) = fresh_cache(Policy::default());
        size(&build_cached(&project.path("one.paco"), &cache, "test").unwrap().binary)
            + size(&build_cached(&project.path("two.paco"), &cache, "test").unwrap().binary)
    };
    let policy = Policy { max_size: two_entries + two_entries / 10, prune_interval: Duration::ZERO, ..Policy::default() };
    let (_dir, cache) = fresh_cache(policy);
    let one = build_cached(&project.path("one.paco"), &cache, "test").unwrap().binary;
    let two = build_cached(&project.path("two.paco"), &cache, "test").unwrap().binary;
    assert!(one.exists() && two.exists(), "the policy must fit both entries before the third is built");
    for manifest in manifests(&cache) {
        let text = fs::read_to_string(&manifest).unwrap();
        let output = if text.contains(&one.file_stem().unwrap().to_string_lossy().to_string()) { 3 } else { 2 };
        set_age(&manifest, output);
    }
    let three = build_cached(&project.path("three.paco"), &cache, "test").unwrap().binary;
    assert!(!one.exists(), "the oldest entry is removed");
    assert!(two.exists() && three.exists());
    assert_eq!(manifests(&cache).len(), 2);
}

#[test]
fn pruning_removes_entries_unused_for_longer_than_the_age_limit() {
    let project = Project::new(&[("old.paco", "fn main() {\n    print(1);\n}\n"), ("new.paco", "fn main() {\n    print(2);\n}\n")]);
    let policy = Policy { max_age: Duration::from_secs(60 * 60), prune_interval: Duration::ZERO, ..Policy::default() };
    let (_dir, cache) = fresh_cache(policy);
    let old = build_cached(&project.path("old.paco"), &cache, "test").unwrap().binary;
    for manifest in manifests(&cache) {
        set_age(&manifest, 2);
    }
    let new = build_cached(&project.path("new.paco"), &cache, "test").unwrap().binary;
    assert!(!old.exists() && new.exists());
}

#[test]
fn clean_empties_the_cache_and_removes_build_outputs() {
    let project = Project::new(&[("main.paco", "fn main() {\n    print(1);\n}\n")]);
    let (dir, cache) = fresh_cache(Policy::default());
    build_cached(&project.path("main.paco"), &cache, "test").unwrap();
    let paco = |args: &[&str]| {
        let output = Command::new(env!("CARGO_BIN_EXE_paco")).args(args).env("PACO_CACHE", cache.root()).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    };
    paco(&["clean", "--cache"]);
    assert_eq!(fs::read_dir(cache.root()).unwrap().count(), 0);

    let main = project.path("main.paco");
    let main_binary = project.path(&format!("main{}", std::env::consts::EXE_SUFFIX));
    paco(&["build", main.to_str().unwrap()]);
    assert!(main_binary.exists());
    paco(&["clean", main.to_str().unwrap()]);
    assert!(!main_binary.exists());
    assert!(main.exists());
    drop(dir);
}
