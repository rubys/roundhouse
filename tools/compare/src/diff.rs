//! Structural DOM tree diff.
//!
//! Walks two canonical `Node` trees in lockstep and returns the
//! first divergence with a human-readable DOM path (`html > body
//! > form > input[2]`). Not a full minimal-edit diff — the point
//! is to surface the first concrete thing that differs so a
//! developer can inspect it; making it edit-minimal would mask
//! which side introduced the regression.

use crate::dom::Node;

pub enum Outcome {
    Equal,
    Different(Divergence),
}

/// Where and how the two trees differ. `path` is the element
/// breadcrumb from the document root; `kind` names the specific
/// mismatch.
#[derive(Debug, Clone)]
pub struct Divergence {
    pub path: String,
    pub kind: DivergenceKind,
    pub reference_snippet: String,
    pub target_snippet: String,
}

#[derive(Debug, Clone)]
pub enum DivergenceKind {
    NodeKindMismatch,
    TagMismatch,
    AttributeMismatch { attr_name: String },
    AttributeSetMismatch { only_in_reference: Vec<String>, only_in_target: Vec<String> },
    TextMismatch,
    ChildCountMismatch { reference: usize, target: usize },
    DoctypeMismatch,
}

pub fn compare(reference: &Node, target: &Node) -> Outcome {
    let mut path = Vec::new();
    compare_inner(reference, target, &mut path)
}

fn compare_inner(reference: &Node, target: &Node, path: &mut Vec<String>) -> Outcome {
    match (reference, target) {
        (Node::Document { children: rc }, Node::Document { children: tc }) => {
            compare_children(rc, tc, path)
        }
        (Node::Doctype { name: rn }, Node::Doctype { name: tn }) => {
            if rn == tn {
                Outcome::Equal
            } else {
                Outcome::Different(Divergence {
                    path: format_path(path),
                    kind: DivergenceKind::DoctypeMismatch,
                    reference_snippet: format!("<!DOCTYPE {rn}>"),
                    target_snippet: format!("<!DOCTYPE {tn}>"),
                })
            }
        }
        (
            Node::Element { tag: rt, attrs: ra, children: rc },
            Node::Element { tag: tt, attrs: ta, children: tc },
        ) => {
            if rt != tt {
                return Outcome::Different(Divergence {
                    path: format_path(path),
                    kind: DivergenceKind::TagMismatch,
                    reference_snippet: format!("<{rt}>"),
                    target_snippet: format!("<{tt}>"),
                });
            }
            // Attribute comparison — ordered by key because
            // BTreeMap is. Report the first attr that differs, or
            // the symmetric-difference set if one side has keys
            // the other doesn't.
            if ra.keys().collect::<Vec<_>>() != ta.keys().collect::<Vec<_>>() {
                let only_in_reference: Vec<String> = ra
                    .keys()
                    .filter(|k| !ta.contains_key(k.as_str()))
                    .cloned()
                    .collect();
                let only_in_target: Vec<String> = ta
                    .keys()
                    .filter(|k| !ra.contains_key(k.as_str()))
                    .cloned()
                    .collect();
                return Outcome::Different(Divergence {
                    path: format_path(path),
                    kind: DivergenceKind::AttributeSetMismatch {
                        only_in_reference,
                        only_in_target,
                    },
                    reference_snippet: format_element_open(rt, ra),
                    target_snippet: format_element_open(tt, ta),
                });
            }
            for (k, rv) in ra {
                let tv = ta.get(k).expect("BTreeMap key-parity established above");
                if rv != tv {
                    return Outcome::Different(Divergence {
                        path: format_path(path),
                        kind: DivergenceKind::AttributeMismatch {
                            attr_name: k.clone(),
                        },
                        reference_snippet: format!("{k}={rv:?}"),
                        target_snippet: format!("{k}={tv:?}"),
                    });
                }
            }
            // Element tag is on the path for child descent.
            path.push(format!("{rt}"));
            let outcome = compare_children(rc, tc, path);
            path.pop();
            outcome
        }
        (Node::Text { value: rv }, Node::Text { value: tv }) => {
            if rv == tv {
                Outcome::Equal
            } else {
                Outcome::Different(Divergence {
                    path: format_path(path),
                    kind: DivergenceKind::TextMismatch,
                    reference_snippet: format_text(rv),
                    target_snippet: format_text(tv),
                })
            }
        }
        (Node::Comment { value: rv }, Node::Comment { value: tv }) => {
            if rv == tv {
                Outcome::Equal
            } else {
                Outcome::Different(Divergence {
                    path: format_path(path),
                    kind: DivergenceKind::TextMismatch,
                    reference_snippet: format!("<!--{rv}-->"),
                    target_snippet: format!("<!--{tv}-->"),
                })
            }
        }
        (r, t) => Outcome::Different(Divergence {
            path: format_path(path),
            kind: DivergenceKind::NodeKindMismatch,
            reference_snippet: node_kind_name(r).to_string(),
            target_snippet: node_kind_name(t).to_string(),
        }),
    }
}

