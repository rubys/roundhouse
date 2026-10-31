//! Inference probe: how far does roundhouse's whole-program type
//! inference reach on framework-runtime code with NO RBS?
//!
//! The spinel-blog fixture is a metaprogramming-free Ruby blog whose
//! `runtime/ruby/active_record/*.rb` files are hand-shaped specimens of
//! framework code that fits Spinel's documented subset (no eval, no
//! send, no method_missing). They ship without RBS sidecars on
//! purpose: spinel uses pure inference. Running the same files
//! through roundhouse's analyzer with no RBS measures how close our
//! inference gets to spinel's coverage on the same input.
//!
//! This is the diagnostic that justifies the three-gap inference
//! work (return propagation, parameter unification, fixpoint). The
//! count reported here is the residual — what inference still can't
//! reach. If the residual is small enough, the framework-typing
//! prerequisite for Rust no longer requires authoring RBS for
//! runtime files; targeted RBS for public library signatures
//! suffices.
//!
//! Distinct from `runtime_src_integration::every_runtime_method_body_is_fully_typed`,
//! which gates on RBS-paired typing (different path, different
//! contract) and is `#[ignore]`'d while the framework-typing residual
//! closes. This test runs by default and tracks the inference-only
//! number — same fixture surface, different machinery.

use std::fs;
use std::path::Path;

use roundhouse::analyze::Analyzer;
use roundhouse::expr::{Expr, ExprNode, InterpPart};
use roundhouse::ingest::ingest_library_classes;
use roundhouse::ty::Ty;
use roundhouse::App;

const RUNTIME_DIR: &str = "runtime/ruby/active_record";

