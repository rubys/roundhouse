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

use super::walker::walk_body;

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
        FormBuilderMethod::Label => emit_label(
            positional.first().copied(),
            positional.get(1).copied(),
            opts.as_slice(),
            binding,
            ctx,
        ),
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
        FormBuilderMethod::RichTextArea => emit_rich_text_area(
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
        FormBuilderMethod::PasswordField => emit_valueless_input_field(
            "password",
            positional.first().copied(),
            opts.as_slice(),
            binding,
            ctx,
        ),
        FormBuilderMethod::FileField => emit_valueless_input_field(
            "file",
            positional.first().copied(),
            opts.as_slice(),
            binding,
            ctx,
        ),
        FormBuilderMethod::HiddenField => emit_hidden_field(
            positional.first().copied(),
            opts.as_slice(),
            binding,
            ctx,
        ),
        FormBuilderMethod::CheckBox => emit_check_box(
            positional.first().copied(),
            opts.as_slice(),
            binding,
            ctx,
        ),
        FormBuilderMethod::RadioButton => emit_radio_button(
            positional.first().copied(),
            positional.get(1).copied(),
            opts.as_slice(),
            binding,
            ctx,
        ),
        FormBuilderMethod::Select => emit_select(
            positional.first().copied(),
            positional.get(1).copied(),
            opts.as_slice(),
            binding,
            ctx,
        ),
        FormBuilderMethod::Button => {
            emit_button(positional.first().copied(), opts.as_slice(), ctx)
        }
        // `fields_for` without a block renders nothing: its entire
        // output is what the block writes (Rails captures the block and
        // returns it; there is no block here to capture). The block form
        // is `emit_form_builder_block_inline` below. Safe-empty rather
        // than a refusal, matching `form_with`'s no-model fallback — a
        // blockless `fields_for` in a template would be dead markup in
        // Rails too.
        FormBuilderMethod::FieldsFor => {
            vec![accumulator_append_call(lit_str(String::new()), ctx)]
        }
        FormBuilderMethod::UrlField => emit_typed_input_field(
            "url",
            positional.first().copied(),
            opts.as_slice(),
            binding,
            ctx,
        ),
        FormBuilderMethod::EmailField => emit_typed_input_field(
            "email",
            positional.first().copied(),
            opts.as_slice(),
            binding,
            ctx,
        ),
    }
}

/// `<input name="user[f]" type="hidden" value="0" autocomplete="off">
/// <input type="checkbox" value="1" name="user[f]" id="user_f"[ checked]>`
/// — Rails' check_box pair (the hidden shadow makes an unchecked box
/// POST "0"). The checked attr is value-dependent, so it goes through
/// the runtime `checked_box_attr` (CRuby overlay; truthy-and-not-zero).
fn emit_check_box(
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
    let mut parts: Vec<InterpPart> = Vec::new();
    parts.push(InterpPart::Text {
        value: format!(
            // The unchecked-value companion carries no `autocomplete`
            // attribute — Rails renders exactly
            // `<input name="user[f]" type="hidden" value="0">`.
            "<input name=\"{mn}[{f}]\" type=\"hidden\" value=\"0\"><input type=\"checkbox\" value=\"1\"{nid}",
            mn = model_name,
            f = field_str,
            nid = name_id_attrs_for(binding, field_str, opts),
        ),
    });
    // Checked state, typed instead of the runtime `checked_box_attr`
    // seam (an untyped truthiness walk, CRuby-overlay-only). A PROVABLE
    // bool reader (Boolean column / bool typed_store attr / `attribute
    // :x, :boolean`) reduces to a plain ternary on the reader send —
    // the reader is guaranteed synthesized. Anything else reads via the
    // `[]` indexer, which returns nil for names the model doesn't
    // carry: lobsters' `f.check_box :i_am_sure` binds a User attribute
    // that exists NOWHERE (it's only ever read back as a param), and a
    // bare reader send raised NoMethodError mid-replay — the indexer
    // renders it unchecked, byte-identical to the old seam. The
    // fallback test is Rails-truthful over the realistic value space
    // via to_s (nil / "0" / false stay unchecked; "1" / true check).
    let checked = lit_str(" checked=\"checked\"".to_string());
    let is_bool_reader = ctx
        .bool_readers
        .get(model_name.as_str())
        .is_some_and(|s| s.contains(field_str));
    let checked_expr = if is_bool_reader {
        let record_ref = Expr::new(
            Span::synthetic(),
            ExprNode::Var { id: VarId(0), name: binding.record_var.clone() },
        );
        let reader = send(Some(record_ref), field_str, Vec::new(), None, false);
        Expr::new(
            Span::synthetic(),
            ExprNode::If {
                cond: reader,
                then_branch: checked,
                else_branch: lit_str(String::new()),
            },
        )
    } else {
        let value_read = field_value_read(binding, field_sym.clone(), ctx);
        let eq = |s: &str| {
            send(
                Some(to_s(value_read.clone())),
                "==",
                vec![lit_str(s.to_string())],
                None,
                false,
            )
        };
        let cond = Expr::new(
            Span::synthetic(),
            ExprNode::BoolOp {
                op: crate::expr::BoolOpKind::Or,
                surface: crate::expr::BoolOpSurface::Symbol,
                left: eq("1"),
                right: eq("true"),
            },
        );
        Expr::new(
            Span::synthetic(),
            ExprNode::If {
                cond,
                then_branch: checked,
                else_branch: lit_str(String::new()),
            },
        )
    };
    parts.push(InterpPart::Expr { expr: checked_expr });
    append_attr_parts(&mut parts, opts);
    parts.push(InterpPart::Text { value: ">".to_string() });
    vec![accumulator_append_call(string_interp(parts), ctx)]
}

/// `<input type="radio" value="V"[ checked] name="user[f]" id="user_f_v">`
/// — checked when the record's value stringifies equal to V (Rails'
/// comparison); goes through the runtime `radio_checked_attr`.
fn emit_radio_button(
    field: Option<&Expr>,
    value: Option<&Expr>,
    opts: &[(Expr, Expr)],
    binding: &FormBuilderBinding,
    ctx: &ViewCtx,
) -> Vec<Expr> {
    let (Some(field_sym), Some(value)) = (field_symbol(field), value) else {
        return vec![accumulator_append_call(lit_str(String::new()), ctx)];
    };
    let model_name = &binding.model_name;
    let field_str = field_sym.as_str();
    let mut parts: Vec<InterpPart> = Vec::new();
    parts.push(InterpPart::Text { value: "<input type=\"radio\" value=\"".to_string() });
    parts.push(InterpPart::Expr {
        expr: view_helpers_call("html_escape", vec![to_s(value.clone())]),
    });
    parts.push(InterpPart::Text { value: "\"".to_string() });
    // Checked state, inline instead of the runtime `radio_checked_attr`
    // seam. An explicit `checked:` opt wins (lobsters' search radios:
    // `checked: @search.what == "stories"` — previously it leaked into
    // the tag as `checked="false"`, which still CHECKS in HTML); the
    // default is Rails' to_s comparison against the `[]` indexer read
    // (nil-safe for names the model doesn't carry — same rationale as
    // check_box's fallback arm; to_s == to_s types on every target).
    let checked = lit_str(" checked=\"checked\"".to_string());
    let explicit_checked = opts.iter().find_map(|(k, v)| {
        matches!(&*k.node, ExprNode::Lit { value: Literal::Sym { value } }
            if value.as_str() == "checked")
        .then(|| v.clone())
    });
    let cond = match explicit_checked {
        Some(c) => c,
        None => {
            let value_read = field_value_read(binding, field_sym.clone(), ctx);
            send(
                Some(to_s(value_read)),
                "==",
                vec![to_s(value.clone())],
                None,
                false,
            )
        }
    };
    parts.push(InterpPart::Expr {
        expr: Expr::new(
            Span::synthetic(),
            ExprNode::If {
                cond,
                then_branch: checked,
                else_branch: lit_str(String::new()),
            },
        ),
    });
    // Rails suffixes a radio's id with its VALUE (one input per choice,
    // ids must differ): `edit_user_user_prefers_color_scheme_dark`.
    parts.push(InterpPart::Text {
        value: format!(
            " name=\"{model_name}[{field_str}]\" id=\"{}_",
            super::field_id(&binding.id_prefix, model_name, field_str)
        ),
    });
    parts.push(InterpPart::Expr {
        expr: view_helpers_call("html_escape", vec![to_s(value.clone())]),
    });
    parts.push(InterpPart::Text { value: "\"".to_string() });
    // `checked:` is consumed above — it's checked STATE, not an HTML
    // attribute.
    let attr_opts: Vec<(Expr, Expr)> = opts
        .iter()
        .filter(|(k, _)| {
            !matches!(&*k.node, ExprNode::Lit { value: Literal::Sym { value } }
                if value.as_str() == "checked")
        })
        .cloned()
        .collect();
    append_attr_parts(&mut parts, &attr_opts);
    parts.push(InterpPart::Text { value: ">".to_string() });
    vec![accumulator_append_call(string_interp(parts), ctx)]
}

