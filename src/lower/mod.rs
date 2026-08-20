//! Target-neutral lowerings of dialect IR.
//!
//! Phase 4's core contribution over railcar: extract the logic that's
//! identical across target runtimes (validation evaluation, SQL string
//! generation, router dispatch, turbo-stream templating) as IR-level
//! lowerings. Each target emitter consumes the lowered form and renders
//! it in target-specific code, so adding a new target is mostly
//! writing renders, not re-implementing the logic.
//!
//! The lowering IR lives alongside the dialect IR — it doesn't replace
//! it. Surface IR captures what the developer wrote (`validates :title,
//! presence: true`), lowered IR captures what an evaluator needs to do
//! (`Check::Presence { attr: "title" }`). Emitters read both, but the
//! per-target boilerplate shrinks to "render this lowered form."
//!
//! Starting with validations as the pilot — smallest scope that
//! exercises the pattern. If it works, follow-ups cover query algebra,
//! broadcasts orchestration, schema → DDL, and router dispatch tables.

pub mod arel;
pub mod associations;
pub mod blank;
pub mod broadcast_calls;
pub mod broadcasts;
pub mod chain;
pub mod controller;
pub mod controller_test;
pub mod fixtures;
pub mod functionalize;
pub mod model_associations;
pub mod persistence;
pub mod controller_to_library;
pub mod fixture_to_library;
pub mod importmap_to_library;
pub mod jbuilder_to_library;
pub mod library_extras;
pub mod model_to_library;
pub mod routes;
pub mod routes_to_library;
pub mod scope_chain;
pub mod schema_to_library;
pub mod seeds_to_library;
pub mod test_module_to_library;
pub mod create_block;
pub mod params_merge;
pub mod duration;
pub mod and_return;
pub mod case_lambda;
pub mod first_or_create;
pub mod authenticate_by;
pub mod group_count;
pub mod bool_fold;
pub mod dead_default;
pub mod errors_add;
pub mod errors_full_messages;
pub mod job_class_side;
pub mod mailer_class_side;
pub mod as_json_shape;
pub mod as_json_writer;
pub mod as_json_super;
pub mod parameterize;
pub mod presence_in;
pub mod dirty_predicate_kwargs;
pub mod job_test_only;
pub mod sti_scope;
pub mod sum_symbol;
pub mod values_at_splat;
pub mod request_index;
pub mod arel_attribute;
pub mod exclude_predicate;
pub mod in_predicate;
pub mod including;
pub mod enum_symbols;
pub mod has_json;
pub mod update_writer_check;
pub mod route_format_suffix;
pub mod config_reader;
pub mod exists_conditions;
pub mod inquiry;
pub mod byte_size;
pub mod tag_builder;
pub mod kwsplat;
pub mod literal_append;
pub mod html_safe;
pub mod rails_cache;
pub mod session_options;
pub mod relation_residue;
pub mod relation_select_block;
pub mod send_dispatch;
pub(crate) mod secure_password;
pub mod attached;
pub mod column_ops;
pub mod signed_id;
pub(crate) mod secure_token;
pub mod rich_text;
pub mod capture_inline;
pub mod partial_qualify;
pub mod time_current;
pub mod transaction_ground;
pub mod update_kwargs;
pub(crate) mod typed_store;
pub mod ty_coerce_insertion;
pub mod typing;
pub mod validations;
pub mod view;
pub mod view_to_library;

pub use blank::apply_blank_lowering;
pub use create_block::apply_create_block_inline;
pub use duration::apply_duration_lowering;
pub use and_return::apply_and_return_lowering;
pub use case_lambda::apply_case_lambda_lowering;
pub use first_or_create::apply_first_or_create_lowering;
pub use authenticate_by::apply_authenticate_by_lowering;
pub use group_count::apply_group_count_lowering;
pub use dead_default::apply_dead_default_lowering;
pub use errors_add::apply_errors_add_lowering;
pub use errors_full_messages::apply_errors_full_messages_lowering;
pub use mailer_class_side::apply_mailer_class_side;
pub use as_json_super::apply_as_json_super_grounding;
pub use parameterize::apply_parameterize_grounding;
pub use presence_in::apply_presence_in_grounding;
pub use dirty_predicate_kwargs::apply_dirty_predicate_kwargs;
pub use job_test_only::apply_job_test_only_lowering;
pub use sti_scope::apply_sti_scope_lowering;
pub use request_index::apply_request_index_lowering;
pub use arel_attribute::apply_arel_attribute_lowering;
pub use exclude_predicate::apply_exclude_predicate_lowering;
pub use in_predicate::apply_in_predicate_lowering;
pub use including::apply_including_lowering;
pub use relation_select_block::apply_relation_select_block_lowering;
pub use enum_symbols::apply_enum_symbol_lowering;
pub use has_json::apply_has_json_lowering;
pub use route_format_suffix::apply_route_format_suffix_lowering;
pub use config_reader::apply_config_reader_lowering;
pub use exists_conditions::apply_exists_conditions_lowering;
pub use inquiry::apply_inquiry_lowering;
pub use literal_append::apply_literal_append_lowering;
pub use html_safe::apply_html_safe_lowering;
pub use rails_cache::apply_rails_cache_lowering;
pub use session_options::apply_session_options_lowering;
pub use send_dispatch::apply_send_static_dispatch;
pub use capture_inline::apply_capture_inline;
pub use partial_qualify::apply_partial_qualification;
pub use time_current::apply_time_current_lowering;
pub use transaction_ground::apply_transaction_grounding;
pub use update_kwargs::apply_update_kwargs_inline;

