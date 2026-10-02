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

mod yaml;

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

    // Parse as YAML values, then resolve merge keys (`<<: *defaults`)
    // the way Psych does before Rails sees the rows. The top level is
    // label → field map; scalars are stringified at load time so the IR
    // stays format-simple, and round-trip tests catch precision loss.
    let mut doc =
        yaml::parse(&split.yaml).map_err(|e| IngestError::Parse {
            file: file.clone(),
            message: format!("yaml: {e}"),
        })?;
    resolve_merge_keys(&mut doc);
    let not_a_set = |what: String| IngestError::Unsupported {
        file: file.clone(),
        message: format!("not a fixture set: {what}, where Rails expects label → fields"),
    };
    let top = match doc {
        serde_yaml_ng::Value::Mapping(m) => m,
        // An empty file (or one that is all comments) is an empty set.
        serde_yaml_ng::Value::Null => serde_yaml_ng::Mapping::new(),
        other => return Err(not_a_set(format!("the top level is {}", yaml_kind(&other)))),
    };

    let mut model_class: Option<Symbol> = None;
    let mut ignored: Vec<String> = Vec::new();
    let mut records: IndexMap<Symbol, IndexMap<Symbol, FixtureValue>> = IndexMap::new();
    for (label, fields) in top {
        let label = yaml_scalar_as_string(&label).unwrap_or_default();
        // `_fixture:` is the file's configuration, not a row: the model
        // class when the path doesn't name it, and the labels that exist
        // only to be anchored and merged into others (Rails'
        // `ActiveRecord::FixtureSet::File#config_row`).
        if label == "_fixture" {
            if let Some(class) = fields.get("model_class").and_then(yaml_scalar_as_string) {
                model_class = Some(Symbol::from(class.as_str()));
            }
            match fields.get("ignore") {
                Some(serde_yaml_ng::Value::Sequence(seq)) => {
                    ignored.extend(seq.iter().filter_map(yaml_scalar_as_string))
                }
                Some(v) => ignored.extend(yaml_scalar_as_string(v)),
                None => {}
            }
            continue;
        }
        let fields = match fields {
            serde_yaml_ng::Value::Mapping(m) => m,
            serde_yaml_ng::Value::Null => serde_yaml_ng::Mapping::new(),
            other => {
                return Err(not_a_set(format!("`{label}` is {}", yaml_kind(&other))));
            }
        };
        let mut field_map: IndexMap<Symbol, FixtureValue> = IndexMap::new();
        for (k, v) in fields {
            let k = yaml_scalar_as_string(&k).unwrap_or_default();
            // A hash or array is a value for a JSON or serialized column:
            // Rails casts it through the column type on insert (JSON for
            // `json`, a YAML dump otherwise). JSON text serves both, since
            // YAML reads JSON. A key JSON can't hold is still reported.
            let s = yaml_scalar_as_string(&v)
                .or_else(|| match &v {
                    serde_yaml_ng::Value::Mapping(_) | serde_yaml_ng::Value::Sequence(_) => {
                        serde_json::to_string(&v).ok()
                    }
                    _ => None,
                })
                .ok_or_else(|| IngestError::Unsupported {
                    file: file.clone(),
                    message: format!("fixture field {label}.{k} is not a scalar"),
                })?;
            let value = match resolve_slot(&s, &values) {
                SlotMatch::None => FixtureValue::Scalar(s),
                SlotMatch::Whole(expr) => FixtureValue::Ruby(expr),
                // `body: "hi <%= name %>"` — the tag is one part of a
                // larger scalar, so the value is a string built at
                // runtime: ERB writes each tag's `to_s` into the text,
                // which is what interpolating it into a Ruby string does.
                SlotMatch::Embedded => {
                    let src = interpolated_source(&s, &split.values);
                    let tag_file = format!("{file} ({label}.{k} with ERB)");
                    FixtureValue::Ruby(super::expr::ingest_ruby_program(&src, &tag_file)?)
                }
            };
            field_map.insert(Symbol::from(k.as_str()), value);
        }
        records.insert(Symbol::from(label.as_str()), field_map);
    }
    // After merging, so a row that merges an ignored one still has its fields.
    for label in &ignored {
        records.shift_remove(&Symbol::from(label.as_str()));
    }

    Ok(Fixture {
        name: Symbol::from(name.as_str()),
        path: Symbol::from(rel.as_str()),
        records,
        preamble,
        model_class,
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

/// A scalar holding ERB slots among other text, as the source of a Ruby
/// string that interpolates each slot's expression.
fn interpolated_source(s: &str, values: &[String]) -> String {
    let mut lit = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '\\' => lit.push_str("\\\\"),
            '"' => lit.push_str("\\\""),
            '#' => lit.push_str("\\#"),
            '\n' => lit.push_str("\\n"),
            _ => lit.push(c),
        }
    }
    // Highest slot first, so `_1_` can't match inside `_12_`'s name.
    for (idx, src) in values.iter().enumerate().rev() {
        lit = lit.replace(&erb_slot(idx), &format!("#{{({src})}}"));
    }
    format!("\"{lit}\"")
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

/// Resolve YAML merge keys, innermost first: an anchor is copied where
/// it is aliased, so in a chain (`c` merges `b`, which merges `a`) the
/// copy of `b` under `c`'s `<<` still carries its own `<<`, and has to
/// be resolved before `c` takes its fields. A key the mapping sets
/// itself wins over a merged one, and within `<<: [*a, *b]` the earlier
/// mapping wins, as in Psych.
fn resolve_merge_keys(v: &mut serde_yaml_ng::Value) {
    match v {
        serde_yaml_ng::Value::Mapping(m) => {
            for (_, child) in m.iter_mut() {
                resolve_merge_keys(child);
            }
            let Some(merge) = m.remove("<<") else { return };
            let sources = match merge {
                serde_yaml_ng::Value::Sequence(seq) => seq,
                other => vec![other],
            };
            for source in sources {
                if let serde_yaml_ng::Value::Mapping(src) = source {
                    for (k, val) in src {
                        m.entry(k).or_insert(val);
                    }
                }
            }
        }
        serde_yaml_ng::Value::Sequence(seq) => seq.iter_mut().for_each(resolve_merge_keys),
        serde_yaml_ng::Value::Tagged(t) => resolve_merge_keys(&mut t.value),
        _ => {}
    }
}

fn yaml_kind(v: &serde_yaml_ng::Value) -> &'static str {
    match v {
        serde_yaml_ng::Value::Sequence(_) => "a sequence",
        serde_yaml_ng::Value::Mapping(_) => "a mapping",
        serde_yaml_ng::Value::Tagged(_) => "a tagged value",
        _ => "a scalar",
    }
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
