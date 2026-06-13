//! Source Map v3 serializer — turns the printer's `Mapping` stream
//! (generated line/col + originating `Span`) into the JSON+VLQ
//! format browsers, Node (`--enable-source-maps`), tsx, and esbuild
//! consume.
//!
//! Each segment carries 4 VLQ fields (no `names` entries):
//! generated-column delta, source-index delta, source-line delta,
//! source-column delta. Generated-column deltas reset per line
//! (`;`-separated); the source-side deltas run across the whole file.
//!
//! `sources` entries are app-relative (the `App::root` ingest prefix
//! is stripped, so fs- and map-VFS ingest produce identical maps —
//! the original `.rb`/`.erb` files don't exist in the emitted tree,
//! so no emitted-relative path would resolve anyway; the entries are
//! labels). `sourcesContent` embeds the full text
//! so consumers display the Ruby/ERB source without needing the
//! original tree on disk. View spans index the raw template (the
//! ERB offset translation from ingest), so a mapped position lands
//! on the template line the user wrote.

use std::collections::HashMap;

use super::printer::Mapping;
use crate::span::SourceFile;

const BASE64: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Base64-VLQ encode one signed value: sign bit in the lowest bit,
/// then 5-bit groups, continuation bit 0x20.
fn vlq(value: i64, out: &mut String) {
    let mut v: u64 = if value < 0 {
        (((-value) as u64) << 1) | 1
    } else {
        (value as u64) << 1
    };
    loop {
        let mut digit = (v & 0b1_1111) as usize;
        v >>= 5;
        if v != 0 {
            digit |= 0b10_0000;
        }
        out.push(BASE64[digit] as char);
        if v == 0 {
            break;
        }
    }
}

