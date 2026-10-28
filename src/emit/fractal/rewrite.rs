//! Applying passes' proposals to a file, with the checks every pass
//! inherits (see [`super::pass`]).

use serde::Serialize;

use super::pass::{Edit, Proposal, Unit};

/// One edit the driver applied.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Rewrite {
    /// `path:line:col` in the app's source (and, lines being kept, in the
    /// compiled tree).
    pub at: String,
    pub pass: String,
    pub from: String,
    pub to: String,
}

/// A site a pass, or the driver on its behalf, left alone.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Decline {
    pub at: String,
    pub pass: String,
    pub reason: String,
}

/// The outcome for one file.
pub struct Outcome {
    /// The rewritten text, when at least one edit survived.
    pub text: Option<String>,
    pub rewrites: Vec<Rewrite>,
    pub declined: Vec<Decline>,
}

/// Apply `proposals` (in pass order) to `unit`. A proposal is taken
/// whole or not at all: an edit that falls outside the file, overlaps an
/// earlier edit, or changes the number of lines refuses the
/// proposal, and so does a result that parses with more errors or
/// warnings than the file before it.
pub fn apply(unit: &Unit<'_>, proposals: Vec<(&'static str, Proposal)>) -> Outcome {
    let locate = |offset: u32| {
        let mut at = (offset as usize).min(unit.text.len());
        while !unit.text.is_char_boundary(at) {
            at -= 1;
        }
        let before = &unit.text[..at];
        let line = before.matches('\n').count() + 1;
        let column = before.len() - before.rfind('\n').map_or(0, |newline| newline + 1) + 1;
        format!("{}:{line}:{column}", unit.path)
    };
    let (base_errors, base_warnings) = diagnostics(unit.text);
    let mut accepted: Vec<(&'static str, Edit)> = Vec::new();
    let mut declined = Vec::new();
    for (pass, proposal) in proposals {
        for (span, reason) in proposal.declined {
            declined.push(Decline { at: locate(span.start), pass: pass.to_string(), reason });
        }
        if proposal.edits.is_empty() {
            continue;
        }
        let refusal = proposal.edits.iter().enumerate().find_map(|(index, edit)| {
            refuse(unit, edit, &proposal.edits[..index], accepted.iter().map(|(_, accepted_edit)| accepted_edit)).map(|reason| (edit.span.start, reason))
        });
        if let Some((at, reason)) = refusal {
            declined.push(Decline { at: locate(at), pass: pass.to_string(), reason });
            continue;
        }
        let mut trial: Vec<&Edit> = accepted.iter().map(|(_, accepted_edit)| accepted_edit).collect();
        trial.extend(proposal.edits.iter());
        let text = splice(unit.text, &trial);
        let (errors, warnings) = diagnostics(&text);
        if errors > base_errors || warnings > base_warnings {
            let problem = if errors > base_errors { "a parse error" } else { "a parse warning" };
            declined.push(Decline {
                at: locate(proposal.edits[0].span.start),
                pass: pass.to_string(),
                reason: format!("the rewritten file has {problem} the original does not"),
            });
            continue;
        }
        accepted.extend(proposal.edits.into_iter().map(|edit| (pass, edit)));
    }
    if accepted.is_empty() {
        return Outcome { text: None, rewrites: Vec::new(), declined };
    }
    accepted.sort_by_key(|(_, edit)| edit.span.start);
    let rewrites = accepted
        .iter()
        .map(|(pass, edit)| Rewrite {
            at: locate(edit.span.start),
            pass: pass.to_string(),
            from: unit.text[edit.span.start as usize..edit.span.end as usize].to_string(),
            to: edit.text.clone(),
        })
        .collect();
    let edits: Vec<&Edit> = accepted.iter().map(|(_, edit)| edit).collect();
    Outcome { text: Some(splice(unit.text, &edits)), rewrites, declined }
}

fn refuse<'edits>(unit: &Unit<'_>, edit: &Edit, earlier: &[Edit], accepted: impl Iterator<Item = &'edits Edit>) -> Option<String> {
    let (start, end) = (edit.span.start as usize, edit.span.end as usize);
    if edit.span.file != unit.file || start > end || end > unit.text.len() || !unit.text.is_char_boundary(start) || !unit.text.is_char_boundary(end) {
        return Some("an edit falls outside the file".to_string());
    }
    if unit.text[start..end].matches('\n').count() != edit.text.matches('\n').count() {
        return Some("an edit changes the number of lines, which would move every line after it".to_string());
    }
    let overlaps = |other: &Edit| (other.span.start < edit.span.end && edit.span.start < other.span.end) || other.span == edit.span;
    if earlier.iter().any(overlaps) || accepted.into_iter().any(overlaps) {
        return Some("an edit overlaps another edit".to_string());
    }
    None
}

/// `text` with `edits` (non-overlapping) applied.
fn splice(text: &str, edits: &[&Edit]) -> String {
    let mut sorted: Vec<&&Edit> = edits.iter().collect();
    sorted.sort_by_key(|edit| std::cmp::Reverse((edit.span.start, edit.span.end)));
    let mut spliced = text.to_string();
    for edit in sorted {
        spliced.replace_range(edit.span.start as usize..edit.span.end as usize, &edit.text);
    }
    spliced
}

/// (errors, warnings) prism reports for `text`.
fn diagnostics(text: &str) -> (usize, usize) {
    let parsed = ruby_prism::parse(text.as_bytes());
    (parsed.errors().count(), parsed.warnings().count())
}
