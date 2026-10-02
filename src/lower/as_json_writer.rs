//! Build a straight-line JSON writer from an `as_json` pair list.
//!
//! The emit half of monomorphizing inline `render json:`.
//! [`crate::lower::as_json_shape`] settles WHAT keys a model serializes;
//! this turns that into the same string-accumulator shape
//! `jbuilder_to_library` produces for `*.json.jbuilder` templates:
//!
//! ```text
//! io = String.new
//! io << "{"
//! io << "\"short_id\":" << JsonBuilder.encode_value(self.short_id)
//! io << ",\"created_at\":" << JsonBuilder.encode_datetime(self.created_at_raw)
//! io << "}"
//! io
//! ```
//!
//! WHY AN INSTANCE METHOD ON THE MODEL. A `Computed` pair carries the
//! expression verbatim from the `as_json` source, and those expressions
//! read `self` (`h[:avatar_url] = self.avatar_url`). Emitting into a
//! method whose `self` IS the record is what lets them be reused
//! untouched instead of rewritten against some other receiver.
//!
//! ## Commas, and why a conditional key makes them interesting
//!
//! The jbuilder writer alternates `,` literals because its pair list is
//! fixed. Here it is not: lobsters `User#as_json` gates four keys on
//! `is_admin?`, so whether a comma is needed before key N depends on
//! whether any earlier key was actually emitted.
//!
//! The rule this uses is static and exact: a comma is needed before pair
//! P iff some pair BEFORE P is unconditional — that one always emits, so
//! P always has a predecessor. If every predecessor is conditional the
//! answer is genuinely dynamic, and rather than guess (or pay a runtime
//! `first` flag on every pair) this declines and lets the caller ledger
//! it. Every lobsters model clears the rule: User's conditional keys all
//! sit behind four unconditional ones.
//!
//! ## Typing the values
//!
//! `JsonBuilder.encode_value` is a SCALAR encoder: nil/bool/Integer/
//! Float/String, and a `to_s`-and-quote fallback for anything else — so
//! an Array handed to it would ship as the quoted string `"[\"a\"]"`:
//! valid JSON, wrong data. [`writer_method`] (the PORO path) never meets
//! one, since its values are declared `attr_*` readers.
//! [`typed_writer_method`] takes a model's own `as_json`, whose values
//! are computed (`tags: tags.map(&:tag).sort`), and so takes a
//! [`PairEncoding`] per pair that the caller settled from the value's
//! analyzed type; a type with no encoding declines before this is
//! called.

use crate::dialect::{AccessorKind, MethodDef, MethodReceiver};
use crate::effect::EffectSet;
use crate::expr::{Expr, ExprNode, IrHint, Literal};
use crate::ident::{ClassId, Symbol, VarId};
use crate::schema::{ColumnType, Table};
use crate::span::Span;
use crate::ty::Ty;

use super::as_json_shape::{JsonPair, PairValue, ShapeError};
use super::typing::with_ty;

/// Name of the synthesized method. `_json` mirrors the jbuilder views'
/// suffix; `_str` says it hands back the encoded String, not the Hash
/// `as_json` itself returns.
pub const WRITER_METHOD: &str = "as_json_str";

const ACC: &str = "io";

/// The writer as a method on its model: `def as_json_str = <body>`.
///
/// TYPED AS IT IS BUILT. Everything that calls this runs after analysis,
/// and the strict emit reads an untyped node as unknown — an error on the
/// archive build. The pieces below stamp what they know (`io` and every
/// `JsonBuilder` call answer `String`); a `Computed` expression keeps the
/// type analysis gave it in the source `as_json`.
pub fn writer_method(
    owner: &ClassId,
    pairs: &[JsonPair],
    table: Option<&Table>,
    assoc_names: &[Symbol],
) -> Result<MethodDef, ShapeError> {
    let stmts = writer_body(pairs, table, assoc_names)?;
    Ok(MethodDef {
        visibility: crate::dialect::MethodVisibility::Public,
        name_span: Span::synthetic(),
        name: Symbol::from(WRITER_METHOD),
        receiver: MethodReceiver::Instance,
        params: vec![],
        body: with_ty(Expr::new(Span::synthetic(), ExprNode::Seq { exprs: stmts }), Ty::Str),
        signature: None,
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
        mutates_self: false,
        block_param: None,
    })
}

/// Statements for the writer body, or why the pairs could not be turned
/// into one.
///
/// `table` is the model's schema table when it has one — used only to
/// route temporal columns through `encode_datetime`, matching what the
/// jbuilder lowerer does for the same reason. `assoc_names` are the
/// model's association readers; a pair reading one serializes a RECORD,
/// which needs that record's own writer and is not modeled yet.
pub fn writer_body(
    pairs: &[JsonPair],
    table: Option<&Table>,
    assoc_names: &[Symbol],
) -> Result<Vec<Expr>, ShapeError> {
    writer_body_with(pairs, |pair| {
        // A pair reading an association is a nested record. Declining is
        // what keeps the writer honest: `encode_value` would fall back
        // to `to_s` and quote it, which is valid JSON and wrong data.
        if let PairValue::Reader(name) = &pair.value {
            if assoc_names.iter().any(|a| a == name) {
                return Err("a key serializes an associated record");
            }
        }
        Ok(encoded_value(pair, table))
    })
}

