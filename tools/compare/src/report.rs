//! Pretty-print comparison failures.
//!
//! Two levels of output:
//!   - inline per-path line (PASS/FAIL, printed as each URL finishes)
//!   - post-run failure block per divergence, with path + kind +
//!     short excerpts of both sides
//!
//! Verbose mode additionally prints a line-by-line unified diff of
//! the full response bodies — useful when the tree diff points at
//! a text mismatch that's easier to inspect in context.

use similar::{ChangeTag, TextDiff};

use crate::diff::{Divergence, DivergenceKind};

pub struct Failure {
    pub path: String,
    pub kind: FailureKind,
    pub reference_body: String,
    pub target_body: String,
}

pub enum FailureKind {
    Status { reference: u16, target: u16 },
    Dom(Divergence),
}

pub fn print_failure(f: &Failure, verbose: bool) {
    println!("\x1b[31m✗ {}\x1b[0m", f.path);
    match &f.kind {
        FailureKind::Status { reference, target } => {
            println!("  status differs: reference={reference} target={target}");
        }
        FailureKind::Dom(div) => {
            println!("  at  {}", div.path);
            println!("  why {}", describe_kind(&div.kind));
            println!("  ref {}", div.reference_snippet);
            println!("  tgt {}", div.target_snippet);
        }
    }
    if verbose {
        println!();
        println!("  --- full body diff ---");
        print_body_diff(&f.reference_body, &f.target_body);
    }
    println!();
}

fn describe_kind(kind: &DivergenceKind) -> String {
    match kind {
        DivergenceKind::NodeKindMismatch => "node type differs".into(),
        DivergenceKind::TagMismatch => "element tag differs".into(),
        DivergenceKind::AttributeMismatch { attr_name } => {
            format!("attribute {attr_name:?} differs")
        }
        DivergenceKind::AttributeSetMismatch {
            only_in_reference,
            only_in_target,
        } => format!(
            "attribute set differs (ref-only: {:?}, target-only: {:?})",
            only_in_reference, only_in_target,
        ),
        DivergenceKind::TextMismatch => "text content differs".into(),
        DivergenceKind::ChildCountMismatch { reference, target } => {
            format!("child count differs (ref={reference}, target={target})")
        }
        DivergenceKind::DoctypeMismatch => "doctype differs".into(),
    }
}

fn print_body_diff(reference: &str, target: &str) {
    let diff = TextDiff::from_lines(reference, target);
    for change in diff.iter_all_changes() {
        let (sign, color) = match change.tag() {
            ChangeTag::Delete => ("-", "\x1b[31m"),
            ChangeTag::Insert => ("+", "\x1b[32m"),
            ChangeTag::Equal => continue,
        };
        let reset = "\x1b[0m";
        print!("  {color}{sign} {}{reset}", change);
    }
}