/// `<select name="user[f]" id="user_f"<opts>><options></select>` —
/// the choices expression (`[["No e-mails", 0], …]`) and the record's
/// current value go to the runtime `select_options_for`, which builds
/// the `<option>` list with the matching one selected.
fn emit_select(
    field: Option<&Expr>,
    choices: Option<&Expr>,
    opts: &[(Expr, Expr)],
    binding: &FormBuilderBinding,
    ctx: &ViewCtx,
) -> Vec<Expr> {
    let (Some(field_sym), Some(choices)) = (field_symbol(field), choices) else {
        return vec![accumulator_append_call(lit_str(String::new()), ctx)];
    };
    let field_str = field_sym.as_str();
    let value_read = field_value_read(binding, field_sym.clone(), ctx);
    // `include_blank:` is select BEHAVIOR, not an HTML attribute — pull
    // it out before the attr expansion (previously it leaked into the
    // tag as `include_blank="true"`).
    let include_blank = opts.iter().any(|(k, v)| {
        matches!(&*k.node, ExprNode::Lit { value: Literal::Sym { value } }
            if value.as_str() == "include_blank")
            && matches!(&*v.node, ExprNode::Lit { value: Literal::Bool { value: true } })
    });
    // `multiple: true` is select BEHAVIOR too: Rails renders
    // ` multiple="multiple"` (not multiple="true") and marks EVERY
    // option whose value the selected ARRAY includes.
    let multiple = opts.iter().any(|(k, v)| {
        matches!(&*k.node, ExprNode::Lit { value: Literal::Sym { value } }
            if value.as_str() == "multiple")
            && matches!(&*v.node, ExprNode::Lit { value: Literal::Bool { value: true } })
    });
    let attr_opts: Vec<(Expr, Expr)> = opts
        .iter()
        .filter(|(k, _)| {
            !matches!(&*k.node, ExprNode::Lit { value: Literal::Sym { value } }
                if value.as_str() == "include_blank" || value.as_str() == "multiple")
        })
        .cloned()
        .collect();
    let mut parts: Vec<InterpPart> = Vec::new();
    parts.push(InterpPart::Text {
        value: format!("<select{}", name_id_attrs_for(binding, field_str, opts)),
    });
    if multiple {
        parts.push(InterpPart::Text { value: " multiple=\"multiple\"".to_string() });
    }
    append_attr_parts(&mut parts, &attr_opts);
    parts.push(InterpPart::Text { value: ">".to_string() });
    let (setup, options_expr) =
        match emit_select_options(choices, value_read.clone(), include_blank, multiple, field_str, ctx) {
            Some(pair) => pair,
            // Unclassified choices shape — keep the runtime seam (the
            // CRuby overlay's select_options_for). Honest residue: the
            // strict trees will refuse it, naming the site.
            None => (
                Vec::new(),
                view_helpers_call("select_options_for", vec![choices.clone(), value_read]),
            ),
        };
    parts.push(InterpPart::Expr { expr: options_expr });
    parts.push(InterpPart::Text { value: "</select>".to_string() });
    let mut out = setup;
    out.push(accumulator_append_call(string_interp(parts), ctx));
    out
}

/// Compile-time select-option rendering — replaces the runtime
/// `select_options_for` seam (a CRuby-overlay `is_a?`-walk over
/// heterogeneous choices, the shape the typed runtime refuses) with
/// per-shape expansion. Returns `(setup_stmts, options_expr)`; `None`
/// falls back to the runtime seam.
///
/// Shapes (the lobsters corpus, all `f.select` args):
/// - literal pair/scalar array (`[["No e-mails", 0], …]`, settings) —
///   fully static options, per-option selected ternary against the
///   record read;
/// - `options_for_select(container[, selected])` — unwrapped; selection
///   comes ONLY from the explicit arg (Rails does not re-select
///   pre-rendered options against the field);
/// - `options_from_collection_for_select(coll, "v", "t"[, selected])` —
///   loop with STATIC reader calls (the method names are literals);
/// - `A + coll.map { |x| [text, {attrs}, value] }` (messages' hat
///   picker) — static prefix + loop over the map source with the
///   lambda's element exprs inlined (pair `[t, v]` and triple with a
///   literal middle attrs-hash both handled);
/// - any other container expr — a FLAT loop (`<option value="#{el}">`)
///   matching the corpus (`@moderators`, `Category.pluck`): every such
///   site holds plain strings.
///
/// Byte-contract: matches the overlay's `select_options_for` — options
/// concatenated (no newline join), `<option[ selected="selected"]
/// value="V"[ attrs]>TEXT</option>`, to_s comparison for selection —
/// which is what the bench replay has locked in for /settings. The
/// include_blank prefix is Rails' `<option value="" label=" ">
/// </option>` shape.
fn emit_select_options(
    choices: &Expr,
    field_current: Expr,
    include_blank: bool,
    multiple: bool,
    field_str: &str,
    ctx: &ViewCtx,
) -> Option<(Vec<Expr>, Expr)> {
    // Unwrap the options_* helpers to (container, selection).
    let (container, selected): (Expr, Option<Expr>) = match &*choices.node {
        ExprNode::Send { recv: None, method, args, block: None, .. }
            if method.as_str() == "options_for_select" && !args.is_empty() =>
        {
            (args[0].clone(), args.get(1).cloned())
        }
        ExprNode::Send { recv: None, method, args, block: None, .. }
            if method.as_str() == "options_from_collection_for_select" && args.len() >= 3 =>
        {
            let (Some(v), Some(t)) = (str_or_sym_lit(&args[1]), str_or_sym_lit(&args[2]))
            else {
                return None;
            };
            return collection_options(
                &args[0],
                &v,
                &t,
                args.get(3).cloned(),
                include_blank,
                multiple,
                field_str,
                ctx,
            );
        }
        // Bare container straight from `f.select :field, <container>` —
        // Rails selects against the record's current value.
        _ => (choices.clone(), Some(field_current)),
    };

    let blank = blank_option_prefix(include_blank);
    match &*container.node {
        // Fully literal array — static options.
        ExprNode::Array { elements, .. }
            if elements.iter().all(|e| literal_choice(e).is_some()) =>
        {
            let mut parts: Vec<InterpPart> = blank;
            for e in elements {
                let (text, value) = literal_choice(e).expect("checked literal");
                push_static_option(&mut parts, &text, &value, selected.as_ref(), multiple);
            }
            Some((Vec::new(), string_interp(parts)))
        }
        // `<literal array> + <coll>.map { |x| [...] }` — static prefix,
        // then a loop over the map source.
        ExprNode::Send { recv: Some(prefix), method, args, block: None, .. }
            if method.as_str() == "+" && args.len() == 1 =>
        {
            let ExprNode::Array { elements, .. } = &*prefix.node else { return None };
            if !elements.iter().all(|e| literal_choice(e).is_some()) {
                return None;
            }
            let mut parts = blank;
            for e in elements {
                let (text, value) = literal_choice(e).expect("checked literal");
                push_static_option(&mut parts, &text, &value, selected.as_ref(), multiple);
            }
            let (setup, loop_var) =
                map_loop_options(&args[0], selected.as_ref(), multiple, field_str, parts, ctx)?;
            Some((setup, loop_var))
        }
        // Bare `<coll>.map { |x| [...] }`.
        ExprNode::Send { method, block: Some(_), .. } if method.as_str() == "map" => {
            let (setup, loop_var) =
                map_loop_options(&container, selected.as_ref(), multiple, field_str, blank, ctx)?;
            Some((setup, loop_var))
        }
        // Any other container expr — flat scalar loop (the corpus:
        // `@moderators`, `Category.pluck(:category)` — plain strings).
        _ => {
            let el = Symbol::from("_choice");
            let el_ref = Expr::new(
                Span::synthetic(),
                ExprNode::Var { id: VarId(0), name: el.clone() },
            );
            let mut option = Vec::new();
            push_dynamic_option(
                &mut option,
                to_s(el_ref.clone()),
                to_s(el_ref),
                &[],
                selected.as_ref(),
                multiple,
            );
            let (setup, var) =
                each_loop(&container, el, string_interp(option), field_str, blank, ctx);
            Some((setup, var))
        }
    }
}