/// Build a `LowerResidue` diagnostic — the shared assembly a pass emits
/// when it must leave a construct dynamic. Each pass supplies its own
/// `pass`/`construct` tags, `span`, and human-readable `message`; the
/// kind construction, default severity, and field wiring live here so
/// the six residue-emitting passes don't each re-derive them. Callers
/// interpolate `reason` into `message` themselves (the phrasing is
/// per-pass), so it is passed both as a diagnostic field and left to the
/// caller's message text.
pub(crate) fn residue_diagnostic(
    pass: &str,
    construct: &str,
    span: crate::span::Span,
    reason: &str,
    message: String,
) -> crate::diagnostic::Diagnostic {
    use crate::diagnostic::{Diagnostic, DiagnosticKind};
    use crate::ident::Symbol;
    let kind = DiagnosticKind::LowerResidue {
        pass: Symbol::from(pass),
        construct: Symbol::from(construct),
        reason: Symbol::from(reason),
    };
    Diagnostic {
        span,
        severity: Diagnostic::default_severity(&kind),
        kind,
        message,
    }
}

/// Canonical execution order of the post-analyze pass pipeline, and the
/// single authority for its ordering constraints. Each entry is
/// `(pass_name, &[passes_that_must_run_before_it])`; the list itself is
/// the intended call order in [`apply_post_analyze_lowerings`]. Passes
/// with an empty `runs_after` are order-independent.
///
/// This replaces the ordering knowledge that used to live only in prose
/// scattered across the passes ("AFTER send_dispatch, by contract" in
/// `duration.rs` / `send_dispatch.rs`). Those comments now point here.
/// The `fn` pointer is deliberately NOT part of the entry: the passes
/// have heterogeneous signatures (some return `Vec<Diagnostic>`, some
/// take the class `registry`), so a uniform table would need wrappers
/// for zero benefit over the name — the list's job is ordering, not
/// dispatch. Soundness (every predecessor precedes its dependent) is
/// checked by a `debug_assert!` on entry to the pipeline and by the
/// `post_analyze_pass_order_is_sound` unit test.
const POST_ANALYZE_PASS_ORDER: &[(&str, &[&str])] = &[
    // Deletes provably-dead `false && …` tails before any pass can
    // ledger residue for (or rewrite inside) code that cannot run.
    ("bool_fold", &[]),
    ("blank", &[]),
    ("time_current", &[]),
    ("as_json_super", &[]),
    ("parameterize", &[]),
    // `value.presence_in(list)` → `ActiveSupport.presence_in(value,
    // list)`; a receiver-shape rewrite of a name no other pass produces
    // or consumes, so no ordering constraints.
    ("presence_in", &[]),
    // `Rooms::Open.count` → `Room.where(type: "Rooms::Open").count`.
    // Produces a `where` at a model Const root, which is vocabulary
    // every later pass already reads; consumes nothing any pass
    // produces, so no ordering constraints.
    ("sti_scope", &[]),
    // `only: <JobClass>` → `only: ["JobClass"]` in test bodies. Reads a
    // Const literal nothing else produces and writes a String array
    // nothing else reads, so no ordering constraints.
    ("job_test_only", &[]),
    // `x_previously_changed?(to: V)` → the predicate AND a comparison.
    // Reads a kwargs hash on a synthesized predicate name and writes
    // reads of the same synthesis; no pass produces or consumes either
    // shape, so no ordering constraints.
    ("dirty_predicate_kwargs", &[]),
    // `sum(:col)` → block form; no ordering constraints (rewrites a
    // literal-symbol arg shape no other pass produces or consumes).
    ("sum_symbol", &[]),
    // `values_at(*keys)` → keys.map block form; same no-constraints
    // rationale.
    ("values_at_splat", &[]),
    // `tag.div(…)` → the HTML string it builds. BEFORE `html_safe`
    // (whose fold would erase the `.html_safe` marker it reads on
    // content, and which must SEE the marker this pass writes so the
    // enclosing helper registers as safe) and before `capture_inline`
    // (which flattens the `capture { … }` this pass synthesizes for the
    // block form).
    ("tag_builder", &[]),
    ("request_index", &[]),
    // `config.session_options[:key]` → `session_cookie_key`; rewrites a
    // receiver chain no other pass produces or consumes.
    ("session_options", &[]),
    // `x.exclude?(y)` → `!x.include?(y)`; total rewrite, no ordering
    // constraints (no other pass produces or consumes `exclude?`).
    ("exclude_predicate", &[]),
    // `x.in?(xs)` → `xs.include?(x)`; the `exclude_predicate` mirror,
    // same total rewrite and same lack of ordering constraints.
    ("in_predicate", &[]),
    // `xs.including(a)` → `xs.to_a + [a]`; same shape and same lack of
    // ordering constraints as `exclude_predicate` above.
    ("including", &[]),
    // `<relation>.select { … }` → `.filter { … }`, so the projection
    // `select(*specs)` is the only thing left on the name and answers a
    // Relation and nothing else. Reads the receiver's analyzer type
    // (Relation, or untyped) and rewrites only the method name, so it
    // has no ordering constraint of its own — it just has to run before
    // the `relation_residue` ledger reads the final chain shape.
    ("relation_select_block", &[]),
    // `x_path(format: :json)` → `x_path() + ".json"`. Must run before
    // the route-helper lowering surveys call sites for query keys; the
    // two do not overlap (`format` is on NON_QUERY_OPTIONS) but the
    // survey should see the shape this pass leaves behind.
    ("route_format_suffix", &[]),
    // `where(role: :bot)` → `where(role: 2)`. Must run BEFORE the arel
    // folding that turns a `where` hash into SQL, so the folded literal
    // is the integer the column stores.
    ("enum_symbols", &[]),
    // `account.settings.foo?` → `account.settings_foo?`. No ordering
    // constraint: the two-hop shape it consumes is one no other pass
    // produces, and the flat send it leaves is an ordinary typed call.
    ("has_json", &[]),
    // Read-only ledger: a `self.update(k: …)` whose `k` no writer backs.
    // Rewrites nothing, so it has no ordering constraint of its own —
    // it just has to see the final tree.
    ("update_writer_check", &[]),
    // `x.inquiry` / `x.<name>?` → equality against the label; total
    // rewrite of a name no other pass produces or consumes.
    ("inquiry", &[]),
    // `Model.exists?(col: v)` → `Model.where(col: v).exists?`, so the
    // conditions form reaches the Relation instead of the by-id
    // primitive. Before the emit-time arel rewrite, which then folds
    // the chain when the values are literal.
    ("exists_conditions", &[]),
    // `Rails.application.config.<key>` → `Rails.application.<key>`, the
    // read half of the config lift; no ordering constraints (no other
    // pass produces or consumes the `config` hop).
    ("config_reader", &[]),
    ("arel_attribute", &[]),
    // `"lit" << x` → `"lit" + x`; local expression rewrite, no ordering
    // constraints.
    ("literal_append", &[]),
    // `5.megabytes` → `5 * 1048576`; local expression rewrite of a name
    // no other pass produces or consumes.
    ("byte_size", &[]),
    // `f(**h)` (erased to `f(h)` at ingest) → `f(k: h[:k], …)` when the
    // callee declares explicit keywords. Reads the arg count against the
    // callee's signature, so it must see the argument list as ingested —
    // before any pass that appends or drops a positional argument.
    ("kwsplat", &[]),
    // `Rails.cache.fetch(k, expires_in: t) { <String> }` → `fetch_str`;
    // must run BEFORE the render lowering, whose rewrite of
    // `render_to_string` into a `Views::` call is the tail this gates on.
    ("rails_cache", &[]),
    // `<e>.html_safe` → `<e>`, recording the producing method on the
    // App. No ordering constraints among the lowerings; the view
    // lowerer reads what it records, and that runs later, at emit.
    ("html_safe", &["tag_builder"]),
    ("transaction_ground", &[]),
    ("column_ops", &[]),
    // `signed_id(purpose: :avatar)` → the runtime SignedId call, with
    // the model name folded into the purpose. BEFORE `duration`: the
    // `expires_in:` argument this wraps in `.to_i` is an
    // `ActiveSupport::Duration`, and `duration` is what grounds one.
    ("signed_id", &[]),
    ("partial_qualify", &[]),
    ("capture_inline", &["tag_builder"]),
    ("and_return", &[]),
    ("case_lambda", &[]),
    ("first_or_create", &[]),
    // `Model.authenticate_by(email: …, password: …)` → bind
    // `find_by(<identifiers>)`, then check `authenticate(<password>)`;
    // macro-inline of a Rails 7.1 name no other pass produces or
    // consumes. Before `relation_residue`, which then sees the grounded
    // `find_by` chain rather than an unresolved send.
    ("authenticate_by", &[]),
    ("group_count", &[]),
    ("dead_default", &[]),
    ("errors_add", &[]),
    // `errors.full_messages` -> `errors`; total rewrite, no ordering
    // constraints (no other pass produces or consumes `full_messages`,
    // and the `errors` receiver it matches is left untouched).
    ("errors_full_messages", &[]),
    ("create_block", &[]),
    // `<params>.merge(k: v)` written a method away from the permit
    // chain → `Model.from_params(p)` + per-key setters, hoisted above
    // the enclosing statement. AFTER `create_block`, whose inlining
    // turns `Model.create!(p.merge(...)) { }` into the `Model.new(...)`
    // shape this pass matches.
    ("params_merge", &["create_block"]),
    ("update_kwargs", &[]),
    ("mailer_class_side", &[]),
    ("job_class_side", &[]),
    ("send_static_dispatch", &[]),
    // Grounds the plural duration-unit calls that send_static_dispatch
    // synthesizes into case arms, so it must observe that pass's output.
    ("duration", &["send_static_dispatch"]),
    // Rails-API broadcast calls in ordinary method bodies (a concern's
    // `def broadcast_create`) → `Broadcasts.<action>(…)`. Late, so the
    // `Views::…` render call it synthesizes is not re-walked by the
    // partial/capture passes.
    ("broadcast_calls", &[]),
    // Pure ledger (no rewrite): counts Relation-typed chains still
    // dynamic after every grounding pass has had its say — last so a
    // chain a pass grounds doesn't false-positive.
    ("relation_residue", &["duration"]),
];

