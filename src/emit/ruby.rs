//! Ruby emitter: App → a set of Ruby source files.
//!
//! The reverse direction of Prism ingest. Together they form the round-trip
//! forcing function: Ruby source → IR → Ruby source should preserve semantics.

use std::fmt::Write;
use std::path::PathBuf;

use super::EmittedFile;
use crate::App;
use crate::dialect::{
    Action, Association, Callback, CallbackHook, Controller, Dependent, Filter, FilterKind,
    HttpMethod, MethodDef, MethodReceiver, Model, RenderTarget, Route, RouteTable, Scope,
    ValidationRule,
};
use crate::expr::{Arm, Expr, ExprNode, LValue, Literal, Pattern};
use crate::ident::{ClassId, Symbol};
use crate::naming::{camelize, habtm_join_table, singularize_camelize, snake_case};
use crate::schema::{Column, ColumnType, Schema, Table};

pub fn emit(app: &App) -> Vec<EmittedFile> {
    let mut files = Vec::new();
    if !app.schema.tables.is_empty() {
        files.push(emit_schema(&app.schema));
    }
    for model in &app.models {
        files.push(emit_model(model));
    }
    for controller in &app.controllers {
        files.push(emit_controller(controller));
    }
    files.push(emit_routes(&app.routes));
    for view in &app.views {
        files.push(emit_view(view));
    }
    files
}

// Schema ----------------------------------------------------------------

fn emit_schema(schema: &Schema) -> EmittedFile {
    let mut s = String::new();
    writeln!(s, "ActiveRecord::Schema.define do").unwrap();
    for table in schema.tables.values() {
        emit_table(&mut s, table);
    }
    for table in schema.tables.values() {
        for fk in &table.foreign_keys {
            writeln!(
                s,
                "  add_foreign_key {:?}, {:?}, column: {:?}, primary_key: {:?}",
                table.name.as_str(),
                fk.to_table.to_string(),
                fk.from_column.as_str(),
                fk.to_column.as_str(),
            )
            .unwrap();
        }
    }
    writeln!(s, "end").unwrap();
    EmittedFile { path: PathBuf::from("db/schema.rb"), content: s }
}

fn emit_table(out: &mut String, table: &Table) {
    writeln!(out, "  create_table {:?}, force: :cascade do |t|", table.name.as_str()).unwrap();
    for col in &table.columns {
        if col.primary_key {
            continue; // Rails synthesizes `id` by default.
        }
        writeln!(out, "    {}", emit_column(col)).unwrap();
    }
    for idx in &table.indexes {
        let cols: Vec<String> = idx.columns.iter().map(|c| format!("{:?}", c.as_str())).collect();
        let unique = if idx.unique { ", unique: true" } else { "" };
        writeln!(
            out,
            "    t.index [{}], name: {:?}{}",
            cols.join(", "),
            idx.name.as_str(),
            unique
        )
        .unwrap();
    }
    writeln!(out, "  end").unwrap();
}

fn emit_column(col: &Column) -> String {
    let method = match &col.col_type {
        ColumnType::Integer => "integer",
        ColumnType::BigInt => "bigint",
        ColumnType::Float => "float",
        ColumnType::Decimal { .. } => "decimal",
        ColumnType::String { .. } => "string",
        ColumnType::Text => "text",
        ColumnType::Boolean => "boolean",
        ColumnType::Date => "date",
        ColumnType::DateTime => "datetime",
        ColumnType::Time => "time",
        ColumnType::Binary => "binary",
        ColumnType::Json => "json",
        ColumnType::Reference { .. } => "references",
    };
    let mut opts: Vec<String> = Vec::new();
    if let ColumnType::String { limit: Some(n) } = &col.col_type {
        opts.push(format!("limit: {n}"));
    }
    if let ColumnType::Decimal { precision, scale } = &col.col_type {
        if let Some(p) = precision { opts.push(format!("precision: {p}")); }
        if let Some(s) = scale { opts.push(format!("scale: {s}")); }
    }
    if !col.nullable { opts.push("null: false".to_string()); }
    if let Some(d) = &col.default {
        opts.push(format!("default: {d:?}"));
    }
    if opts.is_empty() {
        format!("t.{method} {:?}", col.name.as_str())
    } else {
        format!("t.{method} {:?}, {}", col.name.as_str(), opts.join(", "))
    }
}

