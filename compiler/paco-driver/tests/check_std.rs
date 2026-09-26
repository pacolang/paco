use std::path::Path;
use std::process::Command;

#[test]
fn every_std_module_checks_cleanly() {
    let std_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../stdlib");
    let mut files = Vec::new();
    for dir in [std_root.clone(), std_root.join("core")] {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|ext| ext == "paco") {
                files.push(path);
            }
        }
    }
    assert!(!files.is_empty());
    for file in files {
        let output = Command::new(env!("CARGO_BIN_EXE_paco")).arg("check").arg(&file).output().unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success() && stderr.is_empty(), "{}: {stderr}", file.display());
    }
}
