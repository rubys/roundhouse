//! YAML fixture ingestion — `test/fixtures/<name>.yml`. The top-level
//! YAML is a mapping of label → field map. Field values are kept as
//! strings regardless of scalar type; emitters handle per-column-type
//! coercion and Rails's `article: one` fixture-reference shorthand.
//!
//! ERB. Rails renders every fixture file through ERB before YAML sees
//! it, and the tags hold arbitrary Ruby — so the file's data is not
//! knowable to the compiler. It doesn't have to be: the emitted
//! fixture loader IS Ruby (`<Plural>Fixtures._fixtures_load!`), so a
//! tag can ride through as an expression and evaluate where Rails
//! would have evaluated it. That's what `split_erb` does — lift the
//! tags out so the remainder is parseable YAML, ingest each tag's Ruby
//! into an `Expr`, and stitch the two back together.
//!
//! campfire's three ERB fixtures are the forcing function, and between
//! them they cover the whole surface: a statement tag binding a local
//! (`<% password_digest = BCrypt::Password.create("…") %>`), a value
//! tag reading it back (`<%= password_digest %>`), a relative time
//! (`<%= 1.hour.ago %>`), and a call into the app's own model
//! (`<%= User.generate_bot_token %>`).

use std::path::Path;

use indexmap::IndexMap;

use crate::Symbol;
use crate::dialect::{Fixture, FixtureValue};
use crate::expr::Expr;

use super::{IngestError, IngestResult};

/// Placeholder substituted for each `<%= … %>` tag so the remaining
/// text parses as YAML. Deliberately ugly, so it can never collide with
/// real fixture content, and substituted UNQUOTED: a bare
/// `__ROUNDHOUSE_ERB_0__` is a plain YAML scalar on its own
/// (`password_digest: <%= … %>`) and equally fine inside a quoted one
/// (`name: "hi <%= … %>"`), where adding quotes of our own would close
/// the string early and break the parse.
fn erb_slot(idx: usize) -> String {
    format!("__ROUNDHOUSE_ERB_{idx}__")
}

/// A fixture file with its ERB lifted out.
struct SplitErb {
    /// The source with statement tags removed and value tags replaced
    /// by quoted `__ROUNDHOUSE_ERB_<n>__` scalars.
    yaml: String,
    /// Ruby source of each `<% … %>` tag, in source order.
    statements: Vec<String>,
    /// Ruby source of each `<%= … %>` tag, indexed by slot number.
    values: Vec<String>,
}

/// Lift ERB tags out of a fixture file.
///
/// Three tag kinds, per ERB's own grammar: `<%# … %>` is a comment and
/// vanishes; `<%= … %>` is an output tag and becomes a slot; `<% … %>`
/// is a statement and is hoisted into `statements`. A trim marker
/// (`-%>`, `<%-`) affects surrounding whitespace only, which YAML here
/// does not care about, so it is simply stripped.
fn split_erb(source: &str) -> SplitErb {
    let mut out = SplitErb {
        yaml: String::with_capacity(source.len()),
        statements: Vec::new(),
        values: Vec::new(),
    };
    let bytes = source.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let Some(open) = source[i..].find("<%").map(|p| i + p) else {
            out.yaml.push_str(&source[i..]);
            break;
        };
        out.yaml.push_str(&source[i..open]);
        let Some(close) = source[open..].find("%>").map(|p| open + p) else {
            // Unterminated tag — nothing sane to do but keep the text
            // and let the YAML parser report where it breaks.
            out.yaml.push_str(&source[open..]);
            break;
        };
        let raw = &source[open + 2..close];
        let body = raw.trim_start_matches('-').trim_end_matches('-').trim();
        if let Some(rest) = body.strip_prefix('#') {
            let _ = rest; // comment tag: contributes nothing
        } else if let Some(expr) = body.strip_prefix('=') {
            out.yaml.push_str(&erb_slot(out.values.len()));
            out.values.push(expr.trim().to_string());
        } else if !body.is_empty() {
            out.statements.push(body.to_string());
        }
        i = close + 2;
    }
    out
}