/// How a TYPED pair's value is written — the answer the caller reached
/// from the value's analyzed type, so the writer never guesses.
#[derive(Clone, Debug)]
pub enum PairEncoding {
    /// nil / bool / Integer / Float / String: `JsonBuilder.encode_value`.
    Scalar(Ty),
    /// `Array[String]`: `JsonBuilder.encode_string_array`.
    StringArray,
    /// A date-only column: canonical YYYY-MM-DD text or JSON null.
    DateColumn,
    /// A temporal COLUMN, as Rails' `TimeWithZone#as_json` renders it —
    /// `xmlschema(3)` in the app's `config.time_zone`, through the
    /// runtime's `ActiveSupport.json_time` seam over the stored text.
    /// `JsonBuilder.encode_datetime` would spell it in UTC with a `Z`,
    /// which is Rails' answer only for an app that never set a zone.
    ZonedTime,
}

/// The writer for pairs whose encodings the caller settled from their
/// types — a model's own declared `as_json`, which may compute values
/// (`tags.map(&:tag).sort`) no reader list could.
pub fn typed_writer_method(
    owner: &ClassId,
    pairs: &[JsonPair],
    encodings: &[PairEncoding],
) -> Result<MethodDef, ShapeError> {
    assert_eq!(pairs.len(), encodings.len());
    let stmts = writer_body_with(pairs, |pair| {
        let i = pairs.iter().position(|p| std::ptr::eq(p, pair)).expect("pair from this list");
        Ok(typed_value(pair, &encodings[i]))
    })?;
    Ok(MethodDef {
        visibility: crate::dialect::MethodVisibility::Public,
        name_span: Span::synthetic(),
        name: Symbol::from(WRITER_METHOD),
        receiver: MethodReceiver::Instance,
        params: vec![],
        body: with_ty(Expr::new(Span::synthetic(), ExprNode::Seq { exprs: stmts }), Ty::Str),
        signature: None,
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
        mutates_self: false,
        block_param: None,
    })
}

fn typed_value(pair: &JsonPair, enc: &PairEncoding) -> Expr {
    let value = |ty: Ty| match &pair.value {
        PairValue::Computed(e) => e.clone(),
        PairValue::Reader(name) => with_ty(self_send(name.as_str()), ty),
    };
    match enc {
        PairEncoding::Scalar(ty) => json_builder_call("encode_value", value(ty.clone())),
        PairEncoding::StringArray => json_builder_call(
            "encode_string_array",
            value(Ty::Array { elem: Box::new(Ty::Str) }),
        ),
        PairEncoding::DateColumn | PairEncoding::ZonedTime => {
            let PairValue::Reader(name) = &pair.value else {
                unreachable!("temporal encoding requires a column reader")
            };
            let raw = with_ty(
                self_send(&format!("{}_raw", name.as_str())),
                Ty::Union { variants: vec![Ty::Str, Ty::Nil] },
            );
            let text = if matches!(enc, PairEncoding::DateColumn) {
                // Match the public reader, including absent storage
                // encoded as "" for an unset nonnullable raw slot.
                let date = with_ty(
                    send(Some(const_ref("ActiveSupport")), "parse_db_date", vec![raw], true),
                    Ty::Union { variants: vec![Ty::Date, Ty::Nil] },
                );
                with_ty(
                    send(Some(const_ref("ActiveSupport")), "format_db_date", vec![date], true),
                    Ty::Union { variants: vec![Ty::Str, Ty::Nil] },
                )
            } else {
                with_ty(
                    send(Some(const_ref("ActiveSupport")), "json_time", vec![raw], true),
                    Ty::Union { variants: vec![Ty::Str, Ty::Nil] },
                )
            };
            json_builder_call("encode_value", text)
        }
    }
}