fn collect_untyped(e: &Expr, path: &str, out: &mut Vec<String>) {
    let ty_ok = matches!(&e.ty, Some(t) if !matches!(t, Ty::Var { .. }));
    if !ty_ok {
        out.push(format!("{path}: {:?} has ty={:?}", &e.node, e.ty));
    }
    match &*e.node {
        ExprNode::Lit { .. }
        | ExprNode::Var { .. }
        | ExprNode::Ivar { .. }
        | ExprNode::Const { .. }
        | ExprNode::Retry
        | ExprNode::Redo
        | ExprNode::ForwardArgs
        | ExprNode::ForwardKeywords
        | ExprNode::Defined { .. }
        | ExprNode::SelfRef => {}
        ExprNode::ForwardKeywordsWithPairs { entries } => {
            for (key, value) in entries {
                collect_untyped(key, path, out);
                collect_untyped(value, path, out);
            }
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            collect_untyped(cond, &format!("{path}/if.cond"), out);
            collect_untyped(then_branch, &format!("{path}/if.then"), out);
            collect_untyped(else_branch, &format!("{path}/if.else"), out);
        }
        ExprNode::Send { recv, args, block, .. } => {
            if let Some(r) = recv {
                collect_untyped(r, &format!("{path}/send.recv"), out);
            }
            for (i, a) in args.iter().enumerate() {
                collect_untyped(a, &format!("{path}/send.arg[{i}]"), out);
            }
            if let Some(b) = block {
                collect_untyped(b, &format!("{path}/send.block"), out);
            }
        }
        ExprNode::StringInterp { parts } => {
            for (i, p) in parts.iter().enumerate() {
                if let InterpPart::Expr { expr } = p {
                    collect_untyped(expr, &format!("{path}/interp[{i}]"), out);
                }
            }
        }
        ExprNode::Seq { exprs } => {
            for (i, e) in exprs.iter().enumerate() {
                collect_untyped(e, &format!("{path}/seq[{i}]"), out);
            }
        }
        ExprNode::BoolOp { left, right, .. } => {
            collect_untyped(left, &format!("{path}/boolop.left"), out);
            collect_untyped(right, &format!("{path}/boolop.right"), out);
        }
        ExprNode::RescueModifier { expr, fallback } => {
            collect_untyped(expr, &format!("{path}/rescue.expr"), out);
            collect_untyped(fallback, &format!("{path}/rescue.fallback"), out);
        }
        ExprNode::Let { value, body, .. } => {
            collect_untyped(value, &format!("{path}/let.value"), out);
            collect_untyped(body, &format!("{path}/let.body"), out);
        }
        ExprNode::Lambda { body, .. } => {
            collect_untyped(body, &format!("{path}/lambda.body"), out)
        }
        ExprNode::MethodRef { recv, .. } => {
            if let Some(r) = recv {
                collect_untyped(r, &format!("{path}/method_ref.recv"), out);
            }
        }
        ExprNode::Apply { fun, args, block } => {
            collect_untyped(fun, &format!("{path}/apply.fun"), out);
            for (i, a) in args.iter().enumerate() {
                collect_untyped(a, &format!("{path}/apply.arg[{i}]"), out);
            }
            if let Some(b) = block {
                collect_untyped(b, &format!("{path}/apply.block"), out);
            }
        }
        ExprNode::Hash { entries, .. } => {
            for (i, (k, v)) in entries.iter().enumerate() {
                collect_untyped(k, &format!("{path}/hash[{i}].key"), out);
                collect_untyped(v, &format!("{path}/hash[{i}].value"), out);
            }
        }
        ExprNode::Array { elements, .. } => {
            for (i, el) in elements.iter().enumerate() {
                collect_untyped(el, &format!("{path}/array[{i}]"), out);
            }
        }
        ExprNode::Case { scrutinee, arms } => {
            collect_untyped(scrutinee, &format!("{path}/case.scrut"), out);
            for (i, arm) in arms.iter().enumerate() {
                if let Some(g) = &arm.guard {
                    collect_untyped(g, &format!("{path}/case.arm[{i}].guard"), out);
                }
                collect_untyped(&arm.body, &format!("{path}/case.arm[{i}].body"), out);
            }
        }
        ExprNode::CaseMatch { scrutinee, arms, else_body } => {
            collect_untyped(scrutinee, &format!("{path}/case_match.scrut"), out);
            for (i, arm) in arms.iter().enumerate() {
                arm.pattern.for_each_expr(&mut |e| {
                    collect_untyped(e, &format!("{path}/case_match.arm[{i}].pattern"), out);
                });
                if let Some((_, g)) = &arm.guard {
                    collect_untyped(g, &format!("{path}/case_match.arm[{i}].guard"), out);
                }
                collect_untyped(&arm.body, &format!("{path}/case_match.arm[{i}].body"), out);
            }
            if let Some(e) = else_body {
                collect_untyped(e, &format!("{path}/case_match.else"), out);
            }
        }
        ExprNode::MatchPredicate { value, pattern } | ExprNode::MatchRequired { value, pattern } => {
            collect_untyped(value, &format!("{path}/match.value"), out);
            pattern.for_each_expr(&mut |e| {
                collect_untyped(e, &format!("{path}/match.pattern"), out);
            });
        }
        ExprNode::Assign { value, .. } | ExprNode::OpAssign { value, .. } => {
            collect_untyped(value, &format!("{path}/assign.value"), out)
        }
        ExprNode::Yield { args } => {
            for (i, a) in args.iter().enumerate() {
                collect_untyped(a, &format!("{path}/yield.arg[{i}]"), out);
            }
        }
        ExprNode::Raise { value } => {
            collect_untyped(value, &format!("{path}/raise.value"), out)
        }
        ExprNode::Return { value } => {
            collect_untyped(value, &format!("{path}/return.value"), out)
        }
        ExprNode::Super { args } => {
            if let Some(args) = args {
                for (i, a) in args.iter().enumerate() {
                    collect_untyped(a, &format!("{path}/super.arg[{i}]"), out);
                }
            }
        }
        ExprNode::BeginRescue { body, rescues, else_branch, ensure, .. } => {
            collect_untyped(body, &format!("{path}/begin.body"), out);
            for (i, r) in rescues.iter().enumerate() {
                for (j, c) in r.classes.iter().enumerate() {
                    collect_untyped(c, &format!("{path}/begin.rescue[{i}].class[{j}]"), out);
                }
                collect_untyped(&r.body, &format!("{path}/begin.rescue[{i}].body"), out);
            }
            if let Some(e) = else_branch {
                collect_untyped(e, &format!("{path}/begin.else"), out);
            }
            if let Some(e) = ensure {
                collect_untyped(e, &format!("{path}/begin.ensure"), out);
            }
        }
        ExprNode::Next { value } | ExprNode::Break { value } => {
            if let Some(v) = value {
                collect_untyped(v, &format!("{path}/next.value"), out);
            }
        }
        ExprNode::Splat { value } | ExprNode::KeywordSplat { value } => {
            collect_untyped(value, &format!("{path}/splat.value"), out);
        }
        ExprNode::MultiAssign { value, .. } => {
            collect_untyped(value, &format!("{path}/multi_assign.value"), out);
        }
        ExprNode::While { cond, body, .. } => {
            collect_untyped(cond, &format!("{path}/while.cond"), out);
            collect_untyped(body, &format!("{path}/while.body"), out);
        }
        ExprNode::Range { begin, end, .. } => {
            if let Some(b) = begin {
                collect_untyped(b, &format!("{path}/range.begin"), out);
            }
            if let Some(e) = end {
                collect_untyped(e, &format!("{path}/range.end"), out);
            }
        }
        ExprNode::Cast { value, .. } => {
            collect_untyped(value, &format!("{path}/cast.value"), out);
        }
    }
}

