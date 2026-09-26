use std::{fs, path::PathBuf, process::Command};

use clap::Parser;
use paco_driver::{Cli, git, manifest, pkg_cache::{self, RefKind}, run};

fn temp_dir(name: &str) -> PathBuf {
    let mut dir = std::env::temp_dir();
    let suffix = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    dir.push(format!("paco_module_fetch_{}_{}_{}", name, std::process::id(), suffix));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn git(dir: &std::path::Path, args: &[&str]) {
    let output = Command::new("git").arg("-C").arg(dir).args(args).output().expect("git should be installed for this test");
    assert!(output.status.success(), "git {args:?} failed: {}", String::from_utf8_lossy(&output.stderr));
}

/// A throwaway local git repository (standing in for a remote one, no
/// network needed) with `src/<name>.paco` for each of `files`, tagged
/// `tag`.
fn throwaway_dependency_repo(name: &str, tag: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = temp_dir(name);
    git(&dir, &["init", "--quiet"]);
    git(&dir, &["config", "user.email", "test@example.com"]);
    git(&dir, &["config", "user.name", "Test"]);
    fs::create_dir_all(dir.join("src")).unwrap();
    for (file_name, content) in files {
        fs::write(dir.join("src").join(file_name), content).unwrap();
    }
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "--quiet", "-m", "init"]);
    git(&dir, &["tag", tag]);
    dir
}