// Models ----------------------------------------------------------------

fn emit_model(m: &Model) -> EmittedFile {
    let mut s = String::new();
    writeln!(s, "class {} < ApplicationRecord", m.name).unwrap();
    let mut wrote_body = false;
    for assoc in &m.associations {
        writeln!(s, "  {}", emit_association(&m.name, assoc)).unwrap();
        wrote_body = true;
    }
    for validation in &m.validations {
        for rule in &validation.rules {
            writeln!(s, "  {}", emit_validation(&validation.attribute.to_string(), rule)).unwrap();
            wrote_body = true;
        }
    }
    for scope in &m.scopes {
        writeln!(s, "  {}", emit_scope(scope)).unwrap();
        wrote_body = true;
    }
    for cb in &m.callbacks {
        writeln!(s, "  {}", emit_callback(cb)).unwrap();
        wrote_body = true;
    }
    for method in &m.methods {
        if wrote_body { writeln!(s).unwrap(); }
        emit_method(&mut s, method, 1);
        wrote_body = true;
    }
    writeln!(s, "end").unwrap();
    EmittedFile {
        path: PathBuf::from(format!("app/models/{}.rb", snake_case(m.name.0.as_str()))),
        content: s,
    }
}

fn emit_association(owner: &ClassId, a: &Association) -> String {
    match a {
        Association::BelongsTo { name, target, foreign_key, optional } => {
            let default_target = ClassId(Symbol::from(camelize(name.as_str())));
            let default_fk = Symbol::from(format!("{name}_id"));
            let mut opts = Vec::new();
            if target != &default_target {
                opts.push(format!("class_name: {:?}", target.to_string()));
            }
            if foreign_key != &default_fk {
                opts.push(format!("foreign_key: {:?}", foreign_key.as_str()));
            }
            if *optional { opts.push("optional: true".into()); }
            assoc_line("belongs_to", name, &opts)
        }
        Association::HasMany { name, target, foreign_key, through, dependent } => {
            let default_target = ClassId(Symbol::from(singularize_camelize(name.as_str())));
            let default_fk = Symbol::from(format!("{}_id", snake_case(owner.0.as_str())));
            let mut opts = Vec::new();
            if target != &default_target {
                opts.push(format!("class_name: {:?}", target.to_string()));
            }
            if foreign_key != &default_fk {
                opts.push(format!("foreign_key: {:?}", foreign_key.as_str()));
            }
            if let Some(t) = through { opts.push(format!("through: :{t}")); }
            if let Some(d) = emit_dependent(dependent) { opts.push(format!("dependent: {d}")); }
            assoc_line("has_many", name, &opts)
        }
        Association::HasOne { name, target, foreign_key, dependent } => {
            let default_target = ClassId(Symbol::from(camelize(name.as_str())));
            let default_fk = Symbol::from(format!("{}_id", snake_case(owner.0.as_str())));
            let mut opts = Vec::new();
            if target != &default_target {
                opts.push(format!("class_name: {:?}", target.to_string()));
            }
            if foreign_key != &default_fk {
                opts.push(format!("foreign_key: {:?}", foreign_key.as_str()));
            }
            if let Some(d) = emit_dependent(dependent) { opts.push(format!("dependent: {d}")); }
            assoc_line("has_one", name, &opts)
        }
        Association::HasAndBelongsToMany { name, target, join_table } => {
            let default_target = ClassId(Symbol::from(singularize_camelize(name.as_str())));
            let default_jt = habtm_join_table(owner.0.as_str(), name.as_str());
            let mut opts = Vec::new();
            if target != &default_target {
                opts.push(format!("class_name: {:?}", target.to_string()));
            }
            if join_table.as_str() != default_jt {
                opts.push(format!("join_table: {:?}", join_table.as_str()));
            }
            assoc_line("has_and_belongs_to_many", name, &opts)
        }
    }
}

