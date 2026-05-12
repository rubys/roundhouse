//! Structural JSON value diff for `.json` responses.
//!
//! Parses both sides with `serde_json` and walks the resulting
//! `Value` trees in lockstep. Mirrors `diff.rs`'s shape (Equal vs
//! first-divergence-with-path) so report.rs can render both
//! flavors uniformly. The point is the same: surface the first
//! concrete thing that differs so a developer can inspect it.
//!
//! Symmetric canonicalization passes apply before structural
//! compare to mask known per-request / per-build noise, analogous
//! to the CSRF / asset-fingerprint stripping the HTML side does
//! via config.rs's `Compiled*Rule` set. For JSON the dominant
//! source of legitimate noise is ISO 8601 timestamps: Rails emits
//! microsecond precision; roundhouse's `encode_datetime` truncates
//! to milliseconds (runtime/ruby/json_builder.rb:70). Rather than
//! force one side to chase the other's precision, the comparator
//! truncates ISO-8601-looking fractional seconds to a fixed
//! 3-digit width on both sides before string compare.

use serde_json::Value;

pub enum JsonOutcome {
    Equal,
    Different(JsonDivergence),
}

/// Where and how the two JSON values diverge. `path` is a
/// JSON-pointer-ish breadcrumb from the root (`$.articles[2].title`
/// for "the third article's title"); `kind` names the specific
/// mismatch.
#[derive(Debug, Clone)]
pub struct JsonDivergence {
    pub path: String,
    pub kind: JsonDivergenceKind,
    pub reference_snippet: String,
    pub target_snippet: String,
}

