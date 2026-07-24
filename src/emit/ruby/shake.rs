//! Text-level tree shake for the ruby-family emit trees (spinel /
//! ruby / jruby).
//!
//! The other targets shake in IR: `runtime_loader` parses the runtime
//! into `LibraryClass`es and `treeshake::filter_runtime_class` drops
//! unreachable methods before emit. The ruby-family trees instead ship
//! `runtime/ruby/*.rb` verbatim (plus `.rbs` sidecars), so this module
//! shakes the finished file set as TEXT: a `def` is dropped only when
//! its method name appears nowhere else in the whole tree — not in any
//! app file, runtime file, scaffold file, test, or comment.
//!
//! Name-level is deliberately the same precision the IR shake measured
//! it needs: on both fixtures the class-precise pass kept ~nothing that
//! name-level would drop (the `name_reachable` fallback rescued 3
//! methods on the blog, 0 on lobsters). Text-level trades those few
//! for uniformity: one mechanism covers the verbatim runtime, the
//! synthesized model surface, and the `.rbs` sidecars, with no IR
//! plumbing through the emitters. Anything textual counts as usage —
//! a send, a symbol, a `send("...")` string, a comment — so the pass
//! only ever keeps too much, never too little.
//!
//! Shaken files: the four framework runtime dirs
//! (`runtime/{active_record,action_view,action_controller,
//! action_dispatch}/`) and the top-level framework stems (every def is
//! a candidate), plus `app/models/*.rb` (only the lowerer-synthesized
//! shakeable names — presence/dirty predicates, `update!` — are
//! candidates, so user methods can't be touched). Everything else
//! (tep transport, spinel shims, scaffold, tests) is scanned for
//! usages but never edited.
//!
//! Runs to a fixed point: dropping a body removes its calls, which can
//! orphan further defs on the next pass.
//!
//! Kill switch: `ROUNDHOUSE_NO_TREESHAKE=1` skips the pass entirely.

use std::collections::{HashMap, HashSet};

/// Methods Ruby (or the runtime idiom) dispatches without a textual
/// call site: constructors via `.new`, `to_s` via interpolation,
/// `to_a` via splat, `each` via `for`, `call` via `.()`, the
/// method_missing pair via the dispatch protocol. Never candidates.
const EXEMPT: &[&str] = &[
    "initialize",
    "to_s",
    "to_str",
    "to_a",
    "to_ary",
    "to_h",
    "to_hash",
    "to_proc",
    "to_json",
    "inspect",
    "hash",
    "eql?",
    "each",
    "call",
    "method_missing",
    "respond_to_missing?",
    "coerce",
];

fn is_framework_runtime(path: &str) -> bool {
    for dir in [
        "runtime/active_record/",
        "runtime/action_view/",
        "runtime/action_controller/",
        "runtime/action_dispatch/",
    ] {
        if path.starts_with(dir) {
            return true;
        }
    }
    for stem in [
        "runtime/rails.rb",
        "runtime/active_record.rb",
        "runtime/action_view.rb",
        "runtime/action_controller.rb",
        "runtime/action_dispatch.rb",
        "runtime/action_mailer.rb",
        "runtime/active_job.rb",
    ] {
        if path == stem {
            return true;
        }
    }
    false
}

/// `def` line → the method name it defines, when the name is a plain
/// identifier (operator defs like `def [](k)` return None and are
/// never candidates). Handles `def self.name` and both `def name(...)`
/// and `def name;`/bare forms.
fn def_line_name(line: &str) -> Option<String> {
    let t = line.trim_start();
    let rest = t.strip_prefix("def ")?;
    let rest = rest.strip_prefix("self.").unwrap_or(rest);
    let bytes = rest.as_bytes();
    let mut end = 0;
    while end < bytes.len()
        && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_')
    {
        end += 1;
    }
    if end == 0 {
        return None;
    }
    let mut name_end = end;
    if end < bytes.len() && (bytes[end] == b'?' || bytes[end] == b'!') {
        // `def name=` is a writer — backing an attribute; treat like an
        // accessor and never shake it (only ? / ! extend the name).
        name_end += 1;
    }
    if name_end < bytes.len() && bytes[name_end] == b'=' {
        return None;
    }
    Some(rest[..name_end].to_string())
}

