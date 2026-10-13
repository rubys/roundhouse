//! The `acts_as_votable` gem (0.14.0): `ActsAsVotable::Vote` and the
//! `acts_as_votable` / `acts_as_voter` macros, expanded where they are used.
//!
//! The gem is two ActiveRecord mixins and one ordinary model:
//!
//! * `ActsAsVotable::Vote` is an AR model on the `votes` table with two
//!   polymorphic `belongs_to` (`votable`, `voter`), the `up` / `down` /
//!   `for_type` / `by_type` scopes and presence validations on the ids.
//!   When the gem is in `Gemfile.lock` and the schema has a `votes` table
//!   carrying the gem's columns, that model is synthesized from Ruby source
//!   (the same route `discard` takes) so the constant resolves, and
//!   `where(voter: user, ...)`, `pluck`, `sanitize_sql_array` run through
//!   the ordinary model path.
//! * `acts_as_votable` adds `has_many :votes_for, as: :votable,
//!   class_name: "ActsAsVotable::Vote", dependent: :delete_all` and the
//!   voting API (`vote_by`, `vote_up`/`liked_by`..., `unvote`/`unliked_by`,
//!   `get_up_votes`, `count_votes_*`, `update_cached_votes`, ...).
//! * `acts_as_voter` adds `has_many :votes, as: :voter, ...` and
//!   `voted_on?` / `voted_up_on?` / `voted_down_on?` (with their aliases).
//!
//! `update_cached_votes` is generated from the votable's own columns: only
//! `cached_votes_{total,up,down,score}` and `cached_weighted_{total,score,
//! average}` that exist are written, plus the per-scope variants
//! (`cached_scoped_<scope>_votes_*`, `cached_weighted_<scope>_*`) that the
//! table carries, selected by comparing `vote_scope` to each literal scope.
//! This is the gem's `respond_to?(:cached_x=)` check, resolved statically.
//!
//! # Boundary
//!
//! * Gem locked and `votes` table present; otherwise the constant stays
//!   unresolved and a model that calls the macros is refused.
//! * `acts_as_votable cacheable_strategy: :update | :update_columns`
//!   (literal) is honoured; any other argument is refused.
//! * Not generated (so call sites keep failing loudly): the
//!   `votes_for.voters` / `votes.votables` association extensions,
//!   `vote_up_for` / `vote_down_for` / `unvote_for` / `vote` on the voter
//!   (they dispatch on a polymorphic votable), `find_*voted_items`,
//!   `get_voted` / `get_up_voted` / `get_down_voted`, `find_votes` with a
//!   free-form conditions hash, `votable_words`, and the
//!   `ProtectedAttributes` branch.
//! * `vote_weight:` nil defaults to 1; a blank string is not special-cased
//!   (the gem's `present?` makes it 1, `"".to_i` makes it 0 here).
//! * `vote_by` takes keywords (`voter:`, `vote:`, `vote_scope:`,
//!   `vote_weight:`, `duplicate:`), the way every call site writes it.

use crate::dialect::{Model, ModelBodyItem};
use crate::expr::{Expr, ExprNode, Literal};

use super::{survey, IngestError, IngestResult};

const VOTE_COLUMNS: [&str; 7] =
    ["votable_id", "votable_type", "voter_id", "voter_type", "vote_flag", "vote_scope", "vote_weight"];

fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Indices (and arguments) of `acts_as_votable` / `acts_as_voter` calls.
fn macro_calls(model: &Model, name: &str) -> Vec<(usize, Vec<Expr>)> {
    model
        .body
        .iter()
        .enumerate()
        .filter_map(|(i, item)| {
            let ModelBodyItem::Unknown { expr, .. } = item else { return None };
            let ExprNode::Send { recv: None, method, args, block: None, .. } = &*expr.node else {
                return None;
            };
            (method.as_str() == name).then(|| (i, args.clone()))
        })
        .collect()
}

