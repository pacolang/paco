use std::{fs, path::PathBuf};

use clap::Parser;
use paco_driver::{Cli, run};

// `phase-9-comptime` task 7.2: a minimal, non-prelude `Serialize` trait
// (design.md Decision 4 — explicitly not stdlib-grade), proving the derive
// mechanism generalizes to a user-defined `#[derivable(..)]` trait, not
// just the prelude's own `Display` — with genuinely different generated
// semantics (an order-preserving, unlabeled field walk, vs. `Display`'s
// labeled `Name { field: value, .. }` formatting).

const SERIALIZE_TRAIT: &str = r#"
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
"#;

#[test]
fn derive_serialize_produces_an_order_preserving_unlabeled_field_walk() {
    let source = format!(
        r#"
{SERIALIZE_TRAIT}

#[derive(Serialize)]
struct User {{ name: string, age: i64 }}

fn main() {{
    let u = User {{ name: "Alice", age: 30 }};
    print(u.serialize())
}}
"#
    );
    let file = write_temp_paco("derive_serialize_basic", &source);
    let cli = Cli::try_parse_from(["paco", "run", file.to_str().unwrap()]).unwrap();

    let output = run(cli).unwrap();

    assert_eq!(output.stdout, "Alice,30\n");
}

fn write_temp_paco(name: &str, source: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!("paco_derive_serialize_{}_{}_{}.paco", name, std::process::id(), monotonic_suffix()));
    fs::write(&path, source).unwrap();
    path
}

fn monotonic_suffix() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
}