/// `[text_lit, value_lit]` pair or bare scalar literal → compile-time
/// (text, value) strings. Triples and dynamic elements return None.
fn literal_choice(e: &Expr) -> Option<(String, String)> {
    fn scalar(e: &Expr) -> Option<String> {
        match &*e.node {
            ExprNode::Lit { value: Literal::Str { value } } => Some(value.clone()),
            ExprNode::Lit { value: Literal::Int { value } } => Some(value.to_string()),
            _ => None,
        }
    }
    match &*e.node {
        ExprNode::Array { elements, .. } if elements.len() == 2 => {
            Some((scalar(&elements[0])?, scalar(&elements[1])?))
        }
        _ => {
            let s = scalar(e)?;
            Some((s.clone(), s))
        }
    }
}

fn blank_option_prefix(include_blank: bool) -> Vec<InterpPart> {
    if include_blank {
        vec![InterpPart::Text {
            value: "<option value=\"\" label=\" \"></option>".to_string(),
        }]
    } else {
        Vec::new()
    }
}

/// `<option[ selected] value="V">TEXT</option>` with compile-time text
/// and value; the selected ternary is the only dynamic piece.
fn push_static_option(
    parts: &mut Vec<InterpPart>,
    text: &str,
    value: &str,
    selected: Option<&Expr>,
    multiple: bool,
) {
    parts.push(InterpPart::Text { value: "<option".to_string() });
    if let Some(sel) = selected {
        parts.push(selected_attr_part(sel.clone(), lit_str(value.to_string()), multiple));
    }
    parts.push(InterpPart::Text {
        value: format!(
            " value=\"{}\">{}</option>",
            html_escape_static(value),
            html_escape_static(text)
        ),
    });
}

/// `<option[ selected] value="#{he(v)}"[ attrs]>#{he(t)}</option>` with
/// runtime text/value/attr exprs (loop bodies).
fn push_dynamic_option(
    parts: &mut Vec<InterpPart>,
    text: Expr,
    value: Expr,
    attrs: &[(String, Expr)],
    selected: Option<&Expr>,
    multiple: bool,
) {
    parts.push(InterpPart::Text { value: "<option".to_string() });
    if let Some(sel) = selected {
        parts.push(selected_attr_part(sel.clone(), value.clone(), multiple));
    }
    parts.push(InterpPart::Text { value: " value=\"".to_string() });
    parts.push(InterpPart::Expr { expr: view_helpers_call("html_escape", vec![value]) });
    parts.push(InterpPart::Text { value: "\"".to_string() });
    for (name, v) in attrs {
        parts.push(InterpPart::Text { value: format!(" {name}=\"") });
        // `raw(...)` attr values are html_safe by contract (lobsters'
        // `"data-title" => raw(html)`) — splice unescaped.
        if let Some(inner) = raw_call_arg(v) {
            parts.push(InterpPart::Expr { expr: to_s(inner.clone()) });
        } else {
            parts.push(InterpPart::Expr {
                expr: view_helpers_call("html_escape", vec![lit_str_coerce(v.clone())]),
            });
        }
        parts.push(InterpPart::Text { value: "\"".to_string() });
    }
    parts.push(InterpPart::Text { value: ">".to_string() });
    parts.push(InterpPart::Expr { expr: view_helpers_call("html_escape", vec![text]) });
    parts.push(InterpPart::Text { value: "</option>".to_string() });
}

/// ` selected="selected"` when `sel.to_s == value.to_s` — or, for a
/// `multiple: true` select, when the selected ARRAY includes the value
/// (Rails marks every member; lobsters' tags picker selects against
/// `story.tags_a`).
fn selected_attr_part(sel: Expr, value: Expr, multiple: bool) -> InterpPart {
    let cond = if multiple {
        send(Some(sel), "include?", vec![to_s(value)], None, false)
    } else {
        send(Some(to_s(sel)), "==", vec![to_s(value)], None, false)
    };
    InterpPart::Expr {
        expr: Expr::new(
            Span::synthetic(),
            ExprNode::If {
                cond,
                then_branch: lit_str(" selected=\"selected\"".to_string()),
                else_branch: lit_str(String::new()),
            },
        ),
    }
}

/// `options_from_collection_for_select(coll, "v", "t"[, sel])` — loop
/// with STATIC reader calls (`r.v` / `r.t`; the method names are
/// literals at every corpus site, so no runtime-name dispatch
/// survives).
fn collection_options(
    coll: &Expr,
    value_method: &str,
    text_method: &str,
    selected: Option<Expr>,
    include_blank: bool,
    multiple: bool,
    field_str: &str,
    ctx: &ViewCtx,
) -> Option<(Vec<Expr>, Expr)> {
    let el = Symbol::from("_choice");
    let el_ref =
        Expr::new(Span::synthetic(), ExprNode::Var { id: VarId(0), name: el.clone() });
    let value = send(Some(el_ref.clone()), value_method, Vec::new(), None, false);
    let text = send(Some(el_ref), text_method, Vec::new(), None, false);
    let mut option = Vec::new();
    push_dynamic_option(&mut option, to_s(text), to_s(value), &[], selected.as_ref(), multiple);
    let (setup, var) = each_loop(
        coll,
        el,
        string_interp(option),
        field_str,
        blank_option_prefix(include_blank),
        ctx,
    );
    Some((setup, var))
}

/// `<coll>.map { |x| [text, value] / [text, {attrs}, value] }` — loop
/// over the map SOURCE with the lambda's element exprs inlined (the
/// loop rebinds the lambda's own param name, so the exprs read it
/// directly).
fn map_loop_options(
    map_call: &Expr,
    selected: Option<&Expr>,
    multiple: bool,
    field_str: &str,
    prefix: Vec<InterpPart>,
    ctx: &ViewCtx,
) -> Option<(Vec<Expr>, Expr)> {
    let ExprNode::Send { recv: Some(coll), method, block: Some(block), .. } = &*map_call.node
    else {
        return None;
    };
    if method.as_str() != "map" {
        return None;
    }
    let ExprNode::Lambda { params, body, .. } = &*block.node else { return None };
    let el = params.first().cloned()?;
    // The lambda pieces were argument-position code the template walk
    // never touched — run the helper rewrite over them so `h(x)` (the
    // ERB escape alias in lobsters' tags picker) and friends ground to
    // ViewHelpers before hoisting.
    let body = super::walker::rewrite_helpers_in_expr(body, ctx);
    // The lambda body: a bare `[text, …]` literal, or a Seq of
    // per-element statements ENDING in one (lobsters' tags picker
    // builds an `html` local across conditionals before the triple) —
    // the leading statements hoist into the loop body ahead of the
    // option append, where the element param is still in scope.
    let (leading, array_elements): (Vec<Expr>, _) = match &*body.node {
        ExprNode::Array { elements, .. } => (Vec::new(), elements.clone()),
        ExprNode::Seq { exprs } => match exprs.split_last() {
            Some((last, init)) => match &*last.node {
                ExprNode::Array { elements, .. } => (init.to_vec(), elements.clone()),
                _ => return None,
            },
            None => return None,
        },
        _ => return None,
    };
    // Pair `[text, value]`, or triple with the attrs hash in EITHER
    // position Rails accepts: `[text, {attrs}, value]` (messages) and
    // `[text, value, {attrs}]` (the documented form; lobsters' tags).
    let (text, attrs_hash, value) = match array_elements.as_slice() {
        [t, v] if !matches!(&*v.node, ExprNode::Hash { .. }) => (t.clone(), None, v.clone()),
        [t, a, v] if matches!(&*a.node, ExprNode::Hash { .. }) => {
            (t.clone(), Some(a.clone()), v.clone())
        }
        [t, v, a] if matches!(&*a.node, ExprNode::Hash { .. }) => {
            (t.clone(), Some(a.clone()), v.clone())
        }
        _ => return None,
    };
    let mut attrs: Vec<(String, Expr)> = Vec::new();
    if let Some(a) = attrs_hash {
        let ExprNode::Hash { entries, .. } = &*a.node else { return None };
        for (k, v) in entries {
            let name = match &*k.node {
                ExprNode::Lit { value: Literal::Str { value } } => value.clone(),
                ExprNode::Lit { value: Literal::Sym { value } } => value.as_str().to_string(),
                _ => return None,
            };
            attrs.push((name, v.clone()));
        }
    }
    let mut option = Vec::new();
    push_dynamic_option(&mut option, to_s(text), to_s(value), &attrs, selected, multiple);
    let mut loop_body = leading;
    loop_body.push(options_append(
        &Symbol::from(format!("_options_{field_str}")),
        string_interp(option),
    ));
    Some(each_loop_with_body(
        coll,
        el,
        seq_of(loop_body),
        field_str,
        prefix,
        ctx,
    ))
}