/// `acts_as_votable` options: `Ok(strategy)` for no argument or a literal
/// `cacheable_strategy:`; `Err` for anything the gem would take that this
/// expansion cannot reproduce.
fn votable_strategy(args: &[Expr]) -> Result<&'static str, String> {
    match args {
        [] => Ok("update"),
        [arg] => {
            let ExprNode::Hash { entries, .. } = &*arg.node else {
                return Err("the options must be a literal hash".into());
            };
            let mut strategy = "update";
            for (k, v) in entries {
                let key = match &*k.node {
                    ExprNode::Lit { value: Literal::Sym { value } } => value.as_str(),
                    _ => "",
                };
                let val = match &*v.node {
                    ExprNode::Lit { value: Literal::Sym { value } } => value.as_str(),
                    _ => "",
                };
                match (key, val) {
                    ("cacheable_strategy", "update") => strategy = "update",
                    ("cacheable_strategy", "update_columns") => strategy = "update_columns",
                    ("cacheable_strategy", _) => {
                        return Err("`cacheable_strategy:` must be :update or :update_columns".into())
                    }
                    (other, _) => return Err(format!("option `{other}` is not modeled")),
                }
            }
            Ok(strategy)
        }
        _ => Err("expected at most one options hash".into()),
    }
}

fn vote_model_source(parent: &str) -> String {
    format!(
        "module ActsAsVotable\n\
         \x20 class Vote < {parent}\n\
         \x20   self.table_name = \"votes\"\n\
         \x20   belongs_to :votable, polymorphic: true\n\
         \x20   belongs_to :voter, polymorphic: true\n\
         \x20   scope :up, -> {{ where(vote_flag: true) }}\n\
         \x20   scope :down, -> {{ where(vote_flag: false) }}\n\
         \x20   scope :for_type, ->(klass) {{ where(votable_type: klass.to_s) }}\n\
         \x20   scope :by_type, ->(klass) {{ where(voter_type: klass.to_s) }}\n\
         \x20   validates_presence_of :votable_id\n\
         \x20   validates_presence_of :voter_id\n\
         \x20 end\n\
         end\n"
    )
}

/// What the votable's table lets `update_cached_votes` write.
struct Columns {
    names: Vec<String>,
    scopes: Vec<String>,
}

impl Columns {
    fn new(names: Vec<String>) -> Self {
        let mut scopes: Vec<String> = Vec::new();
        for n in &names {
            let mid = n
                .strip_prefix("cached_scoped_")
                .and_then(|r| {
                    ["_votes_total", "_votes_up", "_votes_down", "_votes_score"]
                        .iter()
                        .find_map(|s| r.strip_suffix(s))
                })
                .or_else(|| {
                    n.strip_prefix("cached_weighted_")
                        .and_then(|r| ["_total", "_score", "_average"].iter().find_map(|s| r.strip_suffix(s)))
                });
            if let Some(scope) = mid {
                if is_ident(scope) && !scopes.iter().any(|s| s == scope) {
                    scopes.push(scope.to_string());
                }
            }
        }
        Columns { names, scopes }
    }
    fn has(&self, col: &str) -> bool {
        self.names.iter().any(|n| n == col)
    }
}

/// The cache column for `kind` under `scope` (the gem's `scope_cache_field`).
fn cache_col(kind: &str, scope: Option<&str>) -> String {
    let counted = kind.starts_with("votes_");
    match (counted, scope) {
        (true, None) => format!("cached_{kind}"),
        (true, Some(s)) => format!("cached_scoped_{s}_{kind}"),
        (false, None) => format!("cached_weighted_{kind}"),
        (false, Some(s)) => format!("cached_weighted_{s}_{kind}"),
    }
}

/// `unless skip_cache … return cached` prelude for a counting method.
fn cache_prelude(kind: &str, cols: &Columns) -> String {
    let mut out = String::new();
    if cols.has(&cache_col(kind, None)) {
        out.push_str(&format!("      return {} if vote_scope.nil?\n", cache_col(kind, None)));
    }
    for s in &cols.scopes {
        let c = cache_col(kind, Some(s));
        if cols.has(&c) {
            out.push_str(&format!("      return {c} if vote_scope == \"{s}\"\n"));
        }
    }
    if out.is_empty() {
        out
    } else {
        format!("    unless skip_cache\n{out}    end\n")
    }
}