/// True iff `POST_ANALYZE_PASS_ORDER` is a valid topological order —
/// every pass's declared predecessors appear at an earlier index, and
/// each predecessor name actually exists in the list.
fn post_analyze_pass_order_is_sound() -> bool {
    for (i, (_name, after)) in POST_ANALYZE_PASS_ORDER.iter().enumerate() {
        for pred in *after {
            match POST_ANALYZE_PASS_ORDER.iter().position(|(n, _)| n == pred) {
                Some(j) if j < i => {}
                _ => return false,
            }
        }
    }
    true
}

/// Post-analyze shared lowerings — type-directed IR rewrites every
/// target consumes, run between `Analyzer::analyze` and any emitter.
/// One entry point so the transpile driver, the site build, and the IR
/// dump can't drift as passes accumulate (the LSP/MCP/IDE paths stay
/// off it on purpose: they want source-shaped IR). Returns the residue
/// diagnostics — sites a pass had to leave dynamic, with the reason.
///
/// The call order below is the canonical [`POST_ANALYZE_PASS_ORDER`];
/// keep the two in sync when adding a pass. In debug builds an
/// `executed` list is threaded past each call and asserted equal to the
/// const's names in order, so a pass added to the code but not the const
/// (or vice versa, or reordered) fails every debug test run — the
/// code↔list correspondence the `runs_after` debug_assert alone can't
/// catch.
///
/// `registry` is the analyzer's post-fixpoint class table
/// ([`crate::analyze::Analyzer::class_registry`]) — passes that
/// synthesize dispatches consult it to stamp what analyze would have
/// computed.
pub fn apply_post_analyze_lowerings(
    app: &mut crate::app::App,
    registry: &std::collections::HashMap<crate::ident::ClassId, crate::analyze::ClassInfo>,
) -> Vec<crate::diagnostic::Diagnostic> {
    debug_assert!(
        post_analyze_pass_order_is_sound(),
        "POST_ANALYZE_PASS_ORDER violates a declared runs_after constraint",
    );
    // Debug-only record of the passes actually run, in call order,
    // asserted against POST_ANALYZE_PASS_ORDER at the end. Catches the
    // code↔list drift the `runs_after` check above can't: a pass added
    // here but not to the const (or removed, or reordered) fails the
    // assert. `push` calls sit adjacent to each pass call below.
    #[cfg(debug_assertions)]
    let mut executed: Vec<&str> = Vec::new();
    #[cfg(debug_assertions)]
    macro_rules! ran {
        ($name:expr) => {
            executed.push($name)
        };
    }
    #[cfg(not(debug_assertions))]
    macro_rules! ran {
        ($name:expr) => {};
    }
    bool_fold::apply_bool_fold_lowering(app);
    ran!("bool_fold");
    let mut diags = blank::apply_blank_lowering(app);
    ran!("blank");
    time_current::apply_time_current_lowering(app);
    ran!("time_current");
    as_json_super::apply_as_json_super_grounding(app);
    ran!("as_json_super");
    parameterize::apply_parameterize_grounding(app);
    ran!("parameterize");
    presence_in::apply_presence_in_grounding(app);
    ran!("presence_in");
    sti_scope::apply_sti_scope_lowering(app);
    ran!("sti_scope");
    job_test_only::apply_job_test_only_lowering(app);
    ran!("job_test_only");
    dirty_predicate_kwargs::apply_dirty_predicate_kwargs(app);
    ran!("dirty_predicate_kwargs");
    sum_symbol::apply_sum_symbol_lowering(app);
    ran!("sum_symbol");
    values_at_splat::apply_values_at_splat_lowering(app);
    ran!("values_at_splat");
    diags.extend(tag_builder::apply_tag_builder_lowering(app));
    ran!("tag_builder");
    request_index::apply_request_index_lowering(app);
    ran!("request_index");
    session_options::apply_session_options_lowering(app);
    ran!("session_options");
    exclude_predicate::apply_exclude_predicate_lowering(app);
    ran!("exclude_predicate");
    in_predicate::apply_in_predicate_lowering(app);
    ran!("in_predicate");
    including::apply_including_lowering(app);
    ran!("including");
    relation_select_block::apply_relation_select_block_lowering(app);
    ran!("relation_select_block");
    route_format_suffix::apply_route_format_suffix_lowering(app);
    ran!("route_format_suffix");
    enum_symbols::apply_enum_symbol_lowering(app);
    ran!("enum_symbols");
    diags.extend(has_json::apply_has_json_lowering(app));
    ran!("has_json");
    // Read-only ledger, no rewrite — but it must run AFTER
    // `enum_symbols` so a label that pass already translated isn't
    // mistaken for anything, and after every pass that could introduce
    // an `update` site.
    update_writer_check::apply_update_writer_check(app);
    ran!("update_writer_check");
    inquiry::apply_inquiry_lowering(app);
    ran!("inquiry");
    exists_conditions::apply_exists_conditions_lowering(app);
    ran!("exists_conditions");
    config_reader::apply_config_reader_lowering(app);
    ran!("config_reader");
    arel_attribute::apply_arel_attribute_lowering(app);
    ran!("arel_attribute");
    literal_append::apply_literal_append_lowering(app);
    ran!("literal_append");
    byte_size::apply_byte_size_lowering(app);
    ran!("byte_size");
    diags.extend(kwsplat::apply_kwsplat_expansion(app));
    ran!("kwsplat");
    rails_cache::apply_rails_cache_lowering(app);
    ran!("rails_cache");
    html_safe::apply_html_safe_lowering(app);
    ran!("html_safe");
    transaction_ground::apply_transaction_grounding(app);
    ran!("transaction_ground");
    column_ops::apply_column_ops_lowering(app);
    ran!("column_ops");
    signed_id::apply_signed_id_lowering(app);
    ran!("signed_id");
    partial_qualify::apply_partial_qualification(app);
    ran!("partial_qualify");
    capture_inline::apply_capture_inline(app);
    ran!("capture_inline");
    and_return::apply_and_return_lowering(app);
    ran!("and_return");
    case_lambda::apply_case_lambda_lowering(app);
    ran!("case_lambda");
    first_or_create::apply_first_or_create_lowering(app);
    ran!("first_or_create");
    diags.extend(authenticate_by::apply_authenticate_by_lowering(app));
    ran!("authenticate_by");
    group_count::apply_group_count_lowering(app);
    ran!("group_count");
    dead_default::apply_dead_default_lowering(app, registry);
    ran!("dead_default");
    diags.extend(errors_add::apply_errors_add_lowering(app));
    ran!("errors_add");
    errors_full_messages::apply_errors_full_messages_lowering(app);
    ran!("errors_full_messages");
    diags.extend(create_block::apply_create_block_inline(app));
    ran!("create_block");
    diags.extend(params_merge::apply_params_merge_lowering(app));
    ran!("params_merge");
    diags.extend(update_kwargs::apply_update_kwargs_inline(app));
    ran!("update_kwargs");
    diags.extend(mailer_class_side::apply_mailer_class_side(app));
    ran!("mailer_class_side");
    diags.extend(job_class_side::apply_job_class_side(app));
    ran!("job_class_side");
    diags.extend(send_dispatch::apply_send_static_dispatch(app, registry));
    ran!("send_static_dispatch");
    // AFTER send_dispatch — see POST_ANALYZE_PASS_ORDER (the `duration`
    // entry's runs_after). An all-duration-unit name set dispatches
    // through case arms synthesized as plural unit calls that count on
    // this grounding (`send_dispatch::duration_plural`).
    duration::apply_duration_lowering(app);
    ran!("duration");
    broadcast_calls::apply_broadcast_calls_lowering(app);
    ran!("broadcast_calls");
    diags.extend(relation_residue::apply_relation_residue_ledger(app));
    ran!("relation_residue");
    #[cfg(debug_assertions)]
    debug_assert_eq!(
        executed,
        POST_ANALYZE_PASS_ORDER
            .iter()
            .map(|(n, _)| *n)
            .collect::<Vec<_>>(),
        "apply_post_analyze_lowerings call sequence drifted from POST_ANALYZE_PASS_ORDER",
    );
    diags
}

