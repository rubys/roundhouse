//! `friendly_id :col, use: %i[slugged history]` — friendly_id's model
//! macro, lowered in the shared model lowering (all targets): the
//! class-side `Model.friendly`, the two facts the finder needs (the slug
//! column, whether `:history` is on), and the `to_param` friendly_id
//! installs.
//!
//! The finder itself is runtime (`ActiveRecord::Relation#friendly_find`,
//! runtime/ruby/active_record/relation.rb): it reads friendly_id 5.x's
//! `FinderMethods#find` and `History::FinderMethods` order of operations
//! and is configured per model by the two synthesized class methods
//! below. The analyzer types `Model.friendly` as the model's Relation
//! (`register_friendly_id`), and `Relation#friendly` is a catalog
//! builder, so `Article.friendly.includes(:x).find(p)` and
//! `Article.with_discarded.friendly.find(p)` chain like any relation.
//!
//! ```ruby
//! def self._friendly_slug_column = "slug"   # `slug_column:` if given
//! def self._friendly_history = true         # `use:` includes :history
//! def self.friendly = ActiveRecord::Relation.new(self).friendly
//! def to_param                              # friendly_id.presence || id
//!   s = slug
//!   (s.nil? || s.empty?) ? @id.to_s : s
//! end
//! ```
//!
//! ## Claimed and declined
//!
//! Claimed: one `friendly_id` call naming a column or method (`:title`)
//! with `use:` a symbol or array of symbols that includes `:slugged`
//! and otherwise only `:history`, `:reserved`, `:sequentially_slugged`
//! (they affect slug generation, not lookup); options `slug_column:`,
//! `dependent:` and the generation-only `sequence_separator:` /
//! `reserved_words:` and `routes: :friendly`; on a model with an
//! integer `id` and a String slug column. `extend FriendlyId` is
//! claimed with it. Anything else (`:scoped`, `:finders` — which
//! changes `Model.find` itself — `:simple_i18n`, a configuration block,
//! `routes: :default`, a missing slug column) is declined whole: the
//! model keeps its unsupported warnings and `Model.friendly` stays an
//! unknown method, rather than a finder with different semantics.
//!
//! ## Not modeled: slug generation
//!
//! friendly_id also WRITES the slug (`before_validation`:
//! `slug_candidates`, `normalize_friendly_id`, uniqueness sequencing,
//! `should_generate_new_friendly_id?`) and, under `:history`, a
//! `friendly_id_slugs` row after save. None of that is lowered: a
//! record saves with whatever its slug column holds. `write_gap` reports
//! that as a warning on every claimed declaration, and a model that
//! overrides the generation hooks reports them as well.

use super::model_to_library::fn_sig;
use crate::dialect::{MethodDef, Model, ModelBodyItem};
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;
use crate::span::Span;
use crate::ty::Ty;

/// What one claimed `friendly_id` declaration says.
pub(crate) struct FriendlyConfig {
    pub(crate) slug_column: Symbol,
    pub(crate) history: bool,
    /// The `friendly_id` call, and the `extend FriendlyId` that precedes it.
    pub(crate) call_span: Span,
    pub(crate) extend_span: Option<Span>,
}

/// `use:` modules that only act on slug generation.
const GENERATION_ONLY: &[&str] = &["reserved", "sequentially_slugged"];
/// Options that only act on slug generation (or `dependent:` on the
/// history association's destroy).
const GENERATION_OPTIONS: &[&str] = &["dependent", "sequence_separator", "reserved_words"];

/// The model's friendly_id configuration, or `None` when it declares
/// none or declares one this lowering cannot honor (see the module doc).
pub(crate) fn config(model: &Model) -> Option<FriendlyConfig> {
    let mut found: Option<FriendlyConfig> = None;
    let mut extend_span = None;
    for item in &model.body {
        let ModelBodyItem::Unknown { expr, .. } = item else { continue };
        let ExprNode::Send { recv: None, method, args, block, .. } = &*expr.node else { continue };
        match method.as_str() {
            "extend" if is_friendly_id_const(args.first()) => extend_span = Some(expr.span),
            "friendly_id" => {
                if found.is_some() || block.is_some() {
                    return None;
                }
                found = Some(parse_call(expr.span, args)?);
            }
            _ => {}
        }
    }
    let mut cfg = found?;
    cfg.extend_span = extend_span;
    let integer_id = model.primary_key.as_ref().is_none_or(|k| k.as_str() == "id")
        && model.attributes.fields.get(&Symbol::from("id")).is_none_or(|t| *t == Ty::Int);
    let slug_is_string = model.attributes.fields.get(&cfg.slug_column).is_some_and(|t| match t {
        Ty::Str => true,
        Ty::Union { variants } => variants.iter().all(|v| matches!(v, Ty::Str | Ty::Nil)),
        _ => false,
    });
    (integer_id && slug_is_string).then_some(cfg)
}

fn is_friendly_id_const(arg: Option<&Expr>) -> bool {
    matches!(arg.map(|a| &*a.node), Some(ExprNode::Const { path }) if path.len() == 1 && path[0].as_str() == "FriendlyId")
}

fn sym_or_str(e: &Expr) -> Option<Symbol> {
    match &*e.node {
        ExprNode::Lit { value: Literal::Sym { value } } => Some(value.clone()),
        ExprNode::Lit { value: Literal::Str { value } } => Some(Symbol::from(value.as_str())),
        _ => None,
    }
}

