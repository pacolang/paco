use std::fs;
use std::path::Path;
use std::process::{Command, Output};

fn paco(cache: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_paco"));
    command.args(args).env("PACO_CACHE", cache);
    command
}

/// The entries in `dir`, not counting the `.dwarf` line tables macOS keeps
/// beside each cached binary (checked to match the binaries one to one).
fn entries(cache: &Path, dir: &str) -> usize {
    let paths: Vec<_> = fs::read_dir(cache.join(dir)).map_or(Vec::new(), |entries| entries.map(|entry| entry.unwrap().path()).collect());
    let (dwarf, rest): (Vec<_>, Vec<_>) = paths.into_iter().partition(|path| path.extension().is_some_and(|ext| ext == "dwarf"));
    if dir == "bin" {
        assert_eq!(dwarf.len(), if cfg!(target_os = "macos") { rest.len() } else { 0 }, "{rest:?} {dwarf:?}");
    }
    rest.len()
}

const ARGS_PROGRAM: &str = "use stdlib::env;\nuse stdlib::io;\n\nfn main() -> i64 {\n    let args = env::args();\n    let mut i = 1;\n    while i < args.len() {\n        match args.get(i) {\n            Some(arg) => print(arg),\n            None => {}\n        }\n        i = i + 1;\n    }\n    io::print_err(\"done\");\n    3\n}\n";

fn project(source: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("main.paco");
    fs::write(&file, source).unwrap();
    (dir, file)
}

#[test]
fn run_and_build_produce_the_same_output() {
    let (dir, file) = project(ARGS_PROGRAM);
    let cache = dir.path().join("cache");
    let run = paco(&cache, &["run", file.to_str().unwrap(), "--", "x", "y z"]).output().unwrap();
    let build = paco(&cache, &["build", file.to_str().unwrap()]).output().unwrap();
    assert!(build.status.success(), "{}", String::from_utf8_lossy(&build.stderr));
    let binary: Output = Command::new(file.with_extension(std::env::consts::EXE_EXTENSION)).args(["x", "y z"]).output().unwrap();
    assert_eq!(run.stdout, binary.stdout);
    assert_eq!(run.stderr, binary.stderr);
    assert_eq!(run.status.code(), binary.status.code());
}

#[test]
fn exit_status_and_arguments_are_forwarded() {
    let (dir, file) = project(ARGS_PROGRAM);
    let cache = dir.path().join("cache");
    let run = paco(&cache, &["run", file.to_str().unwrap(), "--", "a", "b"]).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&run.stdout), "a\nb\n");
    assert_eq!(String::from_utf8_lossy(&run.stderr), "done\n");
    assert_eq!(run.status.code(), Some(3));
}

#[test]
fn a_program_that_fails_checking_is_not_executed() {
    let (dir, file) = project("fn main() {\n    print(\"before\");\n    let x: i64 = \"text\";\n}\n");
    let cache = dir.path().join("cache");
    let run = paco(&cache, &["run", file.to_str().unwrap()]).output().unwrap();
    assert!(!run.status.success());
    assert!(String::from_utf8_lossy(&run.stderr).contains("PACO-E0302"), "{}", String::from_utf8_lossy(&run.stderr));
    assert!(run.stdout.is_empty());
    assert_eq!(entries(&cache, "bin"), 0);
    assert_eq!(entries(&cache, "index"), 0);
}

#[test]
fn paco_cache_selects_the_cache_directory() {
    let (dir, file) = project("fn main() {\n    print(1);\n}\n");
    let cache = dir.path().join("elsewhere");
    let run = paco(&cache, &["run", file.to_str().unwrap()]).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&run.stdout), "1\n");
    assert_eq!(entries(&cache, "bin"), 1);
    assert_eq!(entries(&cache, "index"), 1);
    assert!(!file.with_extension(std::env::consts::EXE_EXTENSION).exists(), "`paco run` writes nothing next to the source");
}

#[test]
fn concurrent_runs_of_the_same_program_share_one_complete_entry() {
    let (dir, file) = project("fn main() {\n    print(\"concurrent\");\n}\n");
    let cache = dir.path().join("cache");
    let children: Vec<_> = (0..8)
        .map(|_| paco(&cache, &["run", file.to_str().unwrap()]).stdout(std::process::Stdio::piped()).spawn().unwrap())
        .collect();
    for child in children {
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stdout), "concurrent\n");
    }
    assert_eq!(entries(&cache, "index"), 1);
    assert_eq!(entries(&cache, "bin"), 1);
    assert_eq!(entries(&cache, "tmp"), 0, "no temporary files are left");
    let again = paco(&cache, &["run", file.to_str().unwrap()]).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&again.stdout), "concurrent\n");
}
