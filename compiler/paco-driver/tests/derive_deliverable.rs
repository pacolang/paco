use std::{fs, path::PathBuf};

use clap::Parser;
use paco_driver::{Cli, run};

// `phase-9-comptime` task 7.3: this phase's roadmap deliverable — a
// `.paco` program using `#[derive(Display, Serialize)]` on one struct,
// both derived implementations exercised, run end-to-end through the
// driver (`paco run`).

#[test]
fn derive_display_and_serialize_on_one_struct_run_end_to_end() {
    let source = r#"
#[derivable(derive_serialize)]
trait Serialize { fn serialize(&self) -> string; }

comptime fn derive_serialize(t: type) -> Code {
    let mut acc = quote { #("") };
    let mut first = true;
    for field in fields_of(t) {
        let sep = if first { "" } else { "," };
        first = false;
        acc = quote { string_concat(&string_concat(&#(acc), &#(sep)), &self.#(field.name).display()) }
    }
    quote {
        methods #(t) {
            pub fn serialize(&self) -> string {
                #(acc)
            }
        }
    }
}

#[derive(Display, Serialize)]
struct User { name: string, age: i64 }

fn main() {
    let u = User { name: "Alice", age: 30 };
    print(u.display());
    print(u.serialize())
}
"#;
    let file = write_temp_paco("derive_deliverable", source);
    let cli = Cli::try_parse_from(["paco", "run", file.to_str().unwrap()]).unwrap();

    let output = run(cli).unwrap();

    assert_eq!(output.stdout, "User { name: Alice, age: 30 }\nAlice,30\n");
}

fn write_temp_paco(name: &str, source: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!("paco_derive_deliverable_{}_{}_{}.paco", name, std::process::id(), monotonic_suffix()));
    fs::write(&path, source).unwrap();
    path
}

fn monotonic_suffix() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
}
