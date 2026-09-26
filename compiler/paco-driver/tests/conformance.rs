use std::fs;
use std::path::Path;
use std::process::Command;
use paco_driver::{run, BackendChoice, Cli, Commands, LinkChoice};
use paco_test_harness::{discover_golden_tests, GoldenStatus, TestKind};

/// Every `paco build` leg of the differential check (`paco run` is one
/// more): the dev and release profiles through both backends, each linked
/// statically (musl) and dynamically (glibc).
/// How a paco without the `llvm` feature rejects code only LLVM compiles.
pub const LLVM_MISSING: &str = "the LLVM backend (`paco build --release`) is not built in";

/// How `paco-link` reports a missing `extern` library: on hosts with no
/// vendor-provided BLAS (Windows has none; macOS gets it from the SDK,
/// Linux from `libblas-dev`), `stdlib::blas` programs hit this.
pub const LIBRARY_MISSING: &str = "PACO-E0803";

const BUILD_LEGS: [(&str, bool, BackendChoice, LinkChoice); 8] = [
    ("cranelift-debug-static", false, BackendChoice::Cranelift, LinkChoice::Static),
    ("llvm-release-static", true, BackendChoice::Llvm, LinkChoice::Static),
    ("cranelift-release-static", true, BackendChoice::Cranelift, LinkChoice::Static),
    ("llvm-debug-static", false, BackendChoice::Llvm, LinkChoice::Static),
    ("cranelift-debug-dynamic", false, BackendChoice::Cranelift, LinkChoice::Dynamic),
    ("llvm-release-dynamic", true, BackendChoice::Llvm, LinkChoice::Dynamic),
    ("cranelift-release-dynamic", true, BackendChoice::Cranelift, LinkChoice::Dynamic),
    ("llvm-debug-dynamic", false, BackendChoice::Llvm, LinkChoice::Dynamic),
];

/// Every leg, or with `PACO_CONFORMANCE_LEGS=representative` only the
/// Cranelift debug and LLVM release builds; LLVM legs only when paco has
/// the LLVM backend.
fn build_legs() -> Vec<(&'static str, bool, BackendChoice, LinkChoice)> {
    let legs: &[_] = if std::env::var("PACO_CONFORMANCE_LEGS").as_deref() == Ok("representative") { &BUILD_LEGS[..2] } else { &BUILD_LEGS };
    legs.iter().copied().filter(|&(_, _, backend, _)| crate::llvm_backend() || backend != BackendChoice::Llvm).collect()
}

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

    let tests = discover_golden_tests(&conformance_dir, 2)
        .expect("failed to discover conformance tests");

    let only = std::env::var("PACO_CONFORMANCE_FILTER").ok();
    for test in tests {
        if only.as_deref().is_some_and(|only| !test.path.display().to_string().contains(only)) {
            continue;
        }
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
                format: None,
                args: Vec::new(),
            },
        };

        let expected_content = fs::read_to_string(&test.expected)
            .unwrap_or_else(|e| panic!("failed to read expected file {}: {}", test.expected.display(), e));

        let expected_normalized = expected_content.replace("\r\n", "\n");

        match test.kind {
            TestKind::Run => {
                let (output, status) = match paco_driver::run_program(&test.input, &[]) {
                    Err(err) if !crate::llvm_backend() && err.contains(LLVM_MISSING) => {
                        println!("Skipping test: {} (needs the LLVM backend)", test.path.display());
                        continue;
                    }
                    Err(err) if err.contains(LIBRARY_MISSING) => {
                        println!("Skipping test: {} (a linked library is missing on this host: {err})", test.path.display());
                        continue;
                    }
                    result => result.unwrap_or_else(|err| panic!("`paco run` failed to build {}: {err}", test.path.display())),
                };
                let expected = test.expected_run(false);
                let place = test.path.display();
                let strip = |text: &str| paco_test_harness::strip_dir(&text.replace("\r\n", "\n"), &test.path);
                assert_eq!(strip(&output.stdout), expected.stdout, "`paco run` stdout mismatch for test at {place}");
                if let Some(stderr) = &expected.stderr {
                    assert_eq!(&strip(&output.stderr), stderr, "`paco run` stderr mismatch for test at {place}");
                }
                assert_eq!(status, Some(expected.exit), "`paco run` exit status for test at {place}");
                if test.build {
                    std::thread::scope(|scope| {
                        let handles: Vec<_> = build_legs()
                            .iter()
                            .map(|&(leg, release, backend, link)| {
                                let path = &test.path;
                                scope.spawn(move || (leg, release, build_and_run(path, leg, release, backend, link, None)))
                            })
                            .collect();
                        for handle in handles {
                            let (leg, release, output) = handle.join().expect("build leg panicked");
                            let Some(output) = output else { continue };
                            let expected = test.expected_run(release);
                            let place = test.path.display();
                            assert_eq!(output.stdout, expected.stdout, "`{leg}` stdout mismatch for test at {place}");
                            if let Some(stderr) = expected.stderr {
                                assert_eq!(output.stderr, stderr, "`{leg}` stderr mismatch for test at {place}");
                            }
                            assert_eq!(output.status, Some(expected.exit), "`{leg}` exit status for test at {place}");
                        }
                    });
                }
                if test.comptime_differential {
                    let variant = comptime_variant(&fs::read_to_string(&test.input).unwrap());
                    for (leg, release, backend) in
                        [("comptime-cranelift-debug", false, BackendChoice::Cranelift), ("comptime-llvm-release", true, BackendChoice::Llvm)]
                            .into_iter()
                            .filter(|&(_, _, backend)| crate::llvm_backend() || backend != BackendChoice::Llvm)
                    {
                        let output = build_and_run(&test.path, leg, release, backend, LinkChoice::Static, Some(&variant))
                            .expect("comptime cases link statically");
                        assert_eq!(
                            output.stdout,
                            test.expected_run(release).stdout,
                            "`{leg}` (every print evaluated at compile time) disagrees for test at {}",
                            test.path.display()
                        );
                    }
                }
            }
            TestKind::Fail => {
                match run(cli) {
                    Ok(output) => {
                        panic!(
                            "Test {} failed: expected failure, but successfully ran with stdout: {}",
                            test.path.display(),
                            output.stdout
                        );
                    }
                    Err(err) => {
                        assert_eq!(
                            normalize_error(&err, &test.path),
                            expected_normalized,
                            "stderr mismatch for test at {}",
                            test.path.display()
                        );
                    }
                }
                if test.build {
                    let build = Cli {
                        command: Commands::Build {
                            path: Some(test.input.clone()),
                            release: false,
                            format: None,
                            target: None,
                            backend: None,
                            link: None,
                            sysroot: None,
                        },
                    };
                    let err = run(build).expect_err("expected `paco build` to fail");
                    assert_eq!(
                        normalize_error(&err, &test.path),
                        expected_normalized,
                        "`paco build` stderr mismatch for test at {}",
                        test.path.display()
                    );
                }
            }
        }
    }
}