/// `root` is the `test/fixtures` directory the file was found under.
/// It is what turns `<root>/push/subscriptions.yml` into the fixture-set
/// path `push/subscriptions` — the directory is a NAMESPACE, and only
/// the part below the root is part of the name. A `path` that is not
/// under `root` falls back to the file stem, which is the top-level
/// answer and what every flat fixture wants anyway.
pub fn ingest_fixture_file(source: &[u8], path: &Path, root: &Path) -> IngestResult<Fixture> {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| IngestError::Unsupported {
            file: path.display().to_string(),
            message: "fixture file has no stem".into(),
        })?;
    // `articles`, or `push/subscriptions` for a namespaced set.
    let rel = path
        .strip_prefix(root)
        .ok()
        .and_then(|r| r.with_extension("").to_str().map(str::to_string))
        .unwrap_or_else(|| stem.to_string());
    // Rails' fixture-set name: the path with `/` -> `_`. Both the
    // accessor a test calls and the table the rows land in.
    let name = rel.replace('/', "_");
    let file = path.display().to_string();

    let text = std::str::from_utf8(source).map_err(|e| IngestError::Parse {
        file: file.clone(),
        message: format!("fixture is not utf-8: {e}"),
    })?;
    let split = split_erb(text);

    // Statement tags run once, ahead of every insert. Each is ingested
    // on its own so a single unparseable tag names itself rather than
    // taking the file down with it.
    let mut preamble: Vec<Expr> = Vec::new();
    for (idx, src) in split.statements.iter().enumerate() {
        let tag_file = format!("{file} (ERB statement {idx})");
        preamble.push(super::expr::ingest_ruby_program(src, &tag_file)?);
    }

    // Value tags, ingested once each and cloned into every field that
    // references the slot. (Rails re-evaluates per occurrence, but a
    // slot appears exactly once by construction — the substitution is
    // positional.)
    let mut values: Vec<Expr> = Vec::new();
    for (idx, src) in split.values.iter().enumerate() {
        let tag_file = format!("{file} (ERB value {idx})");
        values.push(super::expr::ingest_ruby_program(src, &tag_file)?);
    }

    // Parse as a nested mapping of String → String → YAML scalar. We
    // stringify scalars at load time so the IR representation stays
    // format-simple; round-trip tests catch any precision loss by
    // comparing re-ingested YAML.
    let raw: IndexMap<String, IndexMap<String, serde_yaml_ng::Value>> =
        serde_yaml_ng::from_str(&split.yaml).map_err(|e| IngestError::Parse {
            file: file.clone(),
            message: format!("yaml: {e}"),
        })?;

    let mut records: IndexMap<Symbol, IndexMap<Symbol, FixtureValue>> = IndexMap::new();
    for (label, fields) in raw {
        let mut field_map: IndexMap<Symbol, FixtureValue> = IndexMap::new();
        for (k, v) in fields {
            let s = yaml_scalar_as_string(&v).ok_or_else(|| IngestError::Unsupported {
                file: file.clone(),
                message: format!("fixture field {label}.{k} is not a scalar"),
            })?;
            let value = match resolve_slot(&s, &values) {
                SlotMatch::None => FixtureValue::Scalar(s),
                SlotMatch::Whole(expr) => FixtureValue::Ruby(expr),
                // `body: "hi <%= name %>"` — the tag is one part of a
                // larger scalar, so the value is a string built at
                // runtime rather than the tag's own result. Reachable
                // Rails, but nothing we ingest writes it; report the
                // field rather than guessing at concatenation.
                SlotMatch::Embedded => {
                    return Err(IngestError::Unsupported {
                        file: file.clone(),
                        message: format!(
                            "fixture field {label}.{k}: ERB tag interpolated into a larger scalar"
                        ),
                    });
                }
            };
            field_map.insert(Symbol::from(k), value);
        }
        records.insert(Symbol::from(label), field_map);
    }

    Ok(Fixture {
        name: Symbol::from(name.as_str()),
        path: Symbol::from(rel.as_str()),
        records,
        preamble,
    })
}

enum SlotMatch {
    /// Plain scalar, no ERB.
    None,
    /// The scalar is exactly one slot.
    Whole(Expr),
    /// The scalar contains a slot alongside other text.
    Embedded,
}

fn resolve_slot(s: &str, values: &[Expr]) -> SlotMatch {
    for (idx, expr) in values.iter().enumerate() {
        let slot = erb_slot(idx);
        if s == slot {
            return SlotMatch::Whole(expr.clone());
        }
        if s.contains(&slot) {
            return SlotMatch::Embedded;
        }
    }
    SlotMatch::None
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
