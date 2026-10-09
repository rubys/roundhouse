//! Opt-in canaries for the whole-program fixpoint in
//! [`super::Analyzer::analyze`]. Nothing here runs unless its variable is
//! set to `1`.
//!
//! [`super::FixpointRounds`] reports how each loop's signature check ended.
//! That check compares returns, parameter rows and block-value verdicts, but
//! a round reads more than those from the rounds before it. The canaries
//! cover all of it:
//!
//! - `RH_FIXPOINT_VERIFY=1`: when a loop ends, run one more full round of
//!   that loop, with no dirty frontier, and count what it moved in each part
//!   of the carried state below. A loop that reached a fixpoint moves
//!   nothing. The extra round's result is kept, so with this variable set a
//!   loop that ran to its cap goes one round further than it would without.
//! - `RH_FIXPOINT_DIGEST=1`: a 64-bit digest of each part of the final
//!   carried state, to compare two runs on the same input.
//! - `RH_FIXPOINT_STATS=1`: the loop ends; the expressions whose type holds
//!   `untyped` at any depth; how often #584's bound cut a carried type; and
//!   how often the harvest cut a return that nests its previous round's
//!   (#528's `harvest_untie_cut`).
//!
//! Each prints one `rh-fixpoint:` JSON line on stderr at the end of
//! `analyze`. It holds counts and hashes only: no names, spans or types.
//!
//! # The carried state
//!
//! Everything a round reads that an earlier round wrote. Types hash
//! structurally, with union arms and record fields in a canonical order
//! and inference variables' ids ignored, so the digests of two runs
//! compare what they inferred rather than the order a hash map produced
//! it in.
//!
//! | Part | What it holds |
//! |---|---|
//! | `returns` | every class's `instance_methods` and `class_methods` |
//! | `constants` | every class's `constants`, and `typed_constants` |
//! | `attributes` | every class's `attributes` row |
//! | `block_values` | every class's `block_value_methods`, `relation_derived` and `materializing_scopes` |
//! | `params` | `inferred_params` |
//! | `controller_bindings` | `refined_action_bindings`: the controller ivar bindings Phase B refines and the next round seeds subclasses from |
//! | `controller_cache` | `controller_action_meta_cache`: the concern and action bindings, and the typed concern bodies, that a controller outside the dirty frontier reuses |
//! | `view_seeds` | `view_seeds`: the controller→view channel that a views pass reads |
//! | `copies` | `concern_folded` and `host_folded`: which registry entries are copies a fold may overwrite |
//! | `ir` | every emit-bound expression's type, decisions and diagnostic annotation ([`crate::lower::for_each_emit_body_ref`]). Parameter defaults and empty literals are typed in place and read back by the next typing, so their stamps are here too |
//!
//! Not compared, and why:
//!
//! - `callers_by_target` is rebuilt from the IR by every parameter
//!   unification, and only narrows the dirty frontier, which a verify round
//!   does not use;
//! - `declared_signatures`, `inquirers`, `data_factories`,
//!   `const_resolver`, `adapter`, and each class's `assoc_extensions`,
//!   method kinds, parent and includes are set when the analyzer is built
//!   and never written by a round;
//! - the App's `view_ivar_types`, `partial_local_types`, `view_feeders`,
//!   `render_edges` and `controller_resolutions` are written by each views
//!   pass for consumers after analysis; no round reads them.

use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::hash::{Hash, Hasher};
use std::sync::LazyLock;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::App;
use crate::diagnostic::DiagnosticKind;
use crate::dialect::{Filter, MethodDef};
use crate::expr::Expr;
use crate::ident::{ClassId, Symbol};
use crate::ty::Ty;

use super::typing_mode::{TypingMode, UnifyScope};
use super::{Analyzer, FixpointRounds, LoopEnd};

fn flag(name: &str) -> bool {
    std::env::var(name).is_ok_and(|v| v == "1")
}

static VERIFY: LazyLock<bool> = LazyLock::new(|| flag("RH_FIXPOINT_VERIFY"));
static DIGEST: LazyLock<bool> = LazyLock::new(|| flag("RH_FIXPOINT_DIGEST"));
static STATS: LazyLock<bool> = LazyLock::new(|| flag("RH_FIXPOINT_STATS"));

/// Whether `RH_FIXPOINT_VERIFY` is set.
pub(super) fn verify_on() -> bool {
    *VERIFY
}