/// Build the source-map JSON for one generated file. `sources` is
/// `App::sources` — `FileId(n)` indexes entry `n - 1`. Returns `None`
/// when no mapping resolves (nothing but synthesized glue): the
/// caller skips the `.map` file and the `sourceMappingURL` comment.
///
/// `source_prefix` is prepended to every source path — consumers
/// resolve `sources` relative to the map's own location, so the
/// caller passes enough `../` hops to climb back to the emit root.
/// `root` (`App::root`) is stripped from each registered path first,
/// making the entries app-relative regardless of how the app was
/// ingested (fs paths carry the app dir prefix, map-VFS trees don't).
///
/// FileIds that fall outside `sources` are skipped, not mis-mapped:
/// the per-thread source registry restarts during the runtime-class
/// re-parse, so a stray span minted after ingest's drain could carry
/// an id that collides with an app file's. (No such span reaches
/// this path today — runtime files render through the string bridge
/// — but a wrong file would be strictly worse than a missing entry.)
pub(super) fn build_source_map(
    mappings: &[Mapping],
    generated_file: &str,
    sources: &[SourceFile],
    source_prefix: &str,
    root: &str,
) -> Option<String> {
    // (gen_line, gen_col, src_index, src_line0, src_col0), in
    // emission order. The printer writes linearly so generated
    // positions are already non-decreasing.
    let mut src_index: HashMap<u32, usize> = HashMap::new();
    let mut used: Vec<&SourceFile> = Vec::new();
    let mut resolved: Vec<(u32, u32, usize, u32, u32)> = Vec::new();
    for m in mappings {
        let fid = m.span.file.0;
        if fid == 0 {
            continue;
        }
        let Some(sf) = sources.get(fid as usize - 1) else {
            continue;
        };
        let idx = *src_index.entry(fid).or_insert_with(|| {
            used.push(sf);
            used.len() - 1
        });
        let (line1, col1) = sf.line_col(m.span.start);
        resolved.push((m.gen_line, m.gen_col, idx, line1 - 1, col1 - 1));
    }
    if resolved.is_empty() {
        return None;
    }
    // Several nodes can begin at the same generated position (a
    // statement and its leading expression; a call and its member
    // callee). Keep the LAST mark — the innermost, most token-precise
    // span.
    let mut deduped: Vec<(u32, u32, usize, u32, u32)> = Vec::with_capacity(resolved.len());
    for r in resolved {
        match deduped.last() {
            Some(prev) if prev.0 == r.0 && prev.1 == r.1 => {
                *deduped.last_mut().unwrap() = r;
            }
            _ => deduped.push(r),
        }
    }

    let mut mappings_str = String::new();
    let mut cur_line: u32 = 0;
    let mut prev_gen_col: i64 = 0;
    let mut prev_src: i64 = 0;
    let mut prev_src_line: i64 = 0;
    let mut prev_src_col: i64 = 0;
    let mut line_has_segment = false;
    for (gl, gc, si, sl, sc) in deduped {
        while cur_line < gl {
            mappings_str.push(';');
            cur_line += 1;
            prev_gen_col = 0;
            line_has_segment = false;
        }
        if line_has_segment {
            mappings_str.push(',');
        }
        vlq(gc as i64 - prev_gen_col, &mut mappings_str);
        vlq(si as i64 - prev_src, &mut mappings_str);
        vlq(sl as i64 - prev_src_line, &mut mappings_str);
        vlq(sc as i64 - prev_src_col, &mut mappings_str);
        prev_gen_col = gc as i64;
        prev_src = si as i64;
        prev_src_line = sl as i64;
        prev_src_col = sc as i64;
        line_has_segment = true;
    }

    let obj = serde_json::json!({
        "version": 3,
        "file": generated_file,
        "sources": used.iter().map(|s| {
            let rel = s.path
                .strip_prefix(root)
                .map(|r| r.trim_start_matches('/'))
                .unwrap_or(&s.path);
            format!("{source_prefix}{rel}")
        }).collect::<Vec<_>>(),
        "sourcesContent": used.iter().map(|s| s.text.as_str()).collect::<Vec<_>>(),
        "names": [],
        "mappings": mappings_str,
    });
    Some(obj.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::span::{FileId, Span};

    fn vlq_str(v: i64) -> String {
        let mut s = String::new();
        vlq(v, &mut s);
        s
    }

    #[test]
    fn vlq_encodes_known_values() {
        // Reference values from the source-map spec / ruby2js + mozilla
        // implementations.
        assert_eq!(vlq_str(0), "A");
        assert_eq!(vlq_str(1), "C");
        assert_eq!(vlq_str(-1), "D");
        assert_eq!(vlq_str(2), "E");
        assert_eq!(vlq_str(16), "gB");
        assert_eq!(vlq_str(123), "2H");
        assert_eq!(vlq_str(-123), "3H");
    }

    fn src(path: &str, text: &str) -> SourceFile {
        SourceFile { path: path.into(), text: text.into() }
    }

    fn mapping(gen_line: u32, gen_col: u32, file: u32, start: u32) -> Mapping {
        Mapping { gen_line, gen_col, span: Span { file: FileId(file), start, end: start + 1 } }
    }

    #[test]
    fn serializes_deltas_per_line_and_resets_gen_col() {
        // Source "ab\ncd\n": offset 0 → (0,0); offset 3 → (1,0).
        let sources = vec![src("a.rb", "ab\ncd\n")];
        let maps = vec![
            mapping(0, 0, 1, 0),
            mapping(0, 4, 1, 3),
            mapping(2, 2, 1, 0),
        ];
        let json = build_source_map(&maps, "a.ts", &sources, "", "").unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["version"], 3);
        assert_eq!(v["file"], "a.ts");
        assert_eq!(v["sources"][0], "a.rb");
        assert_eq!(v["sourcesContent"][0], "ab\ncd\n");
        // Line 0: [0,0,0,0] = "AAAA", then [+4,0,+1,0] = "IACA".
        // Line 1 empty; line 2: [2,0,-1,0] = "EADA".
        assert_eq!(v["mappings"], "AAAA,IACA;;EADA");
    }

    #[test]
    fn synthetic_and_out_of_range_spans_are_skipped() {
        let sources = vec![src("a.rb", "x\n")];
        let maps = vec![
            mapping(0, 0, 0, 0), // synthetic file sentinel
            mapping(0, 2, 9, 0), // id beyond the registry (collision guard)
        ];
        assert!(build_source_map(&maps, "a.ts", &sources, "", "").is_none());
    }

    #[test]
    fn same_generated_position_keeps_innermost_mark() {
        // Statement and its leading expression both mark (0,0); the
        // later (inner) span at offset 3 must win.
        let sources = vec![src("a.rb", "ab\ncd\n")];
        let maps = vec![mapping(0, 0, 1, 0), mapping(0, 0, 1, 3)];
        let json = build_source_map(&maps, "a.ts", &sources, "", "").unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        // Single segment, line 1 col 0 of the source: [0,0,1,0].
        assert_eq!(v["mappings"], "AACA");
    }

    #[test]
    fn second_source_file_gets_its_own_index() {
        let sources = vec![src("a.rb", "x\n"), src("b.rb", "y\n")];
        let maps = vec![mapping(0, 0, 1, 0), mapping(1, 0, 2, 0)];
        let json = build_source_map(&maps, "a.ts", &sources, "", "").unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["sources"], serde_json::json!(["a.rb", "b.rb"]));
        // Line 0: [0,0,0,0]; line 1: [genCol 0, srcIdx +1, line 0, col 0].
        assert_eq!(v["mappings"], "AAAA;ACAA");
    }
}
