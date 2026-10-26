//! The `discard` gem's `Discard::Model`, expanded where it is included.
//!
//! `include Discard::Model` is a mixin whose whole effect is a handful of
//! scopes and methods parameterised by one column (`discard_column`,
//! default `discarded_at`). There is no `Discard::Model` module to emit,
//! so the include is *replaced* by the declarations it stands for, as if
//! the model had written them (the same desugaring `enum` gets):
//!
//! ```ruby
//! scope :kept,           -> { where(col: nil) }            # `undiscarded` in the gem
//! scope :undiscarded,    -> { where(col: nil) }
//! scope :discarded,      -> { where.not(col: nil) }
//! scope :with_discarded, -> { unscope(where: :col) }
//! def discarded?   = !col.nil?
//! def undiscarded? = col.nil?
//! def kept?        = undiscarded?
//! def discard      # run_callbacks(:discard) { update_attribute(col, Time.current) }
//! def discard!     # discard || raise
//! def undiscard / undiscard!
//! ```
//!
//! Runs after the concern splice, so an include that arrived through a
//! concern's `included do` (`Moderatable`) is expanded in each includer.
//!
//! # Boundary
//!
//! * The gem must be in `Gemfile.lock` and the model's table must carry a
//!   `datetime` column of the discard column's name; `self.discard_column
//!   = :other` is honoured when its argument is a literal symbol. Anything
//!   else is refused (and the include stays, so the call sites keep
//!   failing loudly rather than dispatching on nothing).
//! * Callbacks are the gem's own: `define_model_callbacks :discard,
//!   :undiscard` and `run_callbacks(:discard) { update_attribute(..) }`,
//!   which `model_callbacks` builds into the chain every declaration of
//!   them (`before_`/`around_`/`after_discard`, `if:`, a block) joins.
//! * `discard_all` / `undiscard_all` are not generated.
//! * `Discard::RecordNotDiscarded` is not a class here: `discard!` raises
//!   `ActiveRecord::RecordNotSaved` with the gem's message instead.

use crate::dialect::{MethodReceiver, Model, ModelBodyItem};
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;

use super::{survey, IngestError, IngestResult};

const DEFAULT_COLUMN: &str = "discarded_at";