static BOUND_CALLS: AtomicU64 = AtomicU64::new(0);
static BOUND_CUTS: AtomicU64 = AtomicU64::new(0);

/// Counts one call of #584's bound, and whether it cut the type.
pub(super) fn note_bound(cut: bool) {
    if *STATS {
        BOUND_CALLS.fetch_add(1, Ordering::Relaxed);
        if cut {
            BOUND_CUTS.fetch_add(1, Ordering::Relaxed);
        }
    }
}

static UNTIE_CUTS: AtomicU64 = AtomicU64::new(0);

/// Counts one cut of a recursive return by the harvest (#528).
pub(super) fn note_untie_cut() {
    if *STATS {
        UNTIE_CUTS.fetch_add(1, Ordering::Relaxed);
    }
}

/// Which loop of the fixpoint a verify round repeats.
#[derive(Clone, Copy, Debug)]
pub(super) enum Loop {
    Production,
    ViewsAndTests,
    Absorb,
}

impl Loop {
    fn name(self) -> &'static str {
        match self {
            Loop::Production => "production",
            Loop::ViewsAndTests => "views_and_tests",
            Loop::Absorb => "absorb",
        }
    }
}

/// The inputs `analyze` computes once and hands every typing pass.
pub(super) struct RoundInputs<'a> {
    pub dynamic_render_ivars: &'a std::collections::HashSet<Symbol>,
    pub existing_view_names: &'a std::collections::HashSet<Symbol>,
    pub module_methods: &'a HashMap<ClassId, Vec<MethodDef>>,
    pub module_includes: &'a HashMap<ClassId, Vec<ClassId>>,
    pub parent_link_by_name: &'a HashMap<ClassId, Option<ClassId>>,
}

/// What the canaries saw during one `analyze`, printed at its end.
#[derive(Default)]
pub(super) struct Checks {
    /// Per verify round: the loop it repeated and what it moved.
    verified: Vec<(&'static str, BTreeMap<&'static str, u64>)>,
    bound_calls: u64,
    bound_cuts: u64,
    untie_cuts: u64,
}

impl Checks {
    /// Snapshot process-wide counts when `analyze` starts; never reset
    /// the atomics, which another analyzer may still be using.
    pub(super) fn start() -> Self {
        let mut checks = Self::default();
        if *STATS {
            checks.bound_calls = BOUND_CALLS.load(Ordering::Relaxed);
            checks.bound_cuts = BOUND_CUTS.load(Ordering::Relaxed);
            checks.untie_cuts = UNTIE_CUTS.load(Ordering::Relaxed);
        }
        checks
    }
}

/// Structural hash of a type: union arms sorted and deduplicated, record
/// fields sorted by name, inference variables' ids ignored.
pub(super) fn type_hash(t: &Ty) -> u64 {
    let mut h = DefaultHasher::new();
    std::mem::discriminant(t).hash(&mut h);
    match t {
        Ty::Array { elem } => type_hash(elem).hash(&mut h),
        Ty::Hash { key, value } => {
            type_hash(key).hash(&mut h);
            type_hash(value).hash(&mut h);
        }
        Ty::Union { variants } => {
            let mut arms: Vec<u64> = variants.iter().map(type_hash).collect();
            arms.sort_unstable();
            arms.dedup();
            arms.hash(&mut h);
        }
        Ty::Tuple { elems } => {
            for e in elems.iter() {
                type_hash(e).hash(&mut h);
            }
        }
        Ty::Record { row } => {
            let mut fields: Vec<(&str, u64)> =
                row.fields.iter().map(|(k, v)| (k.as_str(), type_hash(v))).collect();
            fields.sort_unstable();
            fields.hash(&mut h);
            row.rest.is_some().hash(&mut h);
        }
        Ty::Class { id, args } => {
            id.0.as_str().hash(&mut h);
            for a in args.iter() {
                type_hash(a).hash(&mut h);
            }
        }
        Ty::Relation { of } => of.0.as_str().hash(&mut h),
        Ty::Fn { params, block, ret, effects } => {
            for p in params.iter() {
                p.name.as_str().hash(&mut h);
                format!("{:?}", p.kind).hash(&mut h);
                type_hash(&p.ty).hash(&mut h);
            }
            block.as_deref().map(type_hash).hash(&mut h);
            type_hash(ret).hash(&mut h);
            format!("{effects:?}").hash(&mut h);
        }
        // Scalars are their discriminant; a `Var`'s id is an allocation
        // identity, not information.
        _ => {}
    }
    h.finish()
}

