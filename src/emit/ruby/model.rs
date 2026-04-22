//! `app/models/*.rb` emission: model class shells, associations,
//! validations, scopes, callbacks, methods.

use std::fmt::Write;
use std::path::PathBuf;

use super::super::EmittedFile;
use super::expr::{emit_expr, emit_literal};
use super::shared::{emit_indented_body, emit_leading_comments};
use crate::dialect::{
    Association, Callback, CallbackHook, Dependent, MethodDef, MethodReceiver, Model, Scope,
    Validation, ValidationRule,
};
use crate::ident::{ClassId, Symbol};
use crate::naming::{camelize, habtm_join_table, singularize_camelize, snake_case};

pub(super) fn emit_model(m: &Model) -> EmittedFile {
    use crate::dialect::ModelBodyItem;

    let mut s = String::new();
    let parent = m
        .parent
        .as_ref()
        .map(|c| c.0.to_string())
        .unwrap_or_else(|| "ApplicationRecord".to_string());
    writeln!(s, "class {} < {}", m.name, parent).unwrap();

    for item in m.body.iter() {
        if item.leading_blank_line() {
            writeln!(s).unwrap();
        }
        emit_leading_comments(&mut s, item.leading_comments(), 1);
        let line = match item {
            ModelBodyItem::Association { assoc, .. } => {
                emit_association(&m.name, assoc)
            }
            ModelBodyItem::Validation { validation, .. } => emit_validation_entry(validation),
            ModelBodyItem::Scope { scope, .. } => emit_scope(scope),
            ModelBodyItem::Callback { callback, .. } => emit_callback(callback),
            ModelBodyItem::Method { method, .. } => {
                emit_method(&mut s, method, 1);
                continue;
            }
            ModelBodyItem::Unknown { expr, .. } => emit_expr(expr),
        };
        writeln!(s, "  {line}").unwrap();
    }

    writeln!(s, "end").unwrap();
    EmittedFile {
        path: PathBuf::from(format!("app/models/{}.rb", snake_case(m.name.0.as_str()))),
        content: s,
    }
}

/// Emit a single `Validation` (one attribute, possibly multiple rules).
/// Rails writes `validates :attr, rule1: …, rule2: …` — one line per
/// validation. If there are multiple rules the attribute appears once
/// with all rules as keyword args; we keep it simple and emit
/// one line per rule, which matches the fixture usage today.
fn emit_validation_entry(v: &Validation) -> String {
    let attr = v.attribute.to_string();
    if v.rules.is_empty() {
        return format!("validates :{attr}");
    }
    let parts: Vec<String> = v.rules.iter().map(|r| format_validation_rule(r)).collect();
    format!("validates :{attr}, {}", parts.join(", "))
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

/// Emit the `key: value` fragment for one validation rule — the part
/// that goes after `validates :attr,`. Multiple rules on the same
/// attribute get joined by commas by the caller.
fn format_validation_rule(rule: &ValidationRule) -> String {
    match rule {
        ValidationRule::Presence => "presence: true".to_string(),
        ValidationRule::Absence => "absence: true".to_string(),
        ValidationRule::Uniqueness { scope, case_sensitive } => {
            let mut inner = Vec::new();
            if !scope.is_empty() {
                let s: Vec<String> = scope.iter().map(|s| format!(":{s}")).collect();
                inner.push(format!("scope: [{}]", s.join(", ")));
            }
            if !*case_sensitive {
                inner.push("case_sensitive: false".into());
            }
            if inner.is_empty() {
                "uniqueness: true".into()
            } else {
                format!("uniqueness: {{ {} }}", inner.join(", "))
            }
        }
        ValidationRule::Length { min, max } => {
            let mut parts = Vec::new();
            if let Some(n) = min { parts.push(format!("minimum: {n}")); }
            if let Some(n) = max { parts.push(format!("maximum: {n}")); }
            format!("length: {{ {} }}", parts.join(", "))
        }
        ValidationRule::Format { pattern } => {
            format!("format: {{ with: /{pattern}/ }}")
        }
        ValidationRule::Numericality { only_integer, gt, lt } => {
            let mut parts = Vec::new();
            if *only_integer { parts.push("only_integer: true".into()); }
            if let Some(n) = gt { parts.push(format!("greater_than: {n}")); }
            if let Some(n) = lt { parts.push(format!("less_than: {n}")); }
            format!("numericality: {{ {} }}", parts.join(", "))
        }
        ValidationRule::Inclusion { values } => {
            let vs: Vec<String> = values.iter().map(emit_literal).collect();
            format!("inclusion: {{ in: [{}] }}", vs.join(", "))
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
    emit_indented_body(out, &emit_expr(&m.body), indent + 1);
    writeln!(out, "{pad}end").unwrap();
}
