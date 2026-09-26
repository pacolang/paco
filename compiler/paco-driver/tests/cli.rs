use clap::CommandFactory;
use clap::Parser;
use paco_driver::{Cli, Commands, run};

#[test]
fn cli_exposes_core_subcommands() {
    let command = Cli::command();
    let names: Vec<_> = command
        .get_subcommands()
        .map(|subcommand| subcommand.get_name().to_string())
        .collect();

    assert_eq!(
        names,
        vec!["build", "check", "run", "test", "fmt", "doc", "clean", "explain", "shapes", "get", "mod", "fix"]
    );
}

#[test]
fn run_subcommand_accepts_project_default_without_file_argument() {
    let cli = Cli::try_parse_from(["paco", "run"]).unwrap();

    assert!(matches!(cli.command, Commands::Run { file: None, .. }));
}

#[test]
fn clean_takes_an_optional_path_and_a_cache_flag() {
    let cli = Cli::try_parse_from(["paco", "clean", "--cache"]).unwrap();
    assert!(matches!(cli.command, Commands::Clean { path: None, cache: true }));
    let cli = Cli::try_parse_from(["paco", "clean", "app.paco"]).unwrap();
    assert!(matches!(cli.command, Commands::Clean { path: Some(_), cache: false }));
}

const GRID: &str = "use stdlib::dims;

struct Grid<T: Numeric + Add, const D: int...> {
    data: []T,
    dims: []i64,

    pub fn extent(&self, axis: i64) -> i64 {
        self.dims[axis]
    }

    pub fn add(&self, other: &Grid<T, D...>) -> Grid<T, D...> {
        Grid { data: slice_of_zeros<T>(self.data.len()), dims: slice_of_zeros<i64>(0) }
    }

    pub fn checked_add<const E: int...>(&self, other: &Grid<T, E...>) -> Result<Grid<T, D...>, dims::DimError> {
        Result::Ok(Grid { data: slice_of_zeros<T>(self.data.len()), dims: slice_of_zeros<i64>(0) })
    }
}
";

fn json_string(text: &str, at: usize) -> (String, usize) {
    let mut out = String::new();
    let mut chars = text[at + 1..].char_indices();
    while let Some((index, c)) = chars.next() {
        match c {
            '"' => return (out, at + 1 + index + 1),
            '\\' => match chars.next().map(|(_, c)| c) {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some(other) => out.push(other),
                None => break,
            },
            c => out.push(c),
        }
    }
    panic!("unterminated JSON string");
}

fn number_after(text: &str, key: &str) -> usize {
    let at = text.find(key).unwrap_or_else(|| panic!("no {key} in {text}")) + key.len();
    text[at..].chars().take_while(char::is_ascii_digit).collect::<String>().parse().unwrap()
}

/// The edits of the first fix of the first diagnostic with `code`.
fn first_fix(json: &str, code: &str) -> Vec<(usize, usize, String)> {
    let line = json.lines().find(|line| line.contains(&format!("\"code\":\"{code}\""))).unwrap_or_else(|| panic!("no {code} in {json}"));
    let fix = &line[line.find("\"rank\":1").expect("a rank-1 fix")..];
    let edits = &fix[..fix.find("]}").expect("end of the edits")];
    let mut out = Vec::new();
    for edit in edits.split("{\"file\"").skip(1) {
        let start = number_after(edit, "\"start\":");
        let end = number_after(edit, "\"end\":");
        let at = edit.find("\"replacement\":").unwrap() + "\"replacement\":".len();
        out.push((start, end, json_string(edit, at).0));
    }
    out
}

#[test]
fn applying_the_first_json_fix_makes_the_program_check() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("step.paco");
    let source = format!(
        "{GRID}
fn step(a: Grid<f32, Dyn, 784>, b: Grid<f32, Dyn, 784>) -> Result<i64, dims::DimError> {{
    let c = a + &b;
    Result::Ok(c.data.len())
}}

fn main() {{
    print(0);
}}
"
    );
    std::fs::write(&path, &source).unwrap();
    let file = path.to_str().unwrap();
    let human = run(Cli::try_parse_from(["paco", "check", file]).unwrap()).unwrap_err();
    assert!(human.starts_with("error[PACO-E0342] "), "{human}");
    let json = run(Cli::try_parse_from(["paco", "check", "--format=json", file]).unwrap()).unwrap_err();
    assert!(json.lines().all(|line| line.starts_with("{\"code\":")), "{json}");
    assert!(json.contains("\"rank\":2") && json.contains("checked_add"), "{json}");
    let mut edits = first_fix(&json, "PACO-E0342");
    edits.sort_by_key(|(start, ..)| std::cmp::Reverse(*start));
    let mut fixed = source.clone();
    for (start, end, replacement) in edits {
        fixed.replace_range(start..end, &replacement);
    }
    std::fs::write(&path, &fixed).unwrap();
    run(Cli::try_parse_from(["paco", "check", file]).unwrap()).unwrap_or_else(|error| panic!("{fixed}\n{error}"));
}

