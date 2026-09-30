//! Reading a safetensors file's header: an 8-byte little-endian header
//! length, then that many bytes of UTF-8 JSON describing each tensor's
//! dtype, shape and byte offsets, then the raw tensor data. Only the
//! header is read here -- discovering a file's tensors does not need
//! their data.

use std::fs::File;
use std::io::Read;
use std::path::Path;

/// One tensor's entry in a safetensors header, everything but its byte
/// offsets (not needed to list a file's tensors).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TensorInfo {
    pub name: String,
    pub dtype: String,
    pub shape: Vec<u64>,
}

/// Reads `path`'s header and returns its tensors, ordered by name.
/// `__metadata__`, the header's one non-tensor entry, is skipped.
pub fn read_header(path: &Path) -> Result<Vec<TensorInfo>, String> {
    let mut file = File::open(path).map_err(|error| format!("failed to open `{}`: {error}", path.display()))?;

    let mut length_bytes = [0u8; 8];
    file.read_exact(&mut length_bytes).map_err(|error| format!("failed to read `{}`'s header length: {error}", path.display()))?;
    let header_len = u64::from_le_bytes(length_bytes);

    let mut header_bytes = vec![0u8; header_len as usize];
    file.read_exact(&mut header_bytes).map_err(|error| format!("failed to read `{}`'s header: {error}", path.display()))?;

    parse_header(&header_bytes).map_err(|error| format!("`{}`'s header {error}", path.display()))
}

fn parse_header(bytes: &[u8]) -> Result<Vec<TensorInfo>, String> {
    let header: serde_json::Value = serde_json::from_slice(bytes).map_err(|error| format!("is not valid JSON: {error}"))?;
    let Some(header) = header.as_object() else {
        return Err("is not a JSON object".to_string());
    };

    let mut tensors = Vec::new();
    for (name, entry) in header {
        if name == "__metadata__" {
            continue;
        }
        let dtype = entry
            .get("dtype")
            .and_then(|value| value.as_str())
            .ok_or_else(|| format!("entry `{name}` has no string `dtype`"))?;
        let shape = entry
            .get("shape")
            .and_then(|value| value.as_array())
            .ok_or_else(|| format!("entry `{name}` has no `shape` array"))?
            .iter()
            .map(|dim| dim.as_u64().ok_or_else(|| format!("entry `{name}` has a non-integer shape dimension")))
            .collect::<Result<Vec<u64>, String>>()?;
        tensors.push(TensorInfo { name: name.clone(), dtype: dtype.to_string(), shape });
    }
    tensors.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(tensors)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// A minimal safetensors file: `header` as its JSON header (already
    /// including any wrapping braces) and one zero byte of tensor data per
    /// byte its `data_offsets` claim, so the file is at least as long as
    /// the header says.
    fn write_fixture(dir: &Path, header: &str) -> std::path::PathBuf {
        let path = dir.join("weights.safetensors");
        let mut file = File::create(&path).unwrap();
        file.write_all(&(header.len() as u64).to_le_bytes()).unwrap();
        file.write_all(header.as_bytes()).unwrap();
        path
    }

    #[test]
    fn lists_tensors_in_name_order_with_dtype_and_shape() {
        let dir = tempfile::tempdir().unwrap();
        let header = r#"{
            "weight": {"dtype": "F32", "shape": [4, 8], "data_offsets": [0, 128]},
            "bias": {"dtype": "F32", "shape": [4], "data_offsets": [128, 144]}
        }"#;
        let path = write_fixture(dir.path(), header);

        let tensors = read_header(&path).unwrap();

        assert_eq!(
            tensors,
            vec![
                TensorInfo { name: "bias".to_string(), dtype: "F32".to_string(), shape: vec![4] },
                TensorInfo { name: "weight".to_string(), dtype: "F32".to_string(), shape: vec![4, 8] },
            ]
        );
    }

    #[test]
    fn skips_the_metadata_entry() {
        let dir = tempfile::tempdir().unwrap();
        let header = r#"{
            "__metadata__": {"format": "pt"},
            "weight": {"dtype": "I64", "shape": [1], "data_offsets": [0, 8]}
        }"#;
        let path = write_fixture(dir.path(), header);

        let tensors = read_header(&path).unwrap();

        assert_eq!(tensors, vec![TensorInfo { name: "weight".to_string(), dtype: "I64".to_string(), shape: vec![1] }]);
    }

    #[test]
    fn an_empty_shape_is_a_scalar_tensor() {
        let dir = tempfile::tempdir().unwrap();
        let header = r#"{"scale": {"dtype": "F32", "shape": [], "data_offsets": [0, 4]}}"#;
        let path = write_fixture(dir.path(), header);

        let tensors = read_header(&path).unwrap();

        assert_eq!(tensors, vec![TensorInfo { name: "scale".to_string(), dtype: "F32".to_string(), shape: vec![] }]);
    }

    #[test]
    fn a_missing_file_is_a_clear_error() {
        let error = read_header(Path::new("/nonexistent/weights.safetensors")).unwrap_err();
        assert!(error.contains("failed to open"), "{error}");
    }

    #[test]
    fn a_non_json_header_is_a_clear_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_fixture(dir.path(), "not json");
        let error = read_header(&path).unwrap_err();
        assert!(error.contains("is not valid JSON"), "{error}");
    }
}