/// `column: expression` pairs `update_cached_votes` writes for `scope`.
fn cache_updates(cols: &Columns, scope: Option<&str>) -> Vec<String> {
    let arg = match scope {
        Some(s) => format!("\"{s}\""),
        None => "nil".to_string(),
    };
    let mut pairs = Vec::new();
    let mut add = |kind: &str, expr: String| {
        let c = cache_col(kind, scope);
        if cols.has(&c) {
            pairs.push(format!("{c}: {expr}"));
        }
    };
    add("votes_up", format!("fresh_votes_up({arg})"));
    add("votes_down", format!("fresh_votes_down({arg})"));
    add("votes_total", format!("fresh_votes_up({arg}) + fresh_votes_down({arg})"));
    add("votes_score", format!("fresh_votes_up({arg}) - fresh_votes_down({arg})"));
    add("total", format!("fresh_weighted_total({arg})"));
    add("score", format!("fresh_weighted_score({arg})"));
    add("average", format!("fresh_weighted_average({arg})"));
    pairs
}

fn votable_source(strategy: &str, cols: &Columns, owner: &str) -> String {
    let mut s = String::from("class VotableSynth < ApplicationRecord\n");
    s.push_str("  has_many :votes_for, class_name: \"ActsAsVotable::Vote\", as: :votable\n");
    s.push_str("  before_destroy :delete_votes_for\n  def delete_votes_for\n    votes_for.delete_all\n  end\n");
    s.push_str(
        "  def self.votable?\n    true\n  end\n\
         \x20 def vote_registered\n    @vote_registered\n  end\n\
         \x20 def vote_registered=(value)\n    @vote_registered = value\n  end\n\
         \x20 def vote_registered?\n    @vote_registered\n  end\n",
    );
    // vote_by: `votable_words.meaning_of` is `!that_mean_false.include?`.
    s.push_str(
        "  def vote_by(voter:, vote: true, vote_scope: nil, vote_weight: nil, duplicate: false)\n\
         \x20   return false if voter.nil?\n\
         \x20   @vote_registered = false\n\
         \x20   existing = find_votes_by(voter, vote_scope)\n\
         \x20   if duplicate || !existing.exists?\n\
         \x20     record = ActsAsVotable::Vote.new(votable: self, voter: voter, vote_scope: vote_scope)\n\
         \x20   else\n\
         \x20     record = existing.last\n\
         \x20   end\n\
         \x20   new_flag = !(vote == \"down\" || vote == \"downvote\" || vote == \"dislike\" || vote == \"disliked\" || vote == \"negative\" || vote == \"no\" || vote == \"bad\" || vote == \"disagree\" || vote == \"false\" || vote == 0 || vote == false)\n\
         \x20   new_weight = vote_weight.nil? ? 1 : vote_weight.to_i\n\
         \x20   changed = record.new_record? || record.vote_flag != new_flag || record.vote_weight != new_weight\n\
         \x20   record.vote_flag = new_flag\n\
         \x20   record.vote_weight = new_weight\n\
         \x20   vote_saved = false\n\
         \x20   ActiveRecord::Base.transaction do\n\
         \x20     @vote_registered = false\n\
         \x20     vote_saved = changed ? record.save : true\n\
         \x20     if vote_saved\n\
         \x20       @vote_registered = true if changed\n\
         \x20       update_cached_votes(vote_scope)\n\
         \x20     end\n\
         \x20   end\n\
         \x20   vote_saved\n\
         \x20 end\n\
         \x20 def unvote(voter:, vote_scope: nil)\n\
         \x20   return false if voter.nil?\n\
         \x20   existing = find_votes_by(voter, vote_scope)\n\
         \x20   ActiveRecord::Base.transaction do\n\
         \x20     deleted_count = existing.delete_all\n\
         \x20     update_cached_votes(vote_scope) if deleted_count > 0\n\
         \x20   end\n\
         \x20   @vote_registered = false\n\
         \x20   true\n\
         \x20 end\n\
         \x20 def vote_up(voter, vote_scope: nil, vote_weight: nil)\n\
         \x20   vote_by(voter: voter, vote: true, vote_scope: vote_scope, vote_weight: vote_weight)\n\
         \x20 end\n\
         \x20 def vote_down(voter, vote_scope: nil, vote_weight: nil)\n\
         \x20   vote_by(voter: voter, vote: false, vote_scope: vote_scope, vote_weight: vote_weight)\n\
         \x20 end\n\
         \x20 def unvote_by(voter, vote_scope: nil)\n\
         \x20   unvote(voter: voter, vote_scope: vote_scope)\n\
         \x20 end\n",
    );
    // Aliases of vote_up / vote_down / unvote_by, as the gem's `included` hook.
    for (target, names) in [
        (
            "vote_up",
            &["up_by", "upvote_by", "like_by", "liked_by", "up_from", "upvote_from", "like_from", "liked_from", "vote_from"][..],
        ),
        ("vote_down", &["down_by", "downvote_by", "dislike_by", "disliked_by", "down_from", "downvote_from"][..]),
    ] {
        for n in names {
            s.push_str(&format!(
                "  def {n}(voter, vote_scope: nil, vote_weight: nil)\n    {target}(voter, vote_scope: vote_scope, vote_weight: vote_weight)\n  end\n"
            ));
        }
    }
    for n in ["unvote_up", "unvote_down", "unliked_by", "undisliked_by"] {
        s.push_str(&format!(
            "  def {n}(voter, vote_scope: nil)\n    unvote_by(voter, vote_scope: vote_scope)\n  end\n"
        ));
    }
    // Finders.
    s.push_str(
        "  def find_votes_by(voter, vote_scope)\n\
         \x20   votes_for.where(voter: voter, vote_scope: vote_scope)\n\
         \x20 end\n\
         \x20 def get_up_votes(vote_scope: nil)\n\
         \x20   if vote_scope.nil?\n      votes_for.where(vote_flag: true)\n    else\n      votes_for.where(vote_flag: true, vote_scope: vote_scope)\n    end\n\
         \x20 end\n\
         \x20 def get_down_votes(vote_scope: nil)\n\
         \x20   if vote_scope.nil?\n      votes_for.where(vote_flag: false)\n    else\n      votes_for.where(vote_flag: false, vote_scope: vote_scope)\n    end\n\
         \x20 end\n\
         \x20 def voted_on_by?(voter)\n\
         \x20   votes_for.where(voter: voter).exists?\n\
         \x20 end\n\
         \x20 def voted_up_by?(voter)\n\
         \x20   votes_for.where(voter: voter, vote_flag: true).exists?\n\
         \x20 end\n\
         \x20 def voted_down_by?(voter)\n\
         \x20   votes_for.where(voter: voter, vote_flag: false).exists?\n\
         \x20 end\n",
    );
    for n in ["get_true_votes", "get_ups", "get_upvotes", "get_likes", "get_positives", "get_for_votes"] {
        s.push_str(&format!("  def {n}(vote_scope: nil)\n    get_up_votes(vote_scope: vote_scope)\n  end\n"));
    }
    for n in ["get_false_votes", "get_downs", "get_downvotes", "get_dislikes", "get_negatives"] {
        s.push_str(&format!("  def {n}(vote_scope: nil)\n    get_down_votes(vote_scope: vote_scope)\n  end\n"));
    }
    // Counting. `fresh_*` always queries; the public `count_votes_*` /
    // `weighted_*` read the cache column first unless `skip_cache`, as the
    // gem's `from_cache` does (resolved statically against the columns).
    s.push_str(
        "  def fresh_votes_total(vote_scope)\n\
         \x20   if vote_scope.nil?\n      votes_for.count\n    else\n      votes_for.where(vote_scope: vote_scope).count\n    end\n\
         \x20 end\n\
         \x20 def fresh_votes_up(vote_scope)\n    get_up_votes(vote_scope: vote_scope).count\n  end\n\
         \x20 def fresh_votes_down(vote_scope)\n    get_down_votes(vote_scope: vote_scope).count\n  end\n\
         \x20 def fresh_weighted_total(vote_scope)\n\
         \x20   if vote_scope.nil?\n      votes_for.pluck(:vote_weight).compact.sum\n    else\n      votes_for.where(vote_scope: vote_scope).pluck(:vote_weight).compact.sum\n    end\n\
         \x20 end\n\
         \x20 def fresh_weighted_score(vote_scope)\n\
         \x20   get_up_votes(vote_scope: vote_scope).pluck(:vote_weight).compact.sum - get_down_votes(vote_scope: vote_scope).pluck(:vote_weight).compact.sum\n\
         \x20 end\n\
         \x20 def fresh_weighted_average(vote_scope)\n\
         \x20   total = fresh_votes_total(vote_scope)\n\
         \x20   if total > 0\n      fresh_weighted_score(vote_scope).to_f / total\n    else\n      0.0\n    end\n\
         \x20 end\n",
    );
    let def = |name: &str, kind: &str, body: &str| {
        format!(
            "  def {name}(skip_cache = false, vote_scope = nil)\n{}{body}  end\n",
            cache_prelude(kind, cols)
        )
    };
    s.push_str(&def("count_votes_total", "votes_total", "    fresh_votes_total(vote_scope)\n"));
    s.push_str(&def("count_votes_up", "votes_up", "    fresh_votes_up(vote_scope)\n"));
    s.push_str(&def("count_votes_down", "votes_down", "    fresh_votes_down(vote_scope)\n"));
    s.push_str(&def(
        "count_votes_score",
        "votes_score",
        "    fresh_votes_up(vote_scope) - fresh_votes_down(vote_scope)\n",
    ));
    s.push_str(&def("weighted_total", "total", "    fresh_weighted_total(vote_scope)\n"));
    s.push_str(&def("weighted_score", "score", "    fresh_weighted_score(vote_scope)\n"));
    s.push_str(&def(
        "weighted_average",
        "average",
        "    total = count_votes_total(skip_cache, vote_scope).to_i\n    if total > 0\n      weighted_score(skip_cache, vote_scope).to_f / total\n    else\n      0.0\n    end\n",
    ));

    // update_cached_votes: only the columns the table has.
    s.push_str("  def update_cached_votes(vote_scope = nil)\n");
    let call = |pairs: &[String]| format!("{strategy}({})", pairs.join(", "));
    let base = cache_updates(cols, None);
    let mut branches: Vec<(String, Vec<String>)> = Vec::new();
    for sc in &cols.scopes {
        let mut pairs = base.clone();
        let scoped = cache_updates(cols, Some(sc));
        if !scoped.is_empty() {
            pairs.extend(scoped);
            branches.push((sc.clone(), pairs));
        }
    }
    if branches.is_empty() {
        if !base.is_empty() {
            s.push_str(&format!("    {}\n", call(&base)));
        } else {
            s.push_str("    nil\n");
        }
    } else {
        for (i, (sc, pairs)) in branches.iter().enumerate() {
            s.push_str(&format!(
                "    {} vote_scope == \"{sc}\"\n      {}\n",
                if i == 0 { "if" } else { "elsif" },
                call(pairs)
            ));
        }
        if !base.is_empty() {
            s.push_str(&format!("    else\n      {}\n", call(&base)));
        }
        s.push_str("    end\n");
    }
    s.push_str("  end\nend\n");
    // `has_many … as:` readers are not relation-seedable in lowering, so the
    // generated bodies query the vote table directly, keyed by the same
    // (id, base-class name) pair the polymorphic association uses.
    s.replace("votes_for.", &format!("ActsAsVotable::Vote.where(votable_id: id, votable_type: \"{owner}\")."))
}

