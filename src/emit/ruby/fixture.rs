//! `test/fixtures/*.yml` emission. Rebuilds YAML via serde_yaml_ng so
//! the round-trip stays clean; values are emitted as strings (matching
//! the IR), unquoted where serde's default style permits.

use std::path::PathBuf;

use super::super::EmittedFile;
use crate::dialect::Fixture;

pub(super) fn emit_fixture(fixture: &Fixture) -> EmittedFile {
    let mut outer = serde_yaml_ng::Mapping::new();
    for (label, fields) in &fixture.records {
        let mut inner = serde_yaml_ng::Mapping::new();
        for (k, v) in fields {
            inner.insert(
                serde_yaml_ng::Value::String(k.as_str().to_string()),
                serde_yaml_ng::Value::String(v.clone()),
            );
        }
        outer.insert(
            serde_yaml_ng::Value::String(label.as_str().to_string()),
            serde_yaml_ng::Value::Mapping(inner),
        );
    }
    let content = serde_yaml_ng::to_string(&serde_yaml_ng::Value::Mapping(outer))
        .unwrap_or_default();
    EmittedFile {
        path: PathBuf::from(format!("test/fixtures/{}.yml", fixture.name)),
        content,
    }
}