fn seq_of(mut exprs: Vec<Expr>) -> Expr {
    if exprs.len() == 1 {
        exprs.remove(0)
    } else {
        Expr::new(Span::synthetic(), ExprNode::Seq { exprs })
    }
}

/// Build `(setup, options_var_ref)`: a `_options_<field>` accumulator
/// seeded with any static prefix, an `each` loop appending one option
/// per element, and the Var read that splices into the `<select>`.
fn each_loop(
    coll: &Expr,
    el: Symbol,
    option_interp: Expr,
    field_str: &str,
    prefix: Vec<InterpPart>,
    ctx: &ViewCtx,
) -> (Vec<Expr>, Expr) {
    let body = options_append(
        &Symbol::from(format!("_options_{field_str}")),
        option_interp,
    );
    each_loop_with_body(coll, el, body, field_str, prefix, ctx)
}

/// Like `each_loop`, but the caller supplies the FULL loop body (the
/// Seq-bodied map arm hoists per-element statements ahead of its
/// option append).
fn each_loop_with_body(
    coll: &Expr,
    el: Symbol,
    loop_body: Expr,
    field_str: &str,
    prefix: Vec<InterpPart>,
    _ctx: &ViewCtx,
) -> (Vec<Expr>, Expr) {
    let var_name = Symbol::from(format!("_options_{field_str}"));
    let mut setup: Vec<Expr> = Vec::new();
    setup.push(super::assign_accumulator_string_new(var_name.as_str()));
    if !prefix.is_empty() {
        setup.push(options_append(&var_name, string_interp(prefix)));
    }
    let lambda = Expr::new(
        Span::synthetic(),
        ExprNode::Lambda {
            params: vec![el],
            block_param: None,
            body: loop_body,
            block_style: crate::expr::BlockStyle::Do,
        },
    );
    setup.push(send(Some(coll.clone()), "each", Vec::new(), Some(lambda), false));
    let var_ref =
        Expr::new(Span::synthetic(), ExprNode::Var { id: VarId(0), name: var_name });
    (setup, var_ref)
}

fn options_append(var: &Symbol, value: Expr) -> Expr {
    let var_ref =
        Expr::new(Span::synthetic(), ExprNode::Var { id: VarId(0), name: var.clone() });
    send(Some(var_ref), "<<", vec![value], None, false)
}