fn voter_source(owner: &str) -> String {
    let mut s = String::from("class VoterSynth < ApplicationRecord\n");
    s.push_str("  has_many :votes, class_name: \"ActsAsVotable::Vote\", as: :voter\n");
    s.push_str("  before_destroy :delete_votes_cast\n  def delete_votes_cast\n    votes.delete_all\n  end\n");
    s.push_str("  def self.voter?\n    true\n  end\n");
    for (name, flag) in [("voted_on?", None), ("voted_up_on?", Some("true")), ("voted_down_on?", Some("false"))] {
        let flag_cond = flag.map(|f| format!(", vote_flag: {f}")).unwrap_or_default();
        s.push_str(&format!(
            "  def {name}(votable, vote_scope: nil)\n\
             \x20   votes.where(votable: votable, vote_scope: vote_scope{flag_cond}).exists?\n\
             \x20 end\n"
        ));
    }
    for (alias, target) in [
        ("voted_for?", "voted_on?"),
        ("voted_up_for?", "voted_up_on?"),
        ("liked?", "voted_up_on?"),
        ("voted_down_for?", "voted_down_on?"),
        ("disliked?", "voted_down_on?"),
    ] {
        s.push_str(&format!("  def {alias}(votable, vote_scope: nil)\n    {target}(votable, vote_scope: vote_scope)\n  end\n"));
    }
    s.push_str(
        "  def find_up_votes(vote_scope: nil)\n    votes.where(vote_flag: true, vote_scope: vote_scope)\n  end\n\
         \x20 def find_down_votes(vote_scope: nil)\n    votes.where(vote_flag: false, vote_scope: vote_scope)\n  end\n\
         end\n",
    );
    s.replace("votes.", &format!("ActsAsVotable::Vote.where(voter_id: id, voter_type: \"{owner}\")."))
}

