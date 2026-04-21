//! Ignore-rules config — what to blank out before diffing.
//!
//! The fundamental bar is "same DOM in JS/CSS", but real-world
//! pages have values that are legitimately per-request (CSRF
//! tokens) or per-build (asset fingerprints) and would otherwise
//! trigger false-positive diffs. Config rules name those values
//! and map them to canonical placeholders. With rules applied
//! symmetrically to both trees, a remaining diff means a real
//! rendering divergence.
//!
//! Two rule flavors:
//!   - `elements`: drop a whole element tree matching a selector
//!     (e.g. `<meta name="csrf-token">` in the head).
//!   - `attributes`: keep the element, but blank a specific attr
//!     (value replaced, or stripped entirely). Supports a regex
//!     match on the attribute value so we can target only
//!     fingerprinted asset URLs.
//!
//! Selectors here are a deliberately-narrow subset: tag name +
//! required attribute equalities. No nesting, no combinators.
//! Everything we need for Rails-emitted per-request noise falls
//! into this shape; a full CSS-selector dependency is deferred
//! until a rule demands it.

use std::collections::BTreeMap;

use regex::Regex;
use serde::Deserialize;

/// Top-level config. Serialized as YAML with an `ignore:` key.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub ignore: IgnoreRules,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct IgnoreRules {
    #[serde(default)]
    pub elements: Vec<ElementRule>,
    #[serde(default)]
    pub attributes: Vec<AttributeRule>,
    /// Drop HTML comments before diffing. Default true — comments
    /// aren't DOM-inspectable via most JS idioms and Rails emits
    /// IE-conditional comments that the target would need to
    /// mirror exactly for no behavioral benefit.
    #[serde(default = "default_true")]
    pub drop_comments: bool,
}

fn default_true() -> bool {
    true
}

/// Drop an entire element subtree that matches. E.g.:
/// ```yaml
/// - tag: meta
///   attrs:
///     name: csrf-token
/// ```
#[derive(Debug, Clone, Deserialize)]
pub struct ElementRule {
    pub tag: String,
    #[serde(default)]
    pub attrs: BTreeMap<String, String>,
    /// Optional — if set, replace the matched element with a
    /// placeholder text node containing this string instead of
    /// dropping. Useful when the element's presence matters but
    /// its internal state doesn't.
    #[serde(default)]
    pub replace_with: Option<String>,
}

/// Blank a single attribute on elements that match. E.g.:
/// ```yaml
/// - tag: input
///   attrs: { name: authenticity_token }
///   attribute: value
///   replace: ""
/// ```
#[derive(Debug, Clone, Deserialize)]
pub struct AttributeRule {
    pub tag: String,
    #[serde(default)]
    pub attrs: BTreeMap<String, String>,
    pub attribute: String,
    /// Regex that must match the attribute's current value for
    /// the replacement to fire. When absent, any value matches.
    /// Compiled lazily in `compile()`.
    #[serde(default)]
    pub value_regex: Option<String>,
    /// Replacement value. When None, the attribute is removed
    /// entirely.
    #[serde(default)]
    pub replace: Option<String>,
}

impl Config {
    /// The baked-in default rules — what every roundhouse app
    /// needs to mask for comparison, without a config file.
    pub fn default() -> Self {
        Self {
            ignore: IgnoreRules {
                elements: vec![
                    // CSP nonces are per-request. Rails emits them
                    // on styles/scripts; roundhouse doesn't today,
                    // and either way the nonce itself varies.
                    ElementRule {
                        tag: "meta".into(),
                        attrs: kv("name", "csp-nonce"),
                        replace_with: None,
                    },
                    // Rails' CSRF meta tag — value is per-session.
                    // Both sides emit a tag; the value shouldn't
                    // drive a diff.
                    ElementRule {
                        tag: "meta".into(),
                        attrs: kv("name", "csrf-token"),
                        replace_with: None,
                    },
                    // Same for csrf-param (the name of the CSRF
                    // form field) — technically stable across a
                    // single Rails session, but we don't verify
                    // so we drop it here for symmetry.
                    ElementRule {
                        tag: "meta".into(),
                        attrs: kv("name", "csrf-param"),
                        replace_with: None,
                    },
                ],
                attributes: vec![
                    // `<input name="authenticity_token">` hidden
                    // CSRF field — we already keep the element;
                    // just blank the per-request value.
                    AttributeRule {
                        tag: "input".into(),
                        attrs: kv("name", "authenticity_token"),
                        attribute: "value".into(),
                        value_regex: None,
                        replace: Some(String::new()),
                    },
                    // Stylesheet fingerprints: Rails appends
                    // `?v=...` (Propshaft) or `-<digest>.css`
                    // (Sprockets). Strip to the path.
                    AttributeRule {
                        tag: "link".into(),
                        attrs: kv("rel", "stylesheet"),
                        attribute: "href".into(),
                        value_regex: Some(r"\?.*$".into()),
                        replace: Some(String::new()),
                    },
                    // Script src — same logic.
                    AttributeRule {
                        tag: "script".into(),
                        attrs: BTreeMap::new(),
                        attribute: "src".into(),
                        value_regex: Some(r"\?.*$".into()),
                        replace: Some(String::new()),
                    },
                    // Turbo stream signed-stream-name: both sides
                    // emit this, but the signature portion
                    // (`--unsigned` for roundhouse; HMAC-base64 for
                    // Rails) differs while the base64'd channel
                    // name matches. Strip the `--...` suffix.
                    AttributeRule {
                        tag: "turbo-cable-stream-source".into(),
                        attrs: BTreeMap::new(),
                        attribute: "signed-stream-name".into(),
                        value_regex: Some(r"--.*$".into()),
                        replace: Some(String::new()),
                    },
                ],
                drop_comments: true,
            },
        }
    }
}

fn kv(k: &str, v: &str) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    m.insert(k.into(), v.into());
    m
}

/// Compiled form — regexes parsed once up front. `apply` consumes
/// this against a mutable DOM tree.
pub struct CompiledConfig {
    pub elements: Vec<ElementRule>,
    pub attributes: Vec<CompiledAttributeRule>,
    pub drop_comments: bool,
}

pub struct CompiledAttributeRule {
    pub rule: AttributeRule,
    pub regex: Option<Regex>,
}

impl Config {
    pub fn compile(&self) -> anyhow::Result<CompiledConfig> {
        let attributes = self
            .ignore
            .attributes
            .iter()
            .map(|r| {
                let regex = match &r.value_regex {
                    Some(pat) => Some(
                        Regex::new(pat).map_err(|e| {
                            anyhow::anyhow!("compile attr regex {pat:?}: {e}")
                        })?,
                    ),
                    None => None,
                };
                Ok(CompiledAttributeRule {
                    rule: r.clone(),
                    regex,
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        Ok(CompiledConfig {
            elements: self.ignore.elements.clone(),
            attributes,
            drop_comments: self.ignore.drop_comments,
        })
    }
}
