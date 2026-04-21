//! Parse an HTML string into a canonical immutable tree and apply
//! ignore rules.
//!
//! The canonical shape is deliberately small: just enough to
//! compare two HTML documents for DOM equivalence. Elements hold
//! a sorted-by-key attribute map (so order-of-insertion doesn't
//! leak into comparison); text nodes are verbatim strings (so
//! whitespace is preserved bit-for-bit, per the high-bar compat
//! target); documents hold a flat child list.
//!
//! Parsing delegates to `html5ever` + `markup5ever_rcdom::RcDom`
//! — the same parser Servo uses, which handles the full HTML5
//! spec's quirks (implicit `<html>` / `<head>` / `<body>` wrapping,
//! whitespace text node preservation in block contexts, etc.).
//! After parsing, we walk the RcDom tree, apply the ignore rules,
//! and emit our immutable `Node` tree.

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use html5ever::driver::parse_document;
use html5ever::tendril::TendrilSink;
use markup5ever_rcdom::{Handle, NodeData, RcDom};

use crate::config::{CompiledConfig, Config, ElementRule};

/// Canonical immutable DOM node. Elements carry a sorted attr map
/// so attribute-order differences don't drive a false-positive
/// diff; text nodes carry their string verbatim so whitespace
/// divergences DO drive one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    Document {
        children: Vec<Node>,
    },
    Doctype {
        name: String,
    },
    Element {
        tag: String,
        attrs: BTreeMap<String, String>,
        children: Vec<Node>,
    },
    Text {
        value: String,
    },
    Comment {
        value: String,
    },
}

pub fn parse_and_canonicalize(html: &str, config: &Config) -> Result<Node> {
    let compiled = config.compile().context("compile ignore rules")?;
    let dom = parse_document(RcDom::default(), Default::default())
        .from_utf8()
        .read_from(&mut html.as_bytes())
        .context("html5ever parse")?;
    Ok(convert(&dom.document, &compiled))
}

fn convert(handle: &Handle, config: &CompiledConfig) -> Node {
    let node = handle.clone();
    let data = &node.data;
    match data {
        NodeData::Document => Node::Document {
            children: convert_children(handle, config),
        },
        NodeData::Doctype { name, .. } => Node::Doctype {
            name: name.to_string(),
        },
        NodeData::Text { contents } => Node::Text {
            value: contents.borrow().to_string(),
        },
        NodeData::Comment { contents } => Node::Comment {
            value: contents.to_string(),
        },
        NodeData::Element { name, attrs, .. } => {
            let tag = name.local.to_string();
            let mut attr_map: BTreeMap<String, String> = attrs
                .borrow()
                .iter()
                .map(|a| (a.name.local.to_string(), a.value.to_string()))
                .collect();

            // Element-level ignore: if any element rule matches,
            // emit nothing (drop the subtree) or emit a placeholder
            // text node with the configured text.
            for rule in &config.elements {
                if element_matches(&tag, &attr_map, rule) {
                    return match &rule.replace_with {
                        Some(placeholder) => Node::Text {
                            value: placeholder.clone(),
                        },
                        None => Node::Text {
                            value: String::new(),
                        },
                    };
                }
            }

            // Attribute-level ignore: blank or remove specific
            // attributes that match.
            for rule in &config.attributes {
                if !element_matches_simple(&tag, &attr_map, &rule.rule.tag, &rule.rule.attrs) {
                    continue;
                }
                let Some(current) = attr_map.get(&rule.rule.attribute).cloned() else {
                    continue;
                };
                if let Some(rx) = &rule.regex {
                    if !rx.is_match(&current) {
                        continue;
                    }
                    let replaced = match &rule.rule.replace {
                        Some(r) => rx.replace_all(&current, r.as_str()).to_string(),
                        None => rx.replace_all(&current, "").to_string(),
                    };
                    if rule.rule.replace.is_none() && replaced == current {
                        // Regex-match with no explicit replacement
                        // means "strip whatever matched" — if the
                        // replacement left the value unchanged, the
                        // regex didn't actually cut anything, so
                        // there's nothing to do.
                    }
                    attr_map.insert(rule.rule.attribute.clone(), replaced);
                } else {
                    match &rule.rule.replace {
                        Some(r) => {
                            attr_map.insert(rule.rule.attribute.clone(), r.clone());
                        }
                        None => {
                            attr_map.remove(&rule.rule.attribute);
                        }
                    }
                }
            }

            let mut children = convert_children(handle, config);

            // Text-content rewrite rules — apply regex
            // substitutions to any direct-child Text node when the
            // containing element matches a rule's selector. Scoped
            // to immediate children so nested elements aren't
            // accidentally affected. Used for things like the
            // fingerprint-bearing importmap JSON inside `<script
            // type="importmap">`.
            for rule in &config.texts {
                if !element_matches_simple(&tag, &attr_map, &rule.rule.tag, &rule.rule.attrs)
                {
                    continue;
                }
                for child in children.iter_mut() {
                    if let Node::Text { value } = child {
                        *value = rule
                            .regex
                            .replace_all(value, rule.rule.replace.as_str())
                            .to_string();
                    }
                }
            }

            Node::Element {
                tag,
                attrs: attr_map,
                children,
            }
        }
        NodeData::ProcessingInstruction { .. } => Node::Text {
            value: String::new(),
        },
    }
}

