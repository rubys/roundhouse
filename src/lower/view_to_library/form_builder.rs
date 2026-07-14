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
        FormBuilderMethod::Submit => emit_submit(
            positional.first().copied(),
            opts.as_slice(),
            binding,
            ctx,
        ),
        FormBuilderMethod::PasswordField => emit_password_field(
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
            "<input name=\"{mn}[{f}]\" type=\"hidden\" value=\"0\" autocomplete=\"off\"><input type=\"checkbox\" value=\"1\"{nid}",
            mn = model_name,
            f = field_str,
            nid = name_id_attrs(model_name, field_str),
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
        let value_read = field_value_read(binding, field_sym.clone());
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
            let value_read = field_value_read(binding, field_sym.clone());
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
    parts.push(InterpPart::Text {
        value: format!(" name=\"{model_name}[{field_str}]\" id=\"{model_name}_{field_str}_"),
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
    let model_name = &binding.model_name;
    let field_str = field_sym.as_str();
    let value_read = field_value_read(binding, field_sym.clone());
    // `include_blank:` is select BEHAVIOR, not an HTML attribute — pull
    // it out before the attr expansion (previously it leaked into the
    // tag as `include_blank="true"`).
    let include_blank = opts.iter().any(|(k, v)| {
        matches!(&*k.node, ExprNode::Lit { value: Literal::Sym { value } }
            if value.as_str() == "include_blank")
            && matches!(&*v.node, ExprNode::Lit { value: Literal::Bool { value: true } })
    });
    let attr_opts: Vec<(Expr, Expr)> = opts
        .iter()
        .filter(|(k, _)| {
            !matches!(&*k.node, ExprNode::Lit { value: Literal::Sym { value } }
                if value.as_str() == "include_blank")
        })
        .cloned()
        .collect();
    let mut parts: Vec<InterpPart> = Vec::new();
    parts.push(InterpPart::Text {
        value: format!("<select{}", name_id_attrs(model_name, field_str)),
    });
    append_attr_parts(&mut parts, &attr_opts);
    parts.push(InterpPart::Text { value: ">".to_string() });
    let (setup, options_expr) =
        match emit_select_options(choices, value_read.clone(), include_blank, field_str, ctx) {
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
                push_static_option(&mut parts, &text, &value, selected.as_ref());
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
                push_static_option(&mut parts, &text, &value, selected.as_ref());
            }
            let (setup, loop_var) =
                map_loop_options(&args[0], selected.as_ref(), field_str, parts, ctx)?;
            Some((setup, loop_var))
        }
        // Bare `<coll>.map { |x| [...] }`.
        ExprNode::Send { method, block: Some(_), .. } if method.as_str() == "map" => {
            let (setup, loop_var) =
                map_loop_options(&container, selected.as_ref(), field_str, blank, ctx)?;
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
) {
    parts.push(InterpPart::Text { value: "<option".to_string() });
    if let Some(sel) = selected {
        parts.push(selected_attr_part(sel.clone(), lit_str(value.to_string())));
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
) {
    parts.push(InterpPart::Text { value: "<option".to_string() });
    if let Some(sel) = selected {
        parts.push(selected_attr_part(sel.clone(), value.clone()));
    }
    parts.push(InterpPart::Text { value: " value=\"".to_string() });
    parts.push(InterpPart::Expr { expr: view_helpers_call("html_escape", vec![value]) });
    parts.push(InterpPart::Text { value: "\"".to_string() });
    for (name, v) in attrs {
        parts.push(InterpPart::Text { value: format!(" {name}=\"") });
        parts.push(InterpPart::Expr {
            expr: view_helpers_call("html_escape", vec![lit_str_coerce(v.clone())]),
        });
        parts.push(InterpPart::Text { value: "\"".to_string() });
    }
    parts.push(InterpPart::Text { value: ">".to_string() });
    parts.push(InterpPart::Expr { expr: view_helpers_call("html_escape", vec![text]) });
    parts.push(InterpPart::Text { value: "</option>".to_string() });
}

/// ` selected="selected"` when `sel.to_s == value.to_s` (the overlay's
/// comparison, matching Rails' string-side select semantics).
fn selected_attr_part(sel: Expr, value: Expr) -> InterpPart {
    let cond = send(Some(to_s(sel)), "==", vec![to_s(value)], None, false);
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
    field_str: &str,
    ctx: &ViewCtx,
) -> Option<(Vec<Expr>, Expr)> {
    let el = Symbol::from("_choice");
    let el_ref =
        Expr::new(Span::synthetic(), ExprNode::Var { id: VarId(0), name: el.clone() });
    let value = send(Some(el_ref.clone()), value_method, Vec::new(), None, false);
    let text = send(Some(el_ref), text_method, Vec::new(), None, false);
    let mut option = Vec::new();
    push_dynamic_option(&mut option, to_s(text), to_s(value), &[], selected.as_ref());
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
    let ExprNode::Array { elements, .. } = &*body.node else { return None };
    let (text, attrs_hash, value) = match elements.as_slice() {
        [t, v] => (t.clone(), None, v.clone()),
        [t, a, v] if matches!(&*a.node, ExprNode::Hash { .. }) => {
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
    push_dynamic_option(&mut option, to_s(text), to_s(value), &attrs, selected);
    Some(each_loop(coll, el, string_interp(option), field_str, prefix, ctx))
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
    _ctx: &ViewCtx,
) -> (Vec<Expr>, Expr) {
    let var_name = Symbol::from(format!("_options_{field_str}"));
    let mut setup: Vec<Expr> = Vec::new();
    setup.push(super::assign_accumulator_string_new(var_name.as_str()));
    if !prefix.is_empty() {
        setup.push(options_append(&var_name, string_interp(prefix)));
    }
    let loop_body = options_append(&var_name, option_interp);
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
fn emit_button(
    text: Option<&Expr>,
    opts: &[(Expr, Expr)],
    ctx: &ViewCtx,
) -> Vec<Expr> {
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
    if let Some(t) = text {
        parts.push(InterpPart::Expr {
            expr: view_helpers_call("html_escape", vec![to_s(t.clone())]),
        });
    }
    parts.push(InterpPart::Text { value: "</button>".to_string() });
    vec![accumulator_append_call(string_interp(parts), ctx)]
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
    let model_name = &binding.model_name;
    let field_str = field_sym.as_str();
    let value_read = field_value_read(binding, field_sym.clone());
    let mut parts: Vec<InterpPart> = Vec::new();
    parts.push(InterpPart::Text {
        value: format!("<input type=\"{ty}\"{}", name_id_attrs(model_name, field_str)),
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

/// `<input type="password" name="..." id="..."<opts>>` — inline expansion
/// of `form.password_field :field [, opts]`. Rails omits the `value=` attr
/// for password fields (it never echoes a password back), so — unlike
/// `text_field` — no `optional_value_attr` is emitted; a caller-supplied
/// `value:` opt still flows through `append_attr_parts`.
fn emit_password_field(
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
            "<input type=\"password\"{}",
            name_id_attrs(&binding.model_name, field_sym.as_str())
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
            name_id_attrs(&binding.model_name, field_sym.as_str())
        ),
    });
    if !opts_have_value(opts) {
        parts.push(InterpPart::Expr {
            expr: view_helpers_call(
                "optional_value_attr",
                vec![field_value_read(binding, field_sym.clone())],
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
fn name_id_attrs(model_name: &str, field: &str) -> String {
    if model_name.is_empty() {
        format!(" name=\"{field}\" id=\"{field}\"")
    } else {
        format!(" name=\"{model_name}[{field}]\" id=\"{model_name}_{field}\"")
    }
}

/// The `<label for="...">` open fragment, prefixed for resource forms and
/// bare for non-resource forms (see `name_id_attrs`).
fn label_for_attr(model_name: &str, field: &str) -> String {
    if model_name.is_empty() {
        format!("<label for=\"{field}\"")
    } else {
        format!("<label for=\"{model_name}_{field}\"")
    }
}

/// The value expression a field reads: the record's attribute for a
/// resource form, or `nil` for a non-resource form (no record to read —
/// `optional_value_attr(nil)` then omits the `value=` attr, matching Rails
/// rendering an empty non-model field).
fn field_value_read(binding: &FormBuilderBinding, field: Symbol) -> Expr {
    if binding.model_name.is_empty() {
        Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Nil })
    } else {
        record_field_read(binding, field)
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
    let model_name = &binding.model_name;
    let mut parts: Vec<InterpPart> = Vec::new();
    parts.push(InterpPart::Text {
        value: label_for_attr(model_name, field_sym.as_str()),
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
    let model_name = &binding.model_name;
    let field_str = field_sym.as_str();
    let value_read = field_value_read(binding, field_sym.clone());
    let mut parts: Vec<InterpPart> = Vec::new();
    parts.push(InterpPart::Text {
        value: format!("<input type=\"text\"{}", name_id_attrs(model_name, field_str)),
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
    let value_read = field_value_read(binding, field_sym.clone());
    let mut parts: Vec<InterpPart> = Vec::new();
    parts.push(InterpPart::Text {
        value: format!("<textarea{}", name_id_attrs(model_name, field_str)),
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
        if key.as_str() == "data" {
            if let ExprNode::Hash { entries, .. } = &*v.node {
                for (dk, dv) in entries {
                    let ExprNode::Lit { value: Literal::Sym { value: dkey } } = &*dk.node
                    else {
                        continue;
                    };
                    let attr_name = format!(" data-{}=\"", dkey.as_str());
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
fn record_field_read(binding: &FormBuilderBinding, field: Symbol) -> Expr {
    let record_ref = Expr::new(
        Span::synthetic(),
        ExprNode::Var {
            id: VarId(0),
            name: binding.record_var.clone(),
        },
    );
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