fn hash_of(value: impl Hash) -> u64 {
    let mut h = DefaultHasher::new();
    value.hash(&mut h);
    h.finish()
}

/// `name → type` maps hash in name order.
fn bindings_hash(map: &HashMap<Symbol, Ty>) -> u64 {
    let entries: BTreeMap<&str, u64> = map.iter().map(|(k, t)| (k.as_str(), type_hash(t))).collect();
    hash_of(entries)
}

fn names_hash<'a>(names: impl IntoIterator<Item = &'a Symbol>) -> u64 {
    let sorted: BTreeSet<&str> = names.into_iter().map(|s| s.as_str()).collect();
    hash_of(sorted)
}

fn annotation_hash(d: &DiagnosticKind) -> u64 {
    match d {
        DiagnosticKind::SendDispatchFailed { method, recv_ty } => {
            hash_of(("send_dispatch_failed", method.as_str(), type_hash(recv_ty)))
        }
        DiagnosticKind::IncompatibleBinop { op, lhs_ty, rhs_ty } => {
            hash_of(("incompatible_binop", op.as_str(), type_hash(lhs_ty), type_hash(rhs_ty)))
        }
        DiagnosticKind::BlankUnlowered { method, recv_ty, reason } => {
            hash_of(("blank_unlowered", method.as_str(), type_hash(recv_ty), reason.as_str()))
        }
        DiagnosticKind::GraphqlNullableField { field, value_ty } => {
            hash_of(("graphql_nullable_field", field.as_str(), type_hash(value_ty)))
        }
        other => hash_of(serde_json::to_string(other).unwrap_or_default()),
    }
}

/// One expression: (file, start, end, type hash, annotation hash).
type IrRow = (u32, u32, u32, u64, u64);

fn ir_rows(e: &Expr, out: &mut Vec<IrRow>) {
    let ty = e.ty.as_ref().map_or(0, type_hash);
    let note = hash_of((e.decisions, e.diagnostic.as_ref().map(annotation_hash)));
    out.push((e.span.file.0, e.span.start, e.span.end, ty, note));
    e.node.for_each_child(&mut |c| ir_rows(c, out));
}

/// A filter's declaration fields, with typed expressions hashed like
/// the rest of the IR rather than through `Debug`'s variable ids.
fn filter_hash(f: &Filter) -> u64 {
    let mut h = DefaultHasher::new();
    std::mem::discriminant(&f.kind).hash(&mut h);
    f.target.hash(&mut h);
    f.target_span.hash(&mut h);
    f.from_concern.hash(&mut h);
    f.only.hash(&mut h);
    f.except.hash(&mut h);
    std::mem::discriminant(&f.only_style).hash(&mut h);
    std::mem::discriminant(&f.except_style).hash(&mut h);
    f.if_cond.hash(&mut h);
    f.unless_cond.hash(&mut h);
    f.prepend.hash(&mut h);
    for expr in [&f.if_cond_expr, &f.unless_cond_expr, &f.block] {
        expr.is_some().hash(&mut h);
        if let Some(expr) = expr {
            let mut rows = Vec::new();
            ir_rows(expr, &mut rows);
            rows.hash(&mut h);
        }
    }
    h.finish()
}

/// The carried state, as one hash per entry.
pub(super) struct StateFp {
    parts: BTreeMap<&'static str, BTreeMap<String, u64>>,
    /// Sorted, so two fingerprints compare as multisets.
    ir: Vec<IrRow>,
}