#[test]
fn build_and_run_take_the_json_format_too() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bad.paco");
    std::fs::write(&path, "fn main() {\n    let x: i64 = true;\n}\n").unwrap();
    let file = path.to_str().unwrap();
    for command in ["build", "run"] {
        let json = run(Cli::try_parse_from(["paco", command, "--format=json", file]).unwrap()).unwrap_err();
        assert!(json.starts_with("{\"code\":\"PACO-E0302\""), "{command}: {json}");
    }
}

#[test]
fn explain_prints_the_registry_entry_with_or_without_the_prefix() {
    for code in ["E0342", "PACO-E0342"] {
        let output = run(Cli::try_parse_from(["paco", "explain", code]).unwrap()).unwrap();
        assert!(output.stdout.starts_with("PACO-E0342: cannot prove these dimensions are equal"), "{}", output.stdout);
        assert!(output.stdout.contains("polynomial normal forms"), "{}", output.stdout);
        assert!(output.stdout.contains("Fix: "), "{}", output.stdout);
        assert!(output.stdout.contains("0029-named-dynamic-dimensions.md"), "{}", output.stdout);
    }
    let error = run(Cli::try_parse_from(["paco", "explain", "E9999"]).unwrap()).unwrap_err();
    assert!(error.contains("unknown diagnostic code `PACO-E9999`"), "{error}");
}

#[test]
fn a_retired_code_names_its_successor() {
    let registry = "[E0100]\nmessage_template = \"old\"\nexplanation = \"gone\"\nstatus = \"retired\"\nsuperseded_by = \"E0342\"\n";
    let error = paco_driver::explain_in(registry, "E0100").unwrap_err();
    assert!(error.contains("PACO-E0100 is retired") && error.contains("PACO-E0342"), "{error}");
}

#[test]
fn a_retired_code_without_successor_says_why() {
    let registry = "[E0100]\nmessage_template = \"old\"\nexplanation = \"Retired: the check no longer exists.\"\nstatus = \"retired\"\n";
    let error = paco_driver::explain_in(registry, "E0100").unwrap_err();
    assert_eq!(error, "PACO-E0100 is retired: Retired: the check no longer exists.");
}

#[test]
fn autodiff_codes_are_explained() {
    for code in ["E0810", "E0811", "E0812", "E0813", "E0814", "E0815"] {
        let output = run(Cli::try_parse_from(["paco", "explain", code]).unwrap()).unwrap();
        assert!(output.stdout.starts_with(&format!("PACO-{code}: ")), "{}", output.stdout);
        assert!(output.stdout.contains("0026-native-autodiff.md"), "{}", output.stdout);
    }
}

#[test]
fn shapes_prints_every_dimensioned_let_with_its_origins() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("step.paco");
    let source = format!(
        "{GRID}
fn step(x: Grid<f32, Dyn, 784>) -> Result<i64, dims::DimError> {{
    let n = x.dim(0);
    let x: Grid<f32, n, 784> = x.with_dims()?;
    let y = x + &x;
    Result::Ok(n)
}}

fn main() {{
    print(0);
}}
"
    );
    std::fs::write(&path, &source).unwrap();
    let file = path.to_str().unwrap();
    let line = |needle: &str| source.lines().position(|line| line.contains(needle)).unwrap() + 1;
    let (witness, refined, sum) = (line("let n = "), line("let x: "), line("let y = "));
    let output = run(Cli::try_parse_from(["paco", "shapes", file]).unwrap()).unwrap();
    let expected = format!(
        "{file}:{refined} x: Grid<f32, n, 784>\n  n bound at {file}:{witness}\n{file}:{sum} y: Grid<f32, n, 784>\n  n bound at {file}:{witness}\n"
    );
    assert_eq!(output.stdout, expected);
    let json = run(Cli::try_parse_from(["paco", "shapes", "--format=json", file]).unwrap()).unwrap();
    assert_eq!(json.stdout.lines().count(), 2, "{}", json.stdout);
    let json_file = file.replace('\\', "\\\\");
    assert!(
        json.stdout.starts_with(&format!(
            "{{\"file\":\"{json_file}\",\"line\":{refined},\"name\":\"x\",\"type\":\"Grid<f32, n, 784>\",\"names\":[{{\"name\":\"n\",\"file\":\"{json_file}\",\"line\":{witness}}}]}}"
        )),
        "{}",
        json.stdout
    );
    std::fs::write(&path, "fn main() {\n    let x: i64 = true;\n}\n").unwrap();
    let error = run(Cli::try_parse_from(["paco", "shapes", file]).unwrap()).unwrap_err();
    assert!(error.contains("PACO-E0302"), "{error}");
}

#[test]
fn every_named_dimension_code_is_explained() {
    for code in ["E0336", "E0342", "E0343", "E0344", "E0345", "E0346", "E0347", "E0348"] {
        let output = run(Cli::try_parse_from(["paco", "explain", code]).unwrap()).unwrap();
        assert!(output.stdout.starts_with(&format!("PACO-{code}: ")), "{}", output.stdout);
        assert!(output.stdout.contains("ADR: docs/design/decisions/0029-named-dynamic-dimensions.md"), "{code}: {}", output.stdout);
    }
}
