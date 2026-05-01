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
use crate::dialect::{AccessorKind, LibraryClass, MethodDef, MethodReceiver};
use crate::effect::EffectSet;
use crate::ident::{ClassId, Symbol};
use crate::lower::typing::{fn_sig, lit_str};
use crate::ty::Ty;

/// Build an `Importmap` LibraryClass. Returns `None` when the app
/// has no importmap or the importmap is empty.
pub fn lower_importmap_to_library_class(app: &App) -> Option<LibraryClass> {
    let importmap = app.importmap.as_ref()?;
    if importmap.pins.is_empty() {
        return None;
    }
    let owner = ClassId(Symbol::from("Importmap"));
    let json = render_importmap_json(&importmap.pins);
    let tags = format!("<script type=\"importmap\">{json}</script>");

    let json_method = MethodDef {
        name: Symbol::from("json"),
        receiver: MethodReceiver::Class,
        params: Vec::new(),
        body: lit_str(json),
        signature: Some(fn_sig(vec![], Ty::Str)),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
    };
    let tags_method = MethodDef {
        name: Symbol::from("tags"),
        receiver: MethodReceiver::Class,
        params: Vec::new(),
        body: lit_str(tags),
        signature: Some(fn_sig(vec![], Ty::Str)),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
    };

    Some(LibraryClass {
        name: owner,
        is_module: true,
        parent: None,
        includes: Vec::new(),
        methods: vec![json_method, tags_method],
    })
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