/// `.rbs` sig line → declared method name (`def name: ...`).
fn sig_line_name(line: &str) -> Option<String> {
    def_line_name(line.trim_end_matches(|c| c != ':').trim_end_matches(':'))
        .or_else(|| {
            let t = line.trim_start();
            let rest = t.strip_prefix("def ")?;
            let rest = rest.strip_prefix("self.").unwrap_or(rest);
            let name: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            if name.is_empty() {
                None
            } else {
                let rest_after = &rest[name.len()..];
                let name = match rest_after.chars().next() {
                    Some('?') | Some('!') => format!("{name}{}", &rest_after[..1]),
                    _ => name,
                };
                Some(name)
            }
        })
}

/// Identifier tokens (with a trailing `?`/`!` when present) on a line.
fn tokens(line: &str, out: &mut HashSet<String>) {
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b.is_ascii_alphabetic() || b == b'_' {
            let start = i;
            while i < bytes.len()
                && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_')
            {
                i += 1;
            }
            let mut end = i;
            if i < bytes.len() && (bytes[i] == b'?' || bytes[i] == b'!') {
                // Only bind `?`/`!` when not immediately part of an
                // operator like `!=` — a following `=` means the char
                // belonged to the next token, not this name.
                if i + 1 >= bytes.len() || bytes[i + 1] != b'=' {
                    end = i + 1;
                }
            }
            out.insert(line[start..end].to_string());
        } else {
            i += 1;
        }
    }
}

/// Delete `def <name>` spans from a file body. Returns the number of
/// defs removed. Single-line defs (`def x; end`) delete just their
/// line; multi-line defs delete through the matching `end` at the
/// def's indent, plus any contiguous comment block directly above.
fn delete_defs(content: &str, names: &HashSet<&str>) -> (String, usize) {
    let lines: Vec<&str> = content.lines().collect();
    let mut keep = vec![true; lines.len()];
    let mut removed = 0;
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let name = match def_line_name(line) {
            Some(n) => n,
            None => {
                i += 1;
                continue;
            }
        };
        if !names.contains(name.as_str()) {
            i += 1;
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        // Span end: same line for `def x(..); ... end` one-liners,
        // else the matching `end` at the same indent.
        let mut end_idx = i;
        // One-liner: `def x; end` / `def x(a); body; end` — a `;` on
        // the def line with a trailing `end`, whatever the spacing.
        let one_line = line.trim_end().ends_with("end") && line.contains(';');
        if !one_line {
            let mut j = i + 1;
            loop {
                if j >= lines.len() {
                    // No matching end — malformed; refuse to touch.
                    end_idx = i;
                    break;
                }
                let l = lines[j];
                let l_indent = l.len() - l.trim_start().len();
                if l.trim() == "end" && l_indent == indent {
                    end_idx = j;
                    break;
                }
                j += 1;
            }
            if end_idx == i {
                i += 1;
                continue;
            }
        }
        // Contiguous comment block directly above, at the same indent.
        let mut start_idx = i;
        while start_idx > 0 {
            let prev = lines[start_idx - 1];
            let p_trim = prev.trim_start();
            let p_indent = prev.len() - p_trim.len();
            if p_trim.starts_with('#') && p_indent == indent {
                start_idx -= 1;
            } else {
                break;
            }
        }
        for k in start_idx..=end_idx {
            keep[k] = false;
        }
        removed += 1;
        i = end_idx + 1;
    }
    if removed == 0 {
        return (content.to_string(), 0);
    }
    let mut out = String::with_capacity(content.len());
    let mut last_blank = false;
    for (idx, line) in lines.iter().enumerate() {
        if !keep[idx] {
            continue;
        }
        // Squeeze the double blank lines deletion leaves behind.
        let blank = line.trim().is_empty();
        if blank && last_blank {
            continue;
        }
        last_blank = blank;
        out.push_str(line);
        out.push('\n');
    }
    (out, removed)
}