fn parse_call(span: Span, args: &[Expr]) -> Option<FriendlyConfig> {
    let mut slug_column = Symbol::from("slug");
    let mut history = false;
    let mut slugged = false;
    for (i, arg) in args.iter().enumerate() {
        match &*arg.node {
            // The base: a column or method name. It feeds generation only.
            ExprNode::Lit { value: Literal::Sym { .. } } if i == 0 => {}
            ExprNode::Hash { entries, .. } => {
                for (k, v) in entries {
                    let ExprNode::Lit { value: Literal::Sym { value: key } } = &*k.node else {
                        return None;
                    };
                    match key.as_str() {
                        "use" => {
                            let mods: Vec<&Expr> = match &*v.node {
                                ExprNode::Array { elements, .. } => elements.iter().collect(),
                                _ => vec![v],
                            };
                            for m in mods {
                                match sym_or_str(m)?.as_str() {
                                    "slugged" => slugged = true,
                                    "history" => history = true,
                                    g if GENERATION_ONLY.contains(&g) => {}
                                    _ => return None,
                                }
                            }
                        }
                        "slug_column" => slug_column = sym_or_str(v)?,
                        "routes" => {
                            if sym_or_str(v)?.as_str() != "friendly" {
                                return None;
                            }
                        }
                        o if GENERATION_OPTIONS.contains(&o) => {}
                        _ => return None,
                    }
                }
            }
            _ => return None,
        }
    }
    slugged.then_some(FriendlyConfig { slug_column, history, call_span: span, extend_span: None })
}

/// Whether the statement at `span` is a `friendly_id` call or its
/// `extend FriendlyId` on a model this pass claims.
/// `report_unclaimed_unknowns` asks.
pub(crate) fn claims(model: &Model, span: Span) -> bool {
    config(model).is_some_and(|c| c.call_span == span || c.extend_span == Some(span))
}

/// The gap on a claimed model: slug generation is not lowered. The
/// message names which generation hooks the model wrote, since those are
/// the ones whose behavior it can no longer rely on.
pub(crate) fn write_gap(model: &Model) -> Option<(Span, String)> {
    let cfg = config(model)?;
    let hooks: Vec<&str> = ["slug_candidates", "should_generate_new_friendly_id?", "normalize_friendly_id"]
        .into_iter()
        .filter(|h| {
            model.body.iter().any(|item| match item {
                ModelBodyItem::Method { method, .. } => method.name.as_str() == *h,
                _ => false,
            })
        })
        .collect();
    let mut msg = format!(
        "friendly_id on `{}`: the finder (`{}.friendly.find`) and `to_param` are lowered, but slug \
         generation on save is not — nothing writes `{}`{}, so a record saves with the slug it was \
         given",
        model.name.0.as_str(),
        model.name.0.as_str(),
        cfg.slug_column.as_str(),
        if cfg.history { " or a `friendly_id_slugs` history row" } else { "" },
    );
    if !hooks.is_empty() {
        msg.push_str(&format!("; the model's own `{}` is never consulted", hooks.join("`, `")));
    }
    Some((cfg.call_span, msg))
}

/// Synthesize `friendly`, its two configuration readers and `to_param`
/// onto the lowered methods. A method the model wrote itself wins.
pub(crate) fn push_friendly_methods(methods: &mut Vec<MethodDef>, model: &Model) {
    let Some(cfg) = config(model) else { return };
    let class = model.name.0.as_str();
    let slug = cfg.slug_column.as_str();
    let src = format!(
        "class {class}
  def self._friendly_slug_column
    \"{slug}\"
  end

  def self._friendly_history
    {history}
  end

  def self.friendly
    ActiveRecord::Relation.new(self).friendly
  end

  def to_param
    s = {slug}
    (s.nil? || s.empty?) ? @id.to_s : s
  end
end
",
        history = cfg.history,
    );
    let (parsed, diags) = crate::ingest::prism::scope(|| {
        crate::ingest::ingest_library_classes(src.as_bytes(), "<friendly_id>")
    });
    let synthesized: Vec<MethodDef> = match parsed {
        Ok(classes) if diags.is_empty() => classes.into_iter().flat_map(|c| c.methods).collect(),
        Ok(_) => {
            crate::ingest::survey::record_synthesis_failure(
                "<friendly_id>",
                &format!("friendly_id methods for `{class}`"),
                &diags,
            );
            return;
        }
        Err(err) => {
            crate::ingest::survey::record(&err);
            return;
        }
    };
    for mut m in synthesized {
        m.signature = match m.name.as_str() {
            "_friendly_slug_column" => Some(fn_sig(vec![], Ty::Str)),
            "_friendly_history" => Some(fn_sig(vec![], Ty::Bool)),
            "friendly" => Some(fn_sig(vec![], Ty::Relation { of: model.name.clone() })),
            "to_param" => Some(fn_sig(vec![], Ty::Str)),
            other => {
                crate::ingest::survey::record_synthesis_failure(
                    "<friendly_id>",
                    &format!("unexpected synthesized method `{other}` on `{class}`"),
                    &[],
                );
                continue;
            }
        };
        if !methods.iter().any(|x| x.name == m.name && x.receiver == m.receiver) {
            methods.push(m);
        }
    }
}
