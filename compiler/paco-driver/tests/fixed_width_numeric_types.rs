use std::{fs, path::PathBuf};

use clap::Parser;
use paco_driver::{Cli, run};

macro_rules! run_numeric_type_test {
    ($test_name:ident, $ty:literal, $a:literal, $b:literal, $expected:literal) => {
        #[test]
        fn $test_name() {
            let source = format!(
                "fn main() {{ let a: {ty} = {a};\n let b: {ty} = {b};\n print(a + b) }}",
                ty = $ty,
                a = $a,
                b = $b,
            );
            let file = write_temp_paco(stringify!($test_name), &source);
            let cli = Cli::try_parse_from(["paco", "run", file.to_str().unwrap()]).unwrap();

            let output = run(cli).unwrap();

            assert_eq!(output.stdout, concat!($expected, "\n"));
        }
    };
}

run_numeric_type_test!(run_declares_and_prints_i8, "i8", "100", "27", "127");
run_numeric_type_test!(run_declares_and_prints_i16, "i16", "30000", "2767", "32767");
run_numeric_type_test!(run_declares_and_prints_i32, "i32", "2000000000", "147483647", "2147483647");
run_numeric_type_test!(
    run_declares_and_prints_i64,
    "i64",
    "4611686018427387903",
    "1",
    "4611686018427387904"
);
run_numeric_type_test!(run_declares_and_prints_u8, "u8", "200", "55", "255");
run_numeric_type_test!(run_declares_and_prints_u16, "u16", "60000", "5535", "65535");
run_numeric_type_test!(run_declares_and_prints_u32, "u32", "4000000000", "294967295", "4294967295");
run_numeric_type_test!(
    run_declares_and_prints_u64,
    "u64",
    "4000000000000000000",
    "1000000000000000000",
    "5000000000000000000"
);
run_numeric_type_test!(
    run_declares_and_prints_a_u64_sum_above_i64_max,
    "u64",
    "9000000000000000000",
    "9000000000000000000",
    "18000000000000000000"
);
run_numeric_type_test!(run_declares_and_prints_byte, "byte", "200", "55", "255");

#[test]
fn run_declares_and_prints_char() {
    let source = r#"
fn main() {
    let c: char = 'a';
    print(c == 'a');
    print(c == 'b')
}
"#;
    let file = write_temp_paco("run_declares_and_prints_char", source);
    let cli = Cli::try_parse_from(["paco", "run", file.to_str().unwrap()]).unwrap();

    let output = run(cli).unwrap();

    assert_eq!(output.stdout, "true\nfalse\n");
}

#[test]
fn run_narrowing_cast_truncates() {
    let source = r#"
fn main() {
    let x: i64 = 300;
    print(x as u8)
}
"#;
    let file = write_temp_paco("run_narrowing_cast_truncates", source);
    let cli = Cli::try_parse_from(["paco", "run", file.to_str().unwrap()]).unwrap();

    let output = run(cli).unwrap();

    assert_eq!(output.stdout, "44\n");
}

#[test]
fn check_rejects_int_with_dedicated_diagnostic() {
    let source = "fn main() { let x: int = 5;\n print(x) }";
    let file = write_temp_paco("check_rejects_int", source);
    let cli = Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("PACO-E0328"));
    assert!(error.contains("`int` is not a Paco type; use `i64`"));
}

#[test]
fn check_rejects_uint_with_dedicated_diagnostic() {
    let source = "fn main() { let x: uint = 5;\n print(x) }";
    let file = write_temp_paco("check_rejects_uint", source);
    let cli = Cli::try_parse_from(["paco", "check", file.to_str().unwrap()]).unwrap();

    let error = run(cli).unwrap_err();

    assert!(error.contains("PACO-E0328"));
    assert!(error.contains("`uint` is not a Paco type; use `u64`"));
}

fn write_temp_paco(name: &str, source: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "paco_fixed_width_numeric_types_{}_{}_{}.paco",
        name,
        std::process::id(),
        monotonic_suffix()
    ));
    fs::write(&path, source).unwrap();
    path
}

fn monotonic_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}
