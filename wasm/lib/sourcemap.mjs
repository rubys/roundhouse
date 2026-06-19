// Minimal Source Map v3 consumer (base64-VLQ) — just enough to resolve a
// generated (line, column) back to its original (source, line). Used by
// /studio/ (rung D.2, Phase 9) to jump a failing test row to the Ruby source
// line via the emitted token-level `.ts.map`. No dependencies.

const B64 = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const CHAR = (() => { const m = {}; for (let i = 0; i < B64.length; i++) m[B64[i]] = i; return m; })();

// Decode one comma-separated segment into its 1/4/5 VLQ fields.
function decodeSegment(segment) {
  const out = [];
  let i = 0;
  while (i < segment.length) {
    let result = 0, shift = 0, cont, digit;
    do {
      digit = CHAR[segment[i++]];
      if (digit === undefined) return out; // malformed → stop early
      cont = digit & 32;
      digit &= 31;
      result += digit << shift;
      shift += 5;
    } while (cont);
    out.push(result & 1 ? -(result >> 1) : result >> 1);
  }
  return out;
}

// Resolve a generated position (1-based `line1`, 0-based `col0`) to its
// original { source, line (1-based), column (0-based) }, using the mapping on
// that line whose generated column is the greatest ≤ col0 (else the first
// mapping on the line). Returns null if the line carries no mappings.
//
// Source-map fields are cumulative across ALL prior lines (except the
// generated column, which resets each line), so we walk from the top to keep
// the source/original-line/original-column accumulators correct.
export function originalPositionFor(map, line1, col0) {
  const lines = map.mappings.split(";");
  if (line1 < 1 || line1 > lines.length) return null;
  let srcIdx = 0, origLine = 0, origCol = 0;
  let chosen = null;
  for (let ln = 0; ln < line1; ln++) {
    const onTarget = ln === line1 - 1;
    let genCol = 0, first = null, best = null;
    const segs = lines[ln] ? lines[ln].split(",") : [];
    for (const seg of segs) {
      const v = decodeSegment(seg);
      if (v.length === 0) continue;
      genCol += v[0];
      if (v.length >= 4) { srcIdx += v[1]; origLine += v[2]; origCol += v[3]; }
      if (onTarget && v.length >= 4) {
        const cand = { srcIdx, origLine, origCol };
        if (first === null) first = cand;
        if (genCol <= col0) best = cand; // segments are sorted by genCol → last wins
      }
    }
    if (onTarget) chosen = best || first;
  }
  if (!chosen) return null;
  return { source: map.sources[chosen.srcIdx], line: chosen.origLine + 1, column: chosen.origCol };
}

// Collapse "." / ".." segments in a slash path (so a .map `sources` entry like
// "../test/models/x_test.rb" resolves against the map's own directory).
export function normPath(p) {
  const out = [];
  for (const seg of p.split("/")) {
    if (seg === "" || seg === ".") continue;
    if (seg === "..") out.pop();
    else out.push(seg);
  }
  return out.join("/");
}