fn build_app_from_runtime() -> App {
    let mut app = App::new();
    let dir = Path::new(RUNTIME_DIR);
    let mut entries: Vec<_> = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("read_dir {RUNTIME_DIR}: {e}"))
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("rb"))
        .collect();
    entries.sort();
    for path in entries {
        let source = fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let path_str = path.display().to_string();
        let classes = ingest_library_classes(&source, &path_str)
            .unwrap_or_else(|e| panic!("ingest {path_str}: {e:?}"));
        for lc in classes {
            app.library_classes.push(lc);
        }
    }
    app
}

/// Diagnostic probe — runs the analyzer (with the new fixpoint
/// loop) over the spinel-blog runtime files and reports the residual
/// untyped count. Asserts only that the count doesn't regress past a
/// known ceiling, so the test fails loud if a future change breaks
/// inference; tighten the bound as inference improves. The actual
/// count is printed with `--nocapture` so progress is visible.
#[test]
fn untyped_subexpressions_baseline() {
    let mut app = build_app_from_runtime();
    Analyzer::new(&app).analyze(&mut app);

    let mut all_untyped: Vec<String> = Vec::new();
    let mut like_untyped = 0;
    for lc in &app.library_classes {
        for method in &lc.methods {
            let path = format!("{}#{}", lc.name.0.as_str(), method.name.as_str());
            let before = all_untyped.len();
            collect_untyped(&method.body, &path, &mut all_untyped);
            if lc.name.0.as_str() == "ActiveRecord::Base"
                && method.name.as_str() == "sanitize_sql_like"
            {
                like_untyped += all_untyped.len() - before;
            }
        }
    }

    eprintln!(
        "spinel-blog runtime active_record/: {} untyped sub-expressions \
         across {} library classes",
        all_untyped.len(),
        app.library_classes.len()
    );

    // The constants sit on the measured residual. A later drop lowers
    // them in the same commit; they are not a budget with headroom.
    // 2026-10-05: 500 -> 505, MEASURED after the request-key finder
    // refresh onto 132a26c7. With this same analyzer and collector,
    // pristine 4f5a1239 / 132a26c7 runtimes measure 470 / 476; the
    // finder runtime on those bases measures 497 / 505. The eight new
    // candidate sites are Relation#include? (+5) and #more_than? (+3).
    // Two membership call arguments additionally inherit the cast's
    // unresolved input when no signatures/caller context are supplied.
    // Existing finder/parser sites are unchanged. The companion RBS
    // probe and full-context runtime gate still require every new finder
    // method to be fully typed; no node is excluded from this raw probe.
    // Float request inputs: 505 -> 512, MEASURED with the identical
    // collector over the runtime before/after the numeric-value fix.
    // All seven added sites are IntegerKeyCast#parse's unseeded Float
    // guard/input reads and comparisons; no existing site changes.
    // Its RBS-paired method still has the separate zero-residual assertion.
    // TokenFor (#450) on the same collector: 512 -> 519, MEASURED on
    // pristine upstream/main after #450+#438. The seven new sites are
    // ActiveRecord::TokenFor helpers under the spinel-blog runtime
    // probe; the Float ceiling was never re-measured against TokenFor.
    // exists? key dispatch (#549 / #403): 519 -> 523, MEASURED. Four new
    // Base sites from exists? / _exists_primary_key_input (nil guard +
    // cast + adapter). Companion RBS probe stays at zero residual.
    // Pessimistic locking (#644 / #671): 523 -> 529, MEASURED. The six
    // new sites are `with_lock`'s `*args` split (the trailing options
    // Hash and the lock clause) and the `isolation:` / `requires_new:` /
    // `joinable:` pass-through into `transaction`, all accepted and
    // ignored under SQLite. Its RBS-paired methods keep the gradual
    // `untyped` escape `self.transaction` already has.
    // RecordNotFound's model/primary_key/id: 529 -> 534, MEASURED on
    // the runtime after #671. The five new sites are the finder inputs
    // the raises now pass through: `id` in Base#find (x2), Relation#find
    // and Relation#find_ids, and the ruby-family Base#find_by!'s
    // `conditions`. Companion RBS probe stays at zero residual.
    // Rails 8.1 finder-miss wording: 534 -> 540, MEASURED. The net six
    // new sites are all in Relation#find_ids' multi-id message (the
    // `inspect`ed id list and the interpolated model, key and expected
    // count); the single-id messages trade one site for one.
    // Relation#find(nil)'s "without an ID" raise (#689 review):
    // 540 -> 541, MEASURED. The one new site is the `id.nil?` guard's
    // read of the unseeded `id`.
    // `Base.uncached` (campfire's push pool test): 541 -> 547, MEASURED.
    // The six new sites are its `Db.query_cache_*` calls and the block
    // value, read here without the Db contract — the same escape
    // `self.transaction`'s `Db.exec` calls already take.
    // `Relation#order_sql` / `#reorder_sql` (`order(Arel.sql(…))`,
    // campfire's `reorder(Arel.sql("+messages.created_at"))`): 547 ->
    // 549, MEASURED — each one's `fragment` parameter, read here
    // without its RBS.
    // Transaction join/pin + its `ensure`-based non-local-exit fix
    // (spinel-txn-pin #693, matz/spinel#8182), rebased onto #704:
    // 549 -> 561, MEASURED. `self.transaction`'s per-thread nesting
    // depth reads/writes through `Db._txn_depth`/`=` (runtime/ruby/db.rbs)
    // instead of a raw `Thread.current[:ar_txn_depth]`: the companion
    // RBS-paired probe has no model for the `Db` the depth used to
    // bypass, so the raw `Thread.current` read/write there typed as the
    // unresolved `Var`, not the honest gradual `Untyped` — routing it
    // through `Db`'s own declared contract is what brings that probe to
    // zero residual (see its hand-authored RBS and the `insert_db_stub`
    // mirror). This raw-inference probe (zero hand-authored signatures
    // anywhere, not even for the app's own runtime shims) has no such
    // stub for `Db` — confirmed by reverting to the `Thread.current` form
    // and re-measuring: exactly 549 (this branch's base after rebasing),
    // so the old form cost this probe nothing. Eleven of the twelve new
    // sites are the depth variable's reads/writes/arithmetic now going
    // through an unmodeled `Db` method; the twelfth is the `ensure`-based
    // non-local-exit fix (CodeRabbit on #693) — its outer branch's
    // `ensure`'s `if rolled_back then nil else … end` has to unify `nil`
    // against the unresolved `Db.exec("COMMIT")` call. Its RBS-paired
    // method stays at zero residual throughout.
    // `raise ActiveRecord::Rollback` (campfire's messages_count and
    // creation tests): 561 -> 562, MEASURED by removing the one line.
    // The new site is the Rollback arm's `Db.exec("ROLLBACK")`, read
    // here without the Db contract like the other arm's.
    // `clear_query_caches_for_current_thread` and `raw_connection.
    // transaction(:immediate)` (campfire's caching tests and its
    // messages_count trigger repair): 562 -> 569, MEASURED by removing
    // each — 4 for the former's `Db.query_cache_*` calls (the same escape
    // `uncached` takes), 3 for the latter's `Db.exec` calls.
    // `requires_new:` savepoints: 569 -> 578, MEASURED by removing them.
    // All nine are the nested branch's new `Db.exec` SAVEPOINT / RELEASE /
    // ROLLBACK TO calls (and the `if`/`unless` arms holding them) plus the
    // `depth` read in the savepoint name, unmodeled here like the others.
    // #818 AssociationReflection + `reflect_on_association`: 578 -> 583,
    // MEASURED — initialize ivar writes and the name/assoc loop under
    // raw inference (no RBS on this probe).
    // `Model.find_or_initialize_by` / `find_or_create_by` / `find_or_create_by!`
    // and Relation's `find_or_initialize_by` / `find_or_create_by!`:
    // 578 -> 592, MEASURED over that commit.
    // Meta-PR #802 (exists? arity / where-seed / related Relation arms):
    // 592 -> 603, MEASURED on tip 86936194's CI unit shard.
    // Remount remeasure after #818 merge: 603 -> 608.
    // `ErrorDetail`'s full message: 608 -> 610, MEASURED — its
    // initialize ivar write, untyped here like the other three fields'.
    // `ErrorDetail#symbolic`: 610 -> 612, MEASURED — the same write.
    // `ErrorDetail#optionless`: 612 -> 614, MEASURED — the same write.
    // `ActiveRecord::Preloading.records` / `Base#_preload_as_group` (mixed-model
    // `Associations::Preloader`): 614 -> 632, MEASURED — 16 + 2, every
    // read of the `records`/`specs`/`group` parameters, untyped here
    // without the RBS the runtime gate reads them with.
    // `param_ty_with_default` no longer types a `= nil` parameter as
    // always-`Nil` when no call site gives it a type: 632 -> 677,
    // MEASURED — every read of such a parameter (Arel::Table/Attribute
    // column/qualified/subquery/fragment, ErrorDetail's five ivar
    // writes, etc.) is now a real inference var instead of a folded
    // `Nil`, which is the honest "unknown" the RBS already says
    // (`?untyped`) rather than a type that silently zeroed `presence`.

    // `sanitize_sql_like` itself, without its String parameter RBS.
    // MEASURED 4 on 2026-10-19 (the old 27 counted the helper bodies,
    // which the existing ledger now owns). Remeasured 3 after merging
    // main: the `source = value` assign, the `value` read, and the
    // escape call's `value`. The wildcard call's `source` is no longer
    // a var on this analyzer.
    const LIKE_CEILING: usize = 3;
    let existing_untyped = all_untyped.len() - like_untyped;
    eprintln!("inference-only ledger: existing={existing_untyped}, sanitize_sql_like={like_untyped}");
    // Transactional saves (connection.rb: `save`/`destroy`/`touch` in a
    // transaction, commit callbacks enlisted until the outermost COMMIT):
    // 676 -> 757, MEASURED against origin/main 57f9521d3's runtime. The
    // new `_commit_enlisted` (37), `_rollback_enlisted` (23),
    // `_save_committed`/`_destroy_committed` (6), `_touch_owner` /
    // `_adopt_touch_owner` (6), `_restore_after_rollback` (2) and the
    // `transaction` / `update_attribute` / `changes_to_save` additions
    // all read the enlisted list through `Db._txn_records` (or the
    // owner slot) — unmodeled `Db` here, as the depth counter's reads
    // above are; the RBS-paired gate types them all.
    // Non-joinable fixture transactions (savepoints under
    // `use_transactional_tests`): 757 -> 787, MEASURED by swapping
    // connection.rb against this branch's base. The net thirty are
    // `_txn_parent_joinable?` (6), `_txn_note_joinable` (11), six more
    // in `transaction` (the parent-joinable read and the savepoint
    // name/flag), and `_commit_enlisted` moving into
    // `_commit_enlisted_from` (37 -> 44, the extra `from` reads). Same
    // unmodeled `Db` accessors as the depth counter; the RBS-paired
    // gate types them.
    // Raw `transaction` savepoints when SQLite is already open (the
    // fixture BEGIN must not be a second BEGIN): 787 -> 794, MEASURED.
    // The seven new sites are that branch's `Db.transaction_open?`,
    // `Db._txn_depth`, and `Db.exec` calls, unmodeled here like the
    // other `Db` reads. The RBS-paired gate types the method.
    // Remeasured 780 on 2026-10-19 against this tree: the 794 constant
    // had headroom the later runtime no longer uses.
    // MEASURED 766 on 2026-10-19: commit and rollback share
    // `_take_enlisted`, so the `Db._txn_records` slice is read once.
    // MEASURED 765 on 2026-10-19: the savepoint ROLLBACK TO / RELEASE
    // pair lives in `RawConnection._rollback_savepoint`, called from
    // the raw transaction and both rescue arms of `Base.transaction`.
    // Remeasured 634 on 2026-10-19 after merging main. The analyzer's
    // narrowing and the pluck rest-arg removed sites this constant
    // still counted; 765 had that headroom.
    // MEASURED 631 on 2026-10-19: save and destroy share
    // `_enlist_or_run`, so the `Db._txn_depth` / `Db._txn_records`
    // walk is read once.
    // MEASURED 837 on 2026-10-19 after merging main (`sanitize_sql_like`
    // stays 3). Main's ledger was 631. `ActiveRecord::Tasks` and the
    // Rake subset add the unseeded-parameter and `Db` reads this probe
    // does not type; the RBS-paired probe types the same files.
    // MEASURED 844 on 2026-10-19: schema load plans every statement
    // before the first DROP, and migrate refuses a schema dump before
    // it rewrites a table. Those helpers are unseeded here.
    // This probe has no RBS, so a new parameter read is a var. The
    // framework-surface classes add `Arel.sql`'s bind walk,
    // `Arel::Nodes` (`SqlLiteral`, `Quoted`, `Not`, `Case`,
    // `NamedFunction`), `Associations::Preloader`, `Type::String`,
    // `Type::Integer`, and `RecordNotDestroyed`. The RBS-paired
    // runtime gate still has zero vars in those methods.
    // Unmatched-bind checks and quoted CASE values read the same
    // untyped parameters again.
    // MEASURED 993 on 2026-10-19 after rebasing onto main's 844.
    // The framework-surface classes are unseeded on this probe.
    // MEASURED 1014 on 2026-10-20: `Relation#column_predicate` renders a
    // Range as Rails' comparison (#1130). No Range type, so the range,
    // its ends, and Array elements that may hold one are vars here
    // (993 → 1014, +21 on this probe; the RBS-paired gate measures +12
    // gradual sites in relation.rb and bumps its own ceiling to 846).
    // MEASURED 1043 on 2026-10-21 (main measured 1009): `touch_later`
    // defers a `belongs_to …, touch: true` to the COMMIT, enlisting
    // kind 3 on `Db._txn_records` (`touch_later` +13), and
    // `_flush_deferred_touches_from` walks those entries (+16);
    // `_commit_enlisted_from` skips them (+4) and `transaction` calls
    // the flush (+1). Entry reads off `Db._txn_records` are vars on
    // this RBS-less probe, as `_commit_enlisted_from`'s were already.
    // MEASURED 1082 on 2026-10-21: `current_transaction.after_commit`
    // holds its work on `Db._txn_callbacks` until the COMMIT
    // (`_run_after_commits_from` +20, `_drop_after_commits_from` +7,
    // `_after_commit` +6, `transaction` +6 for the savepoint mark and
    // the run/drop calls). The same untyped reads off a `Db` list as the
    // records above.
    // MEASURED 1085 on 2026-10-22: a record's column read by name
    // (`read_attribute`, `write_attribute`, `attribute_in_database`)
    // answers whatever that column holds (+3 in connection.rb's Base).
    // MEASURED 1086 on 2026-10-22: `merge` carries a merged `none` through
    // `other.explicit_none?`, read off the same untyped `other` its
    // `other.null_relation?` already reads (+1 in relation.rb).
    // MEASURED 1085 on 2026-10-23, with `Base.sanitize_sql_for_order(condition)`
    // answering its String unchanged; its parameter is a var on this
    // RBS-less probe (`(String) -> String` in connection.rbs).
    // MEASURED 1092 on 2026-10-26: `record_timestamps=` keeps the switch per
    // class (`TimestampSwitches.store` / `lookup`), and the reader walks the
    // ancestors for it. The class name and value parameters are vars on this
    // RBS-less probe (+7 in connection.rb); the RBS-paired gate types them.
    // MEASURED 1101 on 2026-10-26 (on 1092): `ActiveRecord::Bounds` holds
    // a `where` Range's non-numeric ends for Spinel, and `column_predicate`
    // reads them; its parameters and readers are vars on this RBS-less
    // probe (+9 in relation.rb); the RBS-paired gate types them.
    // MEASURED 1105 on 2026-10-26 (on 1101): the same Bounds element of a
    // condition Array, read untyped where the predicate builds its SQL
    // (+4 in active_record/relation.rb, 1101 -> 1105).
    // MEASURED 1110 on 2026-10-27 (on 1105): `ActiveRecord::EagerFilter`
    // carries an eager load's conditions and orders to the preload
    // walker; its parameters and readers are vars on this RBS-less probe
    // (+5 in active_record/relation.rb); the RBS-paired gate types them.
    // MEASURED 1112 on 2026-10-27 (on 1110): the filter's `where.not`
    // list, its parameter and reader (+2 in the same file), and the
    // preloads nested under the association (+2 more, 1114).
    // MEASURED 1121 on 2026-10-27 (on 1114): `clear_attribute_change` and
    // `restore_attribute!` take the column name; it is a var on this
    // RBS-less probe (+7 in active_record/connection.rb); the RBS-paired
    // gate types it.
    // MEASURED 1127 on 2026-10-27 (on 1121): hydration through the
    // adapter's `each_row` (`_hydrate_all`, `_adapter_all`,
    // `_adapter_find_by_id`, `Relation#load_records`). `ActiveRecord.adapter`
    // is `Var | nil` on this RBS-less probe, so the block's `row` is a var
    // (+4); the `select_rows(...).map` it replaces answered `Untyped`, and
    // `map` bound `row` to that. `_adapter_find_by_id`'s `id` goes through
    // the unmodeled `Db.escape_int` (+2). The RBS-paired gate types all six
    // through `each_row`'s block contract in base.rbs.
    // MEASURED 1136 on 2026-10-28: StaleObjectError accepts arbitrary records
    // and actions. This probe omits their Object? signatures, so their reads
    // are vars here (+9 in errors.rb, 1127 -> 1136); the RBS-paired gate adds 2.
    // MEASURED 1141 on 2026-10-28 with the same analyzer and collector
    // on main fa0b92c6 and this runtime (1136 -> 1141). All five new
    // sites are assign_attributes' two Hash reads, its each call,
    // the block key's to_s receiver and the block value. Existing methods and
    // sanitize_sql_like's three sites are unchanged.
    // MEASURED 1126 on 2026-10-29 after merging main 00e6fc23: the same
    // analyzer and collector measure 1124 total on main and 1129 here.
    // The five additional sites are all in Base#assign_attributes; every
    // other method's count is unchanged. Both totals include the same
    // 3 sanitize_sql_like sites, excluded from this ceiling (1141 -> 1126).
    // MEASURED 1111 after merging main f7ec7427 with the current analyzer:
    // the same collector measures 1109 total on main and 1114 here.
    // Only Base#assign_attributes adds sites (+5). Both totals include
    // the same 3 sanitize_sql_like sites, excluded here (1126 -> 1111).
    // MEASURED 1173 on 2026-10-30 against main dfd6a874 (1111): the bulk
    // writes' rows are attribute Hashes whose values are whatever the app
    // passes, and their parameters are vars on this RBS-less probe. Every
    // added site is in a method the bulk writes add or change: upsert_all
    // (+10), upsert (+1), _returned_rows (15), _insert_rows (11), insert,
    // insert_all, insert!, insert_all! (14), _string_keyed_rows (4),
    // _bulk_stamps (2), _database_now_sql and _same_value_sql (1 each),
    // Arel::Nodes.quote_scalar's boolean escape (2) and
    // Relation#bound_text (1). The RBS-paired gate types them.
    // MEASURED 1192 on 2026-10-30 against main 3dfccb68 (1173): Relation's
    // `and`/`and!` read the other relation's conditions and none (+10),
    // `unscope_parts`/`unscope_part!` read the part names (+8), and `lock`
    // reads its clause (+1). Their parameters are vars on this RBS-less
    // probe; relation.rbs and connection.rbs type them for the RBS-paired gate.
    // MEASURED 1209 on 2026-10-30 (on 1192): `and` refuses structurally
    // different relations, as Rails does, and keeps the other's HAVINGs.
    // `structurally_incompatible_with` reads the other relation's parts
    // (+10), `and!` its HAVINGs and the check (+6), and `structural_values`
    // its own list (+1). relation.rbs types them for the RBS-paired gate.
    // MEASURED 1210 on 2026-10-31 (on 1209): the ruby-family Base#slice
    // parses a temporal column's stored text into the Time its reader
    // answers, reading the untyped `[]` value a second time (+1 in
    // active_record/connection.rb), as `_as_json_only` does.
    const CEILING: usize = 1210;
    assert!(
        existing_untyped <= CEILING && like_untyped <= LIKE_CEILING,
        "{} untyped sub-expressions on spinel-blog runtime — exceeds ledger: \
         existing={existing_untyped}/{CEILING}, sanitize_sql_like={like_untyped}/{LIKE_CEILING}.\n\
         First 20:\n  {}",
        all_untyped.len(),
        all_untyped.iter().take(20).cloned().collect::<Vec<_>>().join("\n  ")
    );
}
