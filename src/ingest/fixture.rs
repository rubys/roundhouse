//! YAML fixture ingestion — `test/fixtures/<name>.yml`. The top-level
//! YAML is a mapping of label → field map. Field values are kept as
//! strings regardless of scalar type; emitters handle per-column-type
//! coercion and Rails's `article: one` fixture-reference shorthand.

use std::path::Path;

use indexmap::IndexMap;

use crate::Symbol;
use crate::dialect::Fixture;

use super::{IngestError, IngestResult};

pub fn ingest_fixture_file(source: &[u8], path: &Path) -> IngestResult<Fixture> {
    let name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| IngestError::Unsupported {
            file: path.display().to_string(),
            message: "fixture file has no stem".into(),
        })?;

    // Parse as a nested mapping of String → String → YAML scalar. We
    // stringify scalars at load time so the IR representation stays
    // format-simple; round-trip tests catch any precision loss by
    // comparing re-ingested YAML.
    let raw: IndexMap<String, IndexMap<String, serde_yaml_ng::Value>> =
        serde_yaml_ng::from_slice(source).map_err(|e| IngestError::Parse {
            file: path.display().to_string(),
            message: format!("yaml: {e}"),
        })?;

    let mut records: IndexMap<Symbol, IndexMap<Symbol, String>> = IndexMap::new();
    for (label, fields) in raw {
        let mut field_map: IndexMap<Symbol, String> = IndexMap::new();
        for (k, v) in fields {
            let s = yaml_scalar_as_string(&v).ok_or_else(|| IngestError::Unsupported {
                file: path.display().to_string(),
                message: format!("fixture field {label}.{k} is not a scalar"),
            })?;
            field_map.insert(Symbol::from(k), s);
        }
        records.insert(Symbol::from(label), field_map);
    }

    Ok(Fixture {
        name: Symbol::from(name),
        records,
    })
}

fn yaml_scalar_as_string(v: &serde_yaml_ng::Value) -> Option<String> {
    match v {
        serde_yaml_ng::Value::String(s) => Some(s.clone()),
        serde_yaml_ng::Value::Number(n) => Some(n.to_string()),
        serde_yaml_ng::Value::Bool(b) => Some(b.to_string()),
        serde_yaml_ng::Value::Null => Some(String::new()),
        // Nested maps/sequences aren't used in the fixtures we handle;
        // return None so the caller can error cleanly.
        serde_yaml_ng::Value::Mapping(_) | serde_yaml_ng::Value::Sequence(_) => None,
        serde_yaml_ng::Value::Tagged(t) => yaml_scalar_as_string(&t.value),
    }
}
