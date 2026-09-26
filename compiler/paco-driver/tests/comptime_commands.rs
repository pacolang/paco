use clap::Parser;
use paco_driver::{Cli, run};

#[test]
fn check_evaluates_comptime_blocks_and_reports_their_failures() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("main.paco");
    std::fs::write(&file, "fn main() {\n    print(comptime {\n        let big: i64 = 9223372036854775807;\n        big + 1\n    });\n}\n").unwrap();
    let error = run(Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap()).unwrap_err();
    assert!(error.contains("main.paco:4:9: comptime evaluation failed: attempt to add with overflow"), "{error}");

    std::fs::write(&file, "fn fib(n: i64) -> i64 {\n    if n < 2 { n } else { fib(n - 1) + fib(n - 2) }\n}\n\nconst N: i64 = comptime { fib(20) };\n\nfn main() {\n    print(N);\n}\n").unwrap();
    let checked = run(Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap()).unwrap();
    assert_eq!(checked.stderr, "");
}