/// Every app body the post-analyze hook owns: model methods, scope
/// bodies, callback conditions and unrecognized class-body exprs;
/// library-class methods; controller actions and unrecognized items;
/// seeds. Param DEFAULTS ride along everywhere a body does — a default
/// is call-time-evaluated body code, and `def initialize(cache_time =
/// 30.minutes)` needs the duration grounding (or `Time.current` its
/// own) exactly as much as a body site; defaults were the one
/// reachable-expr position the hook skipped (lobsters'
/// FlaggedCommenters left an ungrounded `Integer#minutes` send whose
/// untyped result every downstream consumer inherited).
///
/// Class-body CONSTANT INITIALIZERS ride along for the same reason, one
/// step earlier: `CONNECTION_TTL = 60.seconds` is code that runs at
/// load time, and left ungrounded it reaches the emit as a literal
/// `Integer#seconds` send that dies the moment the file is required
/// (campfire's `Membership::Connectable`). A library class's
/// `unknown_calls` join them — the model side already visits its
/// `Unknown` class-body exprs on exactly that argument, and the two
/// fields hold the same kind of replayed class-body code.
///
/// The one definition of the hook's scope — passes iterate through here
/// so they can't drift. View bodies are deliberately excluded (each
/// target's view pipeline still has its own working walkers over
/// source shapes — see the note in [`blank::apply_blank_lowering`];
/// views rejoin when the view pipeline migrates to shared lowerings).
/// Test-module and fixture bodies are excluded too (they run on CRuby
/// lanes; extendable when a strict-target test lane needs it).
/// Every body that runs on a MODEL INSTANCE, in one place.
///
/// Three families, and they are NOT all in `model.body`:
///
///   * the model's own methods;
///   * an ASSOCIATION EXTENSION's methods (`has_many :memberships do
///     def revise(…) … end end`), which hang off the association;
///   * a model CONCERN's methods (`module User::Bannable`), which live
///     in `app.library_classes` under a namespace naming the model.
///
/// This exists because `transaction_ground` hand-rolled the first
/// family, shipped, then needed the second, shipped, then needed the
/// third — three commits for one fact. A pass that rewrites "what a
/// model instance can call" walks all three or it drifts, so the walk
/// is written once and shared.
///
/// NOT the same set as [`for_each_hook_body`], which also covers
/// controllers, plain library classes and seeds. A pass keyed to model
/// instances specifically (a bare `transaction`, a bare `touch`) wants
/// this narrower one — the wider walk would rewrite a helper module's
/// send, which is somebody else's method.
pub(crate) fn for_each_model_body(
    app: &mut crate::app::App,
    f: &mut impl FnMut(&mut crate::expr::Expr),
) {
    for_each_model_body_named(app, &mut |_model, e| f(e));
}