struct LegOutput {
    stdout: String,
    stderr: String,
    status: Option<i32>,
}

/// `None` for a static leg of a program with `extern` blocks, which only
/// links dynamically.
fn build_and_run(
    test_dir: &Path,
    leg: &str,
    release: bool,
    backend: BackendChoice,
    link: LinkChoice,
    source: Option<&str>,
) -> Option<LegOutput> {
    let work = std::env::temp_dir().join(format!(
        "paco-conformance-{}-{}-{leg}",
        std::process::id(),
        test_dir.file_name().unwrap().to_string_lossy()
    ));
    let _ = fs::remove_dir_all(&work);
    copy_dir(test_dir, &work);
    let input = work.join("input.paco");
    if let Some(source) = source {
        fs::write(&input, source).unwrap();
    }
    let build = Cli {
        command: Commands::Build {
            path: Some(input.clone()),
            release,
            format: None,
            target: None,
            backend: Some(backend),
            link: Some(link),
            sysroot: None,
        },
    };
    if let Err(err) = run(build) {
        let _ = fs::remove_dir_all(&work);
        if link == LinkChoice::Static && err.contains("PACO-E0804") {
            return None;
        }
        if err.contains(LIBRARY_MISSING) {
            eprintln!("skipped {} ({leg}): a linked library is missing on this host: {err}", test_dir.display());
            return None;
        }
        panic!("`paco build` ({leg}) failed for {}: {err}", test_dir.display());
    }
    let output = Command::new(input.with_extension(std::env::consts::EXE_EXTENSION)).output().expect("compiled binary should run");
    let _ = fs::remove_dir_all(&work);
    let text = |bytes: &[u8]| paco_test_harness::strip_dir(&String::from_utf8_lossy(bytes).replace("\r\n", "\n"), &work);
    Some(LegOutput { stdout: text(&output.stdout), stderr: text(&output.stderr), status: output.status.code() })
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn normalize_error(err: &str, test_dir: &Path) -> String {
    paco_test_harness::strip_dir(err, test_dir).replace("\r\n", "\n")
}

/// `source` with every `print(e)` statement of `main` rewritten to
/// `print(comptime { e })`.
fn comptime_variant(source: &str) -> String {
    use paco_syntax::ast::{Expr, Item, Stmt};
    let mut sources = paco_span::SourceMap::new();
    let file = sources.add_file("input.paco", source);
    let mut reporter = paco_diag::Reporter::new();
    let tokens = paco_syntax::lex::lex(sources.source(file).unwrap(), file, &mut reporter);
    let module = paco_syntax::parse::parse_module(&tokens, &mut reporter).expect("conformance input parses");
    let mut arguments = Vec::new();
    for item in &module.items {
        let Item::Fn(function) = item else { continue };
        if function.name != "main" {
            continue;
        }
        let statements = function.body.stmts.iter().filter_map(|statement| match statement {
            Stmt::Expr(expr) => Some(expr),
            _ => None,
        });
        for expr in statements.chain(function.body.tail.as_deref()) {
            if let Expr::Call { callee, args, .. } = expr
                && matches!(callee.as_ref(), Expr::Ident(name, _) if name == "print")
                && let [arg] = args.as_slice()
            {
                arguments.push(paco_syntax::parse::expr_span(arg));
            }
        }
    }
    let mut variant = source.to_string();
    for span in arguments.iter().rev() {
        variant.insert_str(span.end(), " }");
        variant.insert_str(span.start(), "comptime { ");
    }
    variant
}
