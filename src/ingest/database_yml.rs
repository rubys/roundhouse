//! `config/database.yml`'s per-environment SQLite `pragmas:`.
//!
//! Rails' SQLite adapter (activerecord 8.1,
//! `SQLite3Adapter#configure_connection`) runs, on every connection it
//! opens, its `DEFAULT_PRAGMAS` merged with the config's own:
//!
//! ```ruby
//! pragmas = @config.fetch(:pragmas, {}).stringify_keys
//! DEFAULT_PRAGMAS.merge(pragmas).each do |pragma, value|
//!   if ::SQLite3::Pragmas.method_defined?("#{pragma}=")
//!     @raw_connection.public_send("#{pragma}=", value)
//!   else
//!     warn "Unknown SQLite pragma: #{pragma}"
//!   end
//! end
//! ```
//!
//! Each Db shim pins its own defaults (journal_mode, synchronous,
//! foreign_keys, ...) when it opens a connection; this lifts the app's
//! additions so the shim can run them after its own, as the merge lets
//! the app's value win. Campfire sets `wal_autocheckpoint: 0` under its
//! `default:` anchor so commits never checkpoint on the writer.
//!
//! Only the environment's `primary` database is read (a two-tier
//! `test: { primary: ... }` or the flat `test: { adapter: ... }`): the
//! Db shims hold one database. A value is spelled into SQL as Rails'
//! pragma setters do for the shapes taken here: an integer, a bare
//! word, or a boolean (`ON`/`OFF`). Any other value (ERB, a quoted
//! string with spaces, a nested map) is left out rather than guessed at.

use std::path::Path;

use crate::vfs::Vfs;

/// `(environment, ["PRAGMA name = value", ...])`, for each environment
/// whose primary config names pragmas. Empty when the file is absent or
/// names none.
pub fn database_pragmas(vfs: &dyn Vfs, dir: &Path) -> Vec<(String, Vec<String>)> {
    let path = dir.join("config/database.yml");
    let Ok(bytes) = vfs.read(&path) else { return Vec::new() };
    let Ok(text) = String::from_utf8(bytes) else { return Vec::new() };
    parse(&text)
}

fn parse(text: &str) -> Vec<(String, Vec<String>)> {
    use serde_yaml_ng::Value;
    // ERB is evaluated before the YAML is read. Values it produces are
    // not ours to know, so each tag reads as an empty scalar; a pragma
    // whose value was ERB is then dropped by `sql_value`.
    let text = strip_erb(text);
    let Ok(mut doc) = serde_yaml_ng::from_str::<Value>(&text) else { return Vec::new() };
    if doc.apply_merge().is_err() {
        return Vec::new();
    }
    let Value::Mapping(envs) = doc else { return Vec::new() };
    let mut out = Vec::new();
    for (env, config) in envs {
        let Value::String(env) = env else { continue };
        let Value::Mapping(config) = config else { continue };
        // Flat (`adapter:` at the top) or one level of named databases.
        let primary = if config.contains_key("adapter") || config.contains_key("database") {
            Some(config)
        } else {
            match config.get("primary") {
                Some(Value::Mapping(m)) => Some(m.clone()),
                _ => None,
            }
        };
        let Some(primary) = primary else { continue };
        let Some(Value::Mapping(pragmas)) = primary.get("pragmas") else { continue };
        let mut statements = Vec::new();
        for (name, value) in pragmas {
            let Value::String(name) = name else { continue };
            if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') || name.is_empty() {
                continue;
            }
            let Some(value) = sql_value(value) else { continue };
            statements.push(format!("PRAGMA {name} = {value}"));
        }
        if !statements.is_empty() {
            out.push((env.clone(), statements));
        }
    }
    out.sort();
    out
}

fn sql_value(value: &serde_yaml_ng::Value) -> Option<String> {
    use serde_yaml_ng::Value;
    match value {
        Value::Number(n) if n.is_i64() || n.is_u64() => Some(n.to_string()),
        Value::Bool(true) => Some("ON".to_string()),
        Value::Bool(false) => Some("OFF".to_string()),
        Value::String(s) if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') => {
            Some(s.clone())
        }
        _ => None,
    }
}

fn strip_erb(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("<%") {
        out.push_str(&rest[..start]);
        match rest[start..].find("%>") {
            Some(end) => {
                out.push_str("\"\"");
                rest = &rest[start + end + 2..];
            }
            None => {
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn campfire_shape_reads_the_anchor_into_each_environment() {
        let yml = r#"
default: &default
  adapter: sqlite3
  pool: <%= ENV.fetch("RAILS_MAX_THREADS") { 10 } %>
  timeout: 5000
  pragmas:
    wal_autocheckpoint: 0

development:
  primary:
    <<: *default
    database: storage/db/development.sqlite3

test:
  <<: *default
  database: storage/db/test.sqlite3
"#;
        let got = parse(yml);
        assert_eq!(
            got,
            vec![
                ("default".to_string(), vec!["PRAGMA wal_autocheckpoint = 0".to_string()]),
                ("development".to_string(), vec!["PRAGMA wal_autocheckpoint = 0".to_string()]),
                ("test".to_string(), vec!["PRAGMA wal_autocheckpoint = 0".to_string()]),
            ]
        );
    }

    #[test]
    fn values_it_cannot_spell_are_left_out() {
        let yml = "test:\n  adapter: sqlite3\n  pragmas:\n    journal_mode: wal\n    foreign_keys: true\n    mmap_size: <%= 1 %>\n    cache_size: \"a b\"\n";
        assert_eq!(
            parse(yml),
            vec![(
                "test".to_string(),
                vec!["PRAGMA journal_mode = wal".to_string(), "PRAGMA foreign_keys = ON".to_string()]
            )]
        );
    }
}
