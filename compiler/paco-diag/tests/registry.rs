use std::collections::BTreeSet;
use std::path::Path;

fn emitted_codes(compiler_dir: &Path) -> BTreeSet<String> {
    let mut codes = BTreeSet::new();
    for entry in walkdir::WalkDir::new(compiler_dir)
        .into_iter()
        .filter_entry(|entry| entry.file_name() != "target" && entry.file_name() != "tests")
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "rs"))
    {
        let source = std::fs::read_to_string(entry.path()).unwrap();
        let mut rest = source.as_str();
        while let Some(at) = rest.find("PACO-") {
            rest = &rest[at + 5..];
            let code: String = rest.chars().take(5).collect();
            let mut chars = code.chars();
            if matches!(chars.next(), Some('E' | 'W')) && code.len() == 5 && chars.all(|c| c.is_ascii_digit()) {
                codes.insert(code);
            }
        }
    }
    codes
}

#[test]
fn every_emitted_code_is_registered() {
    let compiler_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let registry_path = compiler_dir.join("../docs/diagnostics/registry.toml");
    let registry: toml::Table = std::fs::read_to_string(&registry_path).unwrap().parse().unwrap();

    let emitted = emitted_codes(&compiler_dir);
    assert!(emitted.contains("E0112"), "the scan found no codes; is the path right?");
    let missing: Vec<&String> = emitted.iter().filter(|code| !registry.contains_key(code.as_str())).collect();
    assert!(missing.is_empty(), "codes emitted by the compiler but absent from registry.toml: {missing:?}");

    for (code, entry) in &registry {
        for field in ["message_template", "severity", "explanation", "cause", "fix", "spec_ref", "introduced_in", "status"] {
            assert!(entry.get(field).is_some(), "registry entry {code} lacks `{field}`");
        }
    }
}
