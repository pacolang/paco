use std::{
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

const RUNS: usize = 10;

fn paco() -> PathBuf {
    std::env::var_os("PACO_BIN").map_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_paco")), PathBuf::from)
}

fn median(mut samples: Vec<Duration>) -> Duration {
    samples.sort();
    samples[samples.len() / 2]
}

fn timed(command: &mut Command) -> Duration {
    let start = Instant::now();
    let output = command.output().expect("command should start");
    let elapsed = start.elapsed();
    assert!(output.status.success(), "{command:?} failed: {}", String::from_utf8_lossy(&output.stderr));
    elapsed
}

fn run_cmd(file: &Path, cache: &Path) -> Command {
    let mut command = Command::new(paco());
    command.arg("run").arg(file).env("PACO_CACHE", cache);
    command
}

pub struct Latency {
    pub hit: Duration,
    pub miss: Duration,
    pub build: Duration,
    pub binary: Duration,
}

pub fn measure() -> Latency {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("main.paco");
    std::fs::write(&file, "fn main() {\n    print(\"Hello, world!\");\n}\n").unwrap();
    let cache = dir.path().join("cache");

    let miss = median(
        (0..RUNS)
            .map(|_| {
                let _ = std::fs::remove_dir_all(&cache);
                timed(&mut run_cmd(&file, &cache))
            })
            .collect(),
    );
    timed(&mut run_cmd(&file, &cache));
    let hit = median((0..RUNS).map(|_| timed(&mut run_cmd(&file, &cache))).collect());
    let build = median((0..RUNS).map(|_| timed(Command::new(paco()).arg("build").arg(&file))).collect());
    let binary = median((0..RUNS).map(|_| timed(&mut Command::new(file.with_extension(std::env::consts::EXE_EXTENSION)))).collect());
    Latency { hit, miss, build, binary }
}

#[test]
#[ignore = "timing on the reference machine; run with --ignored and PACO_BIN=target/release/paco"]
fn run_latency() {
    let Latency { hit, miss, build, binary } = measure();
    println!("paco run (hit): {hit:?}\npaco run (miss): {miss:?}\npaco build: {build:?}\nbinary: {binary:?}");
    assert!(hit <= binary + Duration::from_millis(10), "a cache hit costs {hit:?} against {binary:?} for the binary alone");
    assert!(miss <= Duration::from_millis(150), "a hello-world miss takes {miss:?}");
}