fn writer_body_with(
    pairs: &[JsonPair],
    encode: impl Fn(&JsonPair) -> Result<Expr, ShapeError>,
) -> Result<Vec<Expr>, ShapeError> {
    if pairs.is_empty() {
        return Err("no pairs to encode");
    }
    let mut out: Vec<Expr> = vec![
        // `io = String.new` — the accumulator every lowered renderer
        // opens with.
        assign_local(ACC, with_ty(send(Some(const_ref("String")), "new", vec![], true), Ty::Str)),
        io_append_lit("{"),
    ];

    for (i, pair) in pairs.iter().enumerate() {
        let needs_comma = match pairs[..i].iter().any(|p| p.cond.is_none()) {
            true => true,
            // Nothing before it always emits. Only safe when P is the
            // very first pair; otherwise the comma is dynamic.
            false if i == 0 => false,
            false => return Err("a conditional key precedes every unconditional one"),
        };

        let key_lit = format!("{}\"{}\":", if needs_comma { "," } else { "" }, pair.key.as_str());
        let stmts = vec![io_append_lit(&key_lit), io_append_call(encode(pair)?)];

        match &pair.cond {
            None => out.extend(stmts),
            // The comma rides INSIDE the guard: if the key is skipped
            // its separator has to be skipped with it.
            Some(cond) => out.push(if_then(cond.clone(), stmts)),
        }
    }

    out.push(io_append_lit("}"));
    // Init/Result stamps, as the jbuilder lowerer leaves them: the
    // accumulator triple every target's string-builder emit keys on.
    let mut result = var_ref(ACC);
    result.hint = Some(IrHint::StringBuilderResult);
    out.push(result);
    Ok(out)
}

/// The `JsonBuilder.<encoder>(<value>)` call for one pair.
fn encoded_value(pair: &JsonPair, table: Option<&Table>) -> Expr {
    match &pair.value {
        PairValue::Computed(e) => json_builder_call("encode_value", e.clone()),
        PairValue::Reader(name) => {
            // A temporal column serializes from its `<col>_raw` storage
            // reader — the stored ISO-8601 text — not the parsing
            // reader. Same call the jbuilder lowerer makes, for the same
            // reason: the string→string reformat is exact and skips a
            // parse/format round-trip per row.
            if is_temporal_column(table, name) {
                let raw = format!("{}_raw", name.as_str());
                json_builder_call("encode_datetime", self_send(&raw))
            } else {
                json_builder_call("encode_value", self_send(name.as_str()))
            }
        }
    }
}

fn is_temporal_column(table: Option<&Table>, name: &Symbol) -> bool {
    let Some(t) = table else { return false };
    t.columns.iter().any(|c| {
        c.name == *name
            && matches!(
                c.col_type,
                ColumnType::DateTime | ColumnType::Date | ColumnType::Time
            )
    })
}

// ── IR construction ────────────────────────────────────────────────

fn io_append_lit(s: &str) -> Expr {
    let mut e = with_ty(send(Some(var_ref(ACC)), "<<", vec![lit_str(s)], false), Ty::Str);
    e.hint = Some(IrHint::StringBuilderAppend);
    e
}

fn io_append_call(call: Expr) -> Expr {
    let mut e = with_ty(send(Some(var_ref(ACC)), "<<", vec![call], false), Ty::Str);
    e.hint = Some(IrHint::StringBuilderAppend);
    e
}

fn json_builder_call(method: &str, value: Expr) -> Expr {
    with_ty(send(Some(const_ref("JsonBuilder")), method, vec![value], true), Ty::Str)
}

/// `self.<reader>` — the attribute readers this serializes declare no
/// type (`attr_accessor`), and `encode_value` takes `untyped`; the stamp
/// says that rather than leaving the node for the diagnostics walker
/// to call unknown.
fn self_send(method: &str) -> Expr {
    with_ty(
        send(
            Some(Expr::new(Span::synthetic(), ExprNode::SelfRef)),
            method,
            vec![],
            false,
        ),
        Ty::Untyped,
    )
}

fn if_then(cond: Expr, then_stmts: Vec<Expr>) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::If {
            cond,
            then_branch: Expr::new(Span::synthetic(), ExprNode::Seq { exprs: then_stmts }),
            else_branch: Expr::new(Span::synthetic(), ExprNode::Seq { exprs: vec![] }),
        },
    )
}

fn assign_local(name: &str, value: Expr) -> Expr {
    let mut e = with_ty(
        Expr::new(
            Span::synthetic(),
            ExprNode::Assign {
                target: crate::expr::LValue::Var {
                    id: VarId(0),
                    name: Symbol::from(name),
                },
                value,
            },
        ),
        Ty::Str,
    );
    e.hint = Some(IrHint::StringBuilderInit);
    e
}

fn send(recv: Option<Expr>, method: &str, args: Vec<Expr>, parenthesized: bool) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv,
            method: Symbol::from(method),
            args,
            block: None,
            parenthesized,
        },
    )
}

fn const_ref(name: &str) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Const {
            path: vec![Symbol::from(name)],
        },
    )
}

fn var_ref(name: &str) -> Expr {
    with_ty(
        Expr::new(
            Span::synthetic(),
            ExprNode::Var {
                id: VarId(0),
                name: Symbol::from(name),
            },
        ),
        Ty::Str,
    )
}

fn lit_str(s: &str) -> Expr {
    with_ty(
        Expr::new(
            Span::synthetic(),
            ExprNode::Lit {
                value: Literal::Str { value: s.to_string() },
            },
        ),
        Ty::Str,
    )
}