/// Delete `def <name>: ...` declarations (plus `|` continuations) from
/// an `.rbs` sidecar.
fn delete_sigs(content: &str, names: &HashSet<&str>) -> String {
    let mut out = String::with_capacity(content.len());
    let mut skipping = false;
    for line in content.lines() {
        let t = line.trim_start();
        if skipping {
            if t.starts_with('|') {
                continue;
            }
            skipping = false;
        }
        if let Some(name) = sig_line_name(line) {
            if names.contains(name.as_str()) {
                skipping = true;
                continue;
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Shake the finished ruby-family file set in place. `synth_shakeable`
/// is the union of the lowerer's shakeable synthesized model-method
/// names (from `lower::model_to_library::shakeable_synthesized_names`);
/// only those names are candidates inside `app/models/`.
pub fn shake_tree(
    files: &mut Vec<(String, String)>,
    synth_shakeable: &HashSet<String>,
    label: &str,
) {
    if std::env::var("ROUNDHOUSE_NO_TREESHAKE").as_deref() == Ok("1") {
        return;
    }
    let mut total_runtime = 0usize;
    let mut total_synth = 0usize;
    for _pass in 0..10 {
        // Usage universe: every token on every line of every file,
        // EXCEPT the name being introduced on a def/sig line itself.
        let mut usage: HashMap<String, usize> = HashMap::new();
        for (path, content) in files.iter() {
            let is_rb = path.ends_with(".rb");
            let is_rbs = path.ends_with(".rbs");
            if !is_rb && !is_rbs {
                continue;
            }
            for line in content.lines() {
                let defined = if is_rb {
                    def_line_name(line)
                } else {
                    sig_line_name(line)
                };
                let mut toks = HashSet::new();
                tokens(line, &mut toks);
                if let Some(d) = defined {
                    toks.remove(&d);
                }
                for t in toks {
                    *usage.entry(t).or_default() += 1;
                }
            }
        }

        // Collect the per-file drop sets.
        let mut drops: Vec<(usize, HashSet<String>, bool)> = Vec::new();
        for (idx, (path, content)) in files.iter().enumerate() {
            if !path.ends_with(".rb") {
                continue;
            }
            let runtime = is_framework_runtime(path);
            let model = path.starts_with("app/models/");
            if !runtime && !model {
                continue;
            }
            let mut dead: HashSet<String> = HashSet::new();
            for line in content.lines() {
                if let Some(name) = def_line_name(line) {
                    if EXEMPT.contains(&name.as_str()) {
                        continue;
                    }
                    if model && !synth_shakeable.contains(&name) {
                        continue;
                    }
                    if usage.get(&name).copied().unwrap_or(0) == 0 {
                        dead.insert(name);
                    }
                }
            }
            if !dead.is_empty() {
                drops.push((idx, dead, runtime));
            }
        }
        if drops.is_empty() {
            break;
        }

        for (idx, dead, runtime) in drops {
            let dead_refs: HashSet<&str> = dead.iter().map(|s| s.as_str()).collect();
            let (new_content, removed) = delete_defs(&files[idx].1, &dead_refs);
            if removed > 0 {
                files[idx].1 = new_content;
                if runtime {
                    total_runtime += removed;
                } else {
                    total_synth += removed;
                }
                // Matching .rbs sidecar (runtime/x.rb → sig/runtime/x.rbs).
                let sig_path = format!(
                    "sig/{}",
                    files[idx].0.trim_end_matches(".rb").to_string() + ".rbs"
                );
                if let Some(sig_idx) = files.iter().position(|(p, _)| *p == sig_path) {
                    files[sig_idx].1 = delete_sigs(&files[sig_idx].1, &dead_refs);
                }
            }
        }
    }
    if total_runtime + total_synth > 0 {
        eprintln!(
            "roundhouse: treeshake ({label}): dropped {total_runtime} runtime defs + \
             {total_synth} synthesized model defs (text-level, whole-tree name scan)"
        );
    }
}