/// [`for_each_model_body`], with the OWNING MODEL's name handed to the
/// callback. A concern's body reads as `User::Avatar`'s and an
/// association extension's as the association's, but Rails runs all
/// three on a `User`; a rewrite that needs the model name (Rails'
/// `combine_signed_id_purposes` prefixes it) must be told which one,
/// because the body itself does not say.
pub(crate) fn for_each_model_body_named(
    app: &mut crate::app::App,
    f: &mut impl FnMut(&str, &mut crate::expr::Expr),
) {
    let model_names: std::collections::HashSet<String> =
        app.models.iter().map(|m| m.name.0.as_str().to_string()).collect();
    for model in &mut app.models {
        let name = model.name.0.as_str().to_string();
        for item in &mut model.body {
            match item {
                crate::dialect::ModelBodyItem::Method { method, .. } => f(&name, &mut method.body),
                crate::dialect::ModelBodyItem::Association {
                    assoc: crate::dialect::Association::HasMany { extension, .. },
                    ..
                } => {
                    for m in extension.iter_mut() {
                        f(&name, &mut m.body);
                    }
                }
                _ => {}
            }
        }
    }
    for lc in &mut app.library_classes {
        if !lc.is_module {
            continue;
        }
        let Some((namespace, _)) = lc.name.0.as_str().rsplit_once("::") else { continue };
        if !model_names.contains(namespace) {
            continue;
        }
        let name = namespace.to_string();
        for m in &mut lc.methods {
            f(&name, &mut m.body);
        }
    }
}

