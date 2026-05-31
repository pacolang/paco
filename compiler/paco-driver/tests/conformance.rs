use std::fs;
use std::path::Path;
use paco_driver::{run, Cli, Commands};
use paco_test_harness::{discover_golden_tests, GoldenStatus, TestKind};

#[test]
fn conformance_tests() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let conformance_dir = manifest_dir
        .parent()
        .expect("failed to get parent of manifest dir")
        .parent()
        .expect("failed to get workspace root")
        .join("tests")
        .join("conformance");

    let tests = discover_golden_tests(&conformance_dir, 1)
        .expect("failed to discover conformance tests");

    for test in tests {
        if let GoldenStatus::Skipped { feature_min } = test.status {
            println!(
                "Skipping test: {} (requires feature level {})",
                test.path.display(),
                feature_min
            );
            continue;
        }

        println!("Running test: {}", test.path.display());
        let cli = Cli {
            command: Commands::Run {
                file: Some(test.input.clone()),
            },
        };

        let result = run(cli);
        let expected_content = fs::read_to_string(&test.expected)
            .unwrap_or_else(|e| panic!("failed to read expected file {}: {}", test.expected.display(), e));

        let expected_normalized = expected_content.replace("\r\n", "\n");

        match test.kind {
            TestKind::Run => {
                let output = result.unwrap_or_else(|err| {
                    panic!(
                        "Test {} failed: expected successful run, but got compiler/runtime error: {}",
                        test.path.display(),
                        err
                    )
                });
                let stdout_normalized = output.stdout.replace("\r\n", "\n");
                assert_eq!(
                    stdout_normalized,
                    expected_normalized,
                    "stdout mismatch for test at {}",
                    test.path.display()
                );
            }
            TestKind::Fail => {
                match result {
                    Ok(output) => {
                        panic!(
                            "Test {} failed: expected failure, but successfully ran with stdout: {}",
                            test.path.display(),
                            output.stdout
                        );
                    }
                    Err(err) => {
                        let input_path_str = test.input.display().to_string();
                        let err_normalized = err
                            .replace(&input_path_str, "input.paco")
                            .replace("\r\n", "\n");
                        assert_eq!(
                            err_normalized,
                            expected_normalized,
                            "stderr mismatch for test at {}",
                            test.path.display()
                        );
                    }
                }
            }
        }
    }
}
