//! The user-global git dependency cache (`git-module-fetch`): where a
//! fetched dependency's cloned repository lives on disk, and the
//! temp-dir-then-atomic-rename protocol that makes populating it safe under
//! concurrent `paco get` runs sharing the same cache (design.md's "Cache
//! location" and "Cache mutation" decisions).

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use crate::git;

/// The package cache root: `PACO_PKG_CACHE`, else `$HOME/.paco/pkg`
/// (design.md: mirrors Go's `$GOPATH/pkg/mod`). `PACO_PKG_CACHE` exists so
/// this proposal's own tests (and anyone else) can point at an isolated
/// temp directory instead of the real developer cache.
pub fn pkg_cache_root(env: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let set = |name: &str| env(name).filter(|value| !value.is_empty()).map(PathBuf::from);
    set("PACO_PKG_CACHE").or_else(|| set("HOME").map(|home| home.join(".paco").join("pkg")))
}

/// Where a dependency `dependency_path` (e.g. `"example.com/team/json"`)
/// pinned to `tag` lives once fetched: every segment but the last becomes a
/// real subdirectory, and the last has `@<tag>` appended (design.md:
/// `<domain>/<path>@<tag>/`).
pub fn cache_dir_for(root: &Path, dependency_path: &str, tag: &str) -> PathBuf {
    let mut segments: Vec<&str> = dependency_path.split('/').collect();
    let last = segments.pop().expect("dependency path is never empty");
    let mut dir = root.to_path_buf();
    for segment in segments {
        dir.push(segment);
    }
    dir.push(format!("{last}@{tag}"));
    dir
}

/// The git URL a domain-shaped dependency path names, per ADR 0005 (a
/// `paco.mod` dependency key is itself the repository's domain path, `go
/// get`-style — `example.com/team/json` fetches from
/// `https://example.com/team/json`).
pub fn dependency_url(dependency_path: &str) -> String {
    format!("https://{dependency_path}")
}

/// One dependency's fetch outcome: the exact commit its cache directory
/// ends up checked out at.
#[derive(Debug)]
pub struct Fetched {
    pub commit: String,
}

/// How to interpret `key` when populating a still-missing cache directory
/// (`git-module-fetch` task 8.3): a tag or branch name both use the same
/// shallow `--branch` clone `clone_at_tag` already does; an exact commit
/// (`rev`) needs a full clone plus `git checkout`, since a shallow
/// `--branch` clone does not accept a bare commit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RefKind {
    TagOrBranch,
    Commit,
}

fn unique_suffix() -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |elapsed| elapsed.as_nanos());
    let count = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("{}-{nanos}-{count}", std::process::id())
}

