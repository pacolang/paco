//! A thin wrapper around the system `git` binary (`git-module-fetch`),
//! shelling out via `std::process::Command` rather than binding a `git2`/
//! libgit2 crate — matching `paco-link`'s existing precedent of shelling out
//! to `cc`/the linker instead of binding a native library (design.md's own
//! "Git operations" decision).

use std::path::Path;
use std::process::Command;

/// A clear, named error if `git` is not on `PATH` (design.md's Risk
/// mitigation), checked before any other git operation runs.
pub fn ensure_available() -> Result<(), String> {
    match Command::new("git").arg("--version").output() {
        Ok(output) if output.status.success() => Ok(()),
        _ => Err("`git` was not found on PATH; install git to use `paco get`".to_string()),
    }
}

/// Clones `url` at `tag` into `dest` (which must not already exist) and
/// returns the resolved commit `dest`'s checkout is at.
pub fn clone_at_tag(url: &str, tag: &str, dest: &Path) -> Result<String, String> {
    ensure_available()?;
    run(Command::new("git").args(["clone", "--quiet", "--branch", tag, "--depth", "1"]).arg(url).arg(dest), "git clone")?;
    resolve_ref(dest, "HEAD")
}

/// Clones `url` fully (not the shallow `--branch` clone `clone_at_tag`
/// uses) into `dest` and checks it out at exactly `commit` — used for a
/// `rev` dependency (`git-module-fetch` task 8.3): a shallow `--branch`
/// clone does not accept a bare commit, only a tag or branch name.
pub fn clone_and_checkout_commit(url: &str, commit: &str, dest: &Path) -> Result<String, String> {
    ensure_available()?;
    run(Command::new("git").args(["clone", "--quiet"]).arg(url).arg(dest), "git clone")?;
    run(Command::new("git").arg("-C").arg(dest).args(["checkout", "--quiet", "--detach", commit]), "git checkout")?;
    resolve_ref(dest, "HEAD")
}

/// Fetches and checks out an exact commit in an already-cloned directory
/// (one with an `origin` remote, e.g. from `clone_at_tag`) — used to pin a
/// fresh clone to `paco.lock`'s exact recorded commit rather than whatever
/// commit a moved tag currently resolves to (design.md's Risks: "A tag can
/// be force-moved... `paco get` with an existing lock entry re-checks out
/// that exact commit, not the tag").
pub fn fetch_and_checkout_commit(repo_dir: &Path, commit: &str) -> Result<(), String> {
    ensure_available()?;
    run(Command::new("git").arg("-C").arg(repo_dir).args(["fetch", "--quiet", "--depth", "1", "origin", commit]), "git fetch")?;
    run(Command::new("git").arg("-C").arg(repo_dir).args(["checkout", "--quiet", "--detach", commit]), "git checkout")?;
    Ok(())
}

