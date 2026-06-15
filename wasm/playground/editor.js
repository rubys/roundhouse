// Editor abstraction for the playground. Default: Monaco (loaded from a CDN
// via its AMD loader — no bundler, no npm, matching the no-container thesis).
// Fallback: a plain <textarea>, so the page still works offline, under a
// strict CSP, or in headless CI where the CDN is unreachable. Both expose the
// same tiny interface: { setValue(text, lang), getValue(), kind }.
//
// The fallback is deliberate, not a stopgap: the playground's data flow
// (source tree -> editable buffer -> debounced transpile -> output) is proven
// through this interface regardless of which widget backs it.

const MONACO_VERSION = "0.52.2";
const MONACO_BASE = `https://cdn.jsdelivr.net/npm/monaco-editor@${MONACO_VERSION}/min/vs`;
const LOAD_TIMEOUT_MS = 8000;

// Memoized: the editor and the output view both load Monaco; concurrent
// callers must share a single AMD-loader injection (a second loader.js
// re-declares `_amdLoaderGlobal` and throws).
let monacoPromise = null;
function loadMonaco() {
  if (monacoPromise) return monacoPromise;
  monacoPromise = new Promise((resolve, reject) => {
    if (window.monaco) return resolve(window.monaco);
    const timer = setTimeout(() => reject(new Error("monaco load timeout")), LOAD_TIMEOUT_MS);
    // Run Monaco's language services in the main thread; avoids cross-origin
    // worker setup. (A worker is a later optimization, not needed to edit.)
    window.MonacoEnvironment = { getWorker: () => ({ postMessage() {}, addEventListener() {}, terminate() {} }) };
    const script = document.createElement("script");
    script.src = `${MONACO_BASE}/loader.js`;
    script.onload = () => {
      window.require.config({ paths: { vs: MONACO_BASE } });
      window.require(["vs/editor/editor.main"], () => { clearTimeout(timer); resolve(window.monaco); },
        (e) => { clearTimeout(timer); reject(e); });
    };
    script.onerror = () => { clearTimeout(timer); reject(new Error("monaco loader.js failed")); };
    document.head.appendChild(script);
  });
  return monacoPromise;
}

// container: the DOM element to mount into. onChange(text): fired on user edits
// only (programmatic setValue does NOT fire it).
export async function createEditor(container, { onChange }) {
  try {
    const monaco = await loadMonaco();
    const ed = monaco.editor.create(container, {
      value: "", language: "ruby", automaticLayout: true,
      minimap: { enabled: false }, fontSize: 13, scrollBeyondLastLine: false,
      tabSize: 2,
    });
    let suppress = false;
    ed.onDidChangeModelContent(() => { if (!suppress) onChange(ed.getValue()); });

    // Inferred-type hovers. `hoverTypes` holds the open file's (span, ty)
    // pairs; the provider returns the smallest span containing the cursor.
    let hoverTypes = [];
    const contains = (t, ln, col) =>
      (ln > t.start_line || (ln === t.start_line && col >= t.start_col)) &&
      (ln < t.end_line || (ln === t.end_line && col <= t.end_col));
    const spanSize = (t) => (t.end_line - t.start_line) * 100000 + (t.end_col - t.start_col);
    monaco.languages.registerHoverProvider(["ruby", "html"], {
      provideHover(_model, position) {
        let best = null;
        for (const t of hoverTypes) {
          if (contains(t, position.lineNumber, position.column) &&
              (!best || spanSize(t) < spanSize(best))) best = t;
        }
        if (!best) return null;
        return {
          range: new monaco.Range(best.start_line, best.start_col, best.end_line, best.end_col),
          contents: [{ value: "inferred type" }, { value: "```rbs\n" + best.ty + "\n```" }],
        };
      },
    });

    return {
      kind: "monaco",
      setTypes(types) { hoverTypes = types; },
      getValue: () => ed.getValue(),
      // Swap in a fresh model rather than mutating the current model's language:
      // monaco's Monarch tokenizer can underflow ("pop an empty stack") when a
      // model's language changes (ruby -> html for an .erb), and a per-file
      // model is the idiomatic file-switcher pattern anyway.
      setValue(text, lang) {
        suppress = true;
        const prev = ed.getModel();
        ed.setModel(monaco.editor.createModel(text, lang || "plaintext"));
        if (prev) prev.dispose();
        suppress = false;
      },
      // Render diagnostics as squiggles on the current model. `diags` are the
      // wasm contract's DiagnosticOut objects (1-based positions), already
      // filtered to the open file.
      setMarkers(diags) {
        const markers = diags.map((d) => ({
          startLineNumber: d.start_line, startColumn: d.start_col,
          endLineNumber: d.end_line, endColumn: d.end_col,
          message: `[${d.code}] ${d.message}`,
          severity: d.severity === "error"
            ? monaco.MarkerSeverity.Error : monaco.MarkerSeverity.Warning,
        }));
        monaco.editor.setModelMarkers(ed.getModel(), "roundhouse", markers);
      },
    };
  } catch (err) {
    console.warn("[playground] Monaco unavailable, using textarea fallback:", err.message);
    const ta = document.createElement("textarea");
    ta.spellcheck = false;
    ta.autocapitalize = "off";
    ta.setAttribute("autocomplete", "off");
    container.appendChild(ta);
    ta.addEventListener("input", () => onChange(ta.value));
    return {
      kind: "textarea",
      getValue: () => ta.value,
      setValue(text) { ta.value = text; },
      setMarkers() {}, // textarea can't render squiggles; status bar shows counts
      setTypes() {}, // no hovers in the textarea fallback
    };
  }
}

// Read-only output view for the emitted code: a second Monaco instance (giving
// the output pane syntax highlighting), with a plain <pre> fallback that
// mirrors the editor's textarea fallback. setValue(text, lang) swaps both the
// content and the highlighting grammar (lang is a Monaco language id; targets
// without a Monaco grammar pass "plaintext").
export async function createOutputView(container) {
  try {
    const monaco = await loadMonaco();
    const ed = monaco.editor.create(container, {
      value: "", language: "plaintext", readOnly: true, domReadOnly: true,
      automaticLayout: true, minimap: { enabled: false }, fontSize: 13,
      scrollBeyondLastLine: false, tabSize: 2,
    });
    return {
      kind: "monaco",
      // Fresh model per file (same reason as the editor's setValue above); the
      // editor's readOnly option applies to whatever model it shows.
      setValue(text, lang) {
        const prev = ed.getModel();
        ed.setModel(monaco.editor.createModel(text, lang || "plaintext"));
        if (prev) prev.dispose();
      },
    };
  } catch (err) {
    const pre = document.createElement("pre");
    container.appendChild(pre);
    return { kind: "pre", setValue(text) { pre.textContent = text; } };
  }
}
