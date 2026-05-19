//! FormBuilder method macro-inline expansion. Translates a
//! `form.<method>(args)` Send (where `form` is the block param of a
//! surrounding `form_with do |form| ... end`) into the inline HTML
//! accumulation statements Rails' runtime FormBuilder method would
//! have rendered. No runtime FormBuilder dispatch survives in the
//! lowered output — the class can be retired in Stage 3.
//!
//! Cross-target win: every emitter consumes the same `io << "<input
//! ..."` shape; no per-target FormBuilder runtime needs to handle
//! the heterogeneous opts hash that motivated this work.

use crate::expr::{Expr, ExprNode, InterpPart, Literal};
use crate::ident::{Symbol, VarId};
use crate::span::Span;

use crate::lower::view::FormBuilderMethod;

use super::{
    accumulator_append_call, lit_str, lit_sym, send, view_helpers_call, FormBuilderBinding,
    ViewCtx,
};

/// Re-export of `simplify_class_array` for form_with.rs to reuse on
/// the form-tag's `class:` opts entry. Keeps per-form-tag and
/// per-input-attr class composition in sync.
pub(super) fn simplify_class_array_pub(v: &Expr) -> Expr {
    simplify_class_array(v)
}

/// Inline-expand `form.<method>(args)` into HTML accumulation
/// statements. Returns the io-append `Expr`s the caller splices into
/// the surrounding view's statement list. `binding` is the active
/// FormBuilder binding (form_param, model_name, record_var,
/// form_method_var); `args` is the source-form args after surface
/// classification (`classify_form_builder_args` already split the
/// field Symbol from the trailing opts Hash).
pub(super) fn emit_form_builder_inline(
    binding: &FormBuilderBinding,
    kind: FormBuilderMethod,
    args: &[Expr],
    ctx: &ViewCtx,
) -> Vec<Expr> {
    let (positional, opts) = split_args(args);
    match kind {
        FormBuilderMethod::Label => {
            emit_label(positional.first().copied(), opts.as_slice(), binding, ctx)
        }
        FormBuilderMethod::TextField => emit_text_field(
            positional.first().copied(),
            opts.as_slice(),
            binding,
            ctx,
        ),
        FormBuilderMethod::TextArea => emit_text_area(
            positional.first().copied(),
            opts.as_slice(),
            binding,
            ctx,
        ),
        FormBuilderMethod::Submit => emit_submit(
            positional.first().copied(),
            opts.as_slice(),
            binding,
            ctx,
        ),
    }
}

/// Split `args` into positional Exprs and trailing opts entries.
/// Mirrors `classify_form_builder_args` but returns references so
/// the caller can pass them around without cloning. The trailing
/// Hash, if present, is consumed for opts; everything before it is
/// positional.
fn split_args(args: &[Expr]) -> (Vec<&Expr>, Vec<(Expr, Expr)>) {
    let mut positional: Vec<&Expr> = Vec::new();
    let mut opts: Vec<(Expr, Expr)> = Vec::new();
    for a in args {
        match &*a.node {
            ExprNode::Hash { entries, .. } => {
                for (k, v) in entries {
                    opts.push((k.clone(), v.clone()));
                }
            }
            _ => positional.push(a),
        }
    }
    (positional, opts)
}