#[test]
fn get_with_no_manifest_present_is_a_clear_error_not_a_panic() {
    let dir = temp_dir("no_manifest");
    let manifest_path = dir.join("paco.mod");

    let cli = Cli::try_parse_from(["paco", "get", manifest_path.to_str().unwrap()]).unwrap();
    let error = run(cli).unwrap_err();

    assert!(error.contains("paco.mod"), "{error}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn get_with_a_malformed_manifest_is_a_named_error_not_a_panic() {
    let dir = temp_dir("malformed_manifest");
    let manifest_path = dir.join("paco.mod");
    fs::write(&manifest_path, "this is not valid toml =\n").unwrap();

    let cli = Cli::try_parse_from(["paco", "get", manifest_path.to_str().unwrap()]).unwrap();
    let error = run(cli).unwrap_err();

    assert!(!error.is_empty());

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn get_with_no_dependencies_writes_an_empty_lock_file() {
    let dir = temp_dir("empty_deps");
    let manifest_path = dir.join("paco.mod");
    fs::write(&manifest_path, "module = \"example.com/myproject\"\n").unwrap();

    let cli = Cli::try_parse_from(["paco", "get", manifest_path.to_str().unwrap()]).unwrap();
    run(cli).unwrap_or_else(|error| panic!("`paco get` failed: {error}"));

    let lock = fs::read_to_string(dir.join("paco.lock")).unwrap();
    assert!(lock.is_empty());

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn mod_tidy_with_no_manifest_present_is_a_clear_error_not_a_panic() {
    let dir = temp_dir("tidy_no_manifest");
    let manifest_path = dir.join("paco.mod");

    let cli = Cli::try_parse_from(["paco", "mod", "tidy", manifest_path.to_str().unwrap()]).unwrap();
    let error = run(cli).unwrap_err();

    assert!(error.contains("paco.mod"), "{error}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn mod_tidy_reports_an_unused_manifest_entry() {
    let dir = temp_dir("tidy_unused");
    let manifest_path = dir.join("paco.mod");
    fs::write(&manifest_path, "module = \"example.com/myproject\"\n\n[dependencies]\n\"example.com/team/json\" = \"v1.2.0\"\n").unwrap();
    fs::write(dir.join("main.paco"), "fn main() {}\n").unwrap();

    let cli = Cli::try_parse_from(["paco", "mod", "tidy", manifest_path.to_str().unwrap()]).unwrap();
    let output = run(cli).unwrap_or_else(|error| panic!("`paco mod tidy` failed: {error}"));

    assert!(output.stdout.contains("unused dependency"), "{}", output.stdout);
    assert!(output.stdout.contains("example.com/team/json"), "{}", output.stdout);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn mod_tidy_reports_a_used_dependency_missing_from_the_manifest() {
    let dir = temp_dir("tidy_undeclared");
    let manifest_path = dir.join("paco.mod");
    fs::write(&manifest_path, "module = \"example.com/myproject\"\n").unwrap();
    fs::write(dir.join("main.paco"), "use example.com/team/json;\n\nfn main() {}\n").unwrap();

    let cli = Cli::try_parse_from(["paco", "mod", "tidy", manifest_path.to_str().unwrap()]).unwrap();
    let output = run(cli).unwrap_or_else(|error| panic!("`paco mod tidy` failed: {error}"));

    assert!(output.stdout.contains("undeclared dependency"), "{}", output.stdout);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn mod_tidy_reports_nothing_when_every_dependency_is_used() {
    let dir = temp_dir("tidy_clean");
    let manifest_path = dir.join("paco.mod");
    fs::write(&manifest_path, "module = \"example.com/myproject\"\n\n[dependencies]\n\"example.com/team/json\" = \"v1.2.0\"\n").unwrap();
    fs::write(dir.join("main.paco"), "use example.com/team/json;\n\nfn main() {}\n").unwrap();

    let cli = Cli::try_parse_from(["paco", "mod", "tidy", manifest_path.to_str().unwrap()]).unwrap();
    let output = run(cli).unwrap_or_else(|error| panic!("`paco mod tidy` failed: {error}"));

    assert!(output.stdout.is_empty(), "{}", output.stdout);

    let _ = fs::remove_dir_all(&dir);
}

// --- End-to-end domain-shaped `use` resolution (git-module-fetch 6.2) ---
//
// `paco get`'s own network step (paco.mod key -> `https://<key>`) can't be
// exercised offline, and task 4 already covers it via
// `pkg_cache::fetch_into_cache` with an explicit local `url`. These tests
// populate the package cache the same way (functionally identical to what
// a successful `paco get` against a throwaway repo would leave behind:
// same cache layout, same lock file shape) and then run the *real*
// compiled `paco` binary — matching the codebase's existing
// `CARGO_BIN_EXE_paco` + scoped `.env(..)` convention (e.g.
// `run_cache.rs`) — to actually exercise task 6.1's resolution end to end.

fn paco() -> Command {
    Command::new(env!("CARGO_BIN_EXE_paco"))
}

#[test]
fn a_fetched_dependencys_item_is_usable_after_use() {
    let repo = throwaway_dependency_repo("e2e_single_repo", "v1.0.0", &[("json.paco", "pub fn value() -> i64 { 42 }\n")]);
    let cache_root = temp_dir("e2e_single_cache");
    let program_dir = temp_dir("e2e_single_program");

    let fetched = pkg_cache::fetch_into_cache(&cache_root, "example.com/team/json", "v1.0.0", RefKind::TagOrBranch, repo.to_str().unwrap(), None).unwrap();
    fs::write(
        program_dir.join("paco.mod"),
        "module = \"example.com/myprogram\"\n\n[dependencies]\n\"example.com/team/json\" = \"v1.0.0\"\n",
    )
    .unwrap();
    manifest::Lockfile {
        packages: vec![manifest::LockedPackage { path: "example.com/team/json".to_string(), tag: Some("v1.0.0".to_string()), commit: fetched.commit, ..Default::default() }],
    }
    .write(&program_dir.join("paco.lock"))
    .unwrap();
    fs::write(program_dir.join("main.paco"), "use example.com/team/json;\n\nfn main() {\n    print(json::value())\n}\n").unwrap();

    let output = paco().arg("run").arg(program_dir.join("main.paco")).env("PACO_PKG_CACHE", &cache_root).output().unwrap();

    assert!(output.status.success(), "paco run failed: {}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(String::from_utf8_lossy(&output.stdout), "42\n");

    let _ = fs::remove_dir_all(&repo);
    let _ = fs::remove_dir_all(&cache_root);
    let _ = fs::remove_dir_all(&program_dir);
}

// --- Compiler version range (git-module-fetch 6b.2) ---

#[test]
fn a_dependency_requiring_a_newer_compiler_fails_before_compiling() {
    let repo = throwaway_dependency_repo("e2e_version_repo", "v1.0.0", &[("json.paco", "pub fn value() -> i64 { 42 }\n")]);
    // The throwaway "dependency" declares its own paco.mod (committed
    // alongside src/), the way a real fetched dependency might.
    fs::write(repo.join("paco.mod"), "module = \"example.com/team/json\"\npaco = \">=0.4\"\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "--quiet", "-m", "add paco.mod"]);
    git(&repo, &["tag", "-f", "v1.0.0"]);

    let cache_root = temp_dir("e2e_version_cache");
    let program_dir = temp_dir("e2e_version_program");
    let fetched = pkg_cache::fetch_into_cache(&cache_root, "example.com/team/json", "v1.0.0", RefKind::TagOrBranch, repo.to_str().unwrap(), None).unwrap();
    fs::write(
        program_dir.join("paco.mod"),
        "module = \"example.com/myprogram\"\n\n[dependencies]\n\"example.com/team/json\" = \"v1.0.0\"\n",
    )
    .unwrap();
    manifest::Lockfile {
        packages: vec![manifest::LockedPackage { path: "example.com/team/json".to_string(), tag: Some("v1.0.0".to_string()), commit: fetched.commit, ..Default::default() }],
    }
    .write(&program_dir.join("paco.lock"))
    .unwrap();
    fs::write(program_dir.join("main.paco"), "use example.com/team/json;\n\nfn main() {\n    print(json::value())\n}\n").unwrap();

    let output = paco()
        .arg("run")
        .arg(program_dir.join("main.paco"))
        .env("PACO_PKG_CACHE", &cache_root)
        .env("PACO_VERSION_OVERRIDE", "0.3.2")
        .output()
        .unwrap();

    assert!(!output.status.success(), "expected `paco run` to fail on an incompatible compiler version");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("example.com/team/json"), "{stderr}");
    assert!(stderr.contains(">=0.4"), "{stderr}");
    assert!(stderr.contains("0.3.2"), "{stderr}");

    let _ = fs::remove_dir_all(&repo);
    let _ = fs::remove_dir_all(&cache_root);
    let _ = fs::remove_dir_all(&program_dir);
}

#[test]
fn no_paco_field_anywhere_builds_with_no_version_check() {
    let repo = throwaway_dependency_repo("e2e_no_range_repo", "v1.0.0", &[("json.paco", "pub fn value() -> i64 { 7 }\n")]);
    let cache_root = temp_dir("e2e_no_range_cache");
    let program_dir = temp_dir("e2e_no_range_program");
    let fetched = pkg_cache::fetch_into_cache(&cache_root, "example.com/team/json", "v1.0.0", RefKind::TagOrBranch, repo.to_str().unwrap(), None).unwrap();
    fs::write(
        program_dir.join("paco.mod"),
        "module = \"example.com/myprogram\"\n\n[dependencies]\n\"example.com/team/json\" = \"v1.0.0\"\n",
    )
    .unwrap();
    manifest::Lockfile {
        packages: vec![manifest::LockedPackage { path: "example.com/team/json".to_string(), tag: Some("v1.0.0".to_string()), commit: fetched.commit, ..Default::default() }],
    }
    .write(&program_dir.join("paco.lock"))
    .unwrap();
    fs::write(program_dir.join("main.paco"), "use example.com/team/json;\n\nfn main() {\n    print(json::value())\n}\n").unwrap();

    let output = paco()
        .arg("run")
        .arg(program_dir.join("main.paco"))
        .env("PACO_PKG_CACHE", &cache_root)
        .env("PACO_VERSION_OVERRIDE", "0.0.1")
        .output()
        .unwrap();

    assert!(output.status.success(), "paco run failed: {}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(String::from_utf8_lossy(&output.stdout), "7\n");

    let _ = fs::remove_dir_all(&repo);
    let _ = fs::remove_dir_all(&cache_root);
    let _ = fs::remove_dir_all(&program_dir);
}

#[test]
fn each_module_in_a_multi_module_dependency_is_independently_importable() {
    let repo = throwaway_dependency_repo(
        "e2e_multi_repo",
        "v1.0.0",
        &[("a.paco", "pub fn value() -> i64 { 1 }\n"), ("b.paco", "pub fn value() -> i64 { 2 }\n")],
    );
    let cache_root = temp_dir("e2e_multi_cache");
    let program_dir = temp_dir("e2e_multi_program");

    let fetched = pkg_cache::fetch_into_cache(&cache_root, "example.com/team/toolkit", "v1.0.0", RefKind::TagOrBranch, repo.to_str().unwrap(), None).unwrap();
    fs::write(
        program_dir.join("paco.mod"),
        "module = \"example.com/myprogram\"\n\n[dependencies]\n\"example.com/team/toolkit\" = \"v1.0.0\"\n",
    )
    .unwrap();
    manifest::Lockfile {
        packages: vec![manifest::LockedPackage {
            path: "example.com/team/toolkit".to_string(),
            tag: Some("v1.0.0".to_string()),
            commit: fetched.commit,
            ..Default::default()
        }],
    }
    .write(&program_dir.join("paco.lock"))
    .unwrap();
    fs::write(
        program_dir.join("main.paco"),
        "use example.com/team/toolkit/a;\nuse example.com/team/toolkit/b;\n\nfn main() {\n    print(a::value() + b::value())\n}\n",
    )
    .unwrap();

    let output = paco().arg("run").arg(program_dir.join("main.paco")).env("PACO_PKG_CACHE", &cache_root).output().unwrap();

    assert!(output.status.success(), "paco run failed: {}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(String::from_utf8_lossy(&output.stdout), "3\n");

    let _ = fs::remove_dir_all(&repo);
    let _ = fs::remove_dir_all(&cache_root);
    let _ = fs::remove_dir_all(&program_dir);
}

// --- Extended dependency sources: local paths and untagged git refs
// (git-module-fetch task 8) ---
//
// paco.mod's own https:// URL construction (dependency key -> `https://
// <key>`, no separate url field) can't be exercised offline, same as task
// 4/6.2's own tests -- these populate the cache the same way a successful
// `paco get` would (pkg_cache::fetch_into_cache with an explicit local
// url) and then run the real `paco` binary to prove resolution/paco run
// works end to end for branch and rev dependencies too, not only tag.
// `paco get`'s own branch/rev resolution and reuse logic (choosing
// RefKind, matching an existing lock entry) is tested directly in
// pkg_cache.rs, the same boundary task 4.2's tests already use.

#[test]
fn a_path_dependency_produces_no_paco_lock_entry() {
    let program_dir = temp_dir("path_no_lock");
    let local_dir = program_dir.join("local");
    fs::create_dir_all(local_dir.join("src")).unwrap();
    fs::write(local_dir.join("src").join("local.paco"), "pub fn value() -> i64 { 1 }\n").unwrap();
    fs::write(
        program_dir.join("paco.mod"),
        "module = \"example.com/myprogram\"\n\n[dependencies]\n\"example.com/team/local\" = { path = \"local\" }\n",
    )
    .unwrap();

    let cli = Cli::try_parse_from(["paco", "get", program_dir.join("paco.mod").to_str().unwrap()]).unwrap();
    run(cli).unwrap_or_else(|error| panic!("`paco get` failed: {error}"));

    let lock = fs::read_to_string(program_dir.join("paco.lock")).unwrap();
    assert!(lock.is_empty(), "a path dependency must produce no paco.lock entry: {lock}");

    let _ = fs::remove_dir_all(&program_dir);
}

#[test]
fn a_path_dependency_resolves_and_runs_with_no_paco_get_and_no_cache() {
    let program_dir = temp_dir("path_e2e");
    let local_dir = program_dir.join("local");
    fs::create_dir_all(local_dir.join("src")).unwrap();
    fs::write(local_dir.join("src").join("local.paco"), "pub fn value() -> i64 { 9 }\n").unwrap();
    fs::write(
        program_dir.join("paco.mod"),
        "module = \"example.com/myprogram\"\n\n[dependencies]\n\"example.com/team/local\" = { path = \"local\" }\n",
    )
    .unwrap();
    fs::write(program_dir.join("main.paco"), "use example.com/team/local;\n\nfn main() {\n    print(local::value())\n}\n").unwrap();

    // No `paco get` run at all, and PACO_PKG_CACHE points at a directory
    // that does not exist -- a path dependency must never touch it.
    let unused_cache_dir = program_dir.join("this-directory-does-not-exist");
    let output = paco().arg("run").arg(program_dir.join("main.paco")).env("PACO_PKG_CACHE", &unused_cache_dir).output().unwrap();

    assert!(output.status.success(), "paco run failed: {}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(String::from_utf8_lossy(&output.stdout), "9\n");
    assert!(!unused_cache_dir.exists());
    assert!(!program_dir.join("paco.lock").exists());

    let _ = fs::remove_dir_all(&program_dir);
}

#[test]
fn a_branch_dependencys_item_is_usable_after_use() {
    let repo = throwaway_dependency_repo("branch_e2e_repo", "v1.0.0", &[("dev.paco", "pub fn value() -> i64 { 5 }\n")]);
    git(&repo, &["branch", "dev"]);
    let cache_root = temp_dir("branch_e2e_cache");
    let program_dir = temp_dir("branch_e2e_program");

    let fetched = pkg_cache::fetch_into_cache(&cache_root, "example.com/team/dev", "dev", RefKind::TagOrBranch, repo.to_str().unwrap(), None).unwrap();
    fs::write(
        program_dir.join("paco.mod"),
        "module = \"example.com/myprogram\"\n\n[dependencies]\n\"example.com/team/dev\" = { branch = \"dev\" }\n",
    )
    .unwrap();
    manifest::Lockfile {
        packages: vec![manifest::LockedPackage {
            path: "example.com/team/dev".to_string(),
            branch: Some("dev".to_string()),
            commit: fetched.commit,
            ..Default::default()
        }],
    }
    .write(&program_dir.join("paco.lock"))
    .unwrap();
    fs::write(program_dir.join("main.paco"), "use example.com/team/dev;\n\nfn main() {\n    print(dev::value())\n}\n").unwrap();

    let output = paco().arg("run").arg(program_dir.join("main.paco")).env("PACO_PKG_CACHE", &cache_root).output().unwrap();

    assert!(output.status.success(), "paco run failed: {}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(String::from_utf8_lossy(&output.stdout), "5\n");

    let _ = fs::remove_dir_all(&repo);
    let _ = fs::remove_dir_all(&cache_root);
    let _ = fs::remove_dir_all(&program_dir);
}

#[test]
fn a_rev_dependencys_item_is_usable_after_use() {
    let repo = throwaway_dependency_repo("rev_e2e_repo", "v1.0.0", &[("pinned.paco", "pub fn value() -> i64 { 6 }\n")]);
    let commit = git::resolve_ref(&repo, "HEAD").unwrap();
    let cache_root = temp_dir("rev_e2e_cache");
    let program_dir = temp_dir("rev_e2e_program");

    let fetched = pkg_cache::fetch_into_cache(&cache_root, "example.com/team/pinned", &commit, RefKind::Commit, repo.to_str().unwrap(), None).unwrap();
    assert_eq!(fetched.commit, commit);
    fs::write(
        program_dir.join("paco.mod"),
        format!("module = \"example.com/myprogram\"\n\n[dependencies]\n\"example.com/team/pinned\" = {{ rev = \"{commit}\" }}\n"),
    )
    .unwrap();
    manifest::Lockfile {
        packages: vec![manifest::LockedPackage {
            path: "example.com/team/pinned".to_string(),
            rev: Some(commit.clone()),
            commit,
            ..Default::default()
        }],
    }
    .write(&program_dir.join("paco.lock"))
    .unwrap();
    fs::write(program_dir.join("main.paco"), "use example.com/team/pinned;\n\nfn main() {\n    print(pinned::value())\n}\n").unwrap();

    let output = paco().arg("run").arg(program_dir.join("main.paco")).env("PACO_PKG_CACHE", &cache_root).output().unwrap();

    assert!(output.status.success(), "paco run failed: {}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(String::from_utf8_lossy(&output.stdout), "6\n");

    let _ = fs::remove_dir_all(&repo);
    let _ = fs::remove_dir_all(&cache_root);
    let _ = fs::remove_dir_all(&program_dir);
}

#[test]
fn mod_tidy_recognizes_every_dependency_kind_as_declared_and_satisfied() {
    let program_dir = temp_dir("tidy_all_kinds");
    fs::create_dir_all(program_dir.join("local").join("src")).unwrap();
    fs::write(
        program_dir.join("paco.mod"),
        "module = \"example.com/myprogram\"\n\n\
         [dependencies]\n\
         \"example.com/team/json\" = \"v1.2.0\"\n\
         \"example.com/team/dev\" = { branch = \"main\" }\n\
         \"example.com/team/pinned\" = { rev = \"a1b2c3d\" }\n\
         \"example.com/team/local\" = { path = \"local\" }\n",
    )
    .unwrap();
    fs::write(
        program_dir.join("main.paco"),
        "use example.com/team/json;\n\
         use example.com/team/dev;\n\
         use example.com/team/pinned;\n\
         use example.com/team/local;\n\n\
         fn main() {}\n",
    )
    .unwrap();

    let cli = Cli::try_parse_from(["paco", "mod", "tidy", program_dir.join("paco.mod").to_str().unwrap()]).unwrap();
    let output = run(cli).unwrap_or_else(|error| panic!("`paco mod tidy` failed: {error}"));

    assert!(output.stdout.is_empty(), "{}", output.stdout);

    let _ = fs::remove_dir_all(&program_dir);
}

#[test]
fn a_tag_only_manifest_and_lockfile_round_trip_unchanged() {
    // git-module-fetch task 8.6: the pre-task-8 shape (a bare-string tag,
    // no branch/rev/path table) is completely unaffected.
    let manifest_source = "module = \"example.com/myproject\"\n\n[dependencies]\n\"example.com/team/json\" = \"v1.2.0\"\n";
    let manifest = manifest::Manifest::parse(manifest_source).unwrap();
    assert_eq!(manifest.dependencies, vec![("example.com/team/json".to_string(), manifest::DependencySource::Tag("v1.2.0".to_string()))]);

    let lockfile = manifest::Lockfile {
        packages: vec![manifest::LockedPackage {
            path: "example.com/team/json".to_string(),
            tag: Some("v1.2.0".to_string()),
            commit: "a1b2c3d4e5f6".to_string(),
            ..Default::default()
        }],
    };
    let lock_toml = lockfile.to_toml();
    assert!(!lock_toml.contains("branch"), "{lock_toml}");
    assert!(!lock_toml.contains("rev ="), "{lock_toml}");
    assert_eq!(manifest::Lockfile::parse(&lock_toml).unwrap(), lockfile);
}

// --- Backward compatibility (git-module-fetch 7.2) ---

/// proposal.md's own compatibility claim: "a program with no `paco.mod` or
/// no domain-shaped `use` paths is entirely unaffected". This program has
/// neither -- a multi-file, plain-`use` program (`cross-file-modules`' own
/// entry-file-relative resolution) with no `paco.mod` anywhere near it --
/// and must build and run exactly as it would have before this change,
/// including `PACO_PKG_CACHE` pointed at a directory that does not even
/// exist: nothing about it should ever be consulted.
#[test]
fn a_program_with_no_paco_mod_and_no_domain_shaped_use_paths_is_entirely_unaffected() {
    let dir = temp_dir("no_impact");
    fs::write(dir.join("a.paco"), "pub fn double(x: i64) -> i64 { x * 2 }\n").unwrap();
    fs::write(dir.join("b.paco"), "use a;\n\nfn main() {\n    print(a::double(21))\n}\n").unwrap();
    let unused_cache_dir = dir.join("this-directory-does-not-exist").join("pkg-cache");

    let output = paco().arg("run").arg(dir.join("b.paco")).env("PACO_PKG_CACHE", &unused_cache_dir).output().unwrap();

    assert!(output.status.success(), "paco run failed: {}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(String::from_utf8_lossy(&output.stdout), "42\n");
    assert!(!unused_cache_dir.exists(), "an unaffected program must never touch the package cache");
    assert!(!dir.join("paco.lock").exists(), "an unaffected program must never write a lock file");

    let _ = fs::remove_dir_all(&dir);
}

// --- `extract-domain-libraries` task 4.1: the `stdlib::numerics`/`stdlib::math`/
// `stdlib::blas` alias table, its deprecation warning and its missing-
// dependency error ---
//
// A `{ path = ... }` dependency (`git-module-fetch` task 8.4) needs no
// cache or lock file at all, so these use it as the throwaway stand-in for
// each module's own library (`github.com/pacolang/tensor`,
// `.../math`, `.../blas`) instead of a git fixture -- simpler, and still
// the real `Domain`-kind resolution path `resolve_domain_use_path`
// exercises for a `path` dependency in `domain_resolution_tests`.

/// A minimal `Tensor` (rank via a repeated `const D: int...` dimension
/// list, `zeros`/`len`/`checked_add`) standing in for
/// `github.com/pacolang/tensor`'s real `src/tensor.paco` -- enough to
/// build and run `numerics::Tensor<f32, 2, 2>::zeros()` and
/// `a.checked_add(&b)`.
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
}
"#;

fn write_fake_library_path_dependency(program_dir: &std::path::Path, dependency_key: &str, module_source: &str, module_file_name: &str) {
    fs::create_dir_all(program_dir.join("library_lib/src")).unwrap();
    fs::write(program_dir.join("library_lib/src").join(module_file_name), module_source).unwrap();
    fs::write(
        program_dir.join("paco.mod"),
        format!("module = \"example.com/myprogram\"\n\n[dependencies]\n\"{dependency_key}\" = {{ path = \"library_lib\" }}\n"),
    )
    .unwrap();
}

#[test]
fn old_numerics_import_with_the_dependency_declared_builds_runs_and_warns() {
    let program_dir = temp_dir("moved_declared");
    write_fake_library_path_dependency(&program_dir, "github.com/pacolang/tensor", FAKE_TENSOR_MODULE, "tensor.paco");
    fs::write(
        program_dir.join("main.paco"),
        "use stdlib::numerics;\n\nfn main() {\n    let a = numerics::Tensor<f32, 2, 2>::zeros();\n    let b = numerics::Tensor<f32, 2, 2>::zeros();\n    match a.checked_add(&b) {\n        Result::Ok(c) => print(c.len()),\n        Result::Err(e) => print(e.axis),\n    }\n}\n",
    )
    .unwrap();

    let output = paco().arg("run").arg(program_dir.join("main.paco")).output().unwrap();

    assert!(output.status.success(), "paco run failed: {}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(String::from_utf8_lossy(&output.stdout), "4\n");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("use github.com/pacolang/tensor;"), "{stderr}");
    assert!(stderr.contains("tensor::"), "{stderr}");
    assert!(stderr.contains("deprecated"), "{stderr}");

    let _ = fs::remove_dir_all(&program_dir);
}

#[test]
fn old_blas_import_without_the_dependency_declared_is_a_build_error() {
    let program_dir = temp_dir("moved_undeclared");
    fs::write(program_dir.join("paco.mod"), "module = \"example.com/myprogram\"\n").unwrap();
    fs::write(program_dir.join("main.paco"), "use stdlib::blas;\n\nfn main() {}\n").unwrap();

    let output = paco().arg("run").arg(program_dir.join("main.paco")).output().unwrap();

    assert!(!output.status.success(), "expected `paco run` to fail with the dependency undeclared");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("use github.com/pacolang/blas;"), "{stderr}");
    assert!(stderr.contains("paco get github.com/pacolang/blas"), "{stderr}");

    let _ = fs::remove_dir_all(&program_dir);
}

#[test]
fn unmoved_std_modules_are_unaffected_by_the_alias_table() {
    let program_dir = temp_dir("unmoved_unaffected");
    // No `paco.mod` at all -- `stdlib::string` must resolve exactly as before.
    fs::write(program_dir.join("main.paco"), "use stdlib::string;\n\nfn main() {\n    print(\"abc\".as_bytes().len())\n}\n").unwrap();

    let output = paco().arg("run").arg(program_dir.join("main.paco")).output().unwrap();

    assert!(output.status.success(), "paco run failed: {}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(String::from_utf8_lossy(&output.stdout), "3\n");
    assert!(output.stderr.is_empty(), "{}", String::from_utf8_lossy(&output.stderr));

    let _ = fs::remove_dir_all(&program_dir);
}

// --- `extract-domain-libraries` task 4.4: the release scenario ---
//
// design.md's migration step 3 keeps `stdlib/numerics.paco`, `stdlib/math.paco`
// and `stdlib/blas.paco` physically present in this checkout for exactly this
// release: tasks.md task 4.4 itself says to "verify that a program with
// the old import and no manifest still builds and warns". The spec
// delta's "Old import without the dependency" scenario ("a program writes
// `use stdlib::blas;` and its paco.mod does not declare the library") is
// phrased in terms of a `paco.mod` that exists but lacks the entry
// (`old_blas_import_without_the_dependency_declared_is_a_build_error`,
// above) — a distinct case from no manifest at all. Reading the two as
// unconditional (any missing declaration, manifest-less or not, is an
// error) was tried first and reverted: it broke every not-yet-migrated
// caller of `stdlib::numerics`/`stdlib::math`/`stdlib::blas` with no `paco.mod` at
// all, including a dozen of this compiler's own pre-existing tests and
// `stdlib/math.paco`/`stdlib/blas.paco`'s own internal composition — exactly
// the "no manifest" case task 4.4 explicitly says must still build. This
// proves the resolution: an entry file with no `paco.mod` next to it at
// all still builds via the old `stdlib` file, with the deprecation warning.

#[test]
fn old_numerics_import_with_no_manifest_at_all_still_builds_and_warns() {
    let program_dir = temp_dir("release_scenario_no_manifest");
    fs::write(
        program_dir.join("main.paco"),
        "use stdlib::numerics;\n\nfn main() {\n    let t = numerics::Tensor<f32, 2, 2>::zeros();\n    print(t.len())\n}\n",
    )
    .unwrap();
    assert!(!program_dir.join("paco.mod").exists());

    let output = paco().arg("run").arg(program_dir.join("main.paco")).output().unwrap();

    assert!(output.status.success(), "paco run failed: {}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(String::from_utf8_lossy(&output.stdout), "4\n");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("use github.com/pacolang/tensor;"), "{stderr}");
    assert!(stderr.contains("deprecated"), "{stderr}");

    let _ = fs::remove_dir_all(&program_dir);
}
