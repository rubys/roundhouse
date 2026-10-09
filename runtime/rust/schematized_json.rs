//! `SchematizedJson` for the Rust target — hand-written port of
//! `runtime/spinel/schematized_json.rb` (the `has_json` column seam).
//!
//! A column holds a FLAT JSON object of scalars (boolean / integer /
//! string). Values are kept as their SOURCE TEXT between parse and
//! render, so keys this code never decoded round-trip verbatim; keys are
//! re-emitted SORTED. A value opening with `{` or `[` ends the scan
//! (truncated read of foreign JSON, never a mis-split one).
//!
//! Types: the column ivar in emitted models is a `serde_json::Value`
//! (Null = Ruby nil, String = the serialized text). Every "serialized"
//! and "value" parameter is therefore `impl Into<serde_json::Value>`,
//! which also accepts `Option<String>`, `String`, `&str`, `Option<&str>`.
//! Writers return `serde_json::Value` (`String(text)` / `Null`), so they
//! assign straight back to the column field. CAVEAT: `Value::to_string()`
//! on a `Value::String` yields the JSON-QUOTED text (`"\"{...}\""`), so
//! callers that want the raw column text should use
//! [`SchematizedJson::text`] (or `.as_str()`), not `.to_string()`.
//!
//! Deviations from the Ruby: `decode_string` also decodes `\uXXXX`
//! (the Ruby leaves it verbatim, yet its own encoder emits `\u003c` for
//! `<`, so Ruby does not round-trip those; this does). A key outside the
//! schema in `assign` panics (Ruby: NoMethodError).

use serde_json::Value;
use std::collections::BTreeMap;

pub struct SchematizedJson;

fn is_ws(c: char) -> bool {
    c == ' ' || c == '\n' || c == '\t' || c == '\r'
}

fn separator(c: char) -> bool {
    is_ws(c) || c == ','
}

/// Index just past the string literal starting at `start` (its opening
/// quote), or the end of input when unterminated.
fn scan_string(s: &[char], start: usize) -> usize {
    let n = s.len();
    let mut i = start + 1;
    while i < n {
        match s[i] {
            '\\' => i += 2,
            '"' => return i + 1,
            _ => i += 1,
        }
    }
    n
}

fn scan_value(s: &[char], start: usize) -> usize {
    if s.get(start) == Some(&'"') {
        return scan_string(s, start);
    }
    let mut i = start;
    while i < s.len() {
        let c = s[i];
        if c == ',' || c == '}' || c == '{' || c == '[' || is_ws(c) {
            break;
        }
        i += 1;
    }
    i
}

fn encode_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '<' => out.push_str("\\u003c"),
            '>' => out.push_str("\\u003e"),
            '&' => out.push_str("\\u0026"),
            c => out.push(c),
        }
    }
    out
}

/// Ruby `to_s` of a JSON scalar (nil -> "").
fn to_s(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Ruby `to_i` of a JSON scalar (String: leading integer, else 0).
fn to_i(v: &Value) -> i64 {
    match v {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)).unwrap_or(0),
        Value::Bool(_) | Value::Null => 0,
        Value::String(s) => {
            let t = s.trim_start();
            let mut end = 0;
            for (i, c) in t.char_indices() {
                if c.is_ascii_digit() || (i == 0 && (c == '-' || c == '+')) {
                    end = i + c.len_utf8();
                } else {
                    break;
                }
            }
            t[..end].parse().unwrap_or(0)
        }
        _ => 0,
    }
}

impl SchematizedJson {
    /// The raw column text of a writer's result (`None` for nil).
    pub fn text(v: &Value) -> Option<String> {
        match v {
            Value::Null => None,
            Value::String(s) => Some(s.clone()),
            other => Some(other.to_string()),
        }
    }

