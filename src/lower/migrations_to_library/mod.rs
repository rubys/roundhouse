//! Lower the app's migrations (`App::migrations`, from `db/migrate`) into
//! a `Migrations` LibraryFunction: `Migrations.all -> Array<Array<String>>`,
//! one Array per migration in version order: its version, then "" or why
//! it cannot run, then the SQLite statements Rails' adapter would issue
//! for it ([`render_migration_step`]). The runtime's
//! `ActiveRecord::Migrator` (runtime/ruby/active_record/migrator.rb) runs
//! the ones a database has not recorded, as `rails db:migrate` does.
//!
//! One homogeneous Array per migration (rather than a record type) keeps
//! the literal typed `Array<Array<String>>` on every target, the way
//! `Schema.statements` is an `Array<String>`.
//!
//! `Migrations.schema_versions -> Array<String>` lists the versions
//! `Schema.statements` already includes, which loading the schema records.

use crate::app::App;
use crate::diagnostic::{Diagnostic, Severity};
use crate::dialect::LibraryFunction;
use crate::effect::EffectSet;
use crate::emit::shared::schema_sql::render_migration_step;
use crate::expr::{ArrayStyle, Expr, ExprNode};
use crate::ident::Symbol;
use crate::lower::typing::{fn_sig, lit_str, with_ty};
use crate::span::Span;
use crate::ty::Ty;

/// The statements for one migration, or why it cannot run.
fn migration_statements(m: &crate::schema::Migration) -> Result<Vec<String>, String> {
    if m.version.is_empty() {
        return Err(format!("{} has no version prefix", m.file));
    }
    if let Some(why) = &m.not_renderable {
        return Err(why.clone());
    }
    let mut out = Vec::new();
    for step in &m.steps {
        out.extend(render_migration_step(step)?);
    }
    Ok(out)
}

/// `Migrations.all`, defined even without migrations (`[]`), as the
/// emitted boot reads it unconditionally. A migration that cannot run
/// is reported as a warning (the modeling-debt ledger), not an error: a
/// database that already has it never runs it, and a fresh one gets its
/// end state from `Schema.statements`.
pub fn lower_migrations_module_functions(app: &App) -> Vec<LibraryFunction> {
    let str_array = Ty::Array {
        elem: Box::new(Ty::Str),
    };
    let mut elements: Vec<Expr> = Vec::new();
    for m in &app.migrations {
        let (gap, statements) = match migration_statements(m) {
            Ok(s) => (String::new(), s),
            Err(why) => {
                let mut d = Diagnostic::unsupported(
                    Span::synthetic(),
                    None,
                    "migration",
                    format!(
                        "{} ({}): {why}; the emitted db:migrate stops at it when a database has not run it",
                        m.version, m.file
                    ),
                );
                d.severity = Severity::Warning;
                crate::emit::diagnostics::push(d);
                (why, Vec::new())
            }
        };
        let mut row: Vec<Expr> = vec![lit_str(m.version.clone()), lit_str(gap)];
        row.extend(statements.into_iter().map(lit_str));
        elements.push(with_ty(
            Expr::new(
                Span::synthetic(),
                ExprNode::Array {
                    elements: row,
                    style: ArrayStyle::Brackets,
                },
            ),
            str_array.clone(),
        ));
    }
    let ret = Ty::Array {
        elem: Box::new(str_array.clone()),
    };
    let body = with_ty(
        Expr::new(
            Span::synthetic(),
            ExprNode::Array {
                elements,
                style: ArrayStyle::Brackets,
            },
        ),
        ret.clone(),
    );
    let versions = with_ty(
        Expr::new(
            Span::synthetic(),
            ExprNode::Array {
                elements: app
                    .migrations
                    .iter()
                    .filter(|m| m.in_schema)
                    .map(|m| lit_str(m.version.clone()))
                    .collect(),
                style: ArrayStyle::Brackets,
            },
        ),
        str_array.clone(),
    );
    vec![
        LibraryFunction {
            module_path: vec![Symbol::from("Migrations")],
            name: Symbol::from("all"),
            params: Vec::new(),
            unsupported_formals: None,
            has_anonymous_block: false,
            body,
            signature: Some(fn_sig(vec![], ret)),
            effects: EffectSet::default(),
            is_async: false,
        },
        // the versions Schema.statements already has, recorded as applied
        // when the schema is loaded (schema.rb's `version:` and older)
        LibraryFunction {
            module_path: vec![Symbol::from("Migrations")],
            name: Symbol::from("schema_versions"),
            params: Vec::new(),
            unsupported_formals: None,
            has_anonymous_block: false,
            body: versions,
            signature: Some(fn_sig(vec![], str_array)),
            effects: EffectSet::default(),
            is_async: false,
        },
    ]
}