impl StateFp {
    /// Per part: entries added, removed or changed since `before`.
    fn moved(&self, before: &StateFp) -> BTreeMap<&'static str, u64> {
        let mut out = BTreeMap::new();
        let empty = BTreeMap::new();
        for name in self.parts.keys().chain(before.parts.keys()) {
            let now = self.parts.get(name).unwrap_or(&empty);
            let then = before.parts.get(name).unwrap_or(&empty);
            let changed = now.iter().filter(|(k, v)| then.get(*k) != Some(*v)).count()
                + then.keys().filter(|k| !now.contains_key(*k)).count();
            out.insert(*name, changed as u64);
        }
        // Expressions whose (site, type, annotation) is not in the other
        // fingerprint, in whichever direction has more.
        let (mut i, mut j, mut only_now, mut only_then) = (0, 0, 0u64, 0u64);
        let (a, b) = (&self.ir, &before.ir);
        while i < a.len() && j < b.len() {
            match a[i].cmp(&b[j]) {
                std::cmp::Ordering::Equal => {
                    i += 1;
                    j += 1;
                }
                std::cmp::Ordering::Less => {
                    only_now += 1;
                    i += 1;
                }
                std::cmp::Ordering::Greater => {
                    only_then += 1;
                    j += 1;
                }
            }
        }
        only_now += (a.len() - i) as u64;
        only_then += (b.len() - j) as u64;
        out.insert("ir", only_now.max(only_then));
        out
    }

    fn digests(&self) -> BTreeMap<&'static str, String> {
        let mut out: BTreeMap<&'static str, String> =
            self.parts.iter().map(|(name, part)| (*name, format!("{:016x}", hash_of(part)))).collect();
        out.insert("ir", format!("{:016x}", hash_of(&self.ir)));
        out
    }

    fn sizes(&self) -> BTreeMap<&'static str, u64> {
        let mut out: BTreeMap<&'static str, u64> =
            self.parts.iter().map(|(name, part)| (*name, part.len() as u64)).collect();
        out.insert("ir", self.ir.len() as u64);
        out
    }
}

/// Typed expressions, and those whose type holds `untyped`.
#[derive(Default)]
struct Census {
    typed: u64,
    missing: u64,
    bare_untyped: u64,
    untyped_anywhere: u64,
}

fn holds_untyped(t: &Ty) -> bool {
    match t {
        Ty::Untyped => true,
        Ty::Array { elem } => holds_untyped(elem),
        Ty::Hash { key, value } => holds_untyped(key) || holds_untyped(value),
        Ty::Tuple { elems } => elems.iter().any(holds_untyped),
        Ty::Union { variants } => variants.iter().any(holds_untyped),
        Ty::Record { row } => row.fields.values().any(holds_untyped),
        Ty::Class { args, .. } => args.iter().any(holds_untyped),
        Ty::Fn { params, block, ret, .. } => {
            params.iter().any(|p| holds_untyped(&p.ty))
                || block.as_deref().is_some_and(holds_untyped)
                || holds_untyped(ret)
        }
        _ => false,
    }
}

impl Census {
    fn expr(&mut self, e: &Expr) {
        match &e.ty {
            Some(t) => {
                self.typed += 1;
                self.bare_untyped += u64::from(matches!(t, Ty::Untyped));
                self.untyped_anywhere += u64::from(holds_untyped(t));
            }
            None => self.missing += 1,
        }
        e.node.for_each_child(&mut |c| self.expr(c));
    }
}

fn loop_end(end: LoopEnd) -> serde_json::Value {
    match end {
        LoopEnd::Settled(round) => serde_json::json!({"end": "settled", "round": round}),
        LoopEnd::RanToCap => serde_json::json!({"end": "ran_to_cap"}),
        LoopEnd::NotRun => serde_json::json!({"end": "not_run"}),
    }
}