/// The name a polymorphic `*_type` column stores for `models[i]`
/// (`base_class.name`): the root of its STI chain among the app's models.
fn base_name(models: &[Model], i: usize) -> String {
    let mut name = models[i].name.0.as_str().to_string();
    let mut parent = models[i].parent.as_ref().map(|p| p.0.as_str().to_string());
    while let Some(p) = parent {
        let Some(m) = models.iter().find(|m| m.name.0.as_str() == p) else { break };
        if m.name.0.as_str() == "ApplicationRecord" {
            break;
        }
        name = p.clone();
        parent = m.parent.as_ref().map(|q| q.0.as_str().to_string());
    }
    name
}

fn synthesize(src: &str, label: &str, what: &str, app: &crate::App) -> Option<Model> {
    let (parsed, diags) = super::prism::scope(|| {
        super::model::ingest_model(src.as_bytes(), label, &app.schema, &Default::default())
    });
    match parsed {
        Ok(Some(m)) if diags.is_empty() => Some(m),
        Ok(_) => {
            survey::record_synthesis_failure(label, what, &diags);
            None
        }
        Err(err) => {
            survey::record(&err);
            None
        }
    }
}

/// Splice `synth`'s associations and methods into `model`; a name the
/// model declares itself wins (Ruby: its own definition sits in front of
/// the mixin's). The macro calls are replaced by the additions.
fn splice(model: &mut Model, synth: Model, at: usize, macro_idx: &[usize]) {
    let own_methods: Vec<String> = model
        .body
        .iter()
        .filter_map(|it| match it {
            ModelBodyItem::Method { method, .. } => Some(format!("{:?}:{}", method.receiver, method.name.as_str())),
            _ => None,
        })
        .collect();
    let own_assocs: Vec<String> = model
        .body
        .iter()
        .filter_map(|it| match it {
            ModelBodyItem::Association { assoc, .. } => Some(assoc.name().as_str().to_string()),
            _ => None,
        })
        .collect();
    let additions: Vec<ModelBodyItem> = synth
        .body
        .into_iter()
        .filter(|it| match it {
            ModelBodyItem::Method { method, .. } => {
                !own_methods.contains(&format!("{:?}:{}", method.receiver, method.name.as_str()))
            }
            ModelBodyItem::Association { assoc, .. } => !own_assocs.contains(&assoc.name().as_str().to_string()),
            ModelBodyItem::Callback { .. } => true,
            _ => false,
        })
        .collect();
    let mut remove: Vec<usize> = macro_idx.to_vec();
    remove.sort_unstable();
    remove.dedup();
    let insert_at = at - remove.iter().filter(|&&i| i < at).count();
    for &i in remove.iter().rev() {
        model.body.remove(i);
    }
    let pos = insert_at.min(model.body.len());
    model.body.splice(pos..pos, additions);
}