/// Compile-time HTML escape for static option text/values (same 5-char
/// set as `ViewHelpers::HTML_ESCAPES`).
fn html_escape_static(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// A `"literal"` / `:symbol` literal's string value.
fn str_or_sym_lit(e: &Expr) -> Option<String> {
    match &*e.node {
        ExprNode::Lit { value: Literal::Str { value } } => Some(value.clone()),
        ExprNode::Lit { value: Literal::Sym { value } } => Some(value.as_str().to_string()),
        _ => None,
    }
}

/// `<button name="button" type="submit"<opts>>TEXT</button>` — the
/// default type yields to a caller-supplied `type:` opt.
/// Bare `<%= button_tag content, opts %>` — the builder-less sibling
/// of `form.button`, riding the same `<button>` emission (lobsters'
/// story-form "Fetch Title" button). Blockless form only; no corpus
/// site passes a block.
pub(super) fn emit_button_tag(args: &[Expr], ctx: &ViewCtx) -> Vec<Expr> {
    let (positional, opts) = split_args(args);
    emit_button(positional.first().copied(), opts.as_slice(), ctx)
}

/// Bare `<%= check_box_tag name[, value[, checked]][, opts] %>` — the
/// model-less checkbox: `<input type="checkbox" name="N" id="I"
/// value="V"[ checked="checked"][opts]>` in Rails' attr order. The
/// default id is Rails' sanitize_to_id of the name — compile-time for
/// a literal name, the typed runtime helper for an interp (`"tags[
/// #{tag.tag}]"` on the filters page); an explicit `id:` opt wins
/// (messages' delete_all). The third positional is the checked
/// EXPRESSION (`@filtered_tags.include?(tag.id)`) — a plain ternary.
pub(super) fn emit_check_box_tag(args: &[Expr], ctx: &ViewCtx) -> Vec<Expr> {
    let (positional, opts) = split_args(args);
    let Some(name) = positional.first() else {
        return vec![accumulator_append_call(lit_str(String::new()), ctx)];
    };
    let explicit_id = opts.iter().find_map(|(k, v)| {
        matches!(&*k.node, ExprNode::Lit { value: Literal::Sym { value } }
            if value.as_str() == "id")
        .then(|| (*v).clone())
    });
    let attr_opts: Vec<(Expr, Expr)> = opts
        .iter()
        .filter(|(k, _)| {
            !matches!(&*k.node, ExprNode::Lit { value: Literal::Sym { value } }
                if value.as_str() == "id")
        })
        .cloned()
        .collect();

    let mut parts: Vec<InterpPart> = Vec::new();
    parts.push(InterpPart::Text { value: "<input type=\"checkbox\" name=\"".to_string() });
    let name_lit = match &*name.node {
        ExprNode::Lit { value: Literal::Str { value } } => Some(value.clone()),
        _ => None,
    };
    match &name_lit {
        Some(lit) => parts.push(InterpPart::Text { value: html_escape_static(lit) }),
        None => parts.push(InterpPart::Expr {
            expr: view_helpers_call("html_escape", vec![to_s((*name).clone())]),
        }),
    }
    parts.push(InterpPart::Text { value: "\" id=\"".to_string() });
    match explicit_id {
        Some(id) => match &*id.node {
            ExprNode::Lit { value: Literal::Str { value } } => {
                parts.push(InterpPart::Text { value: html_escape_static(value) })
            }
            _ => parts.push(InterpPart::Expr {
                expr: view_helpers_call("html_escape", vec![to_s(id.clone())]),
            }),
        },
        None => match &name_lit {
            Some(lit) => {
                parts.push(InterpPart::Text { value: sanitize_to_id_static(lit) })
            }
            // sanitize_to_id's output alphabet is attr-safe by
            // construction — no escape wrapper needed.
            None => parts.push(InterpPart::Expr {
                expr: view_helpers_call("sanitize_to_id", vec![to_s((*name).clone())]),
            }),
        },
    }
    parts.push(InterpPart::Text { value: "\" value=\"".to_string() });
    match positional.get(1) {
        None => parts.push(InterpPart::Text { value: "1".to_string() }),
        Some(v) => match &*v.node {
            ExprNode::Lit { value: Literal::Str { value } } => {
                parts.push(InterpPart::Text { value: html_escape_static(value) })
            }
            _ => parts.push(InterpPart::Expr {
                expr: view_helpers_call("html_escape", vec![to_s((*v).clone())]),
            }),
        },
    }
    parts.push(InterpPart::Text { value: "\"".to_string() });
    if let Some(checked) = positional.get(2) {
        parts.push(InterpPart::Expr {
            expr: Expr::new(
                Span::synthetic(),
                ExprNode::If {
                    cond: (*checked).clone(),
                    then_branch: lit_str(" checked=\"checked\"".to_string()),
                    else_branch: lit_str(String::new()),
                },
            ),
        });
    }
    append_attr_parts(&mut parts, &attr_opts);
    parts.push(InterpPart::Text { value: ">".to_string() });
    vec![accumulator_append_call(string_interp(parts), ctx)]
}

/// Bare `<%= label_tag name[, content][, opts] %>` and the block form
/// (`<%= label_tag "tags[#{tag.tag}]" do %>…<% end %>`, filters page).
/// `<label for="SANITIZED(name)"[opts]>CONTENT</label>` — the for-attr
/// gets Rails' sanitize_to_id (identity for the settings page's plain
/// `:gravatar`-style names, so the replay-locked bytes hold; the
/// filters page's `tags[…]` names sanitize to match the checkbox ids
/// beside them). Blockless content comes from the second positional
/// (escaped); the block form splices its walked body. A blockless call
/// with NO content and a non-literal name stays on the runtime helper
/// (the humanized-default path; no corpus site).
pub(super) fn emit_label_tag(
    args: &[Expr],
    block: Option<&Expr>,
    ctx: &ViewCtx,
) -> Option<Vec<Expr>> {
    let (positional, opts) = split_args(args);
    let name = positional.first()?;
    let name_lit = match &*name.node {
        ExprNode::Lit { value: Literal::Str { value } } => Some(value.clone()),
        ExprNode::Lit { value: Literal::Sym { value } } => Some(value.as_str().to_string()),
        _ => None,
    };
    let mut open: Vec<InterpPart> = Vec::new();
    open.push(InterpPart::Text { value: "<label for=\"".to_string() });
    match &name_lit {
        Some(lit) => open.push(InterpPart::Text { value: sanitize_to_id_static(lit) }),
        None => open.push(InterpPart::Expr {
            expr: view_helpers_call("sanitize_to_id", vec![to_s((*name).clone())]),
        }),
    }
    open.push(InterpPart::Text { value: "\"".to_string() });
    append_attr_parts(&mut open, &opts);
    open.push(InterpPart::Text { value: ">".to_string() });

    if let Some(block) = block {
        let ExprNode::Lambda { body, .. } = &*block.node else { return None };
        let mut out = vec![accumulator_append_call(string_interp(open), ctx)];
        out.extend(walk_body(body, ctx));
        out.push(accumulator_append_call(lit_str("</label>".to_string()), ctx));
        return Some(out);
    }

    let content = positional.get(1)?;
    let mut parts = open;
    match &*content.node {
        ExprNode::Lit { value: Literal::Str { value } } => {
            parts.push(InterpPart::Text { value: html_escape_static(value) })
        }
        _ => parts.push(InterpPart::Expr {
            expr: view_helpers_call("html_escape", vec![to_s((*content).clone())]),
        }),
    }
    parts.push(InterpPart::Text { value: "</label>".to_string() });
    Some(vec![accumulator_append_call(string_interp(parts), ctx)])
}

/// Compile-time mirror of the runtime `sanitize_to_id` (Rails: drop
/// "]", non-[-a-zA-Z0-9:.] → "_").
fn sanitize_to_id_static(name: &str) -> String {
    name.chars()
        .filter(|c| *c != ']')
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == ':' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// The opening `<button …>` tag. Rails names the control `button` and
/// defaults its type to `submit` unless the caller gave one. Shared by
/// the inline form and the BLOCK form below so the two spellings can't
/// drift in their attribute rendering.
pub(crate) fn button_open_parts(opts: &[(Expr, Expr)]) -> Vec<InterpPart> {
    let has_type = opts.iter().any(|(k, _)| {
        matches!(&*k.node, ExprNode::Lit { value: Literal::Sym { value } }
            if value.as_str() == "type")
    });
    let mut parts: Vec<InterpPart> = Vec::new();
    parts.push(InterpPart::Text {
        value: if has_type {
            "<button name=\"button\"".to_string()
        } else {
            "<button name=\"button\" type=\"submit\"".to_string()
        },
    });
    append_attr_parts(&mut parts, opts);
    parts.push(InterpPart::Text { value: ">".to_string() });
    parts
}

/// Inline-expand `<%= form.button(opts) do %> … <% end %>` — the BLOCK
/// form, where the button's label is template markup rather than a
/// string argument.
///
/// Campfire writes this everywhere a button holds an icon beside its
/// text, including the sign-in page and the message composer. Without
/// it the call falls past the form-builder dispatch (which matches
/// `block: None`) and survives into the emit as a literal
/// `form.button(…) do … end` with `form` unbound.
///
/// Same open/walk/close splice as the tag builder's block form: the
/// block body is template buffer ops, so it is WALKED against the outer
/// accumulator rather than captured.
pub(super) fn emit_form_builder_block_inline(
    binding: &FormBuilderBinding,
    kind: FormBuilderMethod,
    args: &[Expr],
    block: &Expr,
    ctx: &ViewCtx,
) -> Option<Vec<Expr>> {
    let ExprNode::Lambda { params, body, .. } = &*block.node else {
        return None;
    };
    let (positional, opts) = split_args(args);
    match kind {
        FormBuilderMethod::Button => {
            let mut out =
                vec![accumulator_append_call(string_interp(button_open_parts(opts.as_slice())), ctx)];
            let inner = ctx.with_locals(params.iter().map(|p| p.as_str().to_string()));
            out.extend(walk_body(body, &inner));
            out.push(accumulator_append_call(lit_str("</button>".to_string()), ctx));
            Some(out)
        }
        FormBuilderMethod::FieldsFor => {
            emit_fields_for(binding, &positional, params, body, ctx)
        }
        // Every other builder method is blockless in the corpus.
        _ => None,
    }
}

/// `form.fields_for :settings, obj do |settings_form| … end`.
///
/// Renders NO markup of its own — Rails' `fields_for` opens no tag and
/// closes none. What it does is bind a second FormBuilder whose object
/// name is `parent[nested]`, so every field inside names
/// `account[settings][x]` and ids `account_settings_x` (the bracket-to-
/// underscore step is `field_id`'s, transcribed from Rails).
///
/// The nested RECORD is the second positional when given, else Rails'
/// default — the parent object's reader of the same name. It is bound to
/// a synthesized local for the same reason `form_with` binds one: the
/// expression can be an arbitrary chain (`@account.settings`) and the
/// field-value reads want a plain Var to dispatch on.
///
/// GAP worth naming: a nested field with no explicit `value:` reads
/// `<record>.<field>`, and when the "record" is a `has_json` column that
/// object does not exist — the store's readers were flattened onto the
/// PARENT model (`account.settings_x`), so the local holds the raw JSON
/// String. campfire's only `fields_for` passes `value:` at every field,
/// so nothing in the corpus reaches that read; an app that does needs
/// the store-flattening to be taught about nested bindings, not a
/// workaround here.
fn emit_fields_for(
    binding: &FormBuilderBinding,
    positional: &[&Expr],
    params: &[Symbol],
    body: &Expr,
    ctx: &ViewCtx,
) -> Option<Vec<Expr>> {
    let nested = field_symbol(positional.first().copied())?;
    let nested_param = params
        .first()
        .cloned()
        .unwrap_or_else(|| Symbol::from(format!("{}_form", nested.as_str())));
    let nested_param_str = nested_param.as_str().to_string();

    let record_expr = positional.get(1).map(|e| (*e).clone()).unwrap_or_else(|| {
        send(
            Some(Expr::new(
                Span::synthetic(),
                ExprNode::Var { id: VarId(0), name: binding.record_var.clone() },
            )),
            nested.as_str(),
            Vec::new(),
            None,
            false,
        )
    });
    let record_var = Symbol::from(format!("{nested_param_str}_record"));
    let mut out = vec![Expr::new(
        Span::synthetic(),
        ExprNode::Assign {
            target: super::LValue::Var { id: VarId(0), name: record_var.clone() },
            value: record_expr,
        },
    )];

    let mut inner = ctx.with_locals([nested_param_str.clone()]);
    inner.form_records.push(FormBuilderBinding {
        form_param: nested_param_str.clone(),
        model_name: format!("{}[{}]", binding.model_name, nested.as_str()),
        record_var: record_var.clone(),
        // The parent's, unchanged: `form.submit`'s default text and the
        // `_method` override belong to the FORM, and `fields_for` opens
        // no form of its own.
        form_method_var: binding.form_method_var.clone(),
        id_prefix: binding.id_prefix.clone(),
    });
    let body = super::form_with::rewrite_form_object_reads(body, &nested_param_str, &record_var);
    out.extend(walk_body(&body, &inner));
    Some(out)
}

fn emit_button(
    text: Option<&Expr>,
    opts: &[(Expr, Expr)],
    ctx: &ViewCtx,
) -> Vec<Expr> {
    let mut parts = button_open_parts(opts);
    if let Some(t) = text {
        // `raw("Fetch&nbsp;Title")` content is html_safe by contract —
        // unwrap and emit verbatim (a literal folds to static text)
        // instead of double-escaping the entities.
        if let Some(inner) = raw_call_arg(t) {
            if let ExprNode::Lit { value: Literal::Str { value } } = &*inner.node {
                parts.push(InterpPart::Text { value: value.clone() });
            } else {
                parts.push(InterpPart::Expr { expr: to_s(inner.clone()) });
            }
        } else {
            parts.push(InterpPart::Expr {
                expr: view_helpers_call("html_escape", vec![to_s(t.clone())]),
            });
        }
    }
    parts.push(InterpPart::Text { value: "</button>".to_string() });
    vec![accumulator_append_call(string_interp(parts), ctx)]
}

/// The argument of a `raw(...)` call — bare or already
/// ViewHelpers-prefixed (the walker may rewrite helper calls before
/// this classifier sees them).
fn raw_call_arg(e: &Expr) -> Option<&Expr> {
    let ExprNode::Send { recv, method, args, block: None, .. } = &*e.node else {
        return None;
    };
    if method.as_str() != "raw" || args.len() != 1 {
        return None;
    }
    match recv {
        None => Some(&args[0]),
        Some(r) => matches!(&*r.node, ExprNode::Const { path }
            if path.last().is_some_and(|s| s.as_str() == "ViewHelpers"))
        .then(|| &args[0]),
    }
}

/// `<input type="<ty>" …>` — the text_field shape with a different
/// `type` (url_field / email_field).
fn emit_typed_input_field(
    ty: &str,
    field: Option<&Expr>,
    opts: &[(Expr, Expr)],
    binding: &FormBuilderBinding,
    ctx: &ViewCtx,
) -> Vec<Expr> {
    let Some(field_sym) = field_symbol(field) else {
        return vec![accumulator_append_call(lit_str(String::new()), ctx)];
    };
    let field_str = field_sym.as_str();
    let value_read = field_value_read(binding, field_sym.clone(), ctx);
    let mut parts: Vec<InterpPart> = Vec::new();
    parts.push(InterpPart::Text {
        value: format!("<input type=\"{ty}\"{}", name_id_attrs_for(binding, field_str, opts)),
    });
    parts.push(InterpPart::Expr {
        expr: view_helpers_call("optional_value_attr", vec![value_read]),
    });
    append_attr_parts(&mut parts, opts);
    parts.push(InterpPart::Text { value: ">".to_string() });
    vec![accumulator_append_call(string_interp(parts), ctx)]
}

/// `<expr>.to_s` — the coercion the escape helpers expect.
fn to_s(e: Expr) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(e),
            method: Symbol::from("to_s"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    )
}

/// `<input type="<ty>" name="..." id="..."<opts>>` — a typed input that
/// carries NO `value=` attribute, unlike [`emit_typed_input_field`].
///
/// Two field types want this, for different reasons that land on the
/// same shape: Rails never echoes a password back, and a file input
/// cannot hold a value at all (the browser rejects it — a page can't
/// pre-fill a user's filesystem path). A caller-supplied `value:` opt
/// still flows through `append_attr_parts` either way.
fn emit_valueless_input_field(
    ty: &str,
    field: Option<&Expr>,
    opts: &[(Expr, Expr)],
    binding: &FormBuilderBinding,
    ctx: &ViewCtx,
) -> Vec<Expr> {
    let Some(field_sym) = field_symbol(field) else {
        return vec![accumulator_append_call(lit_str(String::new()), ctx)];
    };
    let mut parts: Vec<InterpPart> = Vec::new();
    parts.push(InterpPart::Text {
        value: format!(
            "<input type=\"{ty}\"{}",
            name_id_attrs_for(binding, field_sym.as_str(), opts)
        ),
    });
    append_attr_parts(&mut parts, opts);
    parts.push(InterpPart::Text { value: ">".to_string() });
    vec![accumulator_append_call(string_interp(parts), ctx)]
}

/// `<input type="hidden" name="..." id="..."<value><opts>>` — inline
/// expansion of `form.hidden_field :field [, opts]`. The value comes from
/// an explicit `value:` opt when present (`hidden_field :referer, value:
/// @referer`), otherwise the record's attribute (resource forms) or nil
/// (non-resource) via `optional_value_attr`.
fn emit_hidden_field(
    field: Option<&Expr>,
    opts: &[(Expr, Expr)],
    binding: &FormBuilderBinding,
    ctx: &ViewCtx,
) -> Vec<Expr> {
    let Some(field_sym) = field_symbol(field) else {
        return vec![accumulator_append_call(lit_str(String::new()), ctx)];
    };
    let mut parts: Vec<InterpPart> = Vec::new();
    parts.push(InterpPart::Text {
        value: format!(
            "<input type=\"hidden\"{}",
            name_id_attrs_for(binding, field_sym.as_str(), opts)
        ),
    });
    if !opts_have_value(opts) {
        parts.push(InterpPart::Expr {
            expr: view_helpers_call(
                "optional_value_attr",
                vec![field_value_read(binding, field_sym.clone(), ctx)],
            ),
        });
    }
    append_attr_parts(&mut parts, opts);
    parts.push(InterpPart::Text { value: ">".to_string() });
    vec![accumulator_append_call(string_interp(parts), ctx)]
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

/// The ` name="..." id="..."` fragment for a form field. A resource form
/// nests the field under the model prefix (`user[email]` / `user_email`);
/// a non-resource form (`form_with url:` — empty `model_name`) names the
/// field bare (`email` / `email`), matching Rails' non-model form output.
fn name_id_attrs(binding: &FormBuilderBinding, field: &str) -> String {
    let id = super::field_id(&binding.id_prefix, &binding.model_name, field);
    if binding.model_name.is_empty() {
        format!(" name=\"{field}\" id=\"{id}\"")
    } else {
        format!(" name=\"{}[{field}]\" id=\"{id}\"", binding.model_name)
    }
}

/// Same, minus any attribute the call site set explicitly. Rails' field
/// helpers OVERRIDE the generated `name`/`id` from the options rather
/// than emitting both — `options.fetch("name") { tag_name(...) }`.
/// lobsters' settings form does exactly this
/// (`f.password_field :current_password, name: "current_password"`,
/// so the controller reads it outside `user[...]`), and emitting both
/// produced a duplicate `name=` attribute — the browser keeps the
/// FIRST, so the field posted under the wrong key.
fn name_id_attrs_for(binding: &FormBuilderBinding, field: &str, opts: &[(Expr, Expr)]) -> String {
    let has = |key: &str| {
        opts.iter().any(|(k, _)| {
            matches!(&*k.node, ExprNode::Lit { value: Literal::Sym { value } }
                if value.as_str() == key)
        })
    };
    let full = name_id_attrs(binding, field);
    if !has("name") && !has("id") {
        return full;
    }
    // Rebuild keeping only the halves the call site did NOT set.
    let id = super::field_id(&binding.id_prefix, &binding.model_name, field);
    let name = if binding.model_name.is_empty() {
        field.to_string()
    } else {
        format!("{}[{field}]", binding.model_name)
    };
    let mut out = String::new();
    if !has("name") {
        out.push_str(&format!(" name=\"{name}\""));
    }
    if !has("id") {
        out.push_str(&format!(" id=\"{id}\""));
    }
    out
}

/// The `<label for="...">` open fragment, prefixed for resource forms and
/// bare for non-resource forms (see `name_id_attrs`).
fn label_for_attr(binding: &FormBuilderBinding, field: &str) -> String {
    format!(
        "<label for=\"{}\"",
        super::field_id(&binding.id_prefix, &binding.model_name, field)
    )
}

/// The value expression a field reads: the record's attribute for a
/// resource form, or `nil` for a non-resource form (no record to read —
/// `optional_value_attr(nil)` then omits the `value=` attr, matching Rails
/// rendering an empty non-model field).
fn field_value_read(binding: &FormBuilderBinding, field: Symbol, ctx: &ViewCtx) -> Expr {
    if binding.model_name.is_empty() {
        Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Nil })
    } else {
        record_field_read(binding, field, ctx)
    }
}

