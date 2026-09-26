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

fn build(input: &Path, flags: &[&str]) {
    let output = Command::new(env!("CARGO_BIN_EXE_paco")).arg("build").args(flags).arg(input).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
}

/// Tape size and gradient time against the primal: the scalar loop of
/// `tests/bench/autodiff_scalar.paco` and the projectile example.
/// `cargo test -p paco-driver --release --test main bench_autodiff -- --ignored --nocapture`
#[test]
#[ignore]
fn gradient_time_and_tape_size_against_the_primal() {
    let dir = tempfile::tempdir().unwrap();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let scalar = std::fs::read_to_string(root.join("tests/bench/autodiff_scalar.paco")).unwrap();
    let primal = scalar.replace("    let (v, d) = autodiff::grad(f, 0.5);", "    let v = simulate(0.5);\n    let d = 0.0;");
    let example = std::fs::read_to_string(root.join("tests/conformance/autodiff/projectile_fit/input.paco")).unwrap();
    for (name, source) in [("scalar-gradient", scalar), ("scalar-primal", primal), ("projectile-fit", example)] {
        for (backend, flags) in [("llvm-release", &["--release"][..]), ("cranelift-release", &["--release", "--backend", "cranelift"][..])] {
            let input = dir.path().join(format!("{name}-{backend}.paco"));
            std::fs::write(&input, &source).unwrap();
            build(&input, flags);
            let binary = input.with_extension(std::env::consts::EXE_EXTENSION);
            let median = median_seconds(&binary);
            let stats = Command::new(&binary).env("PACO_AD_STATS", "1").output().unwrap();
            let tape = String::from_utf8_lossy(&stats.stderr).lines().map(str::to_string).max_by_key(|line| line.len()).unwrap_or_default();
            println!("{name} {backend}: {:.1} ms; {tape}", median * 1000.0);
        }
    }
}
