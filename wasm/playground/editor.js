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

function loadMonaco() {
  return new Promise((resolve, reject) => {
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
    return {
      kind: "monaco",
      getValue: () => ed.getValue(),
      setValue(text, lang) {
        suppress = true;
        ed.setValue(text);
        if (lang) monaco.editor.setModelLanguage(ed.getModel(), lang);
        suppress = false;
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
    };
  }
}