/// True when `opts` carries an explicit `value:` — a `hidden_field` with a
/// caller-supplied value uses it instead of reading the record attribute.
fn opts_have_value(opts: &[(Expr, Expr)]) -> bool {
    opts.iter().any(|(k, _)| {
        matches!(&*k.node, ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == "value")
    })
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
    text: Option<&Expr>,
    opts: &[(Expr, Expr)],
    binding: &FormBuilderBinding,
    ctx: &ViewCtx,
) -> Vec<Expr> {
    let Some(field_sym) = field_symbol(field) else {
        return vec![accumulator_append_call(lit_str(String::new()), ctx)];
    };
    let mut parts: Vec<InterpPart> = Vec::new();
    parts.push(InterpPart::Text {
        value: label_for_attr(binding, field_sym.as_str()),
    });
    append_attr_parts(&mut parts, opts);
    parts.push(InterpPart::Text { value: ">".to_string() });
    // Explicit text positional (`f.label :username, "Username:"`) wins
    // over the humanized field name; a literal folds into the text run,
    // anything else escapes at runtime.
    match text {
        Some(t) => match &*t.node {
            ExprNode::Lit { value: Literal::Str { value } } => {
                parts.push(InterpPart::Text { value: value.clone() });
            }
            _ => parts.push(InterpPart::Expr {
                expr: view_helpers_call("html_escape", vec![to_s(t.clone())]),
            }),
        },
        None => parts.push(InterpPart::Text {
            value: capitalize_ascii(field_sym.as_str()),
        }),
    }
    parts.push(InterpPart::Text { value: "</label>".to_string() });
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
    let field_str = field_sym.as_str();
    let value_read = field_value_read(binding, field_sym.clone(), ctx);
    let mut parts: Vec<InterpPart> = Vec::new();
    parts.push(InterpPart::Text {
        value: format!("<input type=\"text\"{}", name_id_attrs_for(binding, field_str, opts)),
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

/// A textarea has no `size` attribute. Rails splits `size: "100x5"`
/// into `cols="100" rows="5"` and drops the `size` key (a bare
/// `size: 100` sets cols only). lobsters' settings page writes
/// `f.text_area :about, size: "100x5"`, which reached the page as a
/// literal `size="100x5"` — an attribute no browser acts on, so the
/// About box rendered at its default dimensions.
fn expand_textarea_size(opts: &mut Vec<(Expr, Expr)>) {
    let Some(size) = super::attr_parts::take_opt(opts, "size") else {
        return;
    };
    let Some(spec) = str_or_sym_lit(&size) else {
        // A computed size can't be split at compile time. Rails would,
        // at request time; putting the un-split value back keeps the
        // old behavior rather than dropping the author's intent.
        opts.push((lit_sym(Symbol::from("size")), size));
        return;
    };
    let (cols, rows) = match spec.split_once('x') {
        Some((c, r)) => (c.to_string(), Some(r.to_string())),
        None => (spec, None),
    };
    opts.push((lit_sym(Symbol::from("cols")), lit_str(cols)));
    if let Some(rows) = rows {
        opts.push((lit_sym(Symbol::from("rows")), lit_str(rows)));
    }
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
    let field_str = field_sym.as_str();
    // A textarea has no `value` ATTRIBUTE — Rails deletes `value:` from
    // the options and renders it as the element's body instead, so an
    // explicit `value:` overrides the record read. lobsters' comment box
    // relies on this (`f.text_area "comment", value: comment.comment`);
    // rendered as an attribute it produced an empty box carrying a
    // stray `value=""`.
    let mut opts = opts.to_vec();
    let body_value = super::attr_parts::take_opt(&mut opts, "value")
        .unwrap_or_else(|| field_value_read(binding, field_sym.clone(), ctx));
    expand_textarea_size(&mut opts);
    let mut parts: Vec<InterpPart> = Vec::new();
    parts.push(InterpPart::Text {
        value: format!("<textarea{}", name_id_attrs_for(binding, field_str, &opts)),
    });
    append_attr_parts(&mut parts, &opts);
    // Rails opens a textarea's content with a newline (the HTML spec
    // lets a parser swallow one, so it protects a body that itself
    // starts with one).
    parts.push(InterpPart::Text {
        value: ">\n".to_string(),
    });
    parts.push(InterpPart::Expr {
        expr: view_helpers_call("escape_or_empty", vec![body_value]),
    });
    parts.push(InterpPart::Text {
        value: "</textarea>".to_string(),
    });
    vec![accumulator_append_call(string_interp(parts), ctx)]
}

/// The custom element the rich-text editor instantiates on, and the
/// default class Rails gives it.
///
/// ONE PLACE, on purpose. Action Text's server side is editor-neutral
/// — the same `has_rich_text`, the same `ActionText::RichText` row,
/// the same canonical `<action-text-attachment>` markup — and the only
/// thing that changes between front ends is this pair. Trix is what
/// Rails ships and what the corpus runs; Lexxy (the Basecamp editor
/// that supersedes it) is `("lexxy-editor", "lexxy-content")` plus its
/// own two data attributes, and nothing else in this file would move.
///
/// It is a constant rather than a setting because nothing yet reads
/// the app's bundle: roundhouse does not ingest the Gemfile, so there
/// is no evidence to switch on. When it does, this is the switch.
const EDITOR_TAG: &str = "trix-editor";
const EDITOR_DEFAULT_CLASS: &str = "trix-content";

/// Active Storage's conventional mount points, which Trix reads to
/// upload a dropped file and to build a blob URL.
///
/// LITERAL, and stated as such: Rails fills these from
/// `rails_direct_uploads_url` / `rails_service_blob_url`, route
/// helpers the Active Storage engine installs. Active Storage is not
/// modeled, so the routes do not exist here and neither does anything
/// that could serve an upload. Emitting the conventional paths keeps
/// the markup Rails-shaped for the editor's own initialization (Trix
/// reads both attributes at connect time) and costs nothing; what it
/// does NOT do is make attachments work.
const DIRECT_UPLOAD_URL: &str = "/rails/active_storage/direct_uploads";
const BLOB_URL_TEMPLATE: &str =
    "/rails/active_storage/blobs/redirect/:signed_id/:filename";

/// `form.rich_text_area :body [, opts]` — Rails' `rich_textarea_tag`,
/// inline.
///
/// Two elements, in Rails' order:
///
/// ```html
/// <input type="hidden" name="message[body]" id="message_body_trix_input" value="…" autocomplete="off">
/// <trix-editor id="message_body" input="message_body_trix_input" class="trix-content" data-…></trix-editor>
/// ```
///
/// The hidden input is the one that submits: Trix writes the editor's
/// markup into it on every change, so `message[body]` arrives as HTML
/// and lands on the `body=` writer `has_rich_text` synthesized. The
/// editor element carries the id pairing (`input=`) that binds them.
///
/// Two departures from Rails, both deliberate:
///
/// * The input's id is `<field_id>_trix_input`, where Rails appends
///   the record's dom id (`message_body_trix_input_message_5`). Rails
///   needs the suffix because two forms for DIFFERENT records can
///   share a page; the suffix is what keeps their hidden inputs
///   distinct. Same-page duplicates are therefore a real (and
///   detectable) divergence — and the fix is a `dom_id(record, …)`
///   call here once a fixture exercises it.
///
/// * `class:` REPLACES `trix-content` rather than adding to it, which
///   is Rails' own `options[:class] ||=` semantics — campfire passes
///   `class: "input"` and gets exactly that.
fn emit_rich_text_area(
    field: Option<&Expr>,
    opts: &[(Expr, Expr)],
    binding: &FormBuilderBinding,
    ctx: &ViewCtx,
) -> Vec<Expr> {
    let Some(field_sym) = field_symbol(field) else {
        return vec![accumulator_append_call(lit_str(String::new()), ctx)];
    };
    let field_str = field_sym.as_str();
    let editor_id = super::field_id(&binding.id_prefix, &binding.model_name, field_str);
    let input_id = format!("{editor_id}_trix_input");
    let name = if binding.model_name.is_empty() {
        field_str.to_string()
    } else {
        format!("{}[{field_str}]", binding.model_name)
    };

    let mut opts = opts.to_vec();
    // Rails deletes `value:` from the editor's options and routes it to
    // the hidden input; without an explicit one the value is the
    // record's own markup.
    let value = super::attr_parts::take_opt(&mut opts, "value").unwrap_or_else(|| {
        rich_text_markup_read(binding, field_sym.clone())
    });

    let mut parts: Vec<InterpPart> = Vec::new();
    parts.push(InterpPart::Text {
        value: format!("<input type=\"hidden\" name=\"{name}\" id=\"{input_id}\" value=\""),
    });
    parts.push(InterpPart::Expr {
        expr: view_helpers_call("escape_or_empty", vec![value]),
    });
    parts.push(InterpPart::Text {
        value: format!(
            "\" autocomplete=\"off\"><{EDITOR_TAG} id=\"{editor_id}\" input=\"{input_id}\""
        ),
    });
    // `class:` from the call site wins; otherwise Rails' default. Taken
    // out of `opts` either way so `append_attr_parts` cannot emit a
    // second `class=`.
    match super::attr_parts::take_opt(&mut opts, "class") {
        Some(cls) => {
            parts.push(InterpPart::Text { value: " class=\"".to_string() });
            parts.push(InterpPart::Expr {
                expr: view_helpers_call("html_escape", vec![simplify_class_array(&cls)]),
            });
            parts.push(InterpPart::Text { value: "\"".to_string() });
        }
        None => parts.push(InterpPart::Text {
            value: format!(" class=\"{EDITOR_DEFAULT_CLASS}\""),
        }),
    }
    parts.push(InterpPart::Text {
        value: format!(
            " data-direct-upload-url=\"{DIRECT_UPLOAD_URL}\" \
             data-blob-url-template=\"{BLOB_URL_TEMPLATE}\""
        ),
    });
    append_attr_parts(&mut parts, &opts);
    parts.push(InterpPart::Text {
        value: format!("></{EDITOR_TAG}>"),
    });
    vec![accumulator_append_call(string_interp(parts), ctx)]
}

/// The markup the hidden input carries: `record.<field>.to_trix_html`.
///
/// Two things this does NOT do, both because a rich-text attribute is
/// not a column:
///
/// * It does not read `record[:field]`, which is how the other field
///   helpers reach a value. The indexer answers from the attributes
///   hash, and there is no `body` key in it — the markup lives one
///   table over. Only the synthesized reader knows that, so the read
///   is a plain method call.
///
/// * It does not stop at the record. `record.body` is the
///   `ActionText::RichText` ROW; interpolating that prints an object.
///   `to_trix_html` is the accessor Rails itself reaches for here
///   (`value.try(:to_trix_html) || value`), and `has_rich_text`
///   guarantees the reader never returns nil, so the `try` has nothing
///   left to guard.
fn rich_text_markup_read(binding: &FormBuilderBinding, field: Symbol) -> Expr {
    if binding.model_name.is_empty() {
        return Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Nil });
    }
    let record_ref = Expr::new(
        Span::synthetic(),
        ExprNode::Var { id: VarId(0), name: binding.record_var.clone() },
    );
    send(
        Some(send(Some(record_ref), field.as_str(), Vec::new(), None, false)),
        "to_trix_html",
        Vec::new(),
        None,
        false,
    )
}

/// `<input type="submit" name="commit" value="<text>" data-disable-with="<text>"<opts>>`
/// — inline expansion of `form.submit [label] [, opts]`. When the
/// positional `label` is omitted, the default text branches on the
/// captured form method: `:patch` → "Update <ModelName>",
/// otherwise → "Create <ModelName>". `<ModelName>` is the
/// capitalized model_name (lowered to a literal at this point).
/// Bare `<%= submit_tag label, opts %>` — the builder-less sibling of
/// `form.submit`, same `<input type="submit" name="commit" …>` shape
/// with Rails' bare default text ("Save changes") instead of the
/// builder's Create/Update branch. Inline-expanded for the same reason
/// the builder methods are: the opts hashes are literal at every call
/// site, and the runtime alternative (the CRuby overlay's
/// `options.each` + `is_a?(Hash)` walk) is the shape the typed
/// runtime refuses. Args split like a builder call: first non-Hash
/// positional = label, first Hash = opts.
pub(super) fn emit_submit_tag(args: &[Expr], ctx: &ViewCtx) -> Vec<Expr> {
    let (positional, opts) = split_args(args);
    let label_expr = positional
        .first()
        .copied()
        .cloned()
        .unwrap_or_else(|| lit_str("Save changes".to_string()));
    emit_submit_input(label_expr, opts.as_slice(), ctx)
}

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
    emit_submit_input(label_expr, opts, ctx)
}

