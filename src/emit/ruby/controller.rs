//! `app/controllers/*.rb` emission: controller class, before-actions,
//! actions, render targets.

use std::fmt::Write;
use std::path::PathBuf;

use super::super::EmittedFile;
use super::expr::emit_expr;
use super::shared::{emit_indented_body, emit_leading_comments};
use crate::App;
use crate::dialect::{Action, Controller, Filter, FilterKind, RenderTarget};
use crate::ident::Symbol;
use crate::lower::lower_controller_to_library_class;
use crate::naming::snake_case;

pub(super) fn emit_controller(c: &Controller) -> EmittedFile {
    use crate::dialect::ControllerBodyItem;

    let mut s = String::new();
    let parent = c.parent.as_ref().map_or_else(
        || "ApplicationController".to_string(),
        |p| p.to_string(),
    );
    writeln!(s, "class {} < {parent}", c.name).unwrap();

    // Rails scaffolds indent methods that appear *after* the `private`
    // marker by an extra level (4 spaces instead of 2). The extra
    // indent is cosmetic — Ruby's visibility semantics don't care —
    // but reproducing it is required for byte-for-byte round-trip.
    let mut past_private = false;
    for item in c.body.iter() {
        let indent_depth = if past_private { 2 } else { 1 };
        let indent = "  ".repeat(indent_depth);
        if item.leading_blank_line() {
            writeln!(s).unwrap();
        }
        emit_leading_comments(&mut s, item.leading_comments(), indent_depth);
        match item {
            ControllerBodyItem::Filter { filter, .. } => {
                writeln!(s, "{indent}{}", emit_filter(filter)).unwrap();
            }
            ControllerBodyItem::Action { action, .. } => {
                emit_action(&mut s, action, indent_depth);
            }
            ControllerBodyItem::PrivateMarker { .. } => {
                writeln!(s, "  private").unwrap();
                past_private = true;
            }
            ControllerBodyItem::Unknown { expr, .. } => {
                writeln!(s, "{indent}{}", emit_expr(expr)).unwrap();
            }
        }
    }

    writeln!(s, "end").unwrap();
    EmittedFile {
        path: PathBuf::from(format!(
            "app/controllers/{}.rb",
            snake_case(c.name.0.as_str())
        )),
        content: s,
    }
}

fn emit_filter(f: &Filter) -> String {
    let name = match f.kind {
        FilterKind::Before => "before_action",
        FilterKind::Around => "around_action",
        FilterKind::After => "after_action",
        FilterKind::Skip => "skip_before_action",
    };
    let mut opts = Vec::new();
    if !f.only.is_empty() {
        opts.push(format!("only: {}", emit_symbol_list(&f.only, f.only_style)));
    }
    if !f.except.is_empty() {
        opts.push(format!(
            "except: {}",
            emit_symbol_list(&f.except, f.except_style)
        ));
    }
    if opts.is_empty() {
        format!("{name} :{}", f.target)
    } else {
        format!("{name} :{}, {}", f.target, opts.join(", "))
    }
}

/// Emit a list of symbols in its source form. `%i[a b]` uses space
/// separation and bare names; `[:a, :b]` uses comma separation with
/// `:` prefixes. `%i` lists in Rails scaffolds conventionally pad
/// with a single space after the opener (`%i[ show edit ]`).
fn emit_symbol_list(syms: &[Symbol], style: crate::expr::ArrayStyle) -> String {
    use crate::expr::ArrayStyle;
    match style {
        ArrayStyle::Brackets => {
            let parts: Vec<String> = syms.iter().map(|s| format!(":{s}")).collect();
            format!("[{}]", parts.join(", "))
        }
        ArrayStyle::BracketsSpaced => {
            let parts: Vec<String> = syms.iter().map(|s| format!(":{s}")).collect();
            format!("[ {} ]", parts.join(", "))
        }
        ArrayStyle::PercentI => {
            let parts: Vec<String> = syms.iter().map(|s| s.to_string()).collect();
            format!("%i[ {} ]", parts.join(" "))
        }
        ArrayStyle::PercentW => {
            // `%w` on a symbol list doesn't make Ruby sense; fall back to brackets.
            let parts: Vec<String> = syms.iter().map(|s| format!(":{s}")).collect();
            format!("[{}]", parts.join(", "))
        }
    }
}

fn emit_action(out: &mut String, a: &Action, indent: usize) {
    let pad = "  ".repeat(indent);
    writeln!(out, "{pad}def {}", a.name).unwrap();
    emit_indented_body(out, &emit_expr(&a.body), indent + 1);
    if let Some(line) = emit_render(&a.renders) {
        writeln!(out, "{pad}  {line}").unwrap();
    }
    writeln!(out, "{pad}end").unwrap();
}

fn emit_render(r: &RenderTarget) -> Option<String> {
    match r {
        RenderTarget::Inferred => None,
        RenderTarget::Template { name, formats } => {
            if formats.is_empty() {
                Some(format!("render :{name}"))
            } else {
                let fs: Vec<String> = formats.iter().map(|f| format!(":{f}")).collect();
                Some(format!("render :{name}, formats: [{}]", fs.join(", ")))
            }
        }
        RenderTarget::Redirect { to } => Some(format!("redirect_to {}", emit_expr(to))),
        RenderTarget::Json { value } => Some(format!("render json: {}", emit_expr(value))),
        RenderTarget::Head { status } => Some(format!("head :{status}")),
    }
}

// ---------------------------------------------------------------------------
// Lowered (spinel-shape) controller emit. The Rails-shape `Controller` is
// lowered to a `LibraryClass` (`process_action` dispatcher + per-action
// methods); this renders the class with controller-specific
// `require_relative` headers and emits to `app/controllers/<stem>.rb`.
// ---------------------------------------------------------------------------

pub(super) fn emit_lowered_controllers(app: &App) -> Vec<EmittedFile> {
    use crate::lower::lower_controllers_to_library_classes;

    // Bulk lower so synthesized siblings (`<Resource>Params`) ride
    // alongside the controller classes. They share the controller
    // lowerer's output vec but get routed to `app/models/` because
    // they're plain holders, not request handlers.
    let lcs = lower_controllers_to_library_classes(&app.controllers, Vec::new());

    // Same synthesized-siblings tracking as `emit_lowered_models`: each
    // tagged class needs an explicit `require_relative` from any file
    // that references it (nothing else loads them).
    let synthesized: Vec<(String, String)> = lcs
        .iter()
        .filter(|lc| lc.origin.is_some())
        .map(|lc| {
            let name = lc.name.0.as_str().to_string();
            let stem = snake_case(&name);
            (name, format!("app/models/{stem}"))
        })
        .collect();

    lcs.iter()
        .map(|lc| {
            let file_stem = snake_case(lc.name.0.as_str());
            let out_path = if lc.origin.is_some() {
                PathBuf::from(format!("app/models/{file_stem}.rb"))
            } else {
                PathBuf::from(format!("app/controllers/{file_stem}.rb"))
            };
            super::library::emit_library_class_decl_with_synthesized(
                lc,
                app,
                out_path,
                &synthesized,
            )
        })
        .collect()
}
