use std::{fs, path::PathBuf};

use clap::Parser;
use paco_driver::{Cli, run};

// `phase-9-comptime` task 7.1: `#[derive(Display)]` generates a real
// `display(&self) -> string` method (dogfooded Paco source, Decision 8),
// run end-to-end through the driver.

#[test]
fn derive_display_formats_the_struct_name_and_every_field() {
    let source = r#"
#[derive(Display)]
struct User { name: string, age: i64 }

fn main() {
    let u = User { name: "Alice", age: 30 };
    print(u.display())
}
"#;
    let file = write_temp_paco("derive_display_basic", source);
    let cli = Cli::try_parse_from(["paco", "run", file.to_str().unwrap()]).unwrap();

    let output = run(cli).unwrap();

    assert_eq!(output.stdout, "User { name: Alice, age: 30 }\n");
}

fn write_temp_paco(name: &str, source: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!("paco_derive_display_{}_{}_{}.paco", name, std::process::id(), monotonic_suffix()));
    fs::write(&path, source).unwrap();
    path
}

fn monotonic_suffix() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
}