fn assoc_line(method: &str, name: &Symbol, opts: &[String]) -> String {
    if opts.is_empty() {
        format!("{method} :{name}")
    } else {
        format!("{method} :{name}, {}", opts.join(", "))
    }
}

fn emit_dependent(d: &Dependent) -> Option<&'static str> {
    match d {
        Dependent::None => None,
        Dependent::Destroy => Some(":destroy"),
        Dependent::DestroyAsync => Some(":destroy_async"),
        Dependent::Delete => Some(":delete"),
        Dependent::DeleteAll => Some(":delete_all"),
        Dependent::Nullify => Some(":nullify"),
        Dependent::Restrict => Some(":restrict_with_exception"),
    }
}

fn emit_validation(attr: &str, rule: &ValidationRule) -> String {
    match rule {
        ValidationRule::Presence => format!("validates :{attr}, presence: true"),
        ValidationRule::Absence => format!("validates :{attr}, absence: true"),
        ValidationRule::Uniqueness { scope, case_sensitive } => {
            let mut opts = vec!["uniqueness:".to_string()];
            let mut inner = Vec::new();
            if !scope.is_empty() {
                let s: Vec<String> = scope.iter().map(|s| format!(":{s}")).collect();
                inner.push(format!("scope: [{}]", s.join(", ")));
            }
            if !*case_sensitive { inner.push("case_sensitive: false".into()); }
            if inner.is_empty() { opts.push("true".into()); }
            else { opts.push(format!("{{ {} }}", inner.join(", "))); }
            format!("validates :{attr}, {}", opts.join(" "))
        }
        ValidationRule::Length { min, max } => {
            let mut parts = Vec::new();
            if let Some(n) = min { parts.push(format!("minimum: {n}")); }
            if let Some(n) = max { parts.push(format!("maximum: {n}")); }
            format!("validates :{attr}, length: {{ {} }}", parts.join(", "))
        }
        ValidationRule::Format { pattern } => {
            format!("validates :{attr}, format: {{ with: /{pattern}/ }}")
        }
        ValidationRule::Numericality { only_integer, gt, lt } => {
            let mut parts = Vec::new();
            if *only_integer { parts.push("only_integer: true".into()); }
            if let Some(n) = gt { parts.push(format!("greater_than: {n}")); }
            if let Some(n) = lt { parts.push(format!("less_than: {n}")); }
            format!("validates :{attr}, numericality: {{ {} }}", parts.join(", "))
        }
        ValidationRule::Inclusion { values } => {
            let vs: Vec<String> = values.iter().map(emit_literal).collect();
            format!("validates :{attr}, inclusion: {{ in: [{}] }}", vs.join(", "))
        }
        ValidationRule::Custom { method } => format!("validate :{method}"),
    }
}

fn emit_scope(scope: &Scope) -> String {
    // `-> { body }`  when params empty; `->(a, b) { body }` otherwise.
    let arrow_params = if scope.params.is_empty() {
        " ".to_string()
    } else {
        let ps: Vec<&str> = scope.params.iter().map(|p| p.as_str()).collect();
        format!("({}) ", ps.join(", "))
    };
    format!("scope :{}, ->{}{{ {} }}", scope.name, arrow_params, emit_expr(&scope.body))
}

