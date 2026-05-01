//! Lower the parsed `config/importmap.rb` into the `Importmap`
//! module: two LibraryFunctions exposing the structured pin data
//! (`pins() -> Array<Hash>` and `entry() -> String`). Per-target
//! consumers render the importmap JSON / `<script>` tag from this
//! data — keeps the IR holding raw structure rather than
//! pre-rendered output.
//!
//! Self-describing IR: each pin is an Array of Hash literals with
//! typed keys. The walker emits the array unchanged across every
//! target.
//!
//! Structured-data form (rather than pre-rendered JSON/HTML
//! strings) is the general shape: lets the runtime add or remove
//! pins dynamically, swap rendering conventions per environment
//! (dev modulepreload vs prod CDN), and matches what Spinel emit
//! independently arrived at. The previous pre-rendered form was an
//! accidentally TS-shaped choice.

use crate::App;
use crate::dialect::LibraryFunction;
use crate::effect::EffectSet;
use crate::expr::{ArrayStyle, Expr, ExprNode};
use crate::ident::Symbol;
use crate::lower::typing::{fn_sig, lit_str, with_ty};
use crate::span::Span;
use crate::ty::Ty;

/// Build the `Importmap` module as two LibraryFunctions:
/// `pins() -> Array<Hash>` returning the structured pin list, and
/// `entry() -> String` naming the importmap entry module
/// (`"application"` by default). Empty when the app has no
/// importmap.
pub fn lower_importmap_to_library_functions(app: &App) -> Vec<LibraryFunction> {
    let Some(importmap) = app.importmap.as_ref() else {
        return Vec::new();
    };
    if importmap.pins.is_empty() {
        return Vec::new();
    }
    let module_path = vec![Symbol::from("Importmap")];
    let pin_hash_ty = Ty::Hash {
        key: Box::new(Ty::Str),
        value: Box::new(Ty::Str),
    };
    let pins_ty = Ty::Array { elem: Box::new(pin_hash_ty.clone()) };
    let pins_body = build_pins_array(&importmap.pins, &pin_hash_ty);

    vec![
        LibraryFunction {
            module_path: module_path.clone(),
            name: Symbol::from("pins"),
            params: Vec::new(),
            body: pins_body,
            signature: Some(fn_sig(vec![], pins_ty)),
            effects: EffectSet::default(),
        },
        LibraryFunction {
            module_path,
            name: Symbol::from("entry"),
            params: Vec::new(),
            body: lit_str("application".to_string()),
            signature: Some(fn_sig(vec![], Ty::Str)),
            effects: EffectSet::default(),
        },
    ]
}

fn build_pins_array(pins: &[crate::app::ImportmapPin], hash_ty: &Ty) -> Expr {
    let elements: Vec<Expr> = pins
        .iter()
        .map(|pin| {
            let entries = vec![
                (
                    lit_str("name".to_string()),
                    lit_str(pin.name.clone()),
                ),
                (
                    lit_str("path".to_string()),
                    lit_str(pin.path.clone()),
                ),
            ];
            with_ty(
                Expr::new(
                    Span::synthetic(),
                    ExprNode::Hash { entries, braced: true },
                ),
                hash_ty.clone(),
            )
        })
        .collect();
    with_ty(
        Expr::new(
            Span::synthetic(),
            ExprNode::Array { elements, style: ArrayStyle::Brackets },
        ),
        Ty::Array { elem: Box::new(hash_ty.clone()) },
    )
}