#[derive(Debug, Clone)]
pub enum JsonDivergenceKind {
    /// One side is e.g. an object, the other an array (or scalar).
    TypeMismatch { reference: &'static str, target: &'static str },
    /// Same shape, different scalar value.
    ValueMismatch,
    /// Arrays of different lengths.
    ArrayLengthMismatch { reference: usize, target: usize },
    /// Object key sets differ. Reported once with both sides'
    /// exclusive keys so the developer doesn't have to chase missing
    /// keys one at a time.
    ObjectKeysMismatch { only_in_reference: Vec<String>, only_in_target: Vec<String> },
    /// Either side failed to parse as JSON. Carried as a kind
    /// rather than a separate Result so the report path is
    /// uniform.
    ParseError { side: &'static str, message: String },
}

pub fn compare_json(reference_body: &str, target_body: &str) -> JsonOutcome {
    let reference = match serde_json::from_str::<Value>(reference_body) {
        Ok(v) => v,
        Err(e) => {
            return JsonOutcome::Different(JsonDivergence {
                path: "$".into(),
                kind: JsonDivergenceKind::ParseError {
                    side: "reference",
                    message: e.to_string(),
                },
                reference_snippet: snippet(reference_body),
                target_snippet: snippet(target_body),
            });
        }
    };
    let target = match serde_json::from_str::<Value>(target_body) {
        Ok(v) => v,
        Err(e) => {
            return JsonOutcome::Different(JsonDivergence {
                path: "$".into(),
                kind: JsonDivergenceKind::ParseError {
                    side: "target",
                    message: e.to_string(),
                },
                reference_snippet: snippet(reference_body),
                target_snippet: snippet(target_body),
            });
        }
    };
    let mut path = vec!["$".to_string()];
    compare_values(&reference, &target, &mut path)
}

fn compare_values(reference: &Value, target: &Value, path: &mut Vec<String>) -> JsonOutcome {
    match (reference, target) {
        (Value::Null, Value::Null) => JsonOutcome::Equal,
        (Value::Bool(r), Value::Bool(t)) => {
            if r == t {
                JsonOutcome::Equal
            } else {
                value_mismatch(path, &reference.to_string(), &target.to_string())
            }
        }
        (Value::Number(r), Value::Number(t)) => {
            // Number compare is by JSON textual form — `30` and
            // `30.0` are distinguishable in serde_json::Number, and
            // Rails / roundhouse should agree on which Ruby type
            // produced the value. If a real benchmark hits drift
            // here (e.g. Float vs Integer for a counter), add a
            // canonicalization step then.
            if r == t {
                JsonOutcome::Equal
            } else {
                value_mismatch(path, &r.to_string(), &t.to_string())
            }
        }
        (Value::String(r), Value::String(t)) => {
            let rc = canon_string(r);
            let tc = canon_string(t);
            if rc == tc {
                JsonOutcome::Equal
            } else {
                value_mismatch(path, &format!("{r:?}"), &format!("{t:?}"))
            }
        }
        (Value::Array(rv), Value::Array(tv)) => {
            if rv.len() != tv.len() {
                return JsonOutcome::Different(JsonDivergence {
                    path: format_path(path),
                    kind: JsonDivergenceKind::ArrayLengthMismatch {
                        reference: rv.len(),
                        target: tv.len(),
                    },
                    reference_snippet: format!("array(len={})", rv.len()),
                    target_snippet: format!("array(len={})", tv.len()),
                });
            }
            for (i, (rc, tc)) in rv.iter().zip(tv.iter()).enumerate() {
                path.push(format!("[{i}]"));
                let result = compare_values(rc, tc, path);
                path.pop();
                if let JsonOutcome::Different(_) = result {
                    return result;
                }
            }
            JsonOutcome::Equal
        }
        (Value::Object(rm), Value::Object(tm)) => {
            if rm.len() != tm.len() || !rm.keys().all(|k| tm.contains_key(k)) {
                let only_in_reference: Vec<String> = rm
                    .keys()
                    .filter(|k| !tm.contains_key(k.as_str()))
                    .cloned()
                    .collect();
                let only_in_target: Vec<String> = tm
                    .keys()
                    .filter(|k| !rm.contains_key(k.as_str()))
                    .cloned()
                    .collect();
                if !only_in_reference.is_empty() || !only_in_target.is_empty() {
                    return JsonOutcome::Different(JsonDivergence {
                        path: format_path(path),
                        kind: JsonDivergenceKind::ObjectKeysMismatch {
                            only_in_reference,
                            only_in_target,
                        },
                        reference_snippet: format!(
                            "object(keys={:?})",
                            rm.keys().collect::<Vec<_>>()
                        ),
                        target_snippet: format!(
                            "object(keys={:?})",
                            tm.keys().collect::<Vec<_>>()
                        ),
                    });
                }
            }
            // Walk in reference key order — deterministic and
            // matches the order json_builder.rb emits.
            for (k, rv) in rm {
                let tv = tm.get(k).expect("key parity established above");
                path.push(format!(".{k}"));
                let result = compare_values(rv, tv, path);
                path.pop();
                if let JsonOutcome::Different(_) = result {
                    return result;
                }
            }
            JsonOutcome::Equal
        }
        _ => JsonOutcome::Different(JsonDivergence {
            path: format_path(path),
            kind: JsonDivergenceKind::TypeMismatch {
                reference: type_name(reference),
                target: type_name(target),
            },
            reference_snippet: snippet(&reference.to_string()),
            target_snippet: snippet(&target.to_string()),
        }),
    }
}

fn value_mismatch(path: &[String], reference: &str, target: &str) -> JsonOutcome {
    JsonOutcome::Different(JsonDivergence {
        path: format_path(path),
        kind: JsonDivergenceKind::ValueMismatch,
        reference_snippet: reference.to_string(),
        target_snippet: target.to_string(),
    })
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn format_path(path: &[String]) -> String {
    let mut out = String::new();
    for seg in path {
        if !out.is_empty() && !seg.starts_with('[') {
            // `.foo` already includes its own leading dot; `[0]`
            // attaches directly. Avoid double dots.
        }
        out.push_str(seg);
    }
    out
}

fn snippet(s: &str) -> String {
    const MAX: usize = 200;
    if s.len() <= MAX {
        s.to_string()
    } else {
        format!("{}…", &s[..MAX])
    }
}

/// Canonicalize a JSON string value before compare. Two passes:
///
/// 1. **ISO 8601 fractional-seconds truncation.** Rails emits
///    `2026-05-10T02:22:28.114670Z`; roundhouse's `encode_datetime`
///    truncates to `2026-05-10T02:22:28.114Z`. Both normalize to
///    millisecond precision so byte-compare succeeds.
/// 2. **Absolute-URL host strip.** Rails renders self-links as
///    `http://localhost:4000/articles/1.json`; roundhouse renders
///    them as `/articles/1.json` (the lowerer emits path-only,
///    on purpose — host plumbing is per-request and per-deployment
///    noise the same way CSRF tokens are per-session noise on the
///    HTML side). Both normalize to the path form.
///
/// Conservatively narrow on both: each pass only fires on strings
/// that match its specific shape, so unrelated text passes
/// through unchanged.
fn canon_string(s: &str) -> String {
    let s = canonicalize_iso8601(s).unwrap_or_else(|| s.to_string());
    canonicalize_absolute_url(&s).unwrap_or(s)
}

/// Returns `Some` only when `s` matches the ISO 8601 datetime
/// shape with a fractional-seconds component (the case that needs
/// canonicalizing). All other strings — including timestamps
/// without fractional parts, plain dates, or unrelated text —
/// fall through to identity compare.
fn canonicalize_iso8601(s: &str) -> Option<String> {
    // Cheap shape gate before the regex-equivalent walk: must be
    // at least `YYYY-MM-DDTHH:MM:SS.f` and contain `T` at index 10
    // and `.` at index 19.
    let bytes = s.as_bytes();
    if bytes.len() < 21 || bytes[10] != b'T' || bytes[19] != b'.' {
        return None;
    }
    // Date segment digits + dashes.
    for (i, &b) in bytes[..10].iter().enumerate() {
        let ok = match i {
            4 | 7 => b == b'-',
            _ => b.is_ascii_digit(),
        };
        if !ok {
            return None;
        }
    }
    // Time segment digits + colons (HH:MM:SS).
    for (i, &b) in bytes[11..19].iter().enumerate() {
        let ok = match i {
            2 | 5 => b == b':',
            _ => b.is_ascii_digit(),
        };
        if !ok {
            return None;
        }
    }
    // Walk the fractional digits.
    let mut frac_end = 20;
    while frac_end < bytes.len() && bytes[frac_end].is_ascii_digit() {
        frac_end += 1;
    }
    if frac_end == 20 {
        return None;
    }
    // Truncate or zero-pad to 3 digits.
    let frac = &s[20..frac_end];
    let mut canon_frac = String::with_capacity(3);
    for c in frac.chars().take(3) {
        canon_frac.push(c);
    }
    while canon_frac.len() < 3 {
        canon_frac.push('0');
    }
    // Whatever's after the fractional digits (timezone marker —
    // typically `Z` or `±HH:MM` — or end-of-string) appends
    // verbatim. Mismatches there will still surface as a string
    // diff.
    let tail = &s[frac_end..];
    Some(format!("{}.{}{}", &s[..19], canon_frac, tail))
}

/// Strip `https?://<host>` prefix from string values that look
/// like absolute URLs. Returns `Some(canonicalized)` only when the
/// input starts with `http://` or `https://` and contains a `/`
/// after the host portion (so the result is a non-empty path).
/// Plain hostnames, mailto: URLs, or strings that merely contain
/// `http://` mid-text pass through unchanged.
fn canonicalize_absolute_url(s: &str) -> Option<String> {
    let after_scheme = if let Some(rest) = s.strip_prefix("https://") {
        rest
    } else if let Some(rest) = s.strip_prefix("http://") {
        rest
    } else {
        return None;
    };
    // First `/` ends the authority component. No `/` → bare host,
    // we don't canonicalize (the comparator would lose info).
    let slash = after_scheme.find('/')?;
    let host = &after_scheme[..slash];
    if host.is_empty() {
        return None;
    }
    // Reject hosts that contain whitespace — they're almost
    // certainly false-positives, not real URLs.
    if host.chars().any(|c| c.is_whitespace()) {
        return None;
    }
    Some(after_scheme[slash..].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso8601_truncates_microseconds_to_milliseconds() {
        assert_eq!(
            canonicalize_iso8601("2026-05-10T02:22:28.114670Z").as_deref(),
            Some("2026-05-10T02:22:28.114Z"),
        );
    }

    #[test]
    fn iso8601_pads_under_three_digits() {
        assert_eq!(
            canonicalize_iso8601("2026-05-10T02:22:28.1Z").as_deref(),
            Some("2026-05-10T02:22:28.100Z"),
        );
    }

    #[test]
    fn iso8601_leaves_unrelated_strings_alone() {
        assert!(canonicalize_iso8601("not a timestamp").is_none());
        assert!(canonicalize_iso8601("2026-05-10").is_none());
        assert!(canonicalize_iso8601("2026-05-10T02:22:28Z").is_none()); // no fraction
    }

    #[test]
    fn equal_objects_compare_equal() {
        let r = r#"{"id":1,"title":"x"}"#;
        let t = r#"{"id":1,"title":"x"}"#;
        assert!(matches!(compare_json(r, t), JsonOutcome::Equal));
    }

    #[test]
    fn datetime_microsecond_vs_millisecond_compares_equal() {
        let r = r#"{"created_at":"2026-05-10T02:22:28.114670Z"}"#;
        let t = r#"{"created_at":"2026-05-10T02:22:28.114Z"}"#;
        assert!(matches!(compare_json(r, t), JsonOutcome::Equal));
    }

    #[test]
    fn absolute_url_host_strips_to_path() {
        assert_eq!(
            canonicalize_absolute_url("http://localhost:4000/articles/1.json").as_deref(),
            Some("/articles/1.json"),
        );
        assert_eq!(
            canonicalize_absolute_url("https://example.com/foo/bar").as_deref(),
            Some("/foo/bar"),
        );
    }

    #[test]
    fn url_canonicalization_leaves_non_urls_alone() {
        assert!(canonicalize_absolute_url("hello world").is_none());
        assert!(canonicalize_absolute_url("http://localhost").is_none()); // bare host
        assert!(canonicalize_absolute_url("see http://x.com/").is_none()); // not a prefix
    }

    #[test]
    fn rails_absolute_url_compares_equal_to_path() {
        let r = r#"{"url":"http://localhost:4000/articles/1.json"}"#;
        let t = r#"{"url":"/articles/1.json"}"#;
        assert!(matches!(compare_json(r, t), JsonOutcome::Equal));
    }

    #[test]
    fn array_length_mismatch_reports_lengths() {
        let r = r#"[1, 2, 3]"#;
        let t = r#"[1, 2]"#;
        let outcome = compare_json(r, t);
        match outcome {
            JsonOutcome::Different(d) => {
                assert!(matches!(
                    d.kind,
                    JsonDivergenceKind::ArrayLengthMismatch {
                        reference: 3,
                        target: 2,
                    },
                ));
                assert_eq!(d.path, "$");
            }
            _ => panic!("expected length mismatch"),
        }
    }

    #[test]
    fn nested_value_mismatch_reports_path() {
        let r = r#"{"articles":[{"id":1,"title":"x"},{"id":2,"title":"y"}]}"#;
        let t = r#"{"articles":[{"id":1,"title":"x"},{"id":2,"title":"DIFFERENT"}]}"#;
        let outcome = compare_json(r, t);
        match outcome {
            JsonOutcome::Different(d) => {
                assert!(matches!(d.kind, JsonDivergenceKind::ValueMismatch));
                assert_eq!(d.path, "$.articles[1].title");
            }
            _ => panic!("expected value mismatch"),
        }
    }

    #[test]
    fn object_keys_mismatch_reports_both_sides() {
        let r = r#"{"id":1,"title":"x"}"#;
        let t = r#"{"id":1,"name":"x"}"#;
        let outcome = compare_json(r, t);
        match outcome {
            JsonOutcome::Different(d) => match d.kind {
                JsonDivergenceKind::ObjectKeysMismatch {
                    only_in_reference,
                    only_in_target,
                } => {
                    assert_eq!(only_in_reference, vec!["title".to_string()]);
                    assert_eq!(only_in_target, vec!["name".to_string()]);
                }
                _ => panic!("expected keys mismatch"),
            },
            _ => panic!("expected divergence"),
        }
    }
}
