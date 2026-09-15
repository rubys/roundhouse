// Build an analyzer bundle from a checkout on the VISITOR'S disk, in the
// browser, so /ide/ can analyze an app the site never shipped. Same output
// shape as ide/bundle-src.mjs (`{name, src: {path → text}}`), same inclusion
// rules (bundle-rules.mjs) — only the transport differs. Nothing is uploaded:
// the text goes from the File API into the analyzer worker and stays there.
//
// Two entry points, for the two ways a browser can hand over a folder:
//
//   fromDirectoryHandle(handle)  — File System Access API (Chromium). Walks
//                                  only the dirs the rules name, so a
//                                  node_modules/ next to app/ is never
//                                  opened; the handle can be kept and
//                                  re-read after the visitor edits on disk.
//   fromFileList(files)          — <input type="file" webkitdirectory>
//                                  (everywhere). The browser enumerates the
//                                  whole tree; we filter by path and read
//                                  only what the rules want.
//
// `fromEntries` is the common tail both feed, and the seam the verifier
// drives directly with synthetic {path, text} pairs.

import { WALK_DIRS, SINGLE_FILES, SOURCE_EXT, wantsPath } from "./bundle-rules.mjs";

// Assemble the bundle from already-filtered, root-relative entries.
export function fromEntries(name, entries) {
  const src = {};
  for (const { path, text } of entries) {
    if (wantsPath(path)) src[path] = text;
  }
  return { name, src, local: true };
}

// A `webkitdirectory` FileList: every File carries `webkitRelativePath`
// ("<folder>/app/models/x.rb"). Strip the folder segment, keep what the
// rules want, read only those. Reads run concurrently — a Rails app is a
// few hundred small files, and awaiting them one at a time is what makes
// a folder open feel slow.
export async function fromFileList(files) {
  const list = Array.from(files);
  if (!list.length) return null;
  const folder = list[0].webkitRelativePath.split("/")[0];
  const wanted = list
    .map((f) => ({ file: f, path: f.webkitRelativePath.split("/").slice(1).join("/") }))
    .filter(({ path }) => wantsPath(path));
  const entries = await Promise.all(
    wanted.map(async ({ file, path }) => ({ path, text: await file.text() })),
  );
  return fromEntries(folder, entries);
}

// A FileSystemDirectoryHandle: walk exactly the rule dirs, mirroring
// bundle-src.mjs's `walk` + single-file reads, so an absent dir is a no-op
// and an unrelated sibling (node_modules, tmp, log) is never touched.
export async function fromDirectoryHandle(root) {
  const entries = [];
  async function dirAt(base, segs) {
    let h = base;
    for (const s of segs) {
      try { h = await h.getDirectoryHandle(s); } catch { return null; }
    }
    return h;
  }
  async function walk(dir, prefix) {
    for await (const [name, h] of dir.entries()) {
      const rel = prefix ? `${prefix}/${name}` : name;
      if (h.kind === "directory") await walk(h, rel);
      else if (SOURCE_EXT.test(name)) {
        entries.push({ path: rel, text: await (await h.getFile()).text() });
      }
    }
  }
  for (const sub of WALK_DIRS) {
    const d = await dirAt(root, sub.split("/"));
    if (d) await walk(d, sub);
  }
  for (const single of SINGLE_FILES) {
    const segs = single.split("/");
    const d = await dirAt(root, segs.slice(0, -1));
    if (!d) continue;
    try {
      const fh = await d.getFileHandle(segs[segs.length - 1]);
      entries.push({ path: single, text: await (await fh.getFile()).text() });
    } catch { /* absent — fine, one list serves both layouts */ }
  }
  return fromEntries(root.name, entries);
}

export const hasDirectoryPicker = typeof globalThis.showDirectoryPicker === "function";