/// Resolves `reference` (a tag, branch or `HEAD`) to its commit hash inside
/// the git repository at `repo_dir`.
pub fn resolve_ref(repo_dir: &Path, reference: &str) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_dir)
        .args(["rev-parse", reference])
        .output()
        .map_err(|error| format!("failed to invoke `git`: {error}"))?;
    if !output.status.success() {
        return Err(format!("`git rev-parse {reference}` in `{}` failed: {}", repo_dir.display(), String::from_utf8_lossy(&output.stderr).trim()));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn run(command: &mut Command, name: &str) -> Result<(), String> {
    let output = command.output().map_err(|error| format!("failed to invoke `{name}`: {error}"))?;
    if !output.status.success() {
        return Err(format!("`{name}` failed: {}", String::from_utf8_lossy(&output.stderr).trim()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let mut dir = std::env::temp_dir();
        let suffix = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        dir.push(format!("paco_git_{name}_{}_{suffix}", std::process::id()));
        dir
    }

    fn git(dir: &Path, args: &[&str]) {
        let output = Command::new("git").arg("-C").arg(dir).args(args).output().expect("git should be installed for this test");
        assert!(output.status.success(), "git {args:?} failed: {}", String::from_utf8_lossy(&output.stderr));
    }

    /// A throwaway local repository with one commit and one tag, standing
    /// in for a remote one (no network needed). Returns the repo's
    /// directory and the tagged commit's hash.
    fn throwaway_repo(name: &str, tag: &str) -> (std::path::PathBuf, String) {
        let dir = temp_dir(name);
        fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "--quiet"]);
        git(&dir, &["config", "user.email", "test@example.com"]);
        git(&dir, &["config", "user.name", "Test"]);
        fs::write(dir.join("hello.txt"), "hello\n").unwrap();
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "--quiet", "-m", "init"]);
        git(&dir, &["tag", tag]);
        let commit = resolve_ref(&dir, "HEAD").unwrap();
        (dir, commit)
    }

    #[test]
    fn clone_at_tag_checks_out_matching_content_and_commit() {
        let (repo, expected_commit) = throwaway_repo("clone_source", "v1.0.0");
        let dest = temp_dir("clone_dest");

        let resolved_commit = clone_at_tag(repo.to_str().unwrap(), "v1.0.0", &dest).unwrap();

        assert_eq!(resolved_commit, expected_commit);
        assert_eq!(fs::read_to_string(dest.join("hello.txt")).unwrap(), "hello\n");
        let actual_head = resolve_ref(&dest, "HEAD").unwrap();
        assert_eq!(actual_head, expected_commit);

        let _ = fs::remove_dir_all(&repo);
        let _ = fs::remove_dir_all(&dest);
    }

    #[test]
    fn clone_at_tag_of_an_unreachable_repository_is_a_named_error() {
        let dest = temp_dir("clone_missing");
        let error = clone_at_tag("/no/such/repository/here", "v1.0.0", &dest).unwrap_err();
        assert!(error.contains("git clone"), "{error}");
        assert!(!dest.exists());
    }

    #[test]
    fn fetch_and_checkout_commit_pins_a_clone_to_an_exact_commit() {
        let (repo, first_commit) = throwaway_repo("pin_source", "v1.0.0");
        fs::write(repo.join("hello.txt"), "hello again\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "--quiet", "-m", "second"]);
        git(&repo, &["tag", "-f", "v1.0.0"]);
        let dest = temp_dir("pin_dest");

        // Cloning `v1.0.0` now resolves to the moved (second) commit...
        let moved_commit = clone_at_tag(repo.to_str().unwrap(), "v1.0.0", &dest).unwrap();
        assert_ne!(moved_commit, first_commit);

        // ...but fetch_and_checkout_commit can still pin back to the
        // original, lock-recorded commit.
        fetch_and_checkout_commit(&dest, &first_commit).unwrap();
        assert_eq!(resolve_ref(&dest, "HEAD").unwrap(), first_commit);
        assert_eq!(fs::read_to_string(dest.join("hello.txt")).unwrap(), "hello\n");

        let _ = fs::remove_dir_all(&repo);
        let _ = fs::remove_dir_all(&dest);
    }

    #[test]
    fn clone_and_checkout_commit_checks_out_an_exact_commit_not_a_tag() {
        let (repo, first_commit) = throwaway_repo("rev_source", "v1.0.0");
        fs::write(repo.join("hello.txt"), "second\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "--quiet", "-m", "second"]);
        let dest = temp_dir("rev_dest");

        let resolved_commit = clone_and_checkout_commit(repo.to_str().unwrap(), &first_commit, &dest).unwrap();

        assert_eq!(resolved_commit, first_commit);
        assert_eq!(fs::read_to_string(dest.join("hello.txt")).unwrap(), "hello\n");

        let _ = fs::remove_dir_all(&repo);
        let _ = fs::remove_dir_all(&dest);
    }

    #[test]
    fn clone_and_checkout_commit_of_an_unreachable_repository_is_a_named_error() {
        let dest = temp_dir("rev_missing");
        let error = clone_and_checkout_commit("/no/such/repository/here", "deadbeef", &dest).unwrap_err();
        assert!(error.contains("git clone"), "{error}");
        assert!(!dest.exists());
    }
}