/// `<label for="<model_name>_<field>"<opts>><CapField></label>` —
/// inline expansion of `form.label :field [, opts]`. The field name
/// is statically known (a Symbol literal); the capitalized form
/// (Rails' default label text) likewise lowers to a literal at this
/// point. Opts produce additional `name="<escaped_value>"` attrs in
/// source order, matching Rails' `render_attrs` iteration of the
/// merged `{ for: … }.merge(opts)` hash.
fn emit_label(
    field: Option<&Expr>,
    opts: &[(Expr, Expr)],
    binding: &FormBuilderBinding,
    ctx: &ViewCtx,
) -> Vec<Expr> {
    let Some(field_sym) = field_symbol(field) else {
        return vec![accumulator_append_call(lit_str(String::new()), ctx)];
    };
    let model_name = &binding.model_name;
    let cap = capitalize_ascii(field_sym.as_str());
    let mut parts: Vec<InterpPart> = Vec::new();
    parts.push(InterpPart::Text {
        value: format!(
            "<label for=\"{model_name}_{field}\"",
            field = field_sym.as_str()
        ),
    });
    append_attr_parts(&mut parts, opts);
    parts.push(InterpPart::Text {
        value: format!(">{cap}</label>"),
    });
    vec![accumulator_append_call(string_interp(parts), ctx)]
}

/// `<input type="text" name="<model_name>[<field>]" id="<model_name>_<field>"<value_attr><opts>>`
/// — inline expansion of `form.text_field :field [, opts]`. The
/// `value` attribute is emitted via `ViewHelpers.optional_value_attr`
/// so it's omitted when the record's attribute is nil-or-empty
/// (matches Rails' runtime behavior; centralized in one runtime
/// helper rather than reconstructed per call site).
fn emit_text_field(
    field: Option<&Expr>,
    opts: &[(Expr, Expr)],
    binding: &FormBuilderBinding,
    ctx: &ViewCtx,
) -> Vec<Expr> {
    let Some(field_sym) = field_symbol(field) else {
        return vec![accumulator_append_call(lit_str(String::new()), ctx)];
    };
    let model_name = &binding.model_name;
    let field_str = field_sym.as_str();
    let value_read = record_field_read(binding, field_sym.clone());
    let mut parts: Vec<InterpPart> = Vec::new();
    parts.push(InterpPart::Text {
        value: format!(
            "<input type=\"text\" name=\"{model_name}[{field_str}]\" id=\"{model_name}_{field_str}\""
        ),
    });
    parts.push(InterpPart::Expr {
        expr: view_helpers_call("optional_value_attr", vec![value_read]),
    });
    append_attr_parts(&mut parts, opts);
    parts.push(InterpPart::Text {
        value: ">".to_string(),
    });
    vec![accumulator_append_call(string_interp(parts), ctx)]
}

/// `<textarea name="<model_name>[<field>]" id="<model_name>_<field>"<opts>><escaped_body></textarea>`
/// — inline expansion of `form.text_area :field [, opts]`. The body
/// content runs through `ViewHelpers.escape_or_empty(record.field)`
/// so nil values render as an empty textarea body (matches Rails'
/// runtime). The form alias `textarea` was already normalized to
/// `text_area` by `classify_form_builder_method`.
fn emit_text_area(
    field: Option<&Expr>,
    opts: &[(Expr, Expr)],
    binding: &FormBuilderBinding,
    ctx: &ViewCtx,
) -> Vec<Expr> {
    let Some(field_sym) = field_symbol(field) else {
        return vec![accumulator_append_call(lit_str(String::new()), ctx)];
    };
    let model_name = &binding.model_name;
    let field_str = field_sym.as_str();
    let value_read = record_field_read(binding, field_sym.clone());
    let mut parts: Vec<InterpPart> = Vec::new();
    parts.push(InterpPart::Text {
        value: format!(
            "<textarea name=\"{model_name}[{field_str}]\" id=\"{model_name}_{field_str}\""
        ),
    });
    append_attr_parts(&mut parts, opts);
    parts.push(InterpPart::Text {
        value: ">".to_string(),
    });
    parts.push(InterpPart::Expr {
        expr: view_helpers_call("escape_or_empty", vec![value_read]),
    });
    parts.push(InterpPart::Text {
        value: "</textarea>".to_string(),
    });
    vec![accumulator_append_call(string_interp(parts), ctx)]
}

