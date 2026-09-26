use std::path::Path;
use std::process::Command;
use std::time::Instant;

fn median_seconds(binary: &Path) -> f64 {
    let mut times: Vec<f64> = (0..5)
        .map(|_| {
            let start = Instant::now();
            let output = Command::new(binary).output().unwrap();
            assert!(output.status.success());
            start.elapsed().as_secs_f64()
        })
        .collect();
    times.sort_by(f64::total_cmp);
    times[2]
}

/// `cargo test -p paco-driver --release --test bench_alloc -- --ignored --nocapture`
#[test]
#[ignore]
fn alloc_churn_static_mimalloc_dynamic_mimalloc_and_glibc_malloc() {
    let dir = tempfile::tempdir().unwrap();
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/bench/alloc_churn.paco");
    let mut medians = Vec::new();
    for (name, flags, system_alloc) in [
        ("static-mimalloc", &["--release"][..], false),
        ("dynamic-mimalloc", &["--release", "--link", "dynamic"][..], false),
        ("dynamic-glibc-malloc", &["--release", "--link", "dynamic"][..], true),
    ] {
        let input = dir.path().join(format!("{name}.paco"));
        std::fs::copy(&source, &input).unwrap();
        let mut build = Command::new(env!("CARGO_BIN_EXE_paco"));
        build.arg("build").args(flags).arg(&input);
        if system_alloc {
            build.env("PACO_SYSTEM_ALLOC", "1");
        }
        let output = build.output().unwrap();
        assert!(output.status.success(), "{name}: {}", String::from_utf8_lossy(&output.stderr));
        let median = median_seconds(&input.with_extension(std::env::consts::EXE_EXTENSION));
        println!("{name}: {median:.3}s");
        medians.push(median);
    }
    assert!(medians[0] <= medians[2] * 1.10, "static mimalloc is more than 10% slower than glibc malloc: {medians:?}");
}