/// Shared `<input type="submit" …>` emission for `form.submit` and the
/// bare `submit_tag` — label into both `value` and `data-disable-with`,
/// then the compile-time attr expansion.
fn emit_submit_input(label_expr: Expr, opts: &[(Expr, Expr)], ctx: &ViewCtx) -> Vec<Expr> {
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
        // `data: { confirm: … }` fans out to `data-<key>` attributes at
        // COMPILE time (Rails walks the hash at request time). A
        // non-literal value gets the runtime nil-guard Rails has —
        // `unless dv.nil?` — as an inline conditional part, so a nil
        // `confirm` drops the whole attribute instead of rendering
        // `data-confirm=""` (lobsters' link_post passes `confirm`
        // through optionally).
        // `aria:` fans out by the SAME rule Rails applies to `data:`
        // (`aria: { label: … }` → `aria-label="…"`), and this loop had
        // only ever handled `data`. An `aria:` hash therefore reached
        // the generic branch below and rendered as one attribute whose
        // value was the hash's `to_s` — `aria="{multiline: "true", …}"`,
        // which announces nothing and is not valid markup. campfire
        // labels both its message editors that way, so the composer and
        // the edit box each shipped an unlabelled input.
        if key.as_str() == "data" || key.as_str() == "aria" {
            if let ExprNode::Hash { entries, .. } = &*v.node {
                for (dk, dv) in entries {
                    let ExprNode::Lit { value: Literal::Sym { value: dkey } } = &*dk.node
                    else {
                        continue;
                    };
                    // Rails DASHERIZES the inner key —
                    // `data: { upload_preview_target: … }` renders as
                    // `data-upload-preview-target`, which is what a
                    // Stimulus controller binds to. This loop is
                    // form_builder's own copy of the `data:` rule (see
                    // `attr_parts`, which kebab-cases and which the tag
                    // builder uses); it kept the underscores, so every
                    // multi-word Stimulus target on a form input was
                    // silently inert.
                    let attr_name = format!(
                        " {}-{}=\"",
                        key.as_str(),
                        dkey.as_str().replace('_', "-")
                    );
                    if matches!(&*dv.node, ExprNode::Lit { value: Literal::Str { .. } }) {
                        parts.push(InterpPart::Text { value: attr_name });
                        parts.push(InterpPart::Expr {
                            expr: view_helpers_call("html_escape", vec![dv.clone()]),
                        });
                        parts.push(InterpPart::Text { value: "\"".to_string() });
                    } else {
                        let rendered = string_interp(vec![
                            InterpPart::Text { value: attr_name },
                            InterpPart::Expr {
                                expr: view_helpers_call(
                                    "html_escape",
                                    vec![lit_str_coerce(dv.clone())],
                                ),
                            },
                            InterpPart::Text { value: "\"".to_string() },
                        ]);
                        parts.push(InterpPart::Expr {
                            expr: Expr::new(
                                Span::synthetic(),
                                ExprNode::If {
                                    cond: send(
                                        Some(dv.clone()),
                                        "nil?",
                                        Vec::new(),
                                        None,
                                        false,
                                    ),
                                    then_branch: lit_str(String::new()),
                                    else_branch: rendered,
                                },
                            ),
                        });
                    }
                }
                continue;
            }
        }
        if let Some(decided) = super::attr_parts::tag_option_parts(key.as_str(), v) {
            parts.extend(decided);
            continue;
        }
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

/// `<record_var>[:<field>]` — read the record's attribute via the
/// abstract indexer on `ActiveRecord::Base`. Matches the shape the
/// retired runtime FormBuilder used (`@model[field]`).
///
/// Why `[]` over `.field()`: Crystal's strict-typing flow analysis
/// treats schema-nullable column readers (`property title : String?`)
/// as if they were non-nilable — the body-typer narrows by column
/// type (`Ty::Str`), and the Crystal emit then wraps the read in
/// `.not_nil!` to bridge the gap. For columns the schema says are
/// nullable (e.g. `t.string "title"` without `null: false`), the
/// `.not_nil!` crashes at runtime on new records where `@title` is
/// genuinely nil. The `[]` form lands as a Send with non-empty args
/// (the field Symbol), which the Crystal emit's not-nil rule skips
/// — restoring the prior runtime FormBuilder's parity behavior. The
/// `optional_value_attr` / `escape_or_empty` runtime helpers accept
/// the resulting nullable / untyped value uniformly.
fn record_field_read(binding: &FormBuilderBinding, field: Symbol, ctx: &ViewCtx) -> Expr {
    let record_ref = Expr::new(
        Span::synthetic(),
        ExprNode::Var {
            id: VarId(0),
            name: binding.record_var.clone(),
        },
    );
    // A non-column attribute (typed_store, `attribute` DSL) has no entry
    // in the record's `[]` indexer — that reads back nil and the field
    // renders with no `value=`. Its synthesized reader is the only way
    // to the value. Columns keep the indexer, which is where their cast
    // lives.
    let is_store_read = ctx
        .store_readers
        .get(binding.model_name.as_str())
        .is_some_and(|names| names.contains(field.as_str()));
    if is_store_read {
        return send(Some(record_ref), field.as_str(), Vec::new(), None, false);
    }
    send(
        Some(record_ref),
        "[]",
        vec![lit_sym(field)],
        None,
        false,
    )
}

/// Extract the Symbol payload from a field-name arg (`:title`).
/// Returns None when the arg isn't a Symbol literal — the macro
/// degenerates to an empty append in that case.
/// The field name from `f.<method> :field` — Rails accepts a String
/// spelling too (lobsters' `f.select "hat_id"`), which used to fall
/// through the Sym-only match and silently collapse the whole control
/// to an empty append.
fn field_symbol(field: Option<&Expr>) -> Option<Symbol> {
    let f = field?;
    match &*f.node {
        ExprNode::Lit { value: Literal::Sym { value } } => Some(value.clone()),
        ExprNode::Lit { value: Literal::Str { value } } => Some(Symbol::from(value.as_str())),
        _ => None,
    }
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