/// Every body a TEST MODULE holds — each test's, the `setup` hook's,
/// and each helper method's.
///
/// Deliberately NOT folded into [`for_each_hook_body`]: most passes
/// that use that walk are about app semantics and have their own
/// reasons for skipping test bodies (the blank pass says so in its
/// header). A pass that wants test bodies asks for them by name, which
/// keeps the widening reviewable one pass at a time.
pub(crate) fn for_each_test_body(
    app: &mut crate::app::App,
    f: &mut impl FnMut(&mut crate::expr::Expr),
) {
    for tm in &mut app.test_modules {
        if let Some(setup) = &mut tm.setup {
            f(setup);
        }
        for t in &mut tm.tests {
            f(&mut t.body);
        }
        for h in &mut tm.helpers {
            f(&mut h.body);
        }
    }
}

pub(crate) fn for_each_hook_body(
    app: &mut crate::app::App,
    f: &mut impl FnMut(&mut crate::expr::Expr),
) {
    fn visit_param_defaults(
        params: &mut [crate::dialect::Param],
        f: &mut impl FnMut(&mut crate::expr::Expr),
    ) {
        for p in params {
            if let Some(default) = &mut p.default {
                f(default);
            }
        }
    }
    for model in &mut app.models {
        for item in &mut model.body {
            match item {
                crate::dialect::ModelBodyItem::Method { method, .. } => {
                    visit_param_defaults(&mut method.params, f);
                    f(&mut method.body)
                }
                crate::dialect::ModelBodyItem::Scope { scope, .. } => {
                    visit_param_defaults(&mut scope.params, f);
                    f(&mut scope.body)
                }
                crate::dialect::ModelBodyItem::Callback { callback, .. } => {
                    if let Some(cond) = &mut callback.condition {
                        f(cond);
                    }
                }
                // Unrecognized class-body exprs (constant procs and
                // friends) round-trip verbatim into the emit — their
                // sites are just as reachable.
                crate::dialect::ModelBodyItem::Unknown { expr, .. } => f(expr),
                _ => {}
            }
        }
    }
    for lc in &mut app.library_classes {
        for method in &mut lc.methods {
            visit_param_defaults(&mut method.params, f);
            f(&mut method.body);
        }
        for (_name, value) in &mut lc.constants {
            f(value);
        }
        for call in &mut lc.unknown_calls {
            f(call);
        }
    }
    for controller in &mut app.controllers {
        for item in &mut controller.body {
            match item {
                crate::dialect::ControllerBodyItem::Action { action, .. } => {
                    for (_name, default) in &mut action.opt_params {
                        f(default);
                    }
                    f(&mut action.body)
                }
                crate::dialect::ControllerBodyItem::Unknown { expr, .. } => f(expr),
                _ => {}
            }
        }
    }
    if let Some(seeds) = &mut app.seeds {
        f(seeds);
    }
}