fn emit_callback(cb: &Callback) -> String {
    let hook = match cb.hook {
        CallbackHook::BeforeValidation => "before_validation",
        CallbackHook::AfterValidation => "after_validation",
        CallbackHook::BeforeSave => "before_save",
        CallbackHook::AfterSave => "after_save",
        CallbackHook::BeforeCreate => "before_create",
        CallbackHook::AfterCreate => "after_create",
        CallbackHook::BeforeUpdate => "before_update",
        CallbackHook::AfterUpdate => "after_update",
        CallbackHook::BeforeDestroy => "before_destroy",
        CallbackHook::AfterDestroy => "after_destroy",
        CallbackHook::AfterCommit => "after_commit",
        CallbackHook::AfterRollback => "after_rollback",
    };
    if let Some(cond) = &cb.condition {
        format!("{hook} :{}, if: -> {{ {} }}", cb.target, emit_expr(cond))
    } else {
        format!("{hook} :{}", cb.target)
    }
}

fn emit_method(out: &mut String, m: &MethodDef, indent: usize) {
    let pad = "  ".repeat(indent);
    let prefix = match m.receiver {
        MethodReceiver::Instance => String::new(),
        MethodReceiver::Class => "self.".into(),
    };
    let params = if m.params.is_empty() {
        String::new()
    } else {
        let ps: Vec<&str> = m.params.iter().map(|p| p.as_str()).collect();
        format!("({})", ps.join(", "))
    };
    writeln!(out, "{pad}def {prefix}{}{}", m.name, params).unwrap();
    for line in emit_expr(&m.body).lines() {
        writeln!(out, "{pad}  {line}").unwrap();
    }
    writeln!(out, "{pad}end").unwrap();
}

// Controllers ------------------------------------------------------------

fn emit_controller(c: &Controller) -> EmittedFile {
    let mut s = String::new();
    let parent = c.parent.as_ref().map_or_else(
        || "ApplicationController".to_string(),
        |p| p.to_string(),
    );
    writeln!(s, "class {} < {parent}", c.name).unwrap();
    for filter in &c.filters {
        writeln!(s, "  {}", emit_filter(filter)).unwrap();
    }
    if !c.filters.is_empty() && !c.actions.is_empty() {
        writeln!(s).unwrap();
    }
    for (i, action) in c.actions.iter().enumerate() {
        if i > 0 { writeln!(s).unwrap(); }
        emit_action(&mut s, action, 1);
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
        let os: Vec<String> = f.only.iter().map(|s| format!(":{s}")).collect();
        opts.push(format!("only: [{}]", os.join(", ")));
    }
    if !f.except.is_empty() {
        let os: Vec<String> = f.except.iter().map(|s| format!(":{s}")).collect();
        opts.push(format!("except: [{}]", os.join(", ")));
    }
    if opts.is_empty() {
        format!("{name} :{}", f.target)
    } else {
        format!("{name} :{}, {}", f.target, opts.join(", "))
    }
}