/// Fetches `dependency_path`@`tag` (cloned from `url` — `dependency_url
/// (dependency_path)` in production, a throwaway local repository in
/// tests) into `root`'s cache if it is not already present there; a
/// directory already present is reused with no network operation at all
/// (design.md: fetched, tag-keyed directories are immutable once
/// populated). `locked_commit` — `paco.lock`'s already-recorded commit for
/// this exact `(path, tag)`, if any — is what a fresh clone gets pinned to
/// instead of wherever the tag currently resolves to, so a tag force-moved
/// upstream after the lock file was written does not silently change an
/// existing project's build (design.md's Risks).
///
/// Populating a missing cache directory clones into a sibling temporary
/// directory and atomically renames it into place only if the target is
/// still absent, discarding the temporary directory (and reusing the
/// now-present target) if a concurrent `paco get` already won the race —
/// safe without a lock file, and a clone that never completes (a killed
/// process) leaves only the temporary directory behind, never a
/// partially-populated target.
pub fn fetch_into_cache(
    root: &Path,
    dependency_path: &str,
    key: &str,
    kind: RefKind,
    url: &str,
    locked_commit: Option<&str>,
) -> Result<Fetched, String> {
    let target = cache_dir_for(root, dependency_path, key);
    if target.is_dir() {
        let commit = match locked_commit {
            Some(commit) => commit.to_string(),
            None => git::resolve_ref(&target, "HEAD")?,
        };
        return Ok(Fetched { commit });
    }

    let parent = target.parent().expect("cache dir always has a parent");
    fs::create_dir_all(parent).map_err(|error| format!("failed to create `{}`: {error}", parent.display()))?;
    let leaf = target.file_name().expect("cache dir always has a name").to_string_lossy();
    let tmp = parent.join(format!("{leaf}.tmp-{}", unique_suffix()));

    let mut commit = match kind {
        RefKind::TagOrBranch => git::clone_at_tag(url, key, &tmp),
        RefKind::Commit => git::clone_and_checkout_commit(url, key, &tmp),
    }
    .map_err(|error| format!("could not fetch dependency `{dependency_path}` at `{key}`: {error}"))?;
    if let Some(locked_commit) = locked_commit
        && locked_commit != commit
    {
        git::fetch_and_checkout_commit(&tmp, locked_commit)
            .map_err(|error| format!("could not pin dependency `{dependency_path}` to its locked commit `{locked_commit}`: {error}"))?;
        commit = locked_commit.to_string();
    }

    match fs::rename(&tmp, &target) {
        Ok(()) => Ok(Fetched { commit }),
        Err(_) if target.is_dir() => {
            // A concurrent `paco get` already populated `target`; discard
            // our own clone and defer to theirs.
            let _ = fs::remove_dir_all(&tmp);
            let commit = match locked_commit {
                Some(commit) => commit.to_string(),
                None => git::resolve_ref(&target, "HEAD")?,
            };
            Ok(Fetched { commit })
        }
        Err(error) => Err(format!("failed to move `{}` into place at `{}`: {error}", tmp.display(), target.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn temp_dir(name: &str) -> PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!("paco_pkg_cache_{name}_{}_{}", std::process::id(), unique_suffix()));
        dir
    }

    fn git(dir: &Path, args: &[&str]) {
        let output = Command::new("git").arg("-C").arg(dir).args(args).output().expect("git should be installed for this test");
        assert!(output.status.success(), "git {args:?} failed: {}", String::from_utf8_lossy(&output.stderr));
    }

    fn throwaway_repo(name: &str, tag: &str) -> PathBuf {
        let dir = temp_dir(name);
        fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "--quiet"]);
        git(&dir, &["config", "user.email", "test@example.com"]);
        git(&dir, &["config", "user.name", "Test"]);
        fs::write(dir.join("hello.txt"), "hello\n").unwrap();
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "--quiet", "-m", "init"]);
        git(&dir, &["tag", tag]);
        dir
    }

    #[test]
    fn cache_dir_nests_every_segment_but_the_last_and_suffixes_the_tag() {
        let root = PathBuf::from("/cache");
        let dir = cache_dir_for(&root, "example.com/team/json", "v1.2.0");
        assert_eq!(dir, PathBuf::from("/cache/example.com/team/json@v1.2.0"));
    }

    const DEP_PATH: &str = "example.com/team/repo";

    #[test]
    fn first_fetch_clones_a_repository_not_yet_cached() {
        let repo = throwaway_repo("first", "v1.0.0");
        let root = temp_dir("root_first");
        let url = repo.to_str().unwrap();

        let fetched = fetch_into_cache(&root, DEP_PATH, "v1.0.0", RefKind::TagOrBranch, url, None).unwrap();

        let target = cache_dir_for(&root, DEP_PATH, "v1.0.0");
        assert!(target.is_dir());
        assert_eq!(fs::read_to_string(target.join("hello.txt")).unwrap(), "hello\n");
        assert_eq!(git::resolve_ref(&target, "HEAD").unwrap(), fetched.commit);

        let _ = fs::remove_dir_all(&repo);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_repeat_fetch_with_nothing_changed_does_not_touch_the_cache_directory() {
        let repo = throwaway_repo("repeat", "v1.0.0");
        let root = temp_dir("root_repeat");
        let url = repo.to_str().unwrap();
        fetch_into_cache(&root, DEP_PATH, "v1.0.0", RefKind::TagOrBranch, url, None).unwrap();
        let target = cache_dir_for(&root, DEP_PATH, "v1.0.0");
        let mtime_before = fs::metadata(target.join("hello.txt")).unwrap().modified().unwrap();

        let fetched = fetch_into_cache(&root, DEP_PATH, "v1.0.0", RefKind::TagOrBranch, url, None).unwrap();

        let mtime_after = fs::metadata(target.join("hello.txt")).unwrap().modified().unwrap();
        assert_eq!(mtime_before, mtime_after, "second fetch must not touch an already-cached directory");
        assert_eq!(git::resolve_ref(&target, "HEAD").unwrap(), fetched.commit);

        let _ = fs::remove_dir_all(&repo);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn two_concurrent_fetches_both_end_up_at_the_same_complete_directory_with_no_leftover_tmp() {
        let repo = throwaway_repo("concurrent", "v1.0.0");
        let root = temp_dir("root_concurrent");
        let url = repo.to_str().unwrap().to_string();

        let root_a = root.clone();
        let url_a = url.clone();
        let handle_a = std::thread::spawn(move || fetch_into_cache(&root_a, DEP_PATH, "v1.0.0", RefKind::TagOrBranch, &url_a, None));
        let root_b = root.clone();
        let url_b = url.clone();
        let handle_b = std::thread::spawn(move || fetch_into_cache(&root_b, DEP_PATH, "v1.0.0", RefKind::TagOrBranch, &url_b, None));

        let result_a = handle_a.join().unwrap().unwrap();
        let result_b = handle_b.join().unwrap().unwrap();
        assert_eq!(result_a.commit, result_b.commit);

        let target = cache_dir_for(&root, DEP_PATH, "v1.0.0");
        assert_eq!(fs::read_to_string(target.join("hello.txt")).unwrap(), "hello\n");

        let leftover_tmp: Vec<_> = fs::read_dir(target.parent().unwrap())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp-"))
            .collect();
        assert!(leftover_tmp.is_empty(), "expected no leftover .tmp-* directories, found {leftover_tmp:?}");

        let _ = fs::remove_dir_all(&repo);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_failed_clone_never_leaves_a_partially_populated_target_directory() {
        let repo = throwaway_repo("failed", "v1.0.0");
        let root = temp_dir("root_failed");
        let url = repo.to_str().unwrap();

        // "no-such-tag" was never created in the throwaway repo, so the
        // underlying `git clone --branch` fails.
        let error = fetch_into_cache(&root, DEP_PATH, "no-such-tag", RefKind::TagOrBranch, url, None).unwrap_err();
        assert!(!error.is_empty());

        let target = cache_dir_for(&root, DEP_PATH, "no-such-tag");
        assert!(!target.exists(), "a failed clone must never leave a partially-populated target directory");

        let _ = fs::remove_dir_all(&repo);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn an_unreachable_repository_is_a_clear_error_naming_the_dependency() {
        let root = temp_dir("root_unreachable");
        let error = fetch_into_cache(&root, DEP_PATH, "v1.0.0", RefKind::TagOrBranch, "/no/such/repository/at/all", None).unwrap_err();
        assert!(error.contains(DEP_PATH), "{error}");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_fresh_clone_with_an_existing_lock_entry_pins_to_the_locked_commit_not_a_moved_tag() {
        let repo = throwaway_repo("pin", "v1.0.0");
        let url = repo.to_str().unwrap();
        let first_commit = git::resolve_ref(&repo, "HEAD").unwrap();
        fs::write(repo.join("hello.txt"), "changed\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "--quiet", "-m", "second"]);
        git(&repo, &["tag", "-f", "v1.0.0"]);
        let root = temp_dir("root_pin");

        let fetched = fetch_into_cache(&root, DEP_PATH, "v1.0.0", RefKind::TagOrBranch, url, Some(&first_commit)).unwrap();

        assert_eq!(fetched.commit, first_commit);
        let target = cache_dir_for(&root, DEP_PATH, "v1.0.0");
        assert_eq!(fs::read_to_string(target.join("hello.txt")).unwrap(), "hello\n");

        let _ = fs::remove_dir_all(&repo);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_branch_dependency_locks_to_its_first_resolved_commit_and_does_not_silently_move_forward() {
        // git-module-fetch task 8.3(a): a `branch` dependency's cache key
        // is the branch name itself (RefKind::TagOrBranch, same as a
        // tag), so this exercises `run_get`'s exact code path without
        // going through paco.mod's own https:// URL construction (which
        // needs real network — task 4's tests already establish this
        // testing boundary).
        let repo = throwaway_repo("branch_source", "v1.0.0");
        let original_branch = String::from_utf8(Command::new("git").arg("-C").arg(&repo).args(["rev-parse", "--abbrev-ref", "HEAD"]).output().unwrap().stdout)
            .unwrap()
            .trim()
            .to_string();
        git(&repo, &["branch", "dev"]);
        let root = temp_dir("root_branch");
        let url = repo.to_str().unwrap();

        let first = fetch_into_cache(&root, DEP_PATH, "dev", RefKind::TagOrBranch, url, None).unwrap();

        // The branch moves forward after the first paco get...
        git(&repo, &["checkout", "--quiet", "dev"]);
        fs::write(repo.join("hello.txt"), "third\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "--quiet", "-m", "third on dev"]);
        git(&repo, &["checkout", "--quiet", &original_branch]);
        let target = cache_dir_for(&root, DEP_PATH, "dev");
        let mtime_before = fs::metadata(target.join("hello.txt")).unwrap().modified().unwrap();

        // ...but a second paco get, reusing the locked commit, does not
        // silently move forward with it.
        let second = fetch_into_cache(&root, DEP_PATH, "dev", RefKind::TagOrBranch, url, Some(&first.commit)).unwrap();

        assert_eq!(second.commit, first.commit);
        let mtime_after = fs::metadata(target.join("hello.txt")).unwrap().modified().unwrap();
        assert_eq!(mtime_before, mtime_after, "a locked branch dependency must not be re-fetched");
        assert_eq!(fs::read_to_string(target.join("hello.txt")).unwrap(), "hello\n");

        let _ = fs::remove_dir_all(&repo);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_commit_ref_kind_fetches_an_exact_commit_not_yet_at_any_tag_tip() {
        let repo = throwaway_repo("rev", "v1.0.0");
        let url = repo.to_str().unwrap();
        let first_commit = git::resolve_ref(&repo, "HEAD").unwrap();
        fs::write(repo.join("hello.txt"), "second\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "--quiet", "-m", "second"]);
        let root = temp_dir("root_rev");

        let fetched = fetch_into_cache(&root, DEP_PATH, &first_commit, RefKind::Commit, url, None).unwrap();

        assert_eq!(fetched.commit, first_commit);
        let target = cache_dir_for(&root, DEP_PATH, &first_commit);
        assert_eq!(fs::read_to_string(target.join("hello.txt")).unwrap(), "hello\n");

        let _ = fs::remove_dir_all(&repo);
        let _ = fs::remove_dir_all(&root);
    }
}
