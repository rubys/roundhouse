//! Lower the parsed `config/importmap.rb` into an `Importmap`
//! LibraryClass with two class methods: `json()` returns the
//! importmap as a JSON string, and `tags()` wraps it in the
//! `<script type="importmap">` element that Rails' view layer
//! emits via `javascript_importmap_tags`. The runtime serves
//! either form depending on the layout's needs.
//!
//! Self-describing IR: both methods' bodies are static `Lit::Str`
//! values (the JSON and the wrapped tag are computed at lower
//! time), so no per-target string-building logic is needed.

use crate::App;
use crate::dialect::LibraryFunction;
use crate::effect::EffectSet;
use crate::ident::Symbol;
use crate::lower::typing::{fn_sig, lit_str};
use crate::ty::Ty;

/// Build the `Importmap` module as `LibraryFunction`s — `json()`
/// returns the importmap JSON, `tags()` wraps it in the
/// `<script type="importmap">` element. Empty when the app has no
/// importmap.
pub fn lower_importmap_to_library_functions(app: &App) -> Vec<LibraryFunction> {
    let Some(importmap) = app.importmap.as_ref() else {
        return Vec::new();
    };
    if importmap.pins.is_empty() {
        return Vec::new();
    }
    let module_path = vec![Symbol::from("Importmap")];
    let json = render_importmap_json(&importmap.pins);
    let tags = format!("<script type=\"importmap\">{json}</script>");

    vec![
        LibraryFunction {
            module_path: module_path.clone(),
            name: Symbol::from("json"),
            params: Vec::new(),
            body: lit_str(json),
            signature: Some(fn_sig(vec![], Ty::Str)),
            effects: EffectSet::default(),
        },
        LibraryFunction {
            module_path,
            name: Symbol::from("tags"),
            params: Vec::new(),
            body: lit_str(tags),
            signature: Some(fn_sig(vec![], Ty::Str)),
            effects: EffectSet::default(),
        },
    ]
}

/// Render `{"imports": {<name>: <path>, ...}}` with stable ordering
/// (declaration order — Rails preserves it for modulepreload). The
/// JSON is small enough that hand-rolling avoids pulling in a serde
/// dependency at this layer.
fn render_importmap_json(pins: &[crate::app::ImportmapPin]) -> String {
    let mut out = String::from("{\"imports\":{");
    for (i, pin) in pins.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push('"');
        out.push_str(&escape_json_str(&pin.name));
        out.push_str("\":\"");
        out.push_str(&escape_json_str(&pin.path));
        out.push('"');
    }
    out.push_str("}}");
    out
}

fn escape_json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}