fn compare_children(
    reference: &[Node],
    target: &[Node],
    path: &mut Vec<String>,
) -> Outcome {
    if reference.len() != target.len() {
        return Outcome::Different(Divergence {
            path: format_path(path),
            kind: DivergenceKind::ChildCountMismatch {
                reference: reference.len(),
                target: target.len(),
            },
            reference_snippet: child_summary(reference),
            target_snippet: child_summary(target),
        });
    }
    for (i, (r, t)) in reference.iter().zip(target.iter()).enumerate() {
        path.push(format!("[{i}]"));
        let outcome = compare_inner(r, t, path);
        path.pop();
        if let Outcome::Different(_) = outcome {
            return outcome;
        }
    }
    Outcome::Equal
}

fn format_path(path: &[String]) -> String {
    if path.is_empty() {
        "<document>".to_string()
    } else {
        let mut s = String::new();
        for (i, seg) in path.iter().enumerate() {
            if seg.starts_with('[') {
                s.push_str(seg);
            } else {
                if i > 0 {
                    s.push_str(" > ");
                }
                s.push_str(seg);
            }
        }
        s
    }
}

fn format_element_open(tag: &str, attrs: &std::collections::BTreeMap<String, String>) -> String {
    let mut s = String::new();
    s.push('<');
    s.push_str(tag);
    for (k, v) in attrs {
        s.push(' ');
        s.push_str(k);
        s.push_str("=\"");
        s.push_str(v);
        s.push('"');
    }
    s.push('>');
    s
}

fn format_text(s: &str) -> String {
    format!("{s:?}")
}

fn child_summary(nodes: &[Node]) -> String {
    let names: Vec<String> = nodes
        .iter()
        .map(|n| match n {
            Node::Element { tag, .. } => format!("<{tag}>"),
            Node::Text { value } if value.trim().is_empty() => "«whitespace»".into(),
            Node::Text { .. } => "«text»".into(),
            Node::Comment { .. } => "«comment»".into(),
            Node::Doctype { name } => format!("<!DOCTYPE {name}>"),
            Node::Document { .. } => "«document»".into(),
        })
        .collect();
    format!("[{}]", names.join(", "))
}

fn node_kind_name(n: &Node) -> &'static str {
    match n {
        Node::Document { .. } => "Document",
        Node::Doctype { .. } => "Doctype",
        Node::Element { .. } => "Element",
        Node::Text { .. } => "Text",
        Node::Comment { .. } => "Comment",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::dom::parse_and_canonicalize;

    fn parse(html: &str) -> Node {
        parse_and_canonicalize(html, &Config::default()).unwrap()
    }

    #[test]
    fn identical_trees_are_equal() {
        let a = parse("<html><body><p>Hi</p></body></html>");
        let b = parse("<html><body><p>Hi</p></body></html>");
        assert!(matches!(compare(&a, &b), Outcome::Equal));
    }

    #[test]
    fn tag_mismatch_reports_path() {
        let a = parse("<html><body><p>Hi</p></body></html>");
        let b = parse("<html><body><div>Hi</div></body></html>");
        let Outcome::Different(div) = compare(&a, &b) else {
            panic!("expected divergence");
        };
        assert!(matches!(div.kind, DivergenceKind::TagMismatch));
        assert!(div.path.contains("body"));
    }

    #[test]
    fn text_whitespace_mismatch_surfaces() {
        let a = parse("<html><body><p>Hi</p></body></html>");
        let b = parse("<html><body><p>Hi </p></body></html>");
        let Outcome::Different(div) = compare(&a, &b) else {
            panic!("expected divergence");
        };
        assert!(matches!(div.kind, DivergenceKind::TextMismatch));
    }

    #[test]
    fn attribute_value_mismatch_surfaces() {
        let a = parse("<html><body><a href=\"/foo\">x</a></body></html>");
        let b = parse("<html><body><a href=\"/bar\">x</a></body></html>");
        let Outcome::Different(div) = compare(&a, &b) else {
            panic!("expected divergence");
        };
        match div.kind {
            DivergenceKind::AttributeMismatch { attr_name } => {
                assert_eq!(attr_name, "href");
            }
            _ => panic!("expected attribute mismatch"),
        }
    }
}
