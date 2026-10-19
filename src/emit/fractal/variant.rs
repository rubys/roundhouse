//! Variants: what a region becomes once passes have edited it, and the
//! generated initializer that installs them.
//!
//! The variant of `Owner#name` is the method's own definition with the
//! passes' edits applied, wrapped in the class and module headers that
//! reproduce its lexical nesting (so constants resolve as they do in the
//! original) and evaluated against the original file and line (so
//! backtraces, `__FILE__` and `source_location` point at the app's
//! source). It is a plain `def`: an ISEQ method the JIT can call
//! directly, inline and profile on its own.

use serde::Serialize;

use super::pass::{Edit, Guard, Proposal};
use super::region::Region;

pub const RUNTIME: &str = include_str!("fractal.rb");
pub const INITIALIZER: &str = "config/initializers/fractal.rb";

/// A region with the edits of every pass that proposed one applied.
#[derive(Clone, Debug, Serialize)]
pub struct Variant {
    pub key: String,
    pub owner: String,
    pub method: String,
    pub singleton: bool,
    pub file: String,
    pub line: u32,
    /// The passes whose edits it carries, in order.
    pub passes: Vec<String>,
    pub guards: Vec<Guard>,
    pub replayable: bool,
    #[serde(skip)]
    source: String,
    #[serde(skip)]
    eval_line: i64,
    #[serde(skip)]
    file_size: usize,
    #[serde(skip)]
    file_crc: u32,
}

/// A site a pass, or the driver on its behalf, left alone.
#[derive(Clone, Debug, Serialize)]
pub struct Decline {
    pub at: String,
    pub pass: String,
    pub reason: String,
}

/// Combine the passes' proposals for `region` into one variant. A
/// proposal whose edits fall outside the definition or overlap an earlier
/// pass's is dropped whole and reported. `None` when no edit survives:
/// a method nothing changed keeps the app's definition, with no runtime.
pub fn build(
    region: &Region<'_>,
    file_text: &str,
    proposals: Vec<(&'static str, Proposal)>,
    locate: &dyn Fn(u32) -> String,
    declines: &mut Vec<Decline>,
) -> Option<Variant> {
    let mut edits: Vec<Edit> = Vec::new();
    let mut guards: Vec<Guard> = Vec::new();
    let mut passes = Vec::new();
    let mut replayable = region.knowledge.replayable();
    for (pass, proposal) in proposals {
        for (span, reason) in proposal.declined {
            declines.push(Decline { at: locate(span.start), pass: pass.to_string(), reason });
        }
        if proposal.edits.is_empty() {
            continue;
        }
        let refusal = proposal.edits.iter().enumerate().find_map(|(i, edit)| {
            let def = region.def_span;
            if edit.span.file != def.file || edit.span.start < def.start || edit.span.end > def.end || edit.span.start > edit.span.end {
                return Some((edit.span.start, "an edit falls outside the method's definition".to_string()));
            }
            let overlaps = |other: &Edit| other.span.start < edit.span.end && edit.span.start < other.span.end;
            if edits.iter().any(overlaps) || proposal.edits[..i].iter().any(overlaps) {
                return Some((edit.span.start, "an edit overlaps another edit".to_string()));
            }
            None
        });
        if let Some((at, reason)) = refusal {
            declines.push(Decline { at: locate(at), pass: pass.to_string(), reason });
            continue;
        }
        edits.extend(proposal.edits);
        for guard in proposal.guards {
            if !guards.contains(&guard) {
                guards.push(guard);
            }
        }
        replayable &= proposal.replayable;
        passes.push(pass.to_string());
    }
    if edits.is_empty() {
        return None;
    }
    let (start, end) = (region.def_span.start as usize, region.def_span.end as usize);
    let mut def = file_text[start..end].to_string();
    edits.sort_by_key(|e| std::cmp::Reverse(e.span.start));
    for edit in &edits {
        def.replace_range(edit.span.start as usize - start..edit.span.end as usize - start, &edit.text);
    }
    let mut source = String::new();
    for line in &region.prelude {
        source.push_str(line);
        source.push('\n');
    }
    source.push_str(&def);
    source.push('\n');
    for _ in 0..region.closers {
        source.push_str("end\n");
    }
    Some(Variant {
        key: region.key(),
        owner: region.owner.clone(),
        method: region.name.clone(),
        singleton: region.singleton,
        file: region.path.clone(),
        line: region.line,
        passes,
        guards,
        replayable,
        source,
        eval_line: region.line as i64 - region.prelude.len() as i64,
        file_size: file_text.len(),
        file_crc: crc32(file_text.as_bytes()),
    })
}

/// `config/initializers/fractal.rb`: the runtime, one registration per
/// variant, and the call that hooks them into the app's loading.
pub fn initializer(variants: &[Variant]) -> String {
    let mut out = RUNTIME.to_string();
    out.push('\n');
    for (i, v) in variants.iter().enumerate() {
        let mut terminator = format!("FRACTAL_VARIANT_{}", i + 1);
        while v.source.lines().any(|line| line == terminator) {
            terminator.push('_');
        }
        let guards = v
            .guards
            .iter()
            .map(|Guard::DefinedAt { constant, method, singleton, file, line }| {
                format!(
                    "{{ kind: :defined_at, constant: {}, method: {}, singleton: {singleton}, file: {}, line: {line} }}",
                    ruby_str(constant),
                    ruby_str(method),
                    ruby_str(file)
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!(
            "Fractal.variant(\n  key: {}, owner: {}, method: {}, singleton: {},\n  file: {}, line: {}, file_size: {}, file_crc: {},\n  eval_line: {}, replayable: {}, guards: [{}],\n  source: <<'{terminator}')\n",
            ruby_str(&v.key),
            ruby_str(&v.owner),
            ruby_str(&v.method),
            v.singleton,
            ruby_str(&v.file),
            v.line,
            v.file_size,
            v.file_crc,
            v.eval_line,
            v.replayable,
            guards,
        ));
        out.push_str(&v.source);
        out.push_str(&terminator);
        out.push_str("\n\n");
    }
    out.push_str("Fractal.attach(Rails.application)\n");
    out
}

/// A single-quoted Ruby string literal.
fn ruby_str(s: &str) -> String {
    format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))
}

/// CRC-32 (IEEE), the checksum Ruby's `Zlib.crc32` computes. A change
/// detector for the files a variant was built from, not tamper evidence.
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in bytes {
        crc ^= byte as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xEDB8_8320 & (crc & 1).wrapping_neg());
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_matches_zlib() {
        assert_eq!(crc32(b""), 0);
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b"The quick brown fox jumps over the lazy dog"), 0x414F_A339);
    }

    #[test]
    fn ruby_strings_escape_quotes_and_backslashes() {
        assert_eq!(ruby_str(r"a'b\c"), r"'a\'b\\c'");
    }
}