    /// Flat object -> `key => value source text`. Sorted by key.
    pub fn parse_object(serialized: impl Into<Value>) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        let Some(text) = Self::text(&serialized.into()) else { return out };
        let s: Vec<char> = text.chars().collect();
        let n = s.len();
        let mut i = 0;
        while i < n && s[i] != '{' {
            i += 1;
        }
        i += 1;
        loop {
            while i < n && separator(s[i]) {
                i += 1;
            }
            if i >= n || s[i] != '"' {
                break;
            }
            let key_end = scan_string(&s, i);
            let key = Self::decode_string(&s[i..key_end].iter().collect::<String>());
            i = key_end;
            while i < n && s[i] != ':' {
                i += 1;
            }
            i += 1;
            while i < n && separator(s[i]) {
                i += 1;
            }
            let value_end = scan_value(&s, i);
            if i >= n || value_end == i {
                break;
            }
            out.insert(key, s[i..value_end].iter().collect());
            i = value_end;
        }
        out
    }

    /// The String behind a JSON string literal (quotes included).
    pub fn decode_string(raw: &str) -> String {
        let s: Vec<char> = raw.chars().collect();
        if s.len() < 2 || s[0] != '"' {
            return raw.to_string();
        }
        let last = s.len() - 1;
        let mut out = String::new();
        let mut i = 1;
        while i < last {
            if s[i] == '\\' && i + 1 < s.len() {
                let e = s[i + 1];
                match e {
                    'n' => out.push('\n'),
                    'r' => out.push('\r'),
                    't' => out.push('\t'),
                    'b' => out.push('\u{8}'),
                    'f' => out.push('\u{c}'),
                    'u' if i + 6 <= last => {
                        let hex: String = s[i + 2..i + 6].iter().collect();
                        match u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                            Some(ch) => {
                                out.push(ch);
                                i += 6;
                                continue;
                            }
                            None => out.push('u'),
                        }
                    }
                    other => out.push(other),
                }
                i += 2;
            } else {
                out.push(s[i]);
                i += 1;
            }
        }
        out
    }

    /// Value source text for `key`, `""` when absent.
    pub fn read_raw(data: &BTreeMap<String, String>, key: &str) -> String {
        data.get(key).cloned().unwrap_or_default()
    }

    /// Sorted re-serialization.
    pub fn render_object(data: &BTreeMap<String, String>) -> String {
        let mut out = String::from("{");
        for (i, (k, raw)) in data.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push('"');
            out.push_str(&encode_string(k));
            out.push_str("\":");
            out.push_str(raw);
        }
        out.push('}');
        out
    }

    fn raw_of(serialized: impl Into<Value>, key: &str) -> Option<String> {
        let raw = Self::read_raw(&Self::parse_object(serialized), key);
        if raw.is_empty() || raw == "null" { None } else { Some(raw) }
    }

    /// Absent key or stored null -> `fallback`.
    pub fn read_boolean(serialized: impl Into<Value>, key: &str, fallback: bool) -> bool {
        match Self::raw_of(serialized, key) {
            None => fallback,
            Some(raw) => raw == "true",
        }
    }

    pub fn read_integer(serialized: impl Into<Value>, key: &str, fallback: i64) -> i64 {
        match Self::raw_of(serialized, key) {
            None => fallback,
            Some(raw) => to_i(&Value::String(raw)),
        }
    }

    pub fn read_string(serialized: impl Into<Value>, key: &str, fallback: impl Into<String>) -> String {
        match Self::raw_of(serialized, key) {
            None => fallback.into(),
            Some(raw) => Self::decode_string(&raw),
        }
    }

    fn write_raw(serialized: impl Into<Value>, key: &str, raw: String) -> Value {
        let mut data = Self::parse_object(serialized);
        data.insert(key.to_string(), raw);
        Value::String(Self::render_object(&data))
    }

    /// Writers return the WHOLE re-serialized object as `Value::String`.
    pub fn write_boolean(serialized: impl Into<Value>, key: &str, value: bool) -> Value {
        Self::write_raw(serialized, key, if value { "true" } else { "false" }.to_string())
    }

    pub fn write_integer(serialized: impl Into<Value>, key: &str, value: i64) -> Value {
        Self::write_raw(serialized, key, value.to_string())
    }

    pub fn write_string(serialized: impl Into<Value>, key: &str, value: &str) -> Value {
        Self::write_raw(serialized, key, format!("\"{}\"", encode_string(value)))
    }

    /// Rails' `<col>=`: `value` Null -> Null; a non-object passes through
    /// as text (hydration's serialized shape); an object is merged key by
    /// key over `serialized` through `schema` (`key => "boolean" |
    /// "integer" | "string"`). A key outside the schema panics.
    pub fn assign<K, T>(
        serialized: impl Into<Value>,
        value: impl Into<Value>,
        schema: impl IntoIterator<Item = (K, T)>,
    ) -> Value
    where
        K: AsRef<str>,
        T: AsRef<str>,
    {
        let value = value.into();
        let map = match value {
            Value::Null => return Value::Null,
            Value::Object(m) => m,
            other => return Value::String(to_s(&other)),
        };
        let schema: BTreeMap<String, String> = schema
            .into_iter()
            .map(|(k, t)| (k.as_ref().to_string(), t.as_ref().to_string()))
            .collect();
        let mut out: Value = serialized.into();
        for (key, v) in map.iter() {
            out = match schema.get(key).map(String::as_str) {
                Some("boolean") => {
                    let s = to_s(v);
                    Self::write_boolean(out, key, s != "0" && !s.is_empty() && s != "false")
                }
                Some("integer") => Self::write_integer(out, key, to_i(v)),
                Some("string") => Self::write_string(out, key, &to_s(v)),
                _ => panic!("undefined method '{}=' for the json column's schema", key),
            };
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashMap;

    fn raw(doc: &BTreeMap<String, String>, k: &str) -> String {
        SchematizedJson::read_raw(doc, k)
    }

    fn schema() -> HashMap<&'static str, &'static str> {
        HashMap::from([("on", "boolean"), ("n", "integer"), ("s", "string")])
    }

    #[test]
    fn parse_empty_sources() {
        for src in [Value::Null, json!(""), json!("{}")] {
            assert_eq!(raw(&SchematizedJson::parse_object(src), "a"), "");
        }
        assert_eq!(raw(&SchematizedJson::parse_object(None::<String>), "a"), "");
    }

    #[test]
    fn parse_keeps_source_text() {
        let d = SchematizedJson::parse_object("{\"a\":true,\"b\":42,\"c\":\"hi\",\"d\":null}");
        assert_eq!(raw(&d, "a"), "true");
        assert_eq!(raw(&d, "b"), "42");
        assert_eq!(raw(&d, "c"), "\"hi\"");
        assert_eq!(raw(&d, "d"), "null");
    }

    #[test]
    fn parse_whitespace_escapes_nesting() {
        let d = SchematizedJson::parse_object("{ \"a\" : 1 , \"b\" : 2 }");
        assert_eq!((raw(&d, "a"), raw(&d, "b")), ("1".into(), "2".into()));
        let d = SchematizedJson::parse_object("{\"a\\\"b\":\"c\\\"d\"}");
        assert_eq!(raw(&d, "a\"b"), "\"c\\\"d\"");
        let d = SchematizedJson::parse_object("{\"a\":{\"b\":1},\"c\":2}");
        assert_eq!((raw(&d, "a"), raw(&d, "c")), ("".into(), "".into()));
    }

    #[test]
    fn render_round_trips_and_sorts() {
        let d = SchematizedJson::parse_object("{\"b\":1,\"a\":\"x\"}");
        assert_eq!(SchematizedJson::render_object(&d), "{\"a\":\"x\",\"b\":1}");
    }

    #[test]
    fn decode_string_unescapes() {
        assert_eq!(SchematizedJson::decode_string("\"a\\\"b\""), "a\"b");
        assert_eq!(SchematizedJson::decode_string("\"a\\nb\""), "a\nb");
        assert_eq!(SchematizedJson::decode_string("\"a\\\\b\""), "a\\b");
        assert_eq!(SchematizedJson::decode_string("\"a\\u003cb\""), "a<b");
        assert_eq!(SchematizedJson::decode_string("\"\\u00e9\""), "é");
    }

    #[test]
    fn read_boolean_cases() {
        let doc = "{\"on\":true,\"off\":false,\"void\":null}";
        assert!(SchematizedJson::read_boolean(doc, "on", false));
        assert!(!SchematizedJson::read_boolean(doc, "off", true));
        assert!(SchematizedJson::read_boolean(doc, "void", true));
        assert!(SchematizedJson::read_boolean(doc, "missing", true));
        assert!(!SchematizedJson::read_boolean(Value::Null, "on", false));
        assert!(SchematizedJson::read_boolean(Some(doc.to_string()), "on", false));
    }

    #[test]
    fn read_integer_string() {
        let doc = "{\"n\":42,\"neg\":-7,\"g\":\"hello\"}";
        assert_eq!(SchematizedJson::read_integer(doc, "n", 0), 42);
        assert_eq!(SchematizedJson::read_integer(doc, "neg", 0), -7);
        assert_eq!(SchematizedJson::read_integer(doc, "missing", 5), 5);
        assert_eq!(SchematizedJson::read_string(doc, "g", "hi"), "hello");
        assert_eq!(SchematizedJson::read_string(doc, "missing", "hi"), "hi");
    }

    #[test]
    fn writes() {
        let w = |v: Value| SchematizedJson::text(&v).unwrap();
        assert_eq!(w(SchematizedJson::write_boolean(Value::Null, "on", true)), "{\"on\":true}");
        assert_eq!(w(SchematizedJson::write_integer("", "n", 42)), "{\"n\":42}");
        assert_eq!(w(SchematizedJson::write_string("{}", "s", "hi")), "{\"s\":\"hi\"}");
        assert_eq!(
            w(SchematizedJson::write_boolean("{\"b\":1,\"a\":\"x\"}", "c", false)),
            "{\"a\":\"x\",\"b\":1,\"c\":false}"
        );
        assert_eq!(w(SchematizedJson::write_boolean("{\"on\":true}", "on", false)), "{\"on\":false}");
        assert_eq!(w(SchematizedJson::write_string("{}", "s", "a\"b")), "{\"s\":\"a\\\"b\"}");
    }

    #[test]
    fn write_then_read_round_trips() {
        let doc = SchematizedJson::write_string(
            SchematizedJson::write_boolean(Value::Null, "on", true),
            "s",
            "a\"b<&>",
        );
        assert!(SchematizedJson::read_boolean(doc.clone(), "on", false));
        assert_eq!(SchematizedJson::read_string(doc, "s", ""), "a\"b<&>");
    }

    #[test]
    fn assign_casts_through_schema() {
        let v = SchematizedJson::assign(
            Value::Null,
            json!({"on": "true", "n": "7", "s": 3}),
            schema(),
        );
        assert_eq!(v, json!("{\"n\":7,\"on\":true,\"s\":\"3\"}"));
    }

    #[test]
    fn assign_merges_and_zero_is_false() {
        let v = SchematizedJson::assign("{\"n\":1,\"on\":true}", json!({"on": "0"}), schema());
        assert_eq!(v, json!("{\"n\":1,\"on\":false}"));
        let v = SchematizedJson::assign("{}", json!({"on": true}), schema());
        assert_eq!(v, json!("{\"on\":true}"));
    }

    #[test]
    fn assign_passes_text_and_nil_through() {
        assert_eq!(SchematizedJson::assign(Value::Null, "{\"on\":true}", schema()), json!("{\"on\":true}"));
        assert_eq!(SchematizedJson::assign("{\"on\":true}", Value::Null, schema()), Value::Null);
    }

    #[test]
    #[should_panic(expected = "undefined method 'other='")]
    fn assign_panics_outside_schema() {
        SchematizedJson::assign(Value::Null, json!({"other": 1}), schema());
    }
}