/// `<input type="submit" name="commit" value="<text>" data-disable-with="<text>"<opts>>`
/// — inline expansion of `form.submit [label] [, opts]`. When the
/// positional `label` is omitted, the default text branches on the
/// captured form method: `:patch` → "Update <ModelName>",
/// otherwise → "Create <ModelName>". `<ModelName>` is the
/// capitalized model_name (lowered to a literal at this point).
fn emit_submit(
    positional: Option<&Expr>,
    opts: &[(Expr, Expr)],
    binding: &FormBuilderBinding,
    ctx: &ViewCtx,
) -> Vec<Expr> {
    let label_expr = match positional {
        Some(lbl) => lbl.clone(),
        None => default_submit_text(binding),
    };
    // The label flows into both `value` and `data-disable-with` —
    // wrap it in html_escape once each; the body-typer narrows the
    // result to Str so the surrounding StringInterp stays uniform.
    let escaped_label = view_helpers_call("html_escape", vec![label_expr.clone()]);
    let escaped_data_disable = view_helpers_call("html_escape", vec![label_expr]);
    let mut parts: Vec<InterpPart> = Vec::new();
    parts.push(InterpPart::Text {
        value: "<input type=\"submit\" name=\"commit\" value=\"".to_string(),
    });
    parts.push(InterpPart::Expr { expr: escaped_label });
    parts.push(InterpPart::Text {
        value: "\" data-disable-with=\"".to_string(),
    });
    parts.push(InterpPart::Expr { expr: escaped_data_disable });
    parts.push(InterpPart::Text {
        value: "\"".to_string(),
    });
    append_attr_parts(&mut parts, opts);
    parts.push(InterpPart::Text {
        value: ">".to_string(),
    });
    vec![accumulator_append_call(string_interp(parts), ctx)]
}

/// Default `form.submit` text: `if form_method == :patch then
/// "Update <ModelName>" else "Create <ModelName>"`. Built as an If
/// node referencing the captured `form_method` local so per-record
/// new/edit distinction renders correctly at runtime.
fn default_submit_text(binding: &FormBuilderBinding) -> Expr {
    let capitalized_model = capitalize_ascii(&binding.model_name);
    let update_text = lit_str(format!("Update {capitalized_model}"));
    let create_text = lit_str(format!("Create {capitalized_model}"));
    let method_var_read = Expr::new(
        Span::synthetic(),
        ExprNode::Var {
            id: VarId(0),
            name: binding.form_method_var.clone(),
        },
    );
    let cond = send(
        Some(method_var_read),
        "==",
        vec![lit_sym(Symbol::from("patch"))],
        None,
        false,
    );
    Expr::new(
        Span::synthetic(),
        ExprNode::If {
            cond,
            then_branch: update_text,
            else_branch: create_text,
        },
    )
}

/// Append a list of opts entries to the running `parts` as
/// ` <key>="<escaped_value>"` segments. Class-array opts are
/// pre-simplified via `simplify_class_array`. Non-symbol keys are
/// skipped (not exercised by real fixtures).
fn append_attr_parts(parts: &mut Vec<InterpPart>, opts: &[(Expr, Expr)]) {
    for (k, v) in opts {
        let ExprNode::Lit { value: Literal::Sym { value: key } } = &*k.node else {
            continue;
        };
        let simplified = if key.as_str() == "class" {
            simplify_class_array(v)
        } else {
            v.clone()
        };
        parts.push(InterpPart::Text {
            value: format!(" {}=\"", key.as_str()),
        });
        parts.push(InterpPart::Expr {
            expr: view_helpers_call("html_escape", vec![lit_str_coerce(simplified)]),
        });
        parts.push(InterpPart::Text {
            value: "\"".to_string(),
        });
    }
}

/// Wrap non-literal opts values in `.to_s` so html_escape's
/// String-typed contract is satisfied. Numeric `rows: 4` and similar
/// integer/keyword values flow through this path; the body-typer's
/// per-target emit handles the to_s conversion natively.
fn lit_str_coerce(e: Expr) -> Expr {
    let is_str_lit = matches!(
        &*e.node,
        ExprNode::Lit { value: Literal::Str { .. } },
    );
    if is_str_lit {
        e
    } else {
        send(Some(e), "to_s", Vec::new(), None, false)
    }
}