/// Read-only twin of [`for_each_hook_body`], for a pass that needs to
/// SURVEY every body before anything rewrites one — `emit::ruby::
/// library::apply_scope_lowering` collects which class methods are
/// reached through an association, and it runs once per emitted family
/// (models, then controllers) over a different `lcs` slice each time.
/// Surveying the App instead of the slice is what makes the two runs
/// agree; disagreeing would thread a relation at the call site into a
/// method that never grew the parameter.
///
/// Kept adjacent to the mutable version deliberately: the two must walk
/// the same set, and the only defence against that drifting is that
/// they are read together.
pub(crate) fn for_each_hook_body_ref(
    app: &crate::app::App,
    f: &mut impl FnMut(&crate::expr::Expr),
) {
    fn visit_param_defaults(
        params: &[crate::dialect::Param],
        f: &mut impl FnMut(&crate::expr::Expr),
    ) {
        for p in params {
            if let Some(default) = &p.default {
                f(default);
            }
        }
    }
    for model in &app.models {
        for item in &model.body {
            match item {
                crate::dialect::ModelBodyItem::Method { method, .. } => {
                    visit_param_defaults(&method.params, f);
                    f(&method.body)
                }
                crate::dialect::ModelBodyItem::Scope { scope, .. } => {
                    visit_param_defaults(&scope.params, f);
                    f(&scope.body)
                }
                crate::dialect::ModelBodyItem::Callback { callback, .. } => {
                    if let Some(cond) = &callback.condition {
                        f(cond);
                    }
                }
                crate::dialect::ModelBodyItem::Unknown { expr, .. } => f(expr),
                _ => {}
            }
        }
    }
    for lc in &app.library_classes {
        for method in &lc.methods {
            visit_param_defaults(&method.params, f);
            f(&method.body);
        }
        for (_name, value) in &lc.constants {
            f(value);
        }
        for call in &lc.unknown_calls {
            f(call);
        }
    }
    for controller in &app.controllers {
        for item in &controller.body {
            match item {
                crate::dialect::ControllerBodyItem::Action { action, .. } => {
                    for (_name, default) in &action.opt_params {
                        f(default);
                    }
                    f(&action.body)
                }
                crate::dialect::ControllerBodyItem::Unknown { expr, .. } => f(expr),
                _ => {}
            }
        }
    }
    if let Some(seeds) = &app.seeds {
        f(seeds);
    }
}