fn emit_action(out: &mut String, a: &Action, indent: usize) {
    let pad = "  ".repeat(indent);
    writeln!(out, "{pad}def {}", a.name).unwrap();
    for line in emit_expr(&a.body).lines() {
        writeln!(out, "{pad}  {line}").unwrap();
    }
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

// Views -----------------------------------------------------------------

fn emit_view(view: &crate::dialect::View) -> EmittedFile {
    let path = PathBuf::from(format!(
        "app/views/{}.{}.erb",
        view.name, view.format
    ));
    let content = reconstruct_erb(&view.body);
    EmittedFile { path, content }
}

/// Walk a view body whose structure is:
///   _buf = ""
///   _buf = _buf + "text"           # text chunk
///   _buf = _buf + (expr).to_s      # <%= expr %>
///   <other ruby statement>         # <% code %> (control flow)
///   _buf                           # epilogue
/// and reconstruct the corresponding ERB source.
fn reconstruct_erb(body: &Expr) -> String {
    let mut out = String::new();
    let stmts: &[Expr] = match &*body.node {
        ExprNode::Seq { exprs } => exprs,
        // Single-statement body — shouldn't happen for compiled ERB but
        // fall through gracefully.
        _ => {
            out.push_str(&emit_buf_stmt(body));
            return out;
        }
    };
    for stmt in stmts {
        out.push_str(&emit_buf_stmt(stmt));
    }
    out
}

fn emit_buf_stmt(stmt: &Expr) -> String {
    match &*stmt.node {
        // Prologue: `_buf = ""` — swallow.
        ExprNode::Assign {
            target: LValue::Var { name, .. },
            value,
        } if name.as_str() == "_buf" => {
            if let ExprNode::Lit { value: Literal::Str { value: s } } = &*value.node {
                if s.is_empty() {
                    return String::new();
                }
            }
            // `_buf = _buf + X` — the working shape.
            if let ExprNode::Send {
                recv: Some(recv),
                method,
                args,
                ..
            } = &*value.node
            {
                if method.as_str() == "+" && args.len() == 1 {
                    if let ExprNode::Var { name: rn, .. } = &*recv.node {
                        if rn.as_str() == "_buf" {
                            return emit_buf_append(&args[0]);
                        }
                    }
                }
            }
            // Unrecognized `_buf = ...` — fall through as code.
            format!("<% {} %>", emit_expr(stmt))
        }
        // Epilogue: bare `_buf` read at end.
        ExprNode::Var { name, .. } if name.as_str() == "_buf" => String::new(),
        // Control flow: `recv.method(args) do |params| body end` inside a
        // template body. Emit the opening as `<% recv.method(args) do |p| %>`,
        // reconstruct the block body template-style, close with `<% end %>`.
        ExprNode::Send {
            recv,
            method,
            args,
            block: Some(block),
            parenthesized,
        } => emit_template_block_send(
            recv.as_ref(),
            method,
            args,
            block,
            *parenthesized,
        ),
        // Conditional: `<% if cond %> then-template <% else %> else-template <% end %>`.
        // A missing else clause is represented by `Lit(Nil)`; when we see it,
        // omit the `<% else %>` segment.
        ExprNode::If { cond, then_branch, else_branch } => {
            let cond_s = emit_expr(cond);
            let then_s = reconstruct_erb(then_branch);
            if matches!(
                &*else_branch.node,
                ExprNode::Lit { value: Literal::Nil }
            ) {
                format!("<% if {} %>{}<% end %>", cond_s, then_s)
            } else {
                let else_s = reconstruct_erb(else_branch);
                format!("<% if {} %>{}<% else %>{}<% end %>", cond_s, then_s, else_s)
            }
        }
        // Anything else is a raw control statement.
        _ => format!("<% {} %>", emit_expr(stmt)),
    }
}

fn emit_template_block_send(
    recv: Option<&Expr>,
    method: &Symbol,
    args: &[Expr],
    block: &Expr,
    parenthesized: bool,
) -> String {
    let ExprNode::Lambda { params, body, .. } = &*block.node else {
        // Unexpected block shape — fall back to raw code emission.
        return format!(
            "<% {} %>",
            emit_do_block(&emit_send_base(recv, method, args, parenthesized), block)
        );
    };
    let base = emit_send_base(recv, method, args, parenthesized);
    let params_clause = if params.is_empty() {
        "do".to_string()
    } else {
        let ps: Vec<String> = params.iter().map(|p| p.to_string()).collect();
        format!("do |{}|", ps.join(", "))
    };
    let inner = reconstruct_erb(body);
    format!("<% {} {} %>{}<% end %>", base, params_clause, inner)
}

/// Emit the argument of `_buf = _buf + ARG` either as a text chunk or
/// as a `<%= expr %>` output interpolation.
fn emit_buf_append(arg: &Expr) -> String {
    // Text chunk: the argument is a string literal.
    if let ExprNode::Lit { value: Literal::Str { value: s } } = &*arg.node {
        return s.clone();
    }
    // Output interpolation: strip the `(expr).to_s` wrapper the compiler
    // added. If somebody wrote `<%= x.to_s %>` explicitly, unwrap once
    // and accept the loss of the explicit `.to_s` — round-trip is stable
    // on the second pass regardless.
    let inner = unwrap_to_s(arg);
    // Output-block case: `<%= recv.method(args) do |p| %>body<% end %>`.
    // The inner expression is a Send with an attached block; the block
    // body is itself a compiled ERB template we can reconstruct.
    if let ExprNode::Send {
        recv,
        method,
        args,
        block: Some(block),
        parenthesized,
    } = &*inner.node
    {
        if let ExprNode::Lambda { params, body, .. } = &*block.node {
            let base = emit_send_base(recv.as_ref(), method, args, *parenthesized);
            let params_clause = if params.is_empty() {
                "do".to_string()
            } else {
                let ps: Vec<String> = params.iter().map(|p| p.to_string()).collect();
                format!("do |{}|", ps.join(", "))
            };
            let inner_erb = reconstruct_erb(body);
            return format!("<%= {} {} %>{}<% end %>", base, params_clause, inner_erb);
        }
    }
    format!("<%= {} %>", emit_expr(inner))
}

fn unwrap_to_s(expr: &Expr) -> &Expr {
    if let ExprNode::Send { recv: Some(inner), method, args, .. } = &*expr.node {
        if method.as_str() == "to_s" && args.is_empty() {
            return inner;
        }
    }
    expr
}

// Routes ----------------------------------------------------------------

fn emit_routes(routes: &RouteTable) -> EmittedFile {
    let mut s = String::new();
    writeln!(s, "Rails.application.routes.draw do").unwrap();
    for route in &routes.routes {
        writeln!(s, "  {}", emit_route(route)).unwrap();
    }
    writeln!(s, "end").unwrap();
    EmittedFile { path: PathBuf::from("config/routes.rb"), content: s }
}

fn emit_route(r: &Route) -> String {
    let verb = match r.method {
        HttpMethod::Get => "get",
        HttpMethod::Post => "post",
        HttpMethod::Put => "put",
        HttpMethod::Patch => "patch",
        HttpMethod::Delete => "delete",
        HttpMethod::Head => "head",
        HttpMethod::Options => "options",
        HttpMethod::Any => "match",
    };
    let mut opts = vec![format!(
        "to: {:?}",
        format!("{}#{}", strip_controller_suffix(r.controller.0.as_str()), r.action)
    )];
    if let Some(name) = &r.as_name {
        opts.push(format!("as: :{name}"));
    }
    if matches!(r.method, HttpMethod::Any) {
        opts.push("via: :all".into());
    }
    format!("{verb} {:?}, {}", r.path, opts.join(", "))
}

fn strip_controller_suffix(s: &str) -> String {
    let base = s.strip_suffix("Controller").unwrap_or(s);
    snake_case(base)
}

// Expressions -----------------------------------------------------------

pub fn emit_expr(e: &Expr) -> String {
    emit_node(&e.node)
}

fn emit_node(n: &ExprNode) -> String {
    match n {
        ExprNode::Lit { value } => emit_literal(value),
        ExprNode::Var { name, .. } => name.to_string(),
        ExprNode::Ivar { name } => format!("@{name}"),
        ExprNode::Const { path } => {
            path.iter().map(|s| s.to_string()).collect::<Vec<_>>().join("::")
        }
        ExprNode::Hash { entries, braced } => emit_hash(entries, *braced),
        ExprNode::Array { elements, style } => emit_array(elements, style),
        ExprNode::Let { name, value, body, .. } => {
            format!("{name} = {}\n{}", emit_expr(value), emit_expr(body))
        }
        ExprNode::Lambda { params, block_param, body } => {
            let mut ps: Vec<String> = params.iter().map(|p| p.to_string()).collect();
            if let Some(b) = block_param { ps.push(format!("&{b}")); }
            if ps.is_empty() {
                format!("-> {{ {} }}", emit_expr(body))
            } else {
                format!("->({}) {{ {} }}", ps.join(", "), emit_expr(body))
            }
        }
        ExprNode::Apply { fun, args, block } => {
            let args_s: Vec<String> = args.iter().map(emit_expr).collect();
            let base = format!("{}.call({})", emit_expr(fun), args_s.join(", "));
            if let Some(b) = block { format!("{base} {{ {} }}", emit_expr(b)) } else { base }
        }
        ExprNode::Send { recv, method, args, block, parenthesized } => {
            let base = emit_send_base(recv.as_ref(), method, args, *parenthesized);
            match block {
                None => base,
                Some(b) => emit_do_block(&base, b),
            }
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            format!(
                "if {}\n{}\nelse\n{}\nend",
                emit_expr(cond),
                indent_lines(&emit_expr(then_branch), 1),
                indent_lines(&emit_expr(else_branch), 1),
            )
        }
        ExprNode::Case { scrutinee, arms } => {
            let mut s = format!("case {}\n", emit_expr(scrutinee));
            for arm in arms {
                s.push_str(&emit_arm(arm));
            }
            s.push_str("end");
            s
        }
        ExprNode::Seq { exprs } => {
            exprs.iter().map(emit_expr).collect::<Vec<_>>().join("\n")
        }
        ExprNode::Assign { target, value } => {
            format!("{} = {}", emit_lvalue(target), emit_expr(value))
        }
        ExprNode::Yield { args } => {
            let args_s: Vec<String> = args.iter().map(emit_expr).collect();
            if args_s.is_empty() { "yield".to_string() } else { format!("yield {}", args_s.join(", ")) }
        }
        ExprNode::Raise { value } => format!("raise {}", emit_expr(value)),
    }
}

fn emit_array(elements: &[Expr], style: &crate::expr::ArrayStyle) -> String {
    use crate::expr::ArrayStyle;
    match style {
        ArrayStyle::Brackets => {
            let parts: Vec<String> = elements.iter().map(emit_expr).collect();
            format!("[{}]", parts.join(", "))
        }
        ArrayStyle::PercentI => {
            // Symbol list: elements must be symbol literals. Emit bare names
            // without the leading `:` and space-separate.
            let parts: Vec<String> = elements
                .iter()
                .map(|e| match &*e.node {
                    ExprNode::Lit { value: Literal::Sym { value } } => value.to_string(),
                    _ => emit_expr(e),
                })
                .collect();
            format!("%i[{}]", parts.join(" "))
        }
        ArrayStyle::PercentW => {
            // Word list: elements must be string literals. Emit without quotes.
            let parts: Vec<String> = elements
                .iter()
                .map(|e| match &*e.node {
                    ExprNode::Lit { value: Literal::Str { value } } => value.to_string(),
                    _ => emit_expr(e),
                })
                .collect();
            format!("%w[{}]", parts.join(" "))
        }
    }
}

fn emit_hash(entries: &[(Expr, Expr)], braced: bool) -> String {
    let parts: Vec<String> = entries
        .iter()
        .map(|(k, v)| {
            // Rails-idiomatic shorthand `key: value` when key is a symbol literal;
            // rocket `k => v` otherwise.
            if let ExprNode::Lit { value: Literal::Sym { value } } = &*k.node {
                format!("{value}: {}", emit_expr(v))
            } else {
                format!("{} => {}", emit_expr(k), emit_expr(v))
            }
        })
        .collect();
    if braced {
        format!("{{ {} }}", parts.join(", "))
    } else {
        parts.join(", ")
    }
}

/// Emit the receiver/method/args portion of a Send without its block.
/// Used by normal Ruby emission and by ERB template reconstruction.
fn emit_send_base(
    recv: Option<&Expr>,
    method: &Symbol,
    args: &[Expr],
    parenthesized: bool,
) -> String {
    let args_s: Vec<String> = args.iter().map(emit_expr).collect();
    match (recv, method.as_str()) {
        (Some(r), "[]") => format!("{}[{}]", emit_expr(r), args_s.join(", ")),
        (None, _) => {
            if args_s.is_empty() {
                method.to_string()
            } else if parenthesized {
                format!("{method}({})", args_s.join(", "))
            } else {
                format!("{method} {}", args_s.join(", "))
            }
        }
        (Some(r), _) => {
            let recv_s = emit_expr(r);
            if args_s.is_empty() {
                format!("{recv_s}.{method}")
            } else {
                format!("{recv_s}.{method}({})", args_s.join(", "))
            }
        }
    }
}

/// Emit a `Send + block` in plain Ruby form (`recv.method(args) do |p| body end`).
/// Used in normal Ruby emission. Template reconstruction has its own path.
fn emit_do_block(base: &str, block: &Expr) -> String {
    let ExprNode::Lambda { params, body, .. } = &*block.node else {
        return format!("{base} {{ {} }}", emit_expr(block));
    };
    let params_clause = if params.is_empty() {
        "do".to_string()
    } else {
        let ps: Vec<String> = params.iter().map(|p| p.to_string()).collect();
        format!("do |{}|", ps.join(", "))
    };
    let body_str = emit_expr(body);
    if body_str.contains('\n') {
        format!(
            "{base} {}\n{}\nend",
            params_clause,
            indent_lines(&body_str, 1),
        )
    } else {
        format!("{base} {} {} end", params_clause, body_str)
    }
}

fn emit_literal(l: &Literal) -> String {
    match l {
        Literal::Nil => "nil".to_string(),
        Literal::Bool { value } => value.to_string(),
        Literal::Int { value } => value.to_string(),
        Literal::Float { value } => {
            let s = value.to_string();
            if s.contains('.') { s } else { format!("{s}.0") }
        }
        Literal::Str { value } => format!("{value:?}"),
        Literal::Sym { value } => format!(":{value}"),
    }
}

fn emit_lvalue(lv: &LValue) -> String {
    match lv {
        LValue::Var { name, .. } => name.to_string(),
        LValue::Ivar { name } => format!("@{name}"),
        LValue::Attr { recv, name } => format!("{}.{name}", emit_expr(recv)),
        LValue::Index { recv, index } => format!("{}[{}]", emit_expr(recv), emit_expr(index)),
    }
}

fn emit_arm(arm: &Arm) -> String {
    let mut s = format!("when {}", emit_pattern(&arm.pattern));
    if let Some(g) = &arm.guard { s.push_str(&format!(" if {}", emit_expr(g))); }
    s.push('\n');
    s.push_str(&indent_lines(&emit_expr(&arm.body), 1));
    s.push('\n');
    s
}

fn emit_pattern(p: &Pattern) -> String {
    match p {
        Pattern::Wildcard => "_".to_string(),
        Pattern::Bind { name } => name.to_string(),
        Pattern::Lit { value } => emit_literal(value),
        Pattern::Array { elems, rest } => {
            let mut parts: Vec<String> = elems.iter().map(emit_pattern).collect();
            if let Some(r) = rest { parts.push(format!("*{r}")); }
            format!("[{}]", parts.join(", "))
        }
        Pattern::Record { fields, rest } => {
            let mut parts: Vec<String> = fields.iter()
                .map(|(k, v)| format!("{k}: {}", emit_pattern(v))).collect();
            if *rest { parts.push("**".into()); }
            format!("{{ {} }}", parts.join(", "))
        }
    }
}

// Helpers ---------------------------------------------------------------

fn indent_lines(s: &str, levels: usize) -> String {
    let pad = "  ".repeat(levels);
    s.lines().map(|l| format!("{pad}{l}")).collect::<Vec<_>>().join("\n")
}