fn const_path(expr: &Expr) -> Option<String> {
    let ExprNode::Const { path } = &*expr.node else { return None };
    Some(path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::"))
}

/// Indices of body items that are `include …` naming `Discard::Model`.
fn discard_includes(model: &Model) -> Vec<usize> {
    model
        .body
        .iter()
        .enumerate()
        .filter(|(_, item)| {
            let ModelBodyItem::Unknown { expr, .. } = item else { return false };
            let ExprNode::Send { recv: None, method, args, block: None, .. } = &*expr.node else {
                return false;
            };
            method.as_str() == "include"
                && args.iter().any(|a| const_path(a).as_deref() == Some("Discard::Model"))
        })
        .map(|(i, _)| i)
        .collect()
}

/// `self.discard_column = :sym` → `Some(Ok(sym))`; any other value
/// → `Some(Err(()))`; no declaration → `None`. Also returns the index.
fn declared_column(model: &Model) -> Option<(usize, Result<String, ()>)> {
    model.body.iter().enumerate().find_map(|(i, item)| {
        let ModelBodyItem::Unknown { expr, .. } = item else { return None };
        let ExprNode::Send { recv: Some(recv), method, args, .. } = &*expr.node else {
            return None;
        };
        if !matches!(&*recv.node, ExprNode::SelfRef) || method.as_str() != "discard_column=" {
            return None;
        }
        let value = match args.as_slice() {
            [arg] => match &*arg.node {
                ExprNode::Lit { value: Literal::Sym { value } } => Ok(value.as_str().to_string()),
                _ => Err(()),
            },
            _ => Err(()),
        };
        Some((i, value))
    })
}

fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn source(col: &str) -> String {
    format!(
        "class DiscardSynth < ApplicationRecord\n\
         \x20 scope :kept, -> {{ where({col}: nil) }}\n\
         \x20 scope :undiscarded, -> {{ where({col}: nil) }}\n\
         \x20 scope :discarded, -> {{ where.not({col}: nil) }}\n\
         \x20 scope :with_discarded, -> {{ unscope(where: :{col}) }}\n\
         \x20 define_model_callbacks :discard\n\
         \x20 define_model_callbacks :undiscard\n\
         \x20 def discarded?\n    !{col}.nil?\n  end\n\
         \x20 def undiscarded?\n    {col}.nil?\n  end\n\
         \x20 def kept?\n    undiscarded?\n  end\n\
         \x20 def discard\n    return false if discarded?\n    run_callbacks(:discard) do\n      update_attribute(:{col}, Time.current)\n    end\n  end\n\
         \x20 def discard!\n    discard || raise(ActiveRecord::RecordNotSaved.new(\"Failed to discard the record\", self))\n  end\n\
         \x20 def undiscard\n    return false unless discarded?\n    run_callbacks(:undiscard) do\n      update_attribute(:{col}, nil)\n    end\n  end\n\
         \x20 def undiscard!\n    undiscard || raise(ActiveRecord::RecordNotSaved.new(\"Failed to undiscard the record\", self))\n  end\n\
         end\n"
    )
}

pub(crate) fn expand_discard_models(app: &mut crate::App) -> IngestResult<()> {
    let locked = app.gem_lock.as_ref().is_some_and(|l| l.has("discard"));
    if !locked {
        return Ok(());
    }
    for mi in 0..app.models.len() {
        let includes = discard_includes(&app.models[mi]);
        if includes.is_empty() {
            continue;
        }
        let name = app.models[mi].name.0.as_str().to_string();
        let refuse = |why: String| {
            survey::continue_or_fail(IngestError::Unsupported {
                file: name.clone(),
                message: format!("`include Discard::Model` on {name} is not supported: {why}"),
            })
        };

        let col = match declared_column(&app.models[mi]) {
            None => DEFAULT_COLUMN.to_string(),
            Some((_, Ok(c))) if is_ident(&c) => c,
            Some(_) => {
                refuse("`self.discard_column =` must be a literal symbol".into())?;
                continue;
            }
        };
        let table = app.models[mi].table.0.as_str().to_string();
        let has_column = app.schema.tables.get(&app.models[mi].table.0).is_some_and(|t| {
            t.columns.iter().any(|c| {
                c.name.as_str() == col && matches!(c.col_type, crate::schema::ColumnType::DateTime)
            })
        });
        if !has_column {
            refuse(format!("table `{table}` has no datetime column `{col}` (the discard column)"))?;
            continue;
        }

        let src = source(&col);
        let (parsed, diags) = super::prism::scope(|| {
            super::model::ingest_model(src.as_bytes(), "<discard>", &app.schema, &Default::default())
        });
        let synth = match parsed {
            Ok(Some(m)) if diags.is_empty() => m,
            Ok(_) => {
                survey::record_synthesis_failure("<discard>", &format!("Discard::Model for `{name}`"), &diags);
                continue;
            }
            Err(err) => {
                survey::record(&err);
                continue;
            }
        };

        // A name the model declares itself wins (Ruby: the model's own
        // definition sits in front of the mixin's).
        let model = &mut app.models[mi];
        let own: Vec<(bool, Symbol)> = model.body.iter().filter_map(|it| match it {
            ModelBodyItem::Scope { scope, .. } => Some((true, scope.name.clone())),
            ModelBodyItem::Method { method, .. } if method.receiver == MethodReceiver::Instance =>
                Some((false, method.name.clone())),
            _ => None,
        }).collect();
        let additions: Vec<ModelBodyItem> = synth.body.into_iter().filter(|it| match it {
            ModelBodyItem::Scope { scope, .. } => !own.contains(&(true, scope.name.clone())),
            ModelBodyItem::Method { method, .. } => !own.contains(&(false, method.name.clone())),
            ModelBodyItem::Unknown { expr, .. } => matches!(&*expr.node, ExprNode::Send { method, .. } if method.as_str() == "define_model_callbacks"),
            _ => false,
        }).collect();

        // Drop the consumed callback/column declarations and the include
        // (or just the `Discard::Model` argument of a list include), then
        // put the expansion where the first include stood.
        let first = includes[0];
        let mut remove: Vec<usize> = Vec::new();
        if let Some((i, _)) = declared_column(model) { remove.push(i); }
        for &i in &includes {
            let ModelBodyItem::Unknown { expr, .. } = &mut model.body[i] else { continue };
            let ExprNode::Send { args, .. } = &mut *expr.node else { continue };
            args.retain(|a| const_path(a).as_deref() != Some("Discard::Model"));
            if args.is_empty() { remove.push(i); }
        }
        let insert_at = first - remove.iter().filter(|&&i| i < first).count();
        remove.sort_unstable();
        remove.dedup();
        for &i in remove.iter().rev() { model.body.remove(i); }
        let at = insert_at.min(model.body.len());
        model.body.splice(at..at, additions);
    }
    Ok(())
}