pub use associations::{
    build_has_many_table, resolve_has_many, resolve_has_many_on_local, HasManyRef, HasManyRow,
};
pub use chain::{collect_chain_modifiers, ChainModifier};
pub use controller_to_library::{
    lower_controller_to_library_class, lower_controllers_to_library_classes,
    lower_controllers_with_arel, lower_controllers_with_arel_and_views,
    lower_controllers_with_arel_views_and_assocs,
    lower_controllers_with_arel_views_assocs_and_routes, LowerControllerOptions,
};
pub use model_to_library::{
    class_info_from_library_class, lower_model_to_library_class, lower_models_to_library_classes,
    lower_models_to_library_classes_with_params, lower_models_with_registry,
    lower_models_with_registry_and_params,
};
pub use fixture_to_library::{lower_fixtures_to_library_classes, rewrite_fixture_calls};
pub use importmap_to_library::lower_importmap_to_library_functions;
pub use library_extras::{extras_from_funcs, extras_from_lcs};
pub use routes_to_library::{
    lower_routes_to_dispatch_functions, lower_routes_to_library_functions,
    url_options_helper_name,
};
pub use schema_to_library::lower_schema_to_library_functions;
pub use seeds_to_library::lower_seeds_to_library_functions;
pub use test_module_to_library::{
    lower_test_module_to_library_class, lower_test_modules_to_library_classes,
    lower_test_modules_with_inner, LoweredTestModule,
};
pub use ty_coerce_insertion::{insert_ty_coercions, insert_ty_coercions_with_extras};
pub use view_to_library::{
    ViewLowerCtx, flatten_lcs_to_functions, lower_view_to_library_class,
    lower_views_to_library_classes, lower_views_to_library_functions,
};
pub use jbuilder_to_library::{
    lower_jbuilder_to_library_class, lower_jbuilder_to_library_classes,
};
pub use broadcasts::{
    app_broadcasts_live, lower_broadcasts, BroadcastAction, LoweredAssocRef, LoweredBroadcast,
    LoweredBroadcasts,
};
pub use controller::{
    chain_target_class, classify_controller_send, default_permitted_fields,
    extract_permitted_from_expr, extract_status_from_kwargs, find_nested_parent,
    has_toplevel_terminal, is_empty_body, is_format_binding, is_params_expr,
    is_query_builder_method, is_resource_params_call, lower_action,
    model_new_with_strong_params, normalize_action_body, permitted_fields_for,
    resolve_before_actions, resource_from_controller_name, singularize_to_model,
    split_public_private, status_sym_to_code, synthesize_implicit_render,
    unwrap_respond_to, update_with_strong_params, walk_controller_ivars,
    ActionKind, LoweredAction, NestedParent, SendKind, WalkedIvars,
};
pub use controller_test::{
    classify_assert_select, classify_controller_test_send, classify_url_expr,
    flatten_params_pairs, test_body_stmts, AssertSelectKind, ControllerTestSend, UrlArg,
    UrlHelperCall,
};
pub use fixtures::{
    lower_fixtures, LoweredFixture, LoweredFixtureField, LoweredFixtureRecord, LoweredFixtureSet,
    LoweredFixtureValue,
};
pub use persistence::{lower_persistence, BelongsToCheck, DependentChild, LoweredPersistence};
pub use routes::{flatten_routes, standard_resource_actions, FlatRoute};
pub use validations::{lower_validations, Check, InclusionValue, LoweredValidation};
pub use view::{
    classify_class_value, classify_errors_field_predicate, classify_form_builder_args,
    classify_form_builder_method, classify_nested_form_child, classify_nested_url_element,
    classify_render_partial, classify_turbo_stream_call, classify_view_helper,
    classify_view_url_arg, ClassValueShape,
    ErrorsFieldPredicate, FormBuilderMethod, NestedFormChild, NestedUrlElement, RenderPartial,
    ViewHelperKind, ViewUrlArg,
};

/// Bundle module-level `LibraryFunction`s (RouteHelpers, Importmap,
/// …) into a module-flavored `LibraryClass` so the per-target
/// library-emit pipelines can render them like any other class-shaped
/// artifact. Each function becomes a class-receiver `MethodDef`.
/// Shared home for the helper that had grown identical copies in the
/// Rust and Go emitters.
pub fn module_funcs_to_library_class(
    name: &str,
    funcs: &[crate::dialect::LibraryFunction],
) -> crate::dialect::LibraryClass {
    use crate::dialect::{AccessorKind, LibraryClass, MethodDef, MethodReceiver};
    use crate::ident::ClassId;
    let methods: Vec<MethodDef> = funcs
        .iter()
        .map(|f| MethodDef {
            name: f.name.clone(),
            receiver: MethodReceiver::Class,
            params: f.params.clone(),
            body: f.body.clone(),
            signature: f.signature.clone(),
            effects: f.effects.clone(),
            enclosing_class: Some(crate::ident::Symbol::from(name)),
            kind: AccessorKind::Method,
            is_async: f.is_async,
            mutates_self: false,
            block_param: None,
        })
        .collect();
    LibraryClass {
        name: ClassId(crate::ident::Symbol::from(name)),
        is_module: true,
        parent: None,
        includes: Vec::new(),
        methods,
        nullable_columns: Vec::new(),
        origin: None,
        constants: Vec::new(),
        unknown_calls: Vec::new(),
    }
}

#[cfg(test)]
mod pass_order_tests {
    use super::{post_analyze_pass_order_is_sound, POST_ANALYZE_PASS_ORDER};
    use std::collections::BTreeSet;

    #[test]
    fn post_analyze_pass_order_is_sound_topologically() {
        // Every declared predecessor precedes its dependent and names a
        // real pass in the list.
        assert!(
            post_analyze_pass_order_is_sound(),
            "POST_ANALYZE_PASS_ORDER is not a valid topological order",
        );
    }

    #[test]
    fn post_analyze_pass_names_are_unique() {
        // Names key the ordering constraints, so duplicates would make a
        // `runs_after` reference ambiguous.
        let mut seen = BTreeSet::new();
        for (name, _) in POST_ANALYZE_PASS_ORDER {
            assert!(seen.insert(*name), "duplicate pass name in order table: {name}");
        }
    }
}