fn convert_children(handle: &Handle, config: &CompiledConfig) -> Vec<Node> {
    let mut out: Vec<Node> = Vec::new();
    for child in handle.children.borrow().iter() {
        let node = convert(child, config);
        // Post-filter comments if configured to drop them.
        if config.drop_comments
            && matches!(node, Node::Comment { .. })
        {
            continue;
        }
        // Element-level ignore rules collapse the element to a
        // Text node (see `convert`). When such a replacement
        // produces an empty string — the common case, because we
        // mask elements like `<meta csrf-token>` entirely — drop
        // it so it doesn't show up as a stray empty text sibling.
        if let Node::Text { value } = &node {
            if value.is_empty() {
                continue;
            }
        }
        // Merge adjacent text nodes. Dropping comments (and
        // masked elements, per the rule above) can leave two text
        // siblings that were separated by the dropped node; for
        // structural equivalence we want them to compare as one.
        if let (Some(Node::Text { value: tail }), Node::Text { value: add }) =
            (out.last_mut(), &node)
        {
            tail.push_str(add);
            continue;
        }
        out.push(node);
    }
    out
}

fn element_matches(
    tag: &str,
    attrs: &BTreeMap<String, String>,
    rule: &ElementRule,
) -> bool {
    element_matches_simple(tag, attrs, &rule.tag, &rule.attrs)
}

fn element_matches_simple(
    tag: &str,
    attrs: &BTreeMap<String, String>,
    match_tag: &str,
    match_attrs: &BTreeMap<String, String>,
) -> bool {
    if tag != match_tag {
        return false;
    }
    for (k, v) in match_attrs {
        match attrs.get(k) {
            Some(val) if val == v => {}
            _ => return false,
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attribute_order_is_canonicalized() {
        let cfg = Config::default();
        let a = parse_and_canonicalize(
            "<html><body><input type=\"text\" name=\"q\"></body></html>",
            &cfg,
        )
        .unwrap();
        let b = parse_and_canonicalize(
            "<html><body><input name=\"q\" type=\"text\"></body></html>",
            &cfg,
        )
        .unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn whitespace_differences_are_significant() {
        let cfg = Config::default();
        let a = parse_and_canonicalize("<html><body><p>Hi</p></body></html>", &cfg).unwrap();
        let b = parse_and_canonicalize("<html><body><p>Hi </p></body></html>", &cfg).unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn csrf_meta_is_dropped() {
        let cfg = Config::default();
        let a = parse_and_canonicalize(
            "<html><head><meta name=\"csrf-token\" content=\"ABC\"></head><body></body></html>",
            &cfg,
        )
        .unwrap();
        let b = parse_and_canonicalize(
            "<html><head><meta name=\"csrf-token\" content=\"XYZ\"></head><body></body></html>",
            &cfg,
        )
        .unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn authenticity_token_value_is_blanked() {
        let cfg = Config::default();
        let a = parse_and_canonicalize(
            "<html><body><form><input type=\"hidden\" name=\"authenticity_token\" value=\"aaa\"></form></body></html>",
            &cfg,
        )
        .unwrap();
        let b = parse_and_canonicalize(
            "<html><body><form><input type=\"hidden\" name=\"authenticity_token\" value=\"bbb\"></form></body></html>",
            &cfg,
        )
        .unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn turbo_stream_signature_is_stripped() {
        let cfg = Config::default();
        let a = parse_and_canonicalize(
            "<html><body><turbo-cable-stream-source signed-stream-name=\"ABC--unsigned\"></turbo-cable-stream-source></body></html>",
            &cfg,
        )
        .unwrap();
        let b = parse_and_canonicalize(
            "<html><body><turbo-cable-stream-source signed-stream-name=\"ABC--HMACgoeshere\"></turbo-cable-stream-source></body></html>",
            &cfg,
        )
        .unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn stylesheet_fingerprint_query_is_stripped() {
        let cfg = Config::default();
        let a = parse_and_canonicalize(
            "<html><head><link rel=\"stylesheet\" href=\"/app.css?v=abc123\"></head></html>",
            &cfg,
        )
        .unwrap();
        let b = parse_and_canonicalize(
            "<html><head><link rel=\"stylesheet\" href=\"/app.css?v=xyz789\"></head></html>",
            &cfg,
        )
        .unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn different_tags_diverge() {
        let cfg = Config::default();
        let a = parse_and_canonicalize("<html><body><p>X</p></body></html>", &cfg).unwrap();
        let b = parse_and_canonicalize("<html><body><div>X</div></body></html>", &cfg).unwrap();
        assert_ne!(a, b);
    }
}