pub(crate) fn expand_acts_as_votable(app: &mut crate::App) -> IngestResult<()> {
    if !app.gem_lock.as_ref().is_some_and(|l| l.has("acts_as_votable")) {
        return Ok(());
    }
    let votes_ok = app.schema.tables.get(&crate::ident::Symbol::from("votes")).is_some_and(|t| {
        VOTE_COLUMNS.iter().all(|c| t.columns.iter().any(|col| col.name.as_str() == *c))
    });
    let uses_macro = app
        .models
        .iter()
        .any(|m| !macro_calls(m, "acts_as_votable").is_empty() || !macro_calls(m, "acts_as_voter").is_empty());
    if !votes_ok {
        if uses_macro {
            survey::continue_or_fail(IngestError::Unsupported {
                file: "Gemfile.lock".into(),
                message: "`acts_as_votable` is not supported: the schema has no `votes` table \
                          with the gem's columns (votable_id/type, voter_id/type, vote_flag, \
                          vote_scope, vote_weight)"
                    .into(),
            })?;
        }
        return Ok(());
    }

    // The Vote model itself.
    if !app.models.iter().any(|m| m.name.0.as_str() == "ActsAsVotable::Vote") {
        let parent = if app.models.iter().any(|m| m.name.0.as_str() == "ApplicationRecord") {
            "ApplicationRecord"
        } else {
            "ActiveRecord::Base"
        };
        if let Some(vote) = synthesize(&vote_model_source(parent), "<acts_as_votable>", "ActsAsVotable::Vote", app) {
            app.models.push(vote);
        }
    }

    for mi in 0..app.models.len() {
        let name = app.models[mi].name.0.as_str().to_string();
        let refuse = |why: String| {
            survey::continue_or_fail(IngestError::Unsupported {
                file: name.clone(),
                message: format!("`acts_as_votable` on {name} is not supported: {why}"),
            })
        };
        let votable = macro_calls(&app.models[mi], "acts_as_votable");
        if let Some((first, args)) = votable.first() {
            // One expansion stands for every call, so the calls must agree:
            // identical repeats collapse, differing options are refused
            // rather than silently keeping the first call's.
            let mut strategies = votable.iter().map(|(_, a)| votable_strategy(a));
            let strategy = match strategies.next().unwrap_or_else(|| votable_strategy(args)) {
                Ok(s) => s,
                Err(why) => {
                    refuse(why)?;
                    continue;
                }
            };
            let mut differing = false;
            for other in strategies {
                match other {
                    Ok(o) if o == strategy => {}
                    Ok(_) => differing = true,
                    Err(why) => {
                        refuse(why)?;
                        differing = true;
                    }
                }
            }
            if differing {
                refuse("`acts_as_votable` is called more than once with different options".into())?;
                continue;
            }
            let cols = app
                .schema
                .tables
                .get(&app.models[mi].table.0)
                .map(|t| t.columns.iter().map(|c| c.name.as_str().to_string()).collect::<Vec<_>>())
                .unwrap_or_default();
            let idx: Vec<usize> = votable.iter().map(|(i, _)| *i).collect();
            let src = votable_source(strategy, &Columns::new(cols), &base_name(&app.models, mi));
            if let Some(synth) = synthesize(&src, "<acts_as_votable>", &format!("acts_as_votable for `{name}`"), app) {
                splice(&mut app.models[mi], synth, *first, &idx);
            }
        }
        let voter = macro_calls(&app.models[mi], "acts_as_voter");
        if let Some((first, args)) = voter.first() {
            if voter.iter().any(|(_, a)| !a.is_empty()) || !args.is_empty() {
                refuse("`acts_as_voter` takes no arguments here".into())?;
                continue;
            }
            let idx: Vec<usize> = voter.iter().map(|(i, _)| *i).collect();
            if let Some(synth) = synthesize(&voter_source(&base_name(&app.models, mi)), "<acts_as_votable>", &format!("acts_as_voter for `{name}`"), app) {
                splice(&mut app.models[mi], synth, *first, &idx);
            }
        }
    }
    Ok(())
}