impl Analyzer {
    /// The carried state (see the module docs), one hash per entry.
    pub(super) fn state_fp(&self, app: &App) -> StateFp {
        let mut parts: BTreeMap<&'static str, BTreeMap<String, u64>> = BTreeMap::new();
        let mut put = |part: &'static str, key: String, hash: u64| {
            parts.entry(part).or_default().insert(key, hash);
        };
        for (id, ci) in &self.classes {
            let c = id.0.as_str();
            for (m, t) in &ci.instance_methods {
                put("returns", format!("{c}#{}", m.as_str()), type_hash(t));
            }
            for (m, t) in &ci.class_methods {
                put("returns", format!("{c}.{}", m.as_str()), type_hash(t));
            }
            for (k, t) in &ci.constants {
                put("constants", format!("{c}::{}", k.as_str()), type_hash(t));
            }
            for (f, t) in &ci.attributes.fields {
                put("attributes", format!("{c}@{}", f.as_str()), type_hash(t));
            }
            put(
                "block_values",
                c.to_string(),
                hash_of((
                    names_hash(&ci.block_value_methods),
                    names_hash(&ci.relation_derived),
                    names_hash(&ci.materializing_scopes),
                )),
            );
        }
        for (decl, t) in &self.typed_constants {
            put("constants", format!("decl {decl:?}"), type_hash(t));
        }
        for ((id, m, side), row) in &self.inferred_params {
            let row: Vec<u64> = row.iter().map(type_hash).collect();
            let sep = if *side == crate::dialect::MethodReceiver::Class { "." } else { "#" };
            put("params", format!("{}{sep}{}", id.0.as_str(), m.as_str()), hash_of(row));
        }
        for ((id, m), bindings) in &self.refined_action_bindings {
            put(
                "controller_bindings",
                format!("{}#{}", id.0.as_str(), m.as_str()),
                bindings_hash(bindings),
            );
        }
        for (id, (bindings, bodies)) in &self.controller_action_meta_cache {
            let bindings: BTreeMap<&str, u64> =
                bindings.iter().map(|(m, b)| (m.as_str(), bindings_hash(b))).collect();
            let bodies: BTreeMap<&str, u64> = bodies
                .iter()
                .map(|(m, body)| {
                    let mut rows = Vec::new();
                    ir_rows(body, &mut rows);
                    (m.as_str(), hash_of(rows))
                })
                .collect();
            put("controller_cache", id.0.as_str().to_string(), hash_of((bindings, bodies)));
        }
        if let Some(seeds) = &self.view_seeds {
            let channels = [
                ("action", &seeds.action_ivars_by_view),
                ("layout", &seeds.layout_ivars_by_view),
                ("content_partial", &seeds.content_partial_ivars),
            ];
            for (channel, by_view) in channels {
                for (view, ivars) in by_view {
                    put("view_seeds", format!("{channel} {}", view.as_str()), bindings_hash(ivars));
                }
            }
            for (view, t) in &seeds.mailer_params_by_view {
                put("view_seeds", format!("mailer {}", view.as_str()), type_hash(t));
            }
            for (view, feeders) in &seeds.view_feeders {
                let feeders: BTreeSet<&str> = feeders.iter().map(|c| c.0.as_str()).collect();
                put("view_seeds", format!("feeders {}", view.as_str()), hash_of(feeders));
            }
            for (id, resolution) in &seeds.controller_resolutions {
                let filters: Vec<(u64, &str, &str, u64)> = resolution
                    .filter_chain
                    .iter()
                    .map(|f| {
                        (
                            filter_hash(&f.filter),
                            f.defined_in.0.as_str(),
                            f.included_via.0.as_str(),
                            bindings_hash(&f.assigns),
                        )
                    })
                    .collect();
                let layout = resolution.layout.as_ref().map(|l| l.as_str());
                put("view_seeds", format!("resolution {}", id.0.as_str()), hash_of((filters, layout)));
            }
        }
        for (id, (instance, class)) in &self.concern_folded {
            put("copies", format!("concern {}", id.0.as_str()), hash_of((names_hash(instance), names_hash(class))));
        }
        for (id, names) in &self.host_folded {
            put("copies", format!("host {}", id.0.as_str()), names_hash(names));
        }
        let mut ir = Vec::new();
        crate::lower::for_each_emit_body_ref(app, &mut |e| ir_rows(e, &mut ir));
        ir.sort_unstable();
        StateFp { parts, ir }
    }

    /// `RH_FIXPOINT_VERIFY`: one more full round of the loop that just
    /// ended, with no dirty frontier, recording what it moved. `snapshot`
    /// is the production+view parameter snapshot the test and absorb
    /// rounds replay.
    pub(super) fn verify_round(
        &mut self,
        app: &mut App,
        inputs: &RoundInputs<'_>,
        which: Loop,
        snapshot: Option<&mut HashMap<super::ParamKey, Vec<Ty>>>,
    ) {
        let before = self.state_fp(app);
        match which {
            Loop::Production | Loop::Absorb => {
                self.run_typing_passes(
                    app,
                    inputs.dynamic_render_ivars,
                    inputs.existing_view_names,
                    inputs.module_methods,
                    inputs.module_includes,
                    inputs.parent_link_by_name,
                    TypingMode::Production { dirty: None },
                );
            }
            Loop::ViewsAndTests => self.type_tests_only(app),
        }
        match which {
            Loop::Production => {
                self.harvest_returns_to_registry(app, false);
                self.unify_params_from_call_sites(app, UnifyScope::Production);
            }
            Loop::ViewsAndTests => {
                self.harvest_returns_to_registry(app, true);
                if let Some(snapshot) = snapshot {
                    self.unify_test_params_onto(app, snapshot);
                }
            }
            Loop::Absorb => {
                self.harvest_returns_to_registry(app, true);
                self.unify_params_from_call_sites(app, UnifyScope::WithViews);
                if let Some(snapshot) = snapshot {
                    snapshot.clone_from(&self.inferred_params);
                }
                self.overlay_test_params(app);
            }
        }
        let moved = self.state_fp(app).moved(&before);
        self.fixpoint_checks.verified.push((which.name(), moved));
    }

    /// The `rh-fixpoint:` line, when any canary is on.
    pub(super) fn report_fixpoint_checks(&self, app: &App) {
        if !(*VERIFY || *DIGEST || *STATS) {
            return;
        }
        let rounds: FixpointRounds = self.fixpoint_rounds;
        let mut line = serde_json::json!({
            "schema": 1,
            "loops": {
                "production": loop_end(rounds.production),
                "views_and_tests": loop_end(rounds.views_and_tests),
                "absorb": loop_end(rounds.absorb),
            },
        });
        if *VERIFY {
            let verified: Vec<serde_json::Value> = self
                .fixpoint_checks
                .verified
                .iter()
                .map(|(which, moved)| serde_json::json!({"loop": which, "moved": moved}))
                .collect();
            line["verify"] = serde_json::json!(verified);
        }
        if *DIGEST {
            let fp = self.state_fp(app);
            line["digest"] = serde_json::json!(fp.digests());
            line["entries"] = serde_json::json!(fp.sizes());
        }
        if *STATS {
            let mut census = Census::default();
            crate::lower::for_each_emit_body_ref(app, &mut |e| census.expr(e));
            line["census"] = serde_json::json!({
                "typed": census.typed,
                "missing": census.missing,
                "bare_untyped": census.bare_untyped,
                "untyped_anywhere": census.untyped_anywhere,
            });
            line["bound"] = serde_json::json!({
                "calls": BOUND_CALLS.load(Ordering::Relaxed) - self.fixpoint_checks.bound_calls,
                "cuts": BOUND_CUTS.load(Ordering::Relaxed) - self.fixpoint_checks.bound_cuts,
            });
            line["harvest_untie_cut"] =
                serde_json::json!(UNTIE_CUTS.load(Ordering::Relaxed) - self.fixpoint_checks.untie_cuts);
        }
        eprintln!("rh-fixpoint: {line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ident::TyVar;

    fn union(variants: Vec<Ty>) -> Ty {
        Ty::Union { variants }
    }

    #[test]
    fn type_hash_ignores_arm_order_and_variable_ids() {
        let a = union(vec![Ty::Int, Ty::Str, Ty::Var { var: TyVar(1) }]);
        let b = union(vec![Ty::Var { var: TyVar(7) }, Ty::Str, Ty::Int]);
        assert_eq!(type_hash(&a), type_hash(&b));
        assert_ne!(type_hash(&a), type_hash(&union(vec![Ty::Int, Ty::Str])));
    }

    #[test]
    fn stats_are_per_analyze() {
        const CHILD: &str = "ROUNDHOUSE_FIXPOINT_STATS_TEST_CHILD";
        if std::env::var_os(CHILD).is_some() {
            let nested = format!("{}1{}", "[".repeat(18), "]".repeat(18));
            let source = format!(r#"
class SamplesController < ActionController::Base
  def index
    @sample = wrap(1)
    @deep = echo({nested})
  end
  private
  def wrap(value)
    [value, wrap(value)]
  end
  def echo(value)
    value
  end
end
"#);
            let tree = HashMap::from([(
                std::path::PathBuf::from("app/controllers/samples_controller.rb"),
                source.into_bytes(),
            )]);
            for _ in 0..2 {
                let mut app = crate::ingest::ingest_app_from_tree(tree.clone()).expect("ingest");
                Analyzer::new(&app).analyze(&mut app);
            }
            return;
        }
        // Set the lazy flag in a fresh process, without changing the
        // environment of other tests. Both analyzes run in that process.
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "analyze::fixpoint_check::tests::stats_are_per_analyze", "--nocapture"])
            .env(CHILD, "1")
            .env("RH_FIXPOINT_STATS", "1")
            .env_remove("RH_FIXPOINT_VERIFY")
            .env_remove("RH_FIXPOINT_DIGEST")
            .output()
            .expect("run stats test");
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(output.status.success(), "{stderr}");
        let reports: Vec<serde_json::Value> = stderr
            .lines()
            .filter_map(|line| line.strip_prefix("rh-fixpoint: "))
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(reports.len(), 2, "{stderr}");
        assert!(reports[0]["bound"]["calls"].as_u64().unwrap() > 0);
        assert!(reports[0]["bound"]["cuts"].as_u64().unwrap() > 0);
        assert!(reports[0]["harvest_untie_cut"].as_u64().unwrap() > 0);
        assert_eq!(reports[0]["bound"], reports[1]["bound"]);
        assert_eq!(reports[0]["harvest_untie_cut"], reports[1]["harvest_untie_cut"]);
    }

    #[test]
    fn view_seed_filters_ignore_arm_order_and_variable_ids() {
        use crate::app::{ControllerResolution, ResolvedFilter};
        use crate::dialect::{Filter, FilterKind};
        use crate::expr::{ArrayStyle, ExprNode, Literal};
        use crate::span::Span;
        use super::super::typing_mode::ViewSeeds;

        let fingerprint = |ty: Ty| {
            let mut child = Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Nil });
            child.ty = Some(ty);
            let expr = Expr::new(Span::synthetic(), ExprNode::Seq { exprs: vec![child] });
            let filter = Filter {
                kind: FilterKind::Before,
                target: Symbol::from("set_value"),
                target_span: Span::synthetic(),
                from_concern: None,
                only: vec![Symbol::from("index")],
                except: Vec::new(),
                only_style: ArrayStyle::Brackets,
                except_style: ArrayStyle::Brackets,
                if_cond: None,
                unless_cond: None,
                if_cond_expr: Some(expr.clone()),
                unless_cond_expr: Some(expr.clone()),
                block: Some(expr),
                prepend: false,
            };
            let id = ClassId(Symbol::from("SamplesController"));
            let resolution = ControllerResolution {
                filter_chain: vec![ResolvedFilter {
                    filter, defined_in: id.clone(), included_via: id.clone(),
                    assigns: HashMap::new(), effects: Default::default(),
                }],
                layout: Some(Symbol::from("layouts/application")),
            };
            let app = App::default();
            let mut analyzer = Analyzer::new(&app);
            analyzer.view_seeds = Some(ViewSeeds {
                controller_resolutions: HashMap::from([(id, resolution)]),
                ..ViewSeeds::default()
            });
            analyzer.state_fp(&app).digests()["view_seeds"].clone()
        };
        assert_eq!(fingerprint(Ty::Var { var: TyVar(1) }), fingerprint(Ty::Var { var: TyVar(7) }));
        assert_eq!(fingerprint(union(vec![Ty::Int, Ty::Str])), fingerprint(union(vec![Ty::Str, Ty::Int])));
        assert_ne!(fingerprint(Ty::Int), fingerprint(Ty::Str));
    }

    #[test]
    fn type_hash_tells_containers_apart() {
        let arr = Ty::Array { elem: Box::new(Ty::Int) };
        let hash = Ty::Hash { key: Box::new(Ty::Int), value: Box::new(Ty::Int) };
        assert_ne!(type_hash(&arr), type_hash(&hash));
        assert_ne!(type_hash(&arr), type_hash(&Ty::Array { elem: Box::new(Ty::Str) }));
    }

    #[test]
    fn untyped_is_found_at_any_depth() {
        let deep = Ty::Hash {
            key: Box::new(Ty::Str),
            value: Box::new(Ty::Array { elem: Box::new(union(vec![Ty::Int, Ty::Untyped])) }),
        };
        assert!(holds_untyped(&deep));
        assert!(!holds_untyped(&Ty::Array { elem: Box::new(Ty::Int) }));
    }

    #[test]
    fn moved_counts_changed_added_and_removed_entries() {
        let fp = |entries: &[(&str, u64)], ir: Vec<IrRow>| StateFp {
            parts: BTreeMap::from([(
                "returns",
                entries.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
            )]),
            ir,
        };
        let before = fp(&[("A#x", 1), ("A#y", 2)], vec![(1, 0, 4, 10, 0), (1, 5, 9, 11, 0)]);
        let after = fp(&[("A#x", 1), ("A#y", 3), ("B#z", 4)], vec![(1, 0, 4, 10, 0), (1, 5, 9, 12, 0)]);
        let moved = after.moved(&before);
        assert_eq!(moved["returns"], 2);
        assert_eq!(moved["ir"], 1);
        assert_eq!(before.moved(&before).values().sum::<u64>(), 0);
    }
}