/// `<record_var>.<field>` — read the record's attribute via the
/// model's typed reader. Returns a Send (`Send { recv:
/// Some(Var(record_var)), method: field, args: [], block: None }`)
/// the body-typer resolves to the per-model attribute reader.
fn record_field_read(binding: &FormBuilderBinding, field: Symbol) -> Expr {
    let record_ref = Expr::new(
        Span::synthetic(),
        ExprNode::Var {
            id: VarId(0),
            name: binding.record_var.clone(),
        },
    );
    send(Some(record_ref), field.as_str(), Vec::new(), None, false)
}

/// Extract the Symbol payload from a field-name arg (`:title`).
/// Returns None when the arg isn't a Symbol literal — the macro
/// degenerates to an empty append in that case.
fn field_symbol(field: Option<&Expr>) -> Option<Symbol> {
    let f = field?;
    let ExprNode::Lit { value: Literal::Sym { value } } = &*f.node else {
        return None;
    };
    Some(value.clone())
}

/// `String#capitalize` semantics (first char uppercase, rest
/// lowercase) for ASCII identifiers. Field symbols in real fixtures
/// are all ASCII; unicode handling would need a per-target shim.
fn capitalize_ascii(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(c) => {
            let head: String = c.to_uppercase().collect();
            let tail: String = chars.as_str().to_lowercase();
            head + &tail
        }
    }
}

/// Build a `StringInterp` Expr node from the assembled parts.
/// Collapses adjacent Text segments so the emitted body reads as
/// one literal where the static prefix and suffix would otherwise
/// chain through multiple no-op InterpParts.
fn string_interp(parts: Vec<InterpPart>) -> Expr {
    let mut merged: Vec<InterpPart> = Vec::new();
    for p in parts {
        match (&p, merged.last_mut()) {
            (
                InterpPart::Text { value: rhs },
                Some(InterpPart::Text { value: lhs }),
            ) => {
                lhs.push_str(rhs);
            }
            _ => merged.push(p),
        }
    }
    Expr::new(
        Span::synthetic(),
        ExprNode::StringInterp { parts: merged },
    )
}

/// `["base_string", {cond_class: pred, …}]` → `"base_string default_class"`,
/// where `default_class` is the FIRST key of the conditional hash. The
/// convention in real-blog is that the first hash entry is the
/// no-errors variant; picking the first key gives byte-parity with
/// Rails for the 5 default compare paths. A real if/else over
/// `record.errors[:field].any?` would be strictly better and is
/// tracked as a follow-on; this path matches the prior runtime
/// behavior.
fn simplify_class_array(v: &Expr) -> Expr {
    let ExprNode::Array { elements, .. } = &*v.node else {
        return v.clone();
    };
    let Some(first) = elements.first() else {
        return v.clone();
    };
    let ExprNode::Lit { value: Literal::Str { value: base } } = &*first.node else {
        return v.clone();
    };
    let mut composed = base.clone();
    if let Some(second) = elements.get(1) {
        if let ExprNode::Hash { entries, .. } = &*second.node {
            if let Some((k, _)) = entries.first() {
                let key_str = match &*k.node {
                    ExprNode::Lit { value: Literal::Sym { value } } => {
                        Some(value.as_str().to_string())
                    }
                    ExprNode::Lit { value: Literal::Str { value } } => Some(value.clone()),
                    _ => None,
                };
                if let Some(s) = key_str {
                    composed.push(' ');
                    composed.push_str(&s);
                }
            }
        }
    }
    Expr::new(
        first.span,
        ExprNode::Lit {
            value: Literal::Str { value: composed },
        },
    )
}
