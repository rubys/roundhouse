//! TypeScript emitter — targets Juntos.
//!
//! Started as the Phase 2 scaffold; Phase 3 upgrades the output to
//! Juntos's runtime shape (the committed TS target per the strategy
//! memory). Changes land incrementally so each commit stays small:
//!
//! - **Models** (this commit): `extends ApplicationRecord`, `static
//!   table_name`, `static columns`. One file per model under
//!   `app/models/<snake>.ts`. Schema-derived instance fields drop
//!   from the class body — Juntos materializes them at runtime from
//!   the `columns` list, and declaring them statically would
//!   collide with the runtime accessors.
//! - Validations, associations, broadcasts: separate Phase 3 commits
//!   once this first shape is in place.
//! - Controllers + router + views: later Phase 3 commits.
//!
//! Ruby → Juntos translation rules come from ruby2js's
//! `lib/ruby2js/filter/rails/model.rb` and `lib/ruby2js/filter/rails/
//! active_record.rb`. Those are the reference; our job is to produce
//! equivalent output driven by the typed IR.
//!
//! Non-goals still (later Phase 3 commits):
//! - Controller shape (extends Controller, ivar-style state).
//! - Router emit (Router.resources calls, not a flat table).
//! - View / template emission.
//! - `tsc --strict` cleanliness.

use std::fmt::Write;
use std::path::PathBuf;

use super::EmittedFile;
use crate::App;
// Trait import for the controller walker. Used by TsEmitter's
// helper methods to call `self.render_expr(...)` (the trait
// method) from inside inherent-impl code.
use crate::lower::CtrlWalker as _;
use crate::dialect::{
    Association, Controller, MethodDef, Model, RouteSpec, Test, TestModule,
};
use crate::ident::Symbol;
use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::lower::{BroadcastAction, LoweredBroadcast, LoweredBroadcasts};
use crate::ty::Ty;

/// Hand-written Juntos-shape stub, copied into every generated project
/// as `src/juntos.ts`. tsconfig's `paths` alias rewrites `"juntos"`
/// imports to this file for type-checking without requiring npm
/// install. Real deployments swap in the actual Juntos package.
const JUNTOS_STUB_SOURCE: &str = include_str!("../../runtime/typescript/juntos.ts");

/// TypeScript HTTP runtime — Phase 4c compile-only stubs. Copied
/// verbatim into generated projects as `src/http.ts` when any
/// controller emits. Mirrors the Rust/Crystal/Go/Elixir twins.
const HTTP_STUB_SOURCE: &str = include_str!("../../runtime/typescript/http.ts");

/// Pass-2 test-support runtime. `TestClient` + `TestResponse` with
/// assertion methods mirroring Rust's `TestResponseExt` trait.
const TEST_SUPPORT_SOURCE: &str = include_str!("../../runtime/typescript/test_support.ts");

/// Pass-2 view-helpers runtime. Rails-compatible `linkTo`,
/// `buttonTo`, `formWrap`, `FormBuilder`, `turboStreamFrom`, etc.
const VIEW_HELPERS_SOURCE: &str = include_str!("../../runtime/typescript/view_helpers.ts");

/// HTTP + Action Cable server runtime. Copied into generated
/// projects as `src/server.ts`. Consumed by `main.ts` to start
/// the HTTP listener + WebSocket upgrade handler.
const SERVER_SOURCE: &str = include_str!("../../runtime/typescript/server.ts");

pub fn emit(app: &App) -> Vec<EmittedFile> {
    // Default adapter for backward-compatible callers. Matches
    // pre-adapter-consumption behavior — nothing suspends, no
    // awaits beyond the `async function` wrapper that was already
    // there.
    emit_with_adapter(app, &crate::adapter::SqliteAdapter)
}

/// Emit with an explicit adapter. Async-capable targets (this one,
/// eventually Rust and Python) consult the adapter's
/// `is_suspending_effect` per Send site and insert `await` where
/// effects suspend. `SqliteAdapter` suspends nothing; `SqliteAsync
/// Adapter` suspends on DB effects — emit a fully-awaited body
/// that can later be pointed at a real async backend (IndexedDB,
/// D1, pg-on-Node) without further emitter changes.
pub fn emit_with_adapter(
    app: &App,
    adapter: &dyn crate::adapter::DatabaseAdapter,
) -> Vec<EmittedFile> {
    let mut files = Vec::new();
    files.push(emit_package_json());
    files.push(emit_tsconfig_json(app));
    files.push(EmittedFile {
        path: PathBuf::from("src/juntos.ts"),
        content: JUNTOS_STUB_SOURCE.to_string(),
    });
    if !app.models.is_empty() {
        files.push(emit_schema_sql_ts(app));
    }
    files.extend(emit_models(app));
    if !app.controllers.is_empty() {
        files.push(EmittedFile {
            path: PathBuf::from("src/http.ts"),
            content: HTTP_STUB_SOURCE.to_string(),
        });
        files.push(EmittedFile {
            path: PathBuf::from("src/test_support.ts"),
            content: TEST_SUPPORT_SOURCE.to_string(),
        });
        files.push(EmittedFile {
            path: PathBuf::from("src/view_helpers.ts"),
            content: VIEW_HELPERS_SOURCE.to_string(),
        });
        files.push(EmittedFile {
            path: PathBuf::from("src/server.ts"),
            content: SERVER_SOURCE.to_string(),
        });
        files.push(emit_ts_route_helpers(app));
        // Always emit `src/importmap.ts` — empty PINS list when
        // the app has no `config/importmap.rb` — so the layout's
        // import line never fails to resolve.
        files.push(emit_ts_importmap(app));
        files.extend(emit_controllers(app, adapter));
        // Note: db/seeds.ts emission deferred. The top-level Ruby
        // transpile path (needed for seeds.rb → runnable TS)
        // requires more careful handling than the controller-body
        // emitter provides today: operator methods (`==` → `===`),
        // bang-stripping on class methods (`Article.create!` →
        // `Article.create`), and statement-structure preservation
        // through nested `if`/`unless` guards. See App::seeds for
        // the ingested expression; Ruby emit round-trips
        // correctly. TS emission picks up in a later bite.
        files.push(emit_main_ts(app));
    }
    files.extend(emit_views(app));
    if !app.routes.entries.is_empty() {
        files.push(emit_routes(app));
    }
    if !app.fixtures.is_empty() {
        let lowered = crate::lower::lower_fixtures(app);
        files.push(emit_ts_fixtures_helper(&lowered));
        for f in &lowered.fixtures {
            files.push(emit_ts_fixture(f));
        }
    }
    if !app.test_modules.is_empty() {
        for tm in &app.test_modules {
            files.push(emit_ts_spec(tm, app));
        }
    }
    files
}

/// Minimal package.json. `"type": "module"` matches the ESM import/
/// export style the emitter produces. `@types/node` is required so
/// tsc can resolve `node:test` / `node:assert/strict` imports in the
/// emitted spec files. The tsconfig `paths` alias resolves `"juntos"`
/// to our local stub.
fn emit_package_json() -> EmittedFile {
    let content = "\
{
  \"name\": \"app\",
  \"version\": \"0.1.0\",
  \"private\": true,
  \"type\": \"module\",
  \"scripts\": {
    \"start\": \"tsx main.ts\"
  },
  \"dependencies\": {
    \"better-sqlite3\": \"^11.5.0\",
    \"ws\": \"^8.18.0\"
  },
  \"devDependencies\": {
    \"@types/node\": \"^20\",
    \"@types/better-sqlite3\": \"^7.6.0\",
    \"@types/ws\": \"^8.5.0\",
    \"typescript\": \"5.7.3\",
    \"tsx\": \"4.19.2\"
  }
}
";
    EmittedFile {
        path: PathBuf::from("package.json"),
        content: content.to_string(),
    }
}

/// `src/schema_sql.ts` — TypeScript constant wrapping the target-neutral
/// DDL produced by `lower::lower_schema`. `setupTestDb` executes the
/// string via `better-sqlite3`'s `Database#exec` (which handles
/// multi-statement batches natively).
fn emit_schema_sql_ts(app: &App) -> EmittedFile {
    let ddl = crate::lower::lower_schema(&app.schema);
    let mut s = String::new();
    writeln!(s, "// Generated by Roundhouse.").unwrap();
    writeln!(s, "export const CREATE_TABLES = `").unwrap();
    s.push_str(&ddl);
    writeln!(s, "`;").unwrap();
    EmittedFile {
        path: PathBuf::from("src/schema_sql.ts"),
        content: s,
    }
}

/// tsconfig.json — strict TS with the two bits that matter for the
/// generated shape: `paths` maps `"juntos"` to the local stub, and
/// `allowJs`/`esModuleInterop` let imports in both styles resolve.
/// As of Phase 4c controllers + http runtime land in the include list
/// since they compile against the `Roundhouse.Http` stubs; views and
/// routes still wait for their own runtime.
fn emit_tsconfig_json(app: &App) -> EmittedFile {
    let mut includes = String::from("\"app/models/**/*.ts\", \"src/juntos.ts\"");
    if !app.models.is_empty() {
        includes.push_str(", \"src/schema_sql.ts\"");
    }
    if !app.controllers.is_empty() {
        includes.push_str(
            ", \"app/controllers/**/*.ts\", \"app/views/**/*.ts\", \"src/http.ts\", \"src/test_support.ts\", \"src/view_helpers.ts\", \"src/route_helpers.ts\", \"src/routes.ts\", \"src/server.ts\", \"main.ts\"",
        );
    }
    if !app.test_modules.is_empty() || !app.fixtures.is_empty() {
        includes.push_str(", \"spec/**/*.ts\"");
    }
    let content = format!(
        "{{
  \"compilerOptions\": {{
    \"target\": \"ES2022\",
    \"module\": \"ESNext\",
    \"moduleResolution\": \"bundler\",
    \"strict\": false,
    \"esModuleInterop\": true,
    \"skipLibCheck\": true,
    \"noEmit\": true,
    \"baseUrl\": \".\",
    \"paths\": {{
      \"juntos\": [\"./src/juntos.ts\"]
    }}
  }},
  \"include\": [{includes}]
}}
"
    );
    EmittedFile {
        path: PathBuf::from("tsconfig.json"),
        content,
    }
}

// Models ---------------------------------------------------------------

/// Emit one `app/models/<snake>.ts` per model. Juntos imports each
/// model as its own module; a flat `models.ts` bundle would make
/// circular-import resolution harder and doesn't match the Rails-
/// per-file convention.
fn emit_models(app: &App) -> Vec<EmittedFile> {
    app.models.iter().map(emit_model_file).collect()
}

fn emit_model_file(model: &Model) -> EmittedFile {
    let name = model.name.0.as_str();
    let file_stem = crate::naming::snake_case(name);
    let mut s = String::new();
    writeln!(s, "// Generated by Roundhouse.").unwrap();
    // Normalize parent references for TS emission:
    // `ActiveRecord::Base` (Rails' root) → `ActiveRecord` (the Juntos
    // import), since `::` is a Ruby path separator, not valid JS.
    // Anything else keeps its IR spelling.
    let parent = model
        .parent
        .as_ref()
        .map(|p| {
            let name = p.0.as_str();
            if name == "ActiveRecord::Base" {
                "ActiveRecord".to_string()
            } else {
                name.to_string()
            }
        })
        .unwrap_or_else(|| "ApplicationRecord".to_string());

    // Import list: the parent itself (so `extends` resolves) plus the
    // proxy / registry symbols any associations on this model need.
    // Using the actual parent rather than hard-coding ApplicationRecord
    // avoids a self-import collision on `application_record.ts` (which
    // defines its own `export class ApplicationRecord extends
    // ActiveRecord`).
    let mut imports: Vec<&str> = vec![parent.as_str()];
    let has_many = model.associations().any(|a| matches!(a, Association::HasMany { .. }));
    let has_one = model.associations().any(|a| matches!(a, Association::HasOne { .. }));
    let has_belongs_to = model
        .associations()
        .any(|a| matches!(a, Association::BelongsTo { .. }));
    // Every model registers itself in modelRegistry for cross-model
    // lookups (belongsToChecks / dependentChildren / CollectionProxy
    // targets), so the import is universal now.
    let _unused_existing_gate = has_many || has_one || has_belongs_to;
    let uses_registry = true;
    if has_many {
        imports.push("CollectionProxy");
    }
    if has_belongs_to {
        imports.push("Reference");
    }
    if has_one {
        imports.push("HasOneReference");
    }
    if uses_registry {
        imports.push("modelRegistry");
    }
    writeln!(
        s,
        "import {{ {} }} from \"juntos\";",
        imports.join(", ")
    )
    .unwrap();
    writeln!(s).unwrap();

    writeln!(s, "export class {name} extends {parent} {{").unwrap();

    let table = model.table.0.as_str();
    writeln!(s, "  static table_name = {table:?};").unwrap();

    // Juntos discovers columns from this static — omitting the `id`
    // primary key since it's universal (matches the ruby2js filter's
    // behavior: columns listed are the non-id schema columns).
    let columns: Vec<String> = model
        .attributes
        .fields
        .keys()
        .filter(|k| k.as_str() != "id")
        .map(|k| format!("{:?}", k.as_str()))
        .collect();
    if columns.is_empty() {
        writeln!(s, "  static columns: string[] = [];").unwrap();
    } else {
        writeln!(s, "  static columns = [{}];", columns.join(", ")).unwrap();
    }

    // belongs_to / has_many(dependent: destroy) metadata drives
    // Juntos's persistence layer — the runtime reads these arrays
    // when `save` or `destroy` needs to validate FK existence or
    // cascade deletes. Target classes are stored as names and looked
    // up through modelRegistry at call time so circular imports
    // don't initialize into `undefined`.
    // Instance-field defaults so `new Article()` has a concrete
    // value for every column. SQLite's NOT NULL constraints reject
    // `undefined`-to-null conversions, and the fixture harness
    // calls `save` before every column is explicitly set.
    let field_defaults: Vec<(String, String, String)> = model
        .attributes
        .fields
        .iter()
        .filter(|(k, _)| k.as_str() != "id")
        .map(|(k, ty)| {
            let name = k.as_str().to_string();
            let (type_ann, default) = ts_field_type_and_default(ty);
            (name, type_ann, default)
        })
        .collect();
    for (name, ty, default) in &field_defaults {
        writeln!(s, "  {name}: {ty} = {default};").unwrap();
    }

    let bt_checks: Vec<String> = model
        .associations()
        .filter_map(|a| match a {
            Association::BelongsTo { foreign_key, target, optional: false, .. } => Some(
                format!(
                    "    {{ fk: {:?}, targetName: {:?} }}",
                    foreign_key.as_str(),
                    target.0.as_str(),
                ),
            ),
            _ => None,
        })
        .collect();
    if !bt_checks.is_empty() {
        writeln!(s, "  static belongsToChecks = [").unwrap();
        writeln!(s, "{}", bt_checks.join(",\n")).unwrap();
        writeln!(s, "  ];").unwrap();
    }

    let dep_children: Vec<String> = model
        .associations()
        .filter_map(|a| match a {
            Association::HasMany {
                foreign_key,
                target,
                dependent: crate::dialect::Dependent::Destroy,
                ..
            } => Some(format!(
                "    {{ fk: {:?}, targetName: {:?} }}",
                foreign_key.as_str(),
                target.0.as_str(),
            )),
            _ => None,
        })
        .collect();
    if !dep_children.is_empty() {
        writeln!(s, "  static dependentChildren = [").unwrap();
        writeln!(s, "{}", dep_children.join(",\n")).unwrap();
        writeln!(s, "  ];").unwrap();
    }

    // Associations: one getter per association. Returns a
    // CollectionProxy (has_many), Reference (belongs_to), or
    // HasOneReference (has_one). Juntos's proxies lazily resolve
    // the target model through modelRegistry, which sidesteps
    // circular-import issues across model files.
    for assoc in model.associations() {
        writeln!(s).unwrap();
        emit_association(&mut s, assoc);
    }

    // Validations: consume the Phase 4 lowered form. One
    // `this.validates_<kind>_of(...)` line per atomic `Check` — Juntos
    // has a primitive for every variant, so the TS render is a direct
    // mapping from `Check` to Juntos's runtime method. Compound source
    // rules (`Length { min, max }`) were already fanned out into
    // separate checks by the lowering, so we never see them as a unit
    // here.
    let lowered = crate::lower::lower_validations(model);
    if !lowered.is_empty() {
        writeln!(s).unwrap();
        writeln!(s, "  validate() {{").unwrap();
        for lv in &lowered {
            for check in &lv.checks {
                if let Some(call) = emit_juntos_validate_call(lv.attribute.as_str(), check) {
                    writeln!(s, "    {call};").unwrap();
                }
            }
        }
        writeln!(s, "  }}").unwrap();
    }

    for method in model.methods() {
        writeln!(s).unwrap();
        emit_model_method(&mut s, method, model);
    }
    writeln!(s, "}}").unwrap();

    // Broadcast callbacks land after the class body. Juntos's
    // Turbo-Streams integration registers them as `Model.afterSave`
    // / `Model.afterDestroy` hooks so they fire on every persist.
    // Parsing is shared with the Rust emitter via
    // `lower::lower_broadcasts` — this file only renders the
    // lowered form as JS registrations.
    let broadcast_lines = render_broadcast_registrations(model);
    if !broadcast_lines.is_empty() {
        writeln!(s).unwrap();
        for line in broadcast_lines {
            writeln!(s, "{line}").unwrap();
        }
    }

    // Register this class in modelRegistry so cross-model metadata
    // (belongsToChecks, dependentChildren, CollectionProxy targets)
    // can resolve by name at call time. Register line needs the
    // `modelRegistry` import; add it if not already imported.
    writeln!(s).unwrap();
    writeln!(s, "modelRegistry[{name:?}] = {name};").unwrap();

    EmittedFile {
        path: PathBuf::from(format!("app/models/{file_stem}.ts")),
        content: s,
    }
}

/// Render a model's lowered broadcast declarations as Juntos
/// `Model.afterSave` / `Model.afterDestroy` registration lines. The
/// parsing (broadcasts_to, after_{create,destroy}_commit, rescue nil
/// unwrapping, belongs_to resolution) lives in
/// `lower::lower_broadcasts`; this function is purely target
/// rendering.
fn render_broadcast_registrations(model: &Model) -> Vec<String> {
    let lowered = crate::lower::lower_broadcasts(model);
    if lowered.is_empty() {
        return Vec::new();
    }
    let model_name = model.name.0.as_str();
    let mut lines = Vec::new();
    for b in &lowered.save {
        lines.push(render_broadcast_registration(
            model_name,
            "afterSave",
            b,
            &lowered,
        ));
    }
    for b in &lowered.destroy {
        lines.push(render_broadcast_registration(
            model_name,
            "afterDestroy",
            b,
            &lowered,
        ));
    }
    lines
}

/// Render one lowered broadcast as a `Model.afterX((record) =>
/// record.broadcastYTo(...))` line. Association-form broadcasts
/// (after_*_commit on a parent) resolve the parent via
/// `modelRegistry` and fire on the reference, matching
/// juntos.ts's Reference shape.
fn render_broadcast_registration(
    model_name: &str,
    hook: &str,
    b: &LoweredBroadcast,
    _all: &LoweredBroadcasts,
) -> String {
    let channel_js = render_broadcast_channel(&b.channel, b.self_param.as_ref());
    let target_js = b
        .target
        .as_ref()
        .map(|t| render_broadcast_channel(t, b.self_param.as_ref()));
    let method = ts_broadcast_method(b.action);

    if let Some(assoc) = &b.on_association {
        // `record.<fk>` is our way of reaching the foreign-key attr;
        // `modelRegistry.<Target>` resolves the target class at call
        // time (avoids a file-level import cycle between models).
        let var = assoc.name.as_str();
        let target_class = assoc.target_class.as_str();
        let fk = assoc.foreign_key.as_str();
        let call = render_broadcast_call(method, &channel_js, target_js.as_deref(), var);
        return format!(
            "{model_name}.{hook}((record) => {{ const {var} = modelRegistry.{target_class}?.find(record.{fk}); if ({var}) {call}; }});"
        );
    }

    let call = render_broadcast_call(method, &channel_js, target_js.as_deref(), "record");
    format!("{model_name}.{hook}((record) => {call});")
}

/// Render the broadcast method call as an expression (no trailing
/// semicolon) — callers insert `;` when they need a statement.
fn render_broadcast_call(
    method: &str,
    channel_js: &str,
    target_js: Option<&str>,
    receiver: &str,
) -> String {
    match (method, target_js) {
        ("broadcastRemoveTo", _) => {
            format!("{receiver}.broadcastRemoveTo({channel_js})")
        }
        (m, Some(t)) => format!("{receiver}.{m}({channel_js}, {{ target: {t} }})"),
        (m, None) => format!("{receiver}.{m}({channel_js})"),
    }
}

fn ts_broadcast_method(action: BroadcastAction) -> &'static str {
    match action {
        BroadcastAction::Prepend => "broadcastPrependTo",
        BroadcastAction::Append => "broadcastAppendTo",
        BroadcastAction::Replace => "broadcastReplaceTo",
        BroadcastAction::Remove => "broadcastRemoveTo",
    }
}

/// Render a broadcast channel/target expression as a JS string (or
/// string-producing expression). Applies the `broadcasts_to` lambda
/// param rewrite — `comment.article_id` becomes `record.article_id`
/// — so the enclosing callback's `record` parameter is the receiver.
fn render_broadcast_channel(expr: &Expr, self_param: Option<&Symbol>) -> String {
    let raw = match &*expr.node {
        ExprNode::Lit { value: Literal::Str { value } } => format!("{value:?}"),
        _ => emit_expr(expr),
    };
    let Some(p) = self_param else { return raw };
    rewrite_record_refs(&raw, Some(p.as_str()))
}

/// If the lambda param is `_article` or similar, the Juntos callback's
/// parameter is conventionally named `record`. Rewrite bare references
/// in the emitted text. `_`-prefixed idents (Ruby's "unused param"
/// convention) get included — we strip the underscore too so
/// `_article.foo` and `article.foo` both map to `record.foo`.
fn rewrite_record_refs(js: &str, param_name: Option<&str>) -> String {
    let Some(orig) = param_name else { return js.to_string() };
    // Consider both the raw param name and its underscore-stripped
    // form, since Ruby's `->(_article) { … }` can reference either
    // spelling in the body (the stripped one is unusual but valid).
    let stripped = orig.strip_prefix('_').unwrap_or(orig).to_string();
    let mut out = replace_word(js, orig, "record");
    if stripped != orig {
        out = replace_word(&out, &stripped, "record");
    }
    out
}

/// Replace every occurrence of `needle` in `haystack` where the match
/// is surrounded by non-identifier characters. Avoids mangling
/// `articles` when replacing `article`. No regex dependency.
fn replace_word(haystack: &str, needle: &str, replacement: &str) -> String {
    if needle.is_empty() {
        return haystack.to_string();
    }
    let bytes = haystack.as_bytes();
    let n = needle.len();
    let mut out = String::with_capacity(haystack.len());
    let mut i = 0;
    while i + n <= bytes.len() {
        if &bytes[i..i + n] == needle.as_bytes() {
            let prev_ok = i == 0 || !is_ident_byte(bytes[i - 1]);
            let next_ok = i + n == bytes.len() || !is_ident_byte(bytes[i + n]);
            if prev_ok && next_ok {
                out.push_str(replacement);
                i += n;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    if i < bytes.len() {
        out.push_str(&haystack[i..]);
    }
    out
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$'
}

/// Emit a Juntos-shaped association getter. Each kind produces a
/// `get <name>()` that constructs the appropriate proxy object; the
/// target class is looked up lazily through `modelRegistry` so model
/// files don't need to import each other.
fn emit_association(out: &mut String, assoc: &Association) {
    match assoc {
        Association::HasMany { name, target, foreign_key, .. } => {
            writeln!(out, "  get {name}() {{").unwrap();
            writeln!(out, "    return new CollectionProxy(this, {{").unwrap();
            writeln!(out, "      name: {:?},", name.as_str()).unwrap();
            writeln!(out, "      type: \"has_many\",").unwrap();
            writeln!(out, "      foreignKey: {:?}", foreign_key.as_str()).unwrap();
            writeln!(out, "    }}, modelRegistry.{});", target.0).unwrap();
            writeln!(out, "  }}").unwrap();
        }
        Association::HasOne { name, target, foreign_key, .. } => {
            writeln!(out, "  get {name}() {{").unwrap();
            writeln!(out, "    return new HasOneReference(this, {{").unwrap();
            writeln!(out, "      name: {:?},", name.as_str()).unwrap();
            writeln!(out, "      type: \"has_one\",").unwrap();
            writeln!(out, "      foreignKey: {:?}", foreign_key.as_str()).unwrap();
            writeln!(out, "    }}, modelRegistry.{});", target.0).unwrap();
            writeln!(out, "  }}").unwrap();
        }
        Association::BelongsTo { name, target, foreign_key, optional } => {
            writeln!(out, "  get {name}() {{").unwrap();
            let fk_access = format!("this.attributes.{}", foreign_key.as_str());
            if *optional {
                // Optional belongs_to returns null when the FK is
                // unset. Matches Rails' `belongs_to :x, optional: true`.
                writeln!(
                    out,
                    "    return {fk_access} ? new Reference(modelRegistry.{}, {fk_access}) : null;",
                    target.0
                )
                .unwrap();
            } else {
                writeln!(
                    out,
                    "    return new Reference(modelRegistry.{}, {fk_access});",
                    target.0
                )
                .unwrap();
            }
            writeln!(out, "  }}").unwrap();
        }
        Association::HasAndBelongsToMany { name, target, join_table } => {
            // Juntos doesn't have a dedicated HABTM proxy class in the
            // reference material we've seen; emit a CollectionProxy
            // with a HABTM type tag plus the join-table name. Worst
            // case the Juntos runtime errors loudly — no fixture
            // exercises this yet.
            writeln!(out, "  get {name}() {{").unwrap();
            writeln!(out, "    return new CollectionProxy(this, {{").unwrap();
            writeln!(out, "      name: {:?},", name.as_str()).unwrap();
            writeln!(out, "      type: \"has_and_belongs_to_many\",").unwrap();
            writeln!(out, "      joinTable: {:?}", join_table.as_str()).unwrap();
            writeln!(out, "    }}, modelRegistry.{});", target.0).unwrap();
            writeln!(out, "  }}").unwrap();
        }
    }
}

/// Render one lowered `Check` as its Juntos-primitive call. Compound
/// source rules already fanned out during lowering — `MinLength` /
/// `MaxLength` are their own checks here, not a combined `Length`. TS
/// still calls Juntos's original `validates_length_of` primitive
/// (which takes `{minimum, maximum}`) so the two checks on the same
/// attribute emit as two calls with one bound each, which Juntos
/// accumulates into a single validation result.
fn emit_juntos_validate_call(attr: &str, check: &crate::lower::Check) -> Option<String> {
    use crate::lower::Check;
    let attr_lit = format!("{attr:?}");
    Some(match check {
        Check::Presence => format!("this.validates_presence_of({attr_lit})"),
        Check::Absence => format!("this.validates_absence_of({attr_lit})"),
        Check::MinLength { n } => {
            format!("this.validates_length_of({attr_lit}, {{minimum: {n}}})")
        }
        Check::MaxLength { n } => {
            format!("this.validates_length_of({attr_lit}, {{maximum: {n}}})")
        }
        Check::GreaterThan { threshold } => format!(
            "this.validates_numericality_of({attr_lit}, {{greater_than: {threshold}}})"
        ),
        Check::LessThan { threshold } => format!(
            "this.validates_numericality_of({attr_lit}, {{less_than: {threshold}}})"
        ),
        Check::OnlyInteger => format!(
            "this.validates_numericality_of({attr_lit}, {{only_integer: true}})"
        ),
        Check::Inclusion { values } => {
            let parts: Vec<String> = values.iter().map(inclusion_value_to_js).collect();
            format!(
                "this.validates_inclusion_of({attr_lit}, {{in: [{}]}})",
                parts.join(", ")
            )
        }
        Check::Format { pattern } => format!(
            "this.validates_format_of({attr_lit}, {{with: /{pattern}/}})"
        ),
        Check::Uniqueness { scope, case_sensitive } => {
            let mut opts = Vec::new();
            if !scope.is_empty() {
                let parts: Vec<String> =
                    scope.iter().map(|s| format!("{:?}", s.as_str())).collect();
                opts.push(format!("scope: [{}]", parts.join(", ")));
            }
            if !*case_sensitive {
                opts.push("case_sensitive: false".to_string());
            }
            if opts.is_empty() {
                format!("this.validates_uniqueness_of({attr_lit})")
            } else {
                format!(
                    "this.validates_uniqueness_of({attr_lit}, {{{}}})",
                    opts.join(", ")
                )
            }
        }
        Check::Custom { method } => {
            // `validate :method_name` — direct method call, not one of
            // Juntos's `validates_<kind>_of` primitives. The method
            // is expected to populate `this.errors` itself.
            format!("this.{method}()")
        }
    })
}

/// Render an `InclusionValue` as a JS literal. Strings use TS double-
/// quote escaping via `Debug`; numbers emit as-is; bools as `true`/
/// `false`.
fn inclusion_value_to_js(v: &crate::lower::InclusionValue) -> String {
    use crate::lower::InclusionValue;
    match v {
        InclusionValue::Str { value } => format!("{value:?}"),
        InclusionValue::Int { value } => value.to_string(),
        InclusionValue::Float { value } => {
            let s = value.to_string();
            if s.contains('.') { s } else { format!("{s}.0") }
        }
        InclusionValue::Bool { value } => value.to_string(),
    }
}

fn emit_model_method(out: &mut String, m: &MethodDef, model: &Model) {
    let name = ts_method_name(m.name.as_str());
    let ret = m.body.ty.clone().unwrap_or(Ty::Nil);
    let ret_annot = if matches!(ret, Ty::Nil) {
        ": void".to_string()
    } else {
        format!(": {}", ts_ty(&ret))
    };
    let is_static = matches!(m.receiver, crate::dialect::MethodReceiver::Class);
    let static_prefix = if is_static { "static " } else { "" };
    writeln!(out, "  {static_prefix}{name}(){ret_annot} {{").unwrap();
    // Ruby's implicit-self `title` becomes TS's `this.title`. Pre-emit
    // rewrite: turn bare-name Sends (no recv, no args) matching an
    // attribute of the enclosing model into Ivar reads — the existing
    // Ivar emission renders them as `this.<field>`.
    let attrs: Vec<crate::ident::Symbol> =
        model.attributes.fields.keys().cloned().collect();
    let rewritten = rewrite_bare_attrs_to_ivars(&m.body, &attrs);
    let body_text = emit_body(&rewritten, &ret);
    for line in body_text.lines() {
        writeln!(out, "    {line}").unwrap();
    }
    writeln!(out, "  }}").unwrap();
}

/// Deep-clone an Expr, rewriting every bare-name Send whose method
/// matches one of `attrs` into an Ivar read with the same name.
/// Callers use this on model method bodies so Ruby's implicit-self
/// `title` renders as TS's `this.title`.
fn rewrite_bare_attrs_to_ivars(
    e: &Expr,
    attrs: &[crate::ident::Symbol],
) -> Expr {
    use crate::expr::{Arm, InterpPart, Pattern};
    let rewrite = |child: &Expr| rewrite_bare_attrs_to_ivars(child, attrs);
    let new_node = match &*e.node {
        ExprNode::Send { recv: None, method, args, block: None, .. }
            if args.is_empty() && attrs.iter().any(|s| s == method) =>
        {
            ExprNode::Ivar { name: method.clone() }
        }
        ExprNode::Send { recv, method, args, block, parenthesized } => ExprNode::Send {
            recv: recv.as_ref().map(&rewrite),
            method: method.clone(),
            args: args.iter().map(&rewrite).collect(),
            block: block.as_ref().map(&rewrite),
            parenthesized: *parenthesized,
        },
        ExprNode::Seq { exprs } => ExprNode::Seq {
            exprs: exprs.iter().map(&rewrite).collect(),
        },
        ExprNode::Array { elements, style } => ExprNode::Array {
            elements: elements.iter().map(&rewrite).collect(),
            style: *style,
        },
        ExprNode::Hash { entries, braced } => ExprNode::Hash {
            entries: entries
                .iter()
                .map(|(k, v)| (rewrite(k), rewrite(v)))
                .collect(),
            braced: *braced,
        },
        ExprNode::If { cond, then_branch, else_branch } => ExprNode::If {
            cond: rewrite(cond),
            then_branch: rewrite(then_branch),
            else_branch: rewrite(else_branch),
        },
        ExprNode::Case { scrutinee, arms } => ExprNode::Case {
            scrutinee: rewrite(scrutinee),
            arms: arms
                .iter()
                .map(|arm| Arm {
                    pattern: arm.pattern.clone(),
                    guard: arm.guard.as_ref().map(&rewrite),
                    body: rewrite(&arm.body),
                })
                .collect(),
        },
        ExprNode::BoolOp { op, surface, left, right } => ExprNode::BoolOp {
            op: *op,
            surface: *surface,
            left: rewrite(left),
            right: rewrite(right),
        },
        ExprNode::StringInterp { parts } => ExprNode::StringInterp {
            parts: parts
                .iter()
                .map(|p| match p {
                    InterpPart::Text { value } => InterpPart::Text { value: value.clone() },
                    InterpPart::Expr { expr } => InterpPart::Expr { expr: rewrite(expr) },
                })
                .collect(),
        },
        ExprNode::Let { id, name, value, body } => ExprNode::Let {
            id: *id,
            name: name.clone(),
            value: rewrite(value),
            body: rewrite(body),
        },
        ExprNode::Lambda { params, block_param, body, block_style } => ExprNode::Lambda {
            params: params.clone(),
            block_param: block_param.clone(),
            body: rewrite(body),
            block_style: *block_style,
        },
        ExprNode::Apply { fun, args, block } => ExprNode::Apply {
            fun: rewrite(fun),
            args: args.iter().map(&rewrite).collect(),
            block: block.as_ref().map(&rewrite),
        },
        ExprNode::Assign { target, value } => {
            let new_target = match target {
                LValue::Var { id, name } => LValue::Var { id: *id, name: name.clone() },
                LValue::Ivar { name } => LValue::Ivar { name: name.clone() },
                LValue::Attr { recv, name } => LValue::Attr {
                    recv: rewrite(recv),
                    name: name.clone(),
                },
                LValue::Index { recv, index } => LValue::Index {
                    recv: rewrite(recv),
                    index: rewrite(index),
                },
            };
            ExprNode::Assign {
                target: new_target,
                value: rewrite(value),
            }
        }
        ExprNode::Yield { args } => ExprNode::Yield {
            args: args.iter().map(&rewrite).collect(),
        },
        ExprNode::Raise { value } => ExprNode::Raise { value: rewrite(value) },
        ExprNode::RescueModifier { expr, fallback } => ExprNode::RescueModifier {
            expr: rewrite(expr),
            fallback: rewrite(fallback),
        },
        // Leaves (no subexpressions): clone unchanged.
        ExprNode::Lit { .. }
        | ExprNode::Var { .. }
        | ExprNode::Ivar { .. }
        | ExprNode::Const { .. } => (*e.node).clone(),
    };
    let _ = Pattern::Wildcard; // keep Pattern import live in case future case-arm edits need it
    Expr {
        span: e.span,
        node: Box::new(new_node),
        ty: e.ty.clone(),
        effects: e.effects.clone(),
        leading_blank_line: e.leading_blank_line,
    }
}

// Controllers ----------------------------------------------------------
//
// Juntos controllers are modules of exported async functions, not
// classes. Each action becomes `export async function name(context)`
// (read actions) or `export async function name(context, params)`
// (write actions — `create` / `update`). Rails instance variables
// (`@foo = ...`) rebind to local `let`s; `params[:key]` rewrites to
// `context.params.<key>`. Matches ruby2js's
// `lib/ruby2js/filter/rails/controller.rb` shape.
//
// Phase 4c: action bodies emit through the shared SendKind classifier
// from `lower::controller`. The TS render table handles HTTP surface
// (`render`/`redirectTo`/`head`), `respondTo` block + FormatRouter,
// `params` access, model / association shapes, and the BangStrip
// variant (TS forbids `!` in idents just like Rust and Go).

/// Emit `main.ts` — the Node entry point. Imports models
/// (triggering self-registration in `modelRegistry`), routes
/// (triggering `Router.resources` / `Router.root` registration),
/// the schema SQL + seeds, then calls `startServer` to boot HTTP
/// + Action Cable. Mirrors railcar's app.ts shape, adapted to
/// roundhouse's Router / schema / seeds surfaces.
fn emit_main_ts(app: &App) -> EmittedFile {
    let mut s = String::new();
    writeln!(s, "// Generated by Roundhouse.").unwrap();
    writeln!(s).unwrap();
    writeln!(s, "import {{ startServer }} from \"./src/server.js\";").unwrap();
    if !app.models.is_empty() {
        writeln!(s, "import {{ CREATE_TABLES }} from \"./src/schema_sql.js\";").unwrap();
    }
    // Import the emitted layout + view barrel so `main.ts` can
    // hand the dispatcher a real layout renderer. The layout reads
    // from view_helpers' module-level yield/slots state — no args
    // needed at the call site. Only wired when the app actually
    // has a `layouts/application` view (skipped for tiny-blog and
    // similar minimal fixtures without a layout ERB).
    let has_app_layout = app
        .views
        .iter()
        .any(|v| v.name.as_str() == "layouts/application");
    if has_app_layout {
        writeln!(s, "import {{ renderLayoutsApplication }} from \"./app/views/layouts/application.html.js\";").unwrap();
    }
    writeln!(s, "import \"./src/routes.js\";").unwrap();
    // Import each model so its class-body side effects (registry
    // self-registration, broadcast callback registrations) run.
    for model in &app.models {
        let file = crate::naming::snake_case(model.name.0.as_str());
        writeln!(
            s,
            "import \"./app/models/{file}.js\";",
        )
        .unwrap();
    }
    writeln!(s).unwrap();
    writeln!(s, "async function main(): Promise<void> {{").unwrap();
    writeln!(s, "  await startServer({{").unwrap();
    if !app.models.is_empty() {
        writeln!(s, "    schemaSql: CREATE_TABLES,").unwrap();
    } else {
        writeln!(s, "    schemaSql: \"\",").unwrap();
    }
    if has_app_layout {
        writeln!(s, "    layout: () => renderLayoutsApplication(undefined as any),").unwrap();
    }
    // seeds wiring deferred — App::seeds is ingested but TS
    // emission of top-level Ruby scripts needs operator + bang +
    // statement-structure improvements first.
    writeln!(s, "  }});").unwrap();
    writeln!(s, "}}").unwrap();
    writeln!(s).unwrap();
    writeln!(s, "main().catch((err) => {{").unwrap();
    writeln!(s, "  console.error(err);").unwrap();
    writeln!(s, "  process.exit(1);").unwrap();
    writeln!(s, "}});").unwrap();
    EmittedFile {
        path: PathBuf::from("main.ts"),
        content: s,
    }
}

// `emit_seeds_ts` — deferred. See note in emit_with_adapter re:
// the top-level Ruby transpile gaps (operator methods, bang-
// stripping on class methods, statement preservation inside
// nested `if` guards) that need addressing before seeds.ts
// emission is correct.

/// Emit `src/route_helpers.ts` — one `export function <as_name>Path
/// (args)` per unique route `as_name`, derived from the flattened
/// route table. Mirrors `src/route_helpers.rs`'s shape, camelCased.
/// Emit `src/importmap.ts` — the app-specific pin list ingested
/// from `config/importmap.rb` (with `pin_all_from` expanded).
/// Exported as a `readonly` tuple array so the layout's
/// `javascript_importmap_tags` call can pass it straight into the
/// runtime helper without re-parsing.
fn emit_ts_importmap(app: &App) -> EmittedFile {
    let mut s = String::new();
    writeln!(s, "// Generated by Roundhouse.").unwrap();
    writeln!(s).unwrap();
    writeln!(
        s,
        "export const PINS: ReadonlyArray<readonly [string, string]> = ["
    )
    .unwrap();
    if let Some(importmap) = &app.importmap {
        for pin in &importmap.pins {
            writeln!(
                s,
                "  [{:?}, {:?}],",
                pin.name, pin.path,
            )
            .unwrap();
        }
    }
    writeln!(s, "];").unwrap();
    EmittedFile {
        path: PathBuf::from("src/importmap.ts"),
        content: s,
    }
}

fn emit_ts_route_helpers(app: &App) -> EmittedFile {
    let flat = crate::lower::flatten_routes(app);
    let mut s = String::new();
    writeln!(s, "// Generated by Roundhouse.").unwrap();
    writeln!(s).unwrap();
    use std::collections::BTreeSet;
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for route in &flat {
        if !seen.insert(route.as_name.clone()) {
            continue;
        }
        let fn_name = format!("{}Path", crate::naming::camelize(&route.as_name));
        // Strip trailing `_Path` camelization oddity — `camelize(
        // "articles")` = "Articles", then we add "Path" to get
        // "ArticlesPath". We want camelCase, so lowercase the first
        // char.
        let fn_name = lower_first_char(&fn_name);
        let sig_params: Vec<String> = route
            .path_params
            .iter()
            .map(|p| format!("{p}: number"))
            .collect();
        let sig = sig_params.join(", ");
        let body = if route.path_params.is_empty() {
            format!("return {:?};", route.path)
        } else {
            let mut tmpl = String::new();
            let mut chars = route.path.chars().peekable();
            while let Some(c) = chars.next() {
                if c == ':' {
                    let mut ident = String::new();
                    while let Some(&nc) = chars.peek() {
                        if nc.is_alphanumeric() || nc == '_' {
                            ident.push(nc);
                            chars.next();
                        } else {
                            break;
                        }
                    }
                    tmpl.push_str("${");
                    tmpl.push_str(&ident);
                    tmpl.push('}');
                } else {
                    tmpl.push(c);
                }
            }
            format!("return `{tmpl}`;")
        };
        writeln!(s, "export function {fn_name}({sig}): string {{ {body} }}").unwrap();
    }
    EmittedFile {
        path: PathBuf::from("src/route_helpers.ts"),
        content: s,
    }
}

/// Compose one modifier onto the running TS expression. `all` +
/// no-op modifiers (includes, preload, joins, distinct, select)
/// pass through. `order` appends a `.sort(...)` with a comparator
/// derived from the modifier's `{field: :dir}` hash. Unknown
/// modifiers drop (returning prev unchanged) so the output still
/// compiles — a compare-tool divergence will signal the gap.
///
/// Chain-walk lives in `src/lower/chain.rs`; this fn just renders
/// one already-classified layer.
fn apply_ts_chain_modifier(prev: String, m: crate::lower::ChainModifier<'_>) -> String {
    match m.method {
        "all" | "includes" | "preload" | "joins" | "distinct" | "select" => prev,
        "order" => {
            let Some(hash) = m.args.first() else { return prev };
            let ExprNode::Hash { entries, .. } = &*hash.node else { return prev };
            let Some((k, v)) = entries.first() else { return prev };
            let field = match &*k.node {
                ExprNode::Lit { value: Literal::Sym { value } } => value.as_str().to_string(),
                ExprNode::Lit { value: Literal::Str { value } } => value.clone(),
                _ => return prev,
            };
            let dir = match &*v.node {
                ExprNode::Lit { value: Literal::Sym { value } } => value.as_str().to_string(),
                ExprNode::Lit { value: Literal::Str { value } } => value.clone(),
                _ => "asc".to_string(),
            };
            let cmp = if dir == "desc" {
                format!(
                    "(a: any, b: any) => (a.{field} < b.{field} ? 1 : a.{field} > b.{field} ? -1 : 0)"
                )
            } else {
                format!(
                    "(a: any, b: any) => (a.{field} < b.{field} ? -1 : a.{field} > b.{field} ? 1 : 0)"
                )
            };
            format!("{prev}.sort({cmp})")
        }
        "limit" => {
            let Some(n) = m.args.first() else { return prev };
            if let ExprNode::Lit { value: Literal::Int { value } } = &*n.node {
                return format!("{prev}.slice(0, {value})");
            }
            prev
        }
        _ => prev,
    }
}

fn lower_first_char(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c.to_ascii_lowercase().to_string() + chars.as_str(),
        None => String::new(),
    }
}


fn emit_controllers(
    app: &App,
    adapter: &dyn crate::adapter::DatabaseAdapter,
) -> Vec<EmittedFile> {
    let known_models: Vec<Symbol> =
        app.models.iter().map(|m| m.name.0.clone()).collect();
    app.controllers
        .iter()
        .map(|c| emit_controller_file(c, &known_models, app, adapter))
        .collect()
}

fn emit_controller_file(
    c: &Controller,
    known_models: &[Symbol],
    app: &App,
    adapter: &dyn crate::adapter::DatabaseAdapter,
) -> EmittedFile {
    let name = c.name.0.as_str();
    let file_stem = crate::naming::snake_case(name);
    let resource = crate::lower::resource_from_controller_name(name);
    let model_class = crate::naming::singularize_camelize(&resource);
    let has_model = known_models.iter().any(|m| m.as_str() == model_class);
    let parent = crate::lower::find_nested_parent(app, name);
    let permitted = crate::lower::permitted_fields_for(c, &resource)
        .unwrap_or_else(|| crate::lower::default_permitted_fields(app, &model_class));

    let mut s = String::new();
    writeln!(s, "// Generated by Roundhouse.").unwrap();
    writeln!(
        s,
        "import type {{ ActionContext, ActionResponse }} from \"juntos\";",
    )
    .unwrap();
    if !known_models.is_empty() {
        let model_imports: Vec<String> = known_models
            .iter()
            .map(|m| {
                format!(
                    "import {{ {} }} from \"../models/{}.js\";",
                    m.as_str(),
                    crate::naming::snake_case(m.as_str()),
                )
            })
            .collect();
        for line in model_imports {
            writeln!(s, "{line}").unwrap();
        }
    }
    writeln!(s, "import * as routeHelpers from \"../../src/route_helpers.js\";").unwrap();
    writeln!(s, "import * as Views from \"../views/all.js\";").unwrap();

    let (public_actions, _private) = crate::lower::split_public_private(c);
    for action in &public_actions {
        writeln!(s).unwrap();
        let la = crate::lower::lower_action(
            action.name.as_str(),
            &resource,
            &model_class,
            has_model,
            parent.as_ref(),
            &permitted,
        );
        emit_ts_action(&mut s, &la, &action.body, known_models, c, adapter);
    }

    // Namespace object — same shape as before but only public actions.
    let action_names: Vec<String> = public_actions
        .iter()
        .map(|a| {
            let raw = a.name.as_str();
            if raw == "new" {
                "new: $new".to_string()
            } else {
                raw.to_string()
            }
        })
        .collect();
    if !action_names.is_empty() {
        writeln!(s).unwrap();
        writeln!(
            s,
            "export const {name} = {{ {} }};",
            action_names.join(", ")
        )
        .unwrap();
    }

    EmittedFile {
        path: PathBuf::from(format!("app/controllers/{file_stem}.ts")),
        content: s,
    }
}

/// Render one LoweredAction as a TS `export async function`. Mangles
/// `new` → `$new` (ruby2js convention — `new` is a JS keyword). Uses
/// the `routeHelpers.<name>Path` import for redirect Location URLs.
fn emit_ts_action(
    out: &mut String,
    la: &crate::lower::LoweredAction,
    body: &Expr,
    known_models: &[Symbol],
    controller: &Controller,
    adapter: &dyn crate::adapter::DatabaseAdapter,
) {
    let name = if la.name == "new" { "$new" } else { la.name.as_str() };
    if la.has_model {
        emit_ts_action_via_walker(out, la, body, known_models, controller, name, adapter);
    } else {
        // Actions on controllers with no associated model (e.g.
        // ApplicationController) emit a 200-empty stub; the walker
        // would try to synthesize a view-fn reference that doesn't
        // resolve.
        writeln!(
            out,
            "export async function {name}(_context: ActionContext): Promise<ActionResponse> {{",
        )
        .unwrap();
        writeln!(out, "  return {{ body: \"\" }};").unwrap();
        writeln!(out, "}}").unwrap();
    }
}

/// Run the normalization pipeline + walker for one action body,
/// then write the `export async function …` wrapper. The walker
/// dispatch lives in `src/lower/controller_walk.rs`; this function
/// builds a `TsEmitter` and hands it off.
fn emit_ts_action_via_walker(
    out: &mut String,
    la: &crate::lower::LoweredAction,
    body: &Expr,
    known_models: &[Symbol],
    controller: &Controller,
    name: &str,
    adapter: &dyn crate::adapter::DatabaseAdapter,
) {
    use crate::lower::CtrlWalker;
    let normalized =
        crate::lower::normalize_action_body(controller, la.name.as_str(), body);
    let rewritten = rewrite_for_controller(&normalized);
    let mut emitter = TsEmitter {
        ctx: crate::lower::WalkCtx {
            known_models,
            model_class: la.model_class.as_str(),
            resource: la.resource.as_str(),
            parent: la.parent.as_ref(),
            permitted: &la.permitted,
            adapter,
        },
        state: crate::lower::WalkState::new(),
    };
    let body_src = emitter.walk_action_body(&rewritten);
    // uses_context is set by SendKind arms that render `context.*`,
    // but rewrite_for_controller can also produce `context.params.k`
    // as a plain chained Send that falls through the generic path.
    // Post-scan the body to catch those.
    let uses_context = emitter.state.uses_context || body_src.contains("context.");
    let ctx_param = if uses_context { "context" } else { "_context" };
    writeln!(
        out,
        "export async function {name}({ctx_param}: ActionContext): Promise<ActionResponse> {{",
    )
    .unwrap();
    out.push_str(&body_src);
    writeln!(out, "}}").unwrap();
}

/// TS's controller-body emitter — implements the shared
/// `CtrlWalker` trait with TS-specific render methods.
struct TsEmitter<'a> {
    ctx: crate::lower::WalkCtx<'a>,
    state: crate::lower::WalkState,
}

impl<'a> crate::lower::CtrlWalker<'a> for TsEmitter<'a> {
    fn ctx(&self) -> &crate::lower::WalkCtx<'a> { &self.ctx }
    fn state_mut(&mut self) -> &mut crate::lower::WalkState { &mut self.state }
    fn indent_unit(&self) -> &'static str { "  " }

    fn suspending_prefix(&self) -> &'static str { "await " }

    fn write_assign(&mut self, name: &str, value: &Expr, indent: &str, out: &mut String) {
        // `render_expr` handles the suspending wrap internally
        // when `value` is a Send — compound-shape renders
        // (ModelFind's coalesce) place the `await` at the
        // suspending sub-expression rather than the outer
        // expression. For non-Send RHS shapes, `render_expr`
        // emits the expression as-is; suspension can only arise
        // from Sends in the current IR, so the walker doesn't
        // need to wrap here.
        let rhs = self.render_expr(value);
        writeln!(out, "{indent}const {name} = {rhs};").unwrap();
    }

    fn write_create_expansion(
        &mut self,
        var_name: &str,
        class: &str,
        indent: &str,
        out: &mut String,
    ) {
        writeln!(out, "{indent}const {var_name} = new {class}();").unwrap();
        if let Some(parent) = self.ctx.parent {
            writeln!(
                out,
                "{indent}({var_name} as any).{0}_id = Number(context.params.{0}_id);",
                parent.singular,
            )
            .unwrap();
            self.state.uses_context = true;
        }
        let permitted: Vec<String> = self.ctx.permitted.iter().cloned().collect();
        let resource = self.ctx.resource.to_string();
        for field in &permitted {
            writeln!(
                out,
                "{indent}({var_name} as any).{field} = context.params[\"{resource}[{field}]\"] ?? \"\";",
            )
            .unwrap();
            self.state.uses_context = true;
        }
    }

    fn write_if(
        &mut self,
        cond: &Expr,
        then_branch: &Expr,
        else_branch: &Expr,
        indent: &str,
        depth: usize,
        is_tail: bool,
        out: &mut String,
    ) {
        // render_expr places the suspending-wrap correctly when
        // cond is a Send; no extra wrapping needed here.
        let cond_s = self.render_expr(cond);
        writeln!(out, "{indent}if ({cond_s}) {{").unwrap();
        self.walk_stmt(then_branch, out, depth + 1, is_tail);
        if !crate::lower::is_empty_body(else_branch) {
            writeln!(out, "{indent}}} else {{").unwrap();
            self.walk_stmt(else_branch, out, depth + 1, is_tail);
        }
        writeln!(out, "{indent}}}").unwrap();
    }

    fn write_update_if(
        &mut self,
        recv: &Expr,
        then_branch: &Expr,
        else_branch: &Expr,
        indent: &str,
        depth: usize,
        is_tail: bool,
        out: &mut String,
    ) {
        let recv_s = self.render_expr(recv);
        let permitted: Vec<String> = self.ctx.permitted.iter().cloned().collect();
        let resource = self.ctx.resource.to_string();
        for field in &permitted {
            writeln!(
                out,
                "{indent}if (context.params[\"{resource}[{field}]\"] !== undefined) {{ ({recv_s} as any).{field} = context.params[\"{resource}[{field}]\"]; }}",
            )
            .unwrap();
            self.state.uses_context = true;
        }
        writeln!(out, "{indent}if ({recv_s}.save) {{").unwrap();
        self.walk_stmt(then_branch, out, depth + 1, is_tail);
        if !crate::lower::is_empty_body(else_branch) {
            writeln!(out, "{indent}}} else {{").unwrap();
            self.walk_stmt(else_branch, out, depth + 1, is_tail);
        }
        writeln!(out, "{indent}}}").unwrap();
    }

    fn write_response_stmt(&mut self, r: &str, _is_tail: bool, indent: &str, out: &mut String) {
        writeln!(out, "{indent}{r}").unwrap();
    }

    fn write_expr_stmt(&mut self, s: &str, indent: &str, out: &mut String) {
        // Note: callers of `walk_stmt` who route Send nodes
        // through `render_send_stmt` produce a rendered fragment
        // and hand it here; the suspension decision has to happen
        // at the walker's Send-dispatch site where the original
        // Expr is still available. See the walker's expression-
        // node handling in `walk_stmt`.
        writeln!(out, "{indent}{s};").unwrap();
    }

    fn render_expr(&mut self, expr: &Expr) -> String {
        if let ExprNode::Send { recv, method, args, block, .. } = &*expr.node {
            // Compute the target-specific suspending prefix from
            // this Send's effects + the adapter's suspending set.
            // Passed into `render_send_stmt` so each variant
            // places the wrapping correctly (outermost for simple
            // shapes, inside compounds for ModelFind's coalesce).
            let prefix = if self.ctx.expr_suspends(expr) {
                self.suspending_prefix()
            } else {
                ""
            };
            if let Some(stmt) = self.render_send_stmt(
                recv.as_ref(), method.as_str(), args, block.as_ref(), prefix,
            ) {
                return match stmt {
                    crate::lower::Stmt::Response(r) => r
                        .trim_start_matches("return ")
                        .trim_end_matches(';')
                        .to_string(),
                    crate::lower::Stmt::Expr(s) => s,
                };
            }
            // Generic fall-through: no SendKind matched. Simple
            // `recv.method(args)` shape; prefix externally.
            let base = emit_send_with_parens(recv.as_ref(), method.as_str(), args, false);
            return if prefix.is_empty() { base } else { format!("{prefix}{base}") };
        }
        emit_expr(expr)
    }

    fn render_send_stmt(
        &mut self,
        recv: Option<&Expr>,
        method: &str,
        args: &[Expr],
        block: Option<&Expr>,
        suspending_prefix: &str,
    ) -> Option<crate::lower::Stmt> {
        use crate::lower::{SendKind, Stmt};
        let kind = crate::lower::classify_controller_send(
            recv, method, args, block, self.ctx.known_models,
        )?;
        let p = suspending_prefix;
        Some(match kind {
            SendKind::ParamsAccess => {
                self.state.uses_context = true;
                Stmt::Expr("context.params".to_string())
            }
            SendKind::ParamsIndex { key } => {
                self.state.uses_context = true;
                let key_s = self.render_expr(key);
                Stmt::Expr(format!("context.params[{key_s}]"))
            }
            SendKind::ParamsExpect { args: pe_args } => {
                self.state.uses_context = true;
                let fragment = match pe_args.first().map(|e| &*e.node) {
                    Some(ExprNode::Lit { value: Literal::Sym { value: key } }) => {
                        format!("context.params.{}", key.as_str())
                    }
                    _ => "context.params /* TODO: params.expect hash */".to_string(),
                };
                Stmt::Expr(fragment)
            }
            SendKind::ModelNew { class } => {
                // `Model.new` is Pure in the catalog — never suspends
                // even under async adapters. Ignore prefix.
                Stmt::Expr(format!("new {}()", class.as_str()))
            }
            SendKind::ModelFind { class, id } => {
                // Compound shape: the `Post.find(id)` call is the
                // suspending piece; the `?? new Post()` fallback is
                // a TS idiom for nullable-lookup coalescing. Under
                // async adapters we need
                //   (await Post.find(id) ?? new Post())
                // which parses as
                //   ((await Post.find(id)) ?? new Post())
                // by precedence (await = 17, ?? = 3) — await binds
                // to the Promise-returning call, not the whole
                // coalesce. Wrong shape was `await (X ?? Y)`:
                // `?? Y` is falsy-coalesce on Promise (truthy), so
                // returns Promise unchanged; `await Promise` then
                // resolves, dropping Y.
                //
                // The same single-paren shape under sync stays
                // `(Post.find(id) ?? new Post())`; stripping
                // `await ` from the async form recovers it
                // exactly, preserving the byte-diff invariant.
                let id_s = self.render_expr(id);
                Stmt::Expr(format!(
                    "({p}{0}.find({id_s}) ?? new {0}())",
                    class.as_str(),
                ))
            }
            SendKind::AssocLookup { target, outer_method } => {
                let target_s = target.as_str();
                match outer_method {
                    "find" => {
                        let id_s = args.first().map(|a| self.render_expr(a))
                            .unwrap_or_else(|| "0".to_string());
                        Stmt::Expr(format!("{p}{target_s}.find({id_s})"))
                    }
                    _ => Stmt::Expr(format!(
                        "new {target_s}() /* TODO: {outer_method} */"
                    )),
                }
            }
            SendKind::QueryChain { target: Some(target), method, args, recv: chain_recv } => {
                // Walk the full chain to collect modifiers, then
                // emit `{target}.all()` + modifier calls. For the
                // scaffold, `order` is the only modifier that
                // changes observable output — the rest (includes,
                // preload, joins, distinct) are no-ops for our
                // sqlite runtime.
                let modifiers = crate::lower::collect_chain_modifiers(method, args, chain_recv);
                let mut s = format!("{}.all()", target.as_str());
                for m in modifiers {
                    s = apply_ts_chain_modifier(s, m);
                }
                Stmt::Expr(format!("{p}{s}"))
            }
            SendKind::QueryChain { target: None, .. } => {
                Stmt::Expr("[] /* TODO: unresolved query chain */".to_string())
            }
            SendKind::PathOrUrlHelper => Stmt::Expr(format!(
                "routeHelpers.{}()",
                lower_first_char(&crate::naming::camelize(method)),
            )),
            SendKind::BangStrip { recv, stripped_method, args: bs_args } => {
                let recv_s = self.render_expr(recv);
                if bs_args.is_empty() {
                    Stmt::Expr(format!("{p}{recv_s}.{stripped_method}"))
                } else {
                    let args_s: Vec<String> =
                        bs_args.iter().map(|a| self.render_expr(a)).collect();
                    Stmt::Expr(format!(
                        "{p}{recv_s}.{stripped_method}({})",
                        args_s.join(", "),
                    ))
                }
            }
            SendKind::InstanceUpdate => {
                Stmt::Expr("false /* TODO: instance update */".to_string())
            }
            SendKind::Render { args: r_args } => {
                // Render/redirect/head are Io effects, not DB —
                // under SqliteAsyncAdapter they don't suspend.
                // Responses stay plain.
                Stmt::Response(self.render_ts_render(r_args))
            }
            SendKind::RedirectTo { args: r_args } => {
                Stmt::Response(self.render_ts_redirect_to(r_args))
            }
            SendKind::Head { args: h_args } => {
                let status = h_args.first().map(|a| self.render_expr(a))
                    .unwrap_or_else(|| "200".to_string());
                Stmt::Response(format!("return {{ status: {status} }};"))
            }
            SendKind::RespondToBlock { .. }
            | SendKind::FormatHtml { .. }
            | SendKind::FormatJson => Stmt::Expr(
                "/* unreachable: respond_to not normalized */".to_string(),
            ),
        })
    }
}

impl<'a> TsEmitter<'a> {
    fn render_ts_render(&mut self, args: &[Expr]) -> String {
        if let Some(first) = args.first() {
            if let ExprNode::Lit { value: Literal::Sym { value: sym } } = &*first.node {
                let suffix = capitalize_ascii(sym.as_str());
                let view_fn = ts_view_fn(self.ctx.model_class, &suffix);
                let arg = self.state.last_local.clone()
                    .unwrap_or_else(|| "undefined as any".to_string());
                let body_part = format!("body: Views.{view_fn}({arg})");
                return match crate::lower::extract_status_from_kwargs(&args[1..]) {
                    Some(status) => format!("return {{ status: {status}, {body_part} }};"),
                    None => format!("return {{ {body_part} }};"),
                };
            }
            let body_s = self.render_expr(first);
            return format!("return {{ body: {body_s} }};");
        }
        "return { body: \"\" };".to_string()
    }

    fn render_ts_redirect_to(&mut self, args: &[Expr]) -> String {
        let Some(first) = args.first() else {
            return "return { status: 303 };".to_string();
        };
        let loc = self.render_expr(first);
        let status = crate::lower::extract_status_from_kwargs(&args[1..]).unwrap_or(303);
        if is_bare_ident(&loc) {
            let helper = lower_first_char(&crate::naming::camelize(&loc));
            let id_access = format!("({} as any).id", loc);
            return format!(
                "return {{ status: {status}, location: routeHelpers.{helper}Path({id_access}) }};",
            );
        }
        format!("return {{ status: {status}, location: {loc} }};")
    }
}

fn is_bare_ident(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.is_empty() {
        return false;
    }
    let first = bytes[0];
    if !(first.is_ascii_lowercase() || first == b'_') {
        return false;
    }
    bytes
        .iter()
        .all(|&b| b.is_ascii_alphanumeric() || b == b'_')
}

fn capitalize_ascii(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c.to_ascii_uppercase().to_string() + chars.as_str(),
        None => String::new(),
    }
}



/// Build a TS view fn name from a model class + action suffix.
/// `Article`, `Show` → `renderArticlesShow`; `Article`, `Index` →
/// `renderArticlesIndex`.
fn ts_view_fn(model_class: &str, suffix: &str) -> String {
    let plural = crate::naming::pluralize_snake(model_class);
    let plural_camel = crate::naming::camelize(&plural);
    format!("render{plural_camel}{suffix}")
}


/// IR-level rewrite pass that reshapes a controller action body to
/// Juntos conventions *before* the generic expression emitter runs:
///
/// - `@foo = x` → `foo = x`        (ivar writes become locals)
/// - `@foo`      → `foo`           (ivar reads become locals)
/// - `params[:k]` → `context.params.k` via a chained Send
///
/// Doing it at the IR level lets the existing `emit_expr` recurse
/// through nested expressions naturally — there's no need to thread
/// a "controller mode" flag through every call site.
fn rewrite_for_controller(expr: &Expr) -> Expr {
    use crate::ident::VarId;
    // Intercept the `params[:key]` pattern as a whole node first.
    if let Some(rewritten) = try_rewrite_params_access(expr) {
        return rewritten;
    }
    let new_node: ExprNode = match &*expr.node {
        ExprNode::Ivar { name } => ExprNode::Var { id: VarId(0), name: name.clone() },
        ExprNode::Assign { target: LValue::Ivar { name }, value } => ExprNode::Assign {
            target: LValue::Var { id: VarId(0), name: name.clone() },
            value: rewrite_for_controller(value),
        },
        ExprNode::Assign { target: LValue::Var { id, name }, value } => ExprNode::Assign {
            target: LValue::Var { id: *id, name: name.clone() },
            value: rewrite_for_controller(value),
        },
        ExprNode::Assign { target: LValue::Attr { recv, name }, value } => ExprNode::Assign {
            target: LValue::Attr {
                recv: rewrite_for_controller(recv),
                name: name.clone(),
            },
            value: rewrite_for_controller(value),
        },
        ExprNode::Assign { target: LValue::Index { recv, index }, value } => ExprNode::Assign {
            target: LValue::Index {
                recv: rewrite_for_controller(recv),
                index: rewrite_for_controller(index),
            },
            value: rewrite_for_controller(value),
        },
        ExprNode::Send { recv, method, args, block, parenthesized } => ExprNode::Send {
            recv: recv.as_ref().map(rewrite_for_controller),
            method: method.clone(),
            args: args.iter().map(rewrite_for_controller).collect(),
            block: block.as_ref().map(rewrite_for_controller),
            parenthesized: *parenthesized,
        },
        ExprNode::Seq { exprs } => ExprNode::Seq {
            exprs: exprs.iter().map(rewrite_for_controller).collect(),
        },
        ExprNode::If { cond, then_branch, else_branch } => ExprNode::If {
            cond: rewrite_for_controller(cond),
            then_branch: rewrite_for_controller(then_branch),
            else_branch: rewrite_for_controller(else_branch),
        },
        ExprNode::BoolOp { op, surface, left, right } => ExprNode::BoolOp {
            op: *op,
            surface: *surface,
            left: rewrite_for_controller(left),
            right: rewrite_for_controller(right),
        },
        ExprNode::Array { elements, style } => ExprNode::Array {
            elements: elements.iter().map(rewrite_for_controller).collect(),
            style: *style,
        },
        ExprNode::Hash { entries, braced } => ExprNode::Hash {
            entries: entries
                .iter()
                .map(|(k, v)| (rewrite_for_controller(k), rewrite_for_controller(v)))
                .collect(),
            braced: *braced,
        },
        // Lambda bodies (the `do ... end` block of `respond_to` or
        // `format.html`) need the same ivar / params rewrites as the
        // surrounding action body. Preserve other Lambda fields.
        ExprNode::Lambda { params, block_param, body, block_style } => ExprNode::Lambda {
            params: params.clone(),
            block_param: block_param.clone(),
            body: rewrite_for_controller(body),
            block_style: *block_style,
        },
        // Other variants don't contain rewritable subtrees we care
        // about at controller-scope today (Literal, Const, Var,
        // Apply, Case, Yield, Raise, RescueModifier, StringInterp,
        // Let). Clone verbatim.
        other => other.clone(),
    };
    Expr {
        span: expr.span,
        node: Box::new(new_node),
        ty: expr.ty.clone(),
        effects: expr.effects.clone(),
        leading_blank_line: expr.leading_blank_line,
    }
}

/// Match the `params[:key]` call shape and rewrite it to the Juntos
/// `context.params.<key>` three-level Send chain. Returns None if the
/// expression isn't the params-access shape — the outer rewriter
/// continues with its generic recursion.
fn try_rewrite_params_access(expr: &Expr) -> Option<Expr> {
    let ExprNode::Send { recv: Some(recv), method, args, .. } = &*expr.node else {
        return None;
    };
    if method.as_str() != "[]" {
        return None;
    }
    let ExprNode::Send {
        recv: None,
        method: recv_method,
        args: recv_args,
        ..
    } = &*recv.node
    else {
        return None;
    };
    if recv_method.as_str() != "params" || !recv_args.is_empty() {
        return None;
    }
    let idx = args.first()?;
    let ExprNode::Lit { value: Literal::Sym { value: key } } = &*idx.node else {
        return None;
    };
    // Build `context.params.<key>` as three chained Sends — emit_expr
    // renders this as `context.params.<key>` via the Ruby-reader path.
    let span = expr.span;
    let context = Expr::new(
        span,
        ExprNode::Send {
            recv: None,
            method: crate::ident::Symbol::from("context"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    let params = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(context),
            method: crate::ident::Symbol::from("params"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    Some(Expr::new(
        span,
        ExprNode::Send {
            recv: Some(params),
            method: key.clone(),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    ))
}

// Views ----------------------------------------------------------------

/// Emit each view as a TypeScript function that takes a `locals`
/// object and returns the rendered HTML string. Walks the compiled-
/// ERB body (a `Seq` of `_buf = _buf + …` assignments) and translates
/// each statement into its JS equivalent:
///
/// ```text
/// _buf = ""                  → dropped (prologue)
/// _buf = _buf + "text"       → _buf += "text";
/// _buf = _buf + (expr).to_s  → _buf += String(expr);
/// _buf                       → return _buf;
/// ```
///
/// Control-flow statements (e.g. `<% if cond %>`) ingested into the
/// body as `If` / Send-with-block nodes pass through the normal
/// `emit_expr` path.
fn emit_views(app: &App) -> Vec<EmittedFile> {
    let known_models: Vec<Symbol> = app.models.iter().map(|m| m.name.0.clone()).collect();
    let attrs_by_class: std::collections::BTreeMap<String, Vec<String>> = app
        .models
        .iter()
        .map(|m| {
            (
                m.name.0.as_str().to_string(),
                m.attributes
                    .fields
                    .keys()
                    .map(|k| k.as_str().to_string())
                    .collect(),
            )
        })
        .collect();

    let mut files: Vec<EmittedFile> = app
        .views
        .iter()
        .map(|v| emit_view_file_pass2(v, &known_models, &attrs_by_class, &app.stylesheets))
        .collect();

    // Stubs for missing standard CRUD views (same posture as Rust).
    if let Some(stubs) = emit_ts_missing_view_stubs(app, &known_models) {
        files.push(stubs);
    }

    // Barrel — one file re-exporting every view fn, so controllers
    // can `import * as Views from "../views/all.js"`.
    files.push(emit_ts_views_barrel(app));

    files
}

fn emit_ts_views_barrel(app: &App) -> EmittedFile {
    let mut s = String::new();
    writeln!(s, "// Generated by Roundhouse.").unwrap();
    for v in &app.views {
        let name = v.name.as_str();
        let path = format!("./{}.{}.js", name, v.format);
        writeln!(s, "export * from {path:?};").unwrap();
    }
    // Stubs file (if emitted) gets re-exported too.
    writeln!(s, "export * from \"./_stubs.js\";").unwrap();
    EmittedFile {
        path: PathBuf::from("app/views/all.ts"),
        content: s,
    }
}

/// Stub view fns for standard CRUD names the fixture doesn't
/// supply — controllers reference `Views.renderArticlesShow` etc.
/// whether or not an ERB template exists. Returns None when the
/// fixture supplies every standard view (no stub file needed).
fn emit_ts_missing_view_stubs(app: &App, known_models: &[Symbol]) -> Option<EmittedFile> {
    use std::collections::BTreeSet;
    let present: BTreeSet<String> =
        app.views.iter().map(|v| v.name.as_str().to_string()).collect();
    let mut s = String::new();
    writeln!(s, "// Generated by Roundhouse.").unwrap();
    let mut any = false;
    for model in &app.models {
        if model.attributes.fields.is_empty() {
            continue;
        }
        let class = model.name.0.as_str();
        let plural = crate::naming::pluralize_snake(class);
        let plural_camel = crate::naming::camelize(&plural);
        for (action, suffix) in [
            ("Index", "index"),
            ("Show", "show"),
            ("New", "new"),
            ("Edit", "edit"),
        ] {
            let view_name = format!("{plural}/{suffix}");
            if present.contains(&view_name) {
                continue;
            }
            any = true;
            let fn_name = format!("render{plural_camel}{action}");
            writeln!(
                s,
                "export function {fn_name}(_record: unknown): string {{ return \"\"; }}",
            )
            .unwrap();
        }
    }
    let _ = known_models;
    if any {
        Some(EmittedFile {
            path: PathBuf::from("app/views/_stubs.ts"),
            content: s,
        })
    } else {
        // Empty stub file so barrel's re-export doesn't break.
        Some(EmittedFile {
            path: PathBuf::from("app/views/_stubs.ts"),
            content: "// Generated by Roundhouse.\nexport {};\n".to_string(),
        })
    }
}

fn emit_view_file_pass2(
    view: &crate::dialect::View,
    known_models: &[Symbol],
    attrs_by_class: &std::collections::BTreeMap<String, Vec<String>>,
    stylesheets: &[String],
) -> EmittedFile {
    let rewritten_body = rewrite_for_controller(&view.body);
    // Apply the shared erubi-trim lowering. Produces a body
    // where every `<% %>` tag's leading indent and trailing
    // newline are already consumed (matching Rails' erubi render
    // behavior), plus the view's trailing whitespace-only text
    // append is dropped. After this, emit can walk the IR
    // straight through without re-applying trim rules per-
    // statement.
    let rewritten_body = crate::lower::erb_trim::trim_view(&rewritten_body);
    let ivar_names = collect_ivar_names(&view.body);
    let fn_name = view_function_name(view.name.as_str());

    let mut s = String::new();
    writeln!(s, "// Generated by Roundhouse.").unwrap();
    writeln!(s).unwrap();
    writeln!(
        s,
        "import * as Helpers from \"../../../src/view_helpers.js\";",
    )
    .unwrap();
    writeln!(s, "import * as routeHelpers from \"../../../src/route_helpers.js\";").unwrap();
    writeln!(s, "import * as Views from \"../all.js\";").unwrap();
    writeln!(s, "import * as Importmap from \"../../../src/importmap.js\";").unwrap();
    if !known_models.is_empty() {
        for m in known_models {
            writeln!(
                s,
                "import {{ {} }} from \"../../models/{}.js\";",
                m.as_str(),
                crate::naming::snake_case(m.as_str()),
            )
            .unwrap();
        }
    }
    writeln!(s).unwrap();

    // Signature: single positional arg matching the resource's
    // singular/plural name. Controllers call with the right record.
    let (sig, arg_name, arg_model) =
        ts_view_signature(view.name.as_str(), known_models);
    let attrs = arg_model
        .as_ref()
        .and_then(|c| attrs_by_class.get(c).cloned())
        .unwrap_or_default();

    writeln!(s, "export function {fn_name}({sig}): string {{").unwrap();
    writeln!(s, "  let _buf = \"\";").unwrap();
    let mut locals: Vec<String> = vec!["_buf".to_string(), arg_name.clone()];
    for n in &ivar_names {
        // Ivars listed (originals) — rewrite_for_controller already
        // mapped them to locals matching the arg name when the ivar
        // is the main record.
        if !locals.iter().any(|x| x == n) {
            locals.push(n.clone());
        }
    }
    let resource_dir = view
        .name
        .as_str()
        .rsplit_once('/')
        .map(|(d, _): (&str, &str)| d.to_string())
        .unwrap_or_default();
    let ctx = TsViewCtx {
        locals,
        arg_name: arg_name.clone(),
        arg_attrs: attrs,
        resource_dir,
        stylesheets: stylesheets.to_vec(),
    };

    let body_lines = emit_ts_view_body(&rewritten_body, &ctx);
    for line in body_lines {
        writeln!(s, "  {line}").unwrap();
    }
    writeln!(s, "  return _buf;").unwrap();
    writeln!(s, "}}").unwrap();

    let path = PathBuf::from(format!("app/views/{}.{}.ts", view.name, view.format));
    EmittedFile { path, content: s }
}

struct TsViewCtx {
    locals: Vec<String>,
    arg_name: String,
    arg_attrs: Vec<String>,
    resource_dir: String,
    /// Discovered stylesheet names (stems). Feeds the
    /// `stylesheet_link_tag :app` expansion in the layout.
    stylesheets: Vec<String>,
}

impl TsViewCtx {
    fn is_local(&self, n: &str) -> bool {
        self.locals.iter().any(|x| x == n)
    }
    fn arg_has_attr(&self, name: &str, attr: &str) -> bool {
        name == self.arg_name && self.arg_attrs.iter().any(|a| a == attr)
    }
}

/// Build the single-arg signature for a view fn + return the arg
/// name and (optional) model class.
fn ts_view_signature(
    view_name: &str,
    known_models: &[Symbol],
) -> (String, String, Option<String>) {
    let (dir, base) = view_name.rsplit_once('/').unwrap_or(("", view_name));
    let is_partial = base.starts_with('_');
    let stem = base.trim_start_matches('_');
    let model_class = crate::naming::singularize_camelize(dir);
    let model_exists = known_models.iter().any(|m| m.as_str() == model_class);
    let singular = crate::naming::singularize(dir);

    if is_partial {
        let arg_name = if model_exists { singular.clone() } else { stem.to_string() };
        if model_exists {
            return (format!("{arg_name}: {model_class}"), arg_name, Some(model_class));
        }
        return (format!("{arg_name}: any"), arg_name, None);
    }
    match stem {
        "index" => {
            let arg_name = dir.to_string();
            if model_exists {
                return (format!("{arg_name}: {model_class}[]"), arg_name, Some(model_class));
            }
            return (format!("{arg_name}: any[]"), arg_name, None);
        }
        _ => {
            let arg_name = singular.clone();
            if model_exists {
                return (format!("{arg_name}: {model_class}"), arg_name, Some(model_class));
            }
            return (format!("{arg_name}: any"), arg_name, None);
        }
    }
}

fn emit_ts_view_body(body: &Expr, ctx: &TsViewCtx) -> Vec<String> {
    // The view body arrives pre-trimmed via
    // `lower::erb_trim::trim_view` (applied once per view at the
    // top of `emit_view_file_pass2`). Nested Seqs/If branches are
    // trimmed in that same pass, so here we just iterate stmts.
    let stmts: Vec<&Expr> = match &*body.node {
        ExprNode::Seq { exprs } => exprs.iter().collect(),
        _ => vec![body],
    };
    let mut out = Vec::new();
    for stmt in &stmts {
        out.extend(emit_ts_view_stmt_pass2(stmt, ctx));
    }
    out
}

fn emit_ts_view_stmt_pass2(stmt: &Expr, ctx: &TsViewCtx) -> Vec<String> {
    match &*stmt.node {
        // Prologue `_buf = ""` — emit_view_body already wrote it.
        ExprNode::Assign { target: LValue::Var { name, .. }, value }
            if name.as_str() == "_buf" =>
        {
            if let ExprNode::Lit { value: Literal::Str { value: s } } = &*value.node {
                if s.is_empty() {
                    return Vec::new();
                }
            }
            // `_buf = _buf + X` — the working shape.
            if let ExprNode::Send { recv: Some(recv), method, args, .. } = &*value.node {
                if method.as_str() == "+" && args.len() == 1 {
                    if let ExprNode::Var { name: rn, .. } = &*recv.node {
                        if rn.as_str() == "_buf" {
                            return vec![emit_ts_view_append_pass2(&args[0], ctx)];
                        }
                    }
                }
            }
            vec![format!("/* TODO ERB: _buf shape */;")]
        }
        // Epilogue: bare `_buf` — dropped; we emit `return _buf;` elsewhere.
        ExprNode::Var { name, .. } if name.as_str() == "_buf" => Vec::new(),
        // `if cond ... else ... end` — degrade cond if complex.
        // Branch-edge trim happens upstream in
        // `lower::erb_trim::trim_view`; the branches arrive with
        // their first/last text stmts already adjusted.
        ExprNode::If { cond, then_branch, else_branch } => {
            let cond_js = if is_ts_simple_expr(cond, ctx) {
                emit_ts_view_expr_raw(cond, ctx)
            } else {
                "false /* TODO ERB cond */".to_string()
            };
            let mut out = vec![format!("if ({cond_js}) {{")];
            for line in emit_ts_view_body(then_branch, ctx) {
                out.push(format!("  {line}"));
            }
            let has_else = !matches!(
                &*else_branch.node,
                ExprNode::Lit { value: Literal::Nil }
            );
            if has_else {
                out.push("} else {".to_string());
                for line in emit_ts_view_body(else_branch, ctx) {
                    out.push(format!("  {line}"));
                }
            }
            out.push("}".to_string());
            out
        }
        // `<% content_for :slot, "value" %>` — statement-form
        // setter. Classifier recognizes the shape; render the
        // side-effect call without a `_buf +=` (nothing goes
        // into the view output — the slot is read later by the
        // layout's `<%= yield :slot %>`).
        ExprNode::Send { recv: None, method, args, block: None, .. }
            if matches!(
                crate::lower::classify_view_helper(method.as_str(), args),
                Some(crate::lower::ViewHelperKind::ContentForSetter { .. })
            ) =>
        {
            if let Some(crate::lower::ViewHelperKind::ContentForSetter { slot, body }) =
                crate::lower::classify_view_helper(method.as_str(), args)
            {
                if is_ts_simple_expr(body, ctx) {
                    let body_s = emit_ts_view_expr_raw(body, ctx);
                    return vec![format!("Helpers.contentFor({slot:?}, {body_s});")];
                }
            }
            vec!["/* TODO ERB: content_for with complex body */".to_string()]
        }
        // `coll.each do |x| ... end` — only if coll is simple.
        ExprNode::Send { recv: Some(recv), method, args, block: Some(block), .. }
            if method.as_str() == "each" && args.is_empty() =>
        {
            if !is_ts_simple_expr(recv, ctx) {
                return vec!["/* TODO ERB: each over complex coll */".to_string()];
            }
            let ExprNode::Lambda { params, body, .. } = &*block.node else {
                return vec!["/* unexpected each block */".to_string()];
            };
            let coll_s = emit_ts_view_expr_raw(recv, ctx);
            let var = params.first().map(|p| p.as_str().to_string()).unwrap_or_else(|| "item".into());
            let inner_ctx = TsViewCtx {
                locals: {
                    let mut l = ctx.locals.clone();
                    if !l.iter().any(|x| x == &var) {
                        l.push(var.clone());
                    }
                    l
                },
                arg_name: ctx.arg_name.clone(),
                arg_attrs: ctx.arg_attrs.clone(),
                resource_dir: ctx.resource_dir.clone(),
                stylesheets: ctx.stylesheets.clone(),
            };
            let mut out = vec![format!("for (const {var} of {coll_s}) {{")];
            for line in emit_ts_view_body(body, &inner_ctx) {
                out.push(format!("  {line}"));
            }
            out.push("}".to_string());
            out
        }
        _ => vec!["/* TODO ERB: unknown stmt */".to_string()],
    }
}

fn emit_ts_view_append_pass2(arg: &Expr, ctx: &TsViewCtx) -> String {
    // Text chunk.
    if let ExprNode::Lit { value: Literal::Str { value: s } } = &*arg.node {
        return format!("_buf += {};", ts_string_literal(s));
    }
    let inner = unwrap_to_s_ts(arg);

    // `render @coll` / `render "partial", locals` — expand.
    if let ExprNode::Send { recv: None, method, args, block: None, .. } = &*inner.node {
        if method.as_str() == "render" {
            if args.len() == 1 {
                return emit_ts_render_call(&args[0], ctx);
            }
            if args.len() == 2 {
                if let (
                    ExprNode::Lit { value: Literal::Str { value: partial } },
                    ExprNode::Hash { entries, .. },
                ) = (&*args[0].node, &*args[1].node)
                {
                    let plural_pascal = crate::naming::camelize(&ctx.resource_dir);
                    let partial_fn = format!(
                        "Views.render{}{}",
                        plural_pascal,
                        crate::naming::camelize(partial),
                    );
                    if let Some((_, v)) = entries.first() {
                        if is_ts_simple_expr(v, ctx) {
                            let arg_expr = emit_ts_view_expr_raw(v, ctx);
                            return format!("_buf += {partial_fn}({arg_expr});");
                        }
                    }
                    return format!("_buf += {partial_fn}(undefined as any);");
                }
            }
        }
    }

    // Capturing helpers (form_with, content_for) — inner buffer.
    if let ExprNode::Send {
        recv: None,
        method,
        args,
        block: Some(block),
        ..
    } = &*inner.node
    {
        if is_ts_capturing_helper(method.as_str()) {
            return emit_ts_captured_helper(method.as_str(), args, block, ctx);
        }
    }

    // `<%= yield %>` / `<%= yield :slot %>` — ExprNode::Yield,
    // not a Send. Maps to the runtime's yield-body / named-slot
    // getters populated by the request dispatcher + content_for
    // setters.
    if let ExprNode::Yield { args } = &*inner.node {
        if args.is_empty() {
            return "_buf += Helpers.getYield();".to_string();
        }
        if args.len() == 1 {
            if let ExprNode::Lit { value: Literal::Sym { value } } = &*args[0].node {
                return format!("_buf += Helpers.getSlot({:?});", value.as_str());
            }
        }
    }

    // Bare view-helper call — single classifier-backed dispatch
    // for every Rails helper we recognize (csrf_meta_tags,
    // dom_id, link_to, etc). The classifier in `lower::view`
    // enforces the method-name + arity match once; this function
    // owns the TS-specific rendering per variant.
    if let ExprNode::Send { recv: None, method, args, block: None, .. } = &*inner.node {
        if let Some(kind) =
            crate::lower::classify_view_helper(method.as_str(), args)
        {
            if let Some(out) = emit_ts_view_helper(&kind, ctx) {
                return out;
            }
        }
    }

    // `content_for(:title) || "Real Blog"` — ingested as a BoolOp
    // (Or) not a Send, so the classifier doesn't see it. Match
    // here; lower to JS `||` which has matching empty-string
    // falsy semantics since `contentFor` returns "" for unset
    // slots.
    if let ExprNode::BoolOp { op: crate::expr::BoolOpKind::Or, left, right, .. } = &*inner.node
    {
        if let ExprNode::Send {
            recv: None, method, args, block: None, ..
        } = &*left.node
        {
            if let Some(crate::lower::ViewHelperKind::ContentForGetter { slot }) =
                crate::lower::classify_view_helper(method.as_str(), args)
            {
                if is_ts_simple_expr(right, ctx) {
                    let fallback = emit_ts_view_expr_raw(right, ctx);
                    return format!(
                        "_buf += (Helpers.contentFor({slot:?}) || {fallback});",
                    );
                }
            }
        }
    }

    // `form.label :title` / `form.text_field :title, class: "..."` /
    // `form.text_area :body, rows: 4, class: "..."` /
    // `form.submit class: "..."` — FormBuilder method calls.
    // Recognize when `form` is a local (bound by the enclosing
    // form_with's block param). Hash arguments (class, rows, etc.)
    // pass through as an options object; non-hash args (first
    // arg is typically a field symbol) go positionally.
    if let ExprNode::Send { recv: Some(r), method, args, block: None, .. } = &*inner.node {
        if let ExprNode::Var { name: recv_name, .. } = &*r.node {
            if ctx.is_local(recv_name.as_str()) {
                if let Some(fb_method) =
                    crate::lower::classify_form_builder_method(method.as_str())
                {
                    // Translate the shared kind into TS's camelCased
                    // FormBuilder API. Other targets pick the
                    // language-appropriate mapping (snake_case in
                    // rust/python, etc.) from the same classifier.
                    let js_method = match fb_method {
                        crate::lower::FormBuilderMethod::Label => "label",
                        crate::lower::FormBuilderMethod::TextField => "textField",
                        crate::lower::FormBuilderMethod::TextArea => "textArea",
                        crate::lower::FormBuilderMethod::Submit => "submit",
                    };
                    return emit_ts_form_builder_call(
                        recv_name.as_str(),
                        js_method,
                        args,
                        ctx,
                    );
                }
            }
        }
    }

    // Simple interpolation.
    if is_ts_simple_expr(inner, ctx) {
        return format!(
            "_buf += String({});",
            emit_ts_view_expr_raw(inner, ctx),
        );
    }

    "_buf += \"\"; /* TODO ERB: complex interpolation */".to_string()
}

fn is_ts_capturing_helper(method: &str) -> bool {
    matches!(method, "form_with" | "content_for")
}

/// Emit a FormBuilder method call (`form.label("title")` etc.)
/// that produces HTML appended to the surrounding view buffer.
/// Separates positional args (first arg, typically a field
/// symbol like `:title`) from the trailing options hash (class,
/// rows, etc.). Hash args pass through as a JS object literal;
/// non-simple expressions inside options fall back to empty
/// so emission doesn't choke on the Rails scaffold's conditional-
/// class arrays (`[..., {cond: ...}]`) which aren't supported
/// today.
fn emit_ts_form_builder_call(
    recv: &str,
    js_method: &str,
    args: &[Expr],
    ctx: &TsViewCtx,
) -> String {
    // `submit` takes a positional String label (not a Sym field),
    // plus optional opts. Merge the label into opts as `label:` so
    // the runtime helper picks it up via its existing API.
    if js_method == "submit" {
        let label_str = args.iter().find_map(|a| match &*a.node {
            ExprNode::Lit { value: Literal::Str { value } } => Some(value.clone()),
            _ => None,
        });
        let opts_hash: Option<String> = args.iter().find_map(|a| match &*a.node {
            ExprNode::Hash { entries, .. } => Some(ts_hash_to_object_literal(entries, ctx)),
            _ => None,
        });
        let opts_with_label = match (label_str, opts_hash) {
            (Some(lbl), Some(opts)) => {
                // Splice `label: "<lbl>"` into the existing hash
                // literal by stripping its closing brace and re-
                // appending. Keeps the other entries intact.
                let trimmed = opts.trim_end_matches(" }").trim_end_matches('}');
                if trimmed.ends_with('{') {
                    format!("{{ \"label\": {lbl:?} }}")
                } else {
                    format!("{trimmed}, \"label\": {lbl:?} }}")
                }
            }
            (Some(lbl), None) => format!("{{ \"label\": {lbl:?} }}"),
            (None, Some(opts)) => opts,
            (None, None) => String::new(),
        };
        return format!("_buf += {recv}.submit({opts_with_label});");
    }

    let (field_arg, opts_arg) = split_form_builder_args(args, ctx);
    let call_args = match (field_arg, opts_arg) {
        (Some(field), Some(opts)) => format!("{field}, {opts}"),
        (Some(field), None) => field,
        (None, Some(opts)) => opts,
        (None, None) => String::new(),
    };
    format!("_buf += {recv}.{js_method}({call_args});")
}

/// Split FormBuilder args into (field, options). Field is the
/// first positional arg (usually a Sym); options is the last
/// Hash arg. For `form.submit class: "..."` there's no field,
/// just options. Scaffold `form.text_field :title, class: [..]`
/// has both; the class array is simplified to its first string
/// element (the conditional-hash part is dropped — good enough
/// for the acceptance test's visual correctness).
fn split_form_builder_args(args: &[Expr], ctx: &TsViewCtx) -> (Option<String>, Option<String>) {
    if args.is_empty() {
        return (None, None);
    }
    // Detect leading symbol arg as the field.
    let (field_arg, rest) = match args.first().and_then(|a| match &*a.node {
        ExprNode::Lit { value: Literal::Sym { value } } => Some(value.as_str().to_string()),
        _ => None,
    }) {
        Some(field) => (Some(format!("{field:?}")), &args[1..]),
        None => (None, args),
    };
    // Remaining args: expect a single Hash (options).
    let opts_arg = rest.iter().find_map(|a| match &*a.node {
        ExprNode::Hash { entries, .. } => Some(ts_hash_to_object_literal(entries, ctx)),
        _ => None,
    });
    (field_arg, opts_arg)
}

/// Render a Ruby hash literal's entries as a JS object literal,
/// simplifying where needed: unsupported values (e.g. arrays
/// with conditional-class hashes) fall back to empty string;
/// simple literals and locals emit verbatim. Good enough for
/// the scaffold form's `class:`/`rows:` kwargs.
/// Lower `link_to "text", url_or_record [, opts]` or the same
/// shape for `button_to`. Returns a TS expression like
/// `Helpers.linkTo(text, url, opts)`. Returns None when the args
/// can't be lowered (non-simple text, unsupported URL shape, etc.)
/// so the caller can fall through to the TODO placeholder.
/// Render a classified `ViewHelperKind` as a TS statement.
/// Returns `Some(line)` on success; `None` when the variant's
/// arg shape isn't renderable here (e.g. the text/URL isn't
/// simple — the caller's fallthrough then emits the degraded
/// placeholder). Covers everything `<%= helper %>`-side; the
/// statement-form `<% content_for :t, "b" %>` is handled by
/// `emit_ts_view_stmt_pass2`.
fn emit_ts_view_helper(
    kind: &crate::lower::ViewHelperKind<'_>,
    ctx: &TsViewCtx,
) -> Option<String> {
    use crate::lower::ViewHelperKind::*;
    match kind {
        CsrfMetaTags => Some("_buf += Helpers.csrfMetaTags();".to_string()),
        CspMetaTag => Some("_buf += Helpers.cspMetaTag();".to_string()),
        JavascriptImportmapTags => {
            Some("_buf += Helpers.javascriptImportmapTags(Importmap.PINS);".to_string())
        }
        TurboStreamFrom { channel } => {
            if !is_ts_simple_expr(channel, ctx) {
                return None;
            }
            let arg = emit_ts_view_expr_raw(channel, ctx);
            Some(format!("_buf += Helpers.turboStreamFrom({arg});"))
        }
        DomId { record, prefix } => {
            if !is_ts_simple_expr(record, ctx) {
                return None;
            }
            let rec = emit_ts_view_expr_raw(record, ctx);
            match prefix {
                None => Some(format!("_buf += Helpers.domId({rec});")),
                Some(p) => {
                    let prefix_expr = match &*p.node {
                        ExprNode::Lit { value: Literal::Sym { value } } => {
                            format!("{:?}", value.as_str())
                        }
                        ExprNode::Lit { value: Literal::Str { value } } => {
                            format!("{value:?}")
                        }
                        _ if is_ts_simple_expr(p, ctx) => emit_ts_view_expr_raw(p, ctx),
                        _ => return Some(format!("_buf += Helpers.domId({rec});")),
                    };
                    Some(format!("_buf += Helpers.domId({rec}, {prefix_expr});"))
                }
            }
        }
        Pluralize { count, word } => {
            if !is_ts_simple_expr(count, ctx) || !is_ts_simple_expr(word, ctx) {
                return None;
            }
            let c = emit_ts_view_expr_raw(count, ctx);
            let w = emit_ts_view_expr_raw(word, ctx);
            Some(format!("_buf += Helpers.pluralize({c}, {w});"))
        }
        Truncate { text, opts } => {
            if !is_ts_simple_expr(text, ctx) {
                return None;
            }
            let t = emit_ts_view_expr_raw(text, ctx);
            let opts_s = match opts {
                Some(e) => match &*e.node {
                    ExprNode::Hash { entries, .. } => ts_hash_to_object_literal(entries, ctx),
                    _ => "{}".to_string(),
                },
                None => "{}".to_string(),
            };
            Some(format!("_buf += Helpers.truncate({t}, {opts_s});"))
        }
        StylesheetLinkTag { name, opts } => {
            let opts_s = match opts {
                Some(e) => match &*e.node {
                    ExprNode::Hash { entries, .. } => ts_hash_to_object_literal(entries, ctx),
                    _ => "{}".to_string(),
                },
                None => "{}".to_string(),
            };
            // `:app` is the scaffold shorthand Rails expands to
            // every stylesheet in the asset path; mirror by
            // emitting one link per ingested stylesheet name.
            if let ExprNode::Lit { value: Literal::Sym { value } } = &*name.node {
                if value.as_str() == "app" && !ctx.stylesheets.is_empty() {
                    let calls: Vec<String> = ctx
                        .stylesheets
                        .iter()
                        .map(|n| format!("_buf += Helpers.stylesheetLinkTag({n:?}, {opts_s});"))
                        .collect();
                    return Some(calls.join("\n_buf += \"\\n\";\n"));
                }
            }
            let name_expr = match &*name.node {
                ExprNode::Lit { value: Literal::Sym { value } } => format!("{:?}", value.as_str()),
                ExprNode::Lit { value: Literal::Str { value } } => format!("{value:?}"),
                _ => return None,
            };
            Some(format!("_buf += Helpers.stylesheetLinkTag({name_expr}, {opts_s});"))
        }
        ContentForGetter { slot } => {
            Some(format!("_buf += Helpers.contentFor({slot:?});"))
        }
        // Statement-form setter is handled by
        // `emit_ts_view_stmt_pass2`; the append-level dispatcher
        // never sees it through `<%= ... %>`.
        ContentForSetter { .. } => None,
        LinkTo { text, url, opts } => {
            let args = build_link_like_args(text, url, *opts);
            try_emit_link_or_button("link_to", &args, ctx).map(|call| format!("_buf += {call};"))
        }
        ButtonTo { text, target, opts } => {
            let args = build_link_like_args(text, target, *opts);
            try_emit_link_or_button("button_to", &args, ctx).map(|call| format!("_buf += {call};"))
        }
    }
}

/// Rebuild the positional-arg slice for link_to / button_to from
/// the classifier's separated fields. The existing
/// `try_emit_link_or_button` takes `&[Expr]` so this is a thin
/// adapter; refactoring it to take structured args is a separate
/// pass.
fn build_link_like_args<'a>(
    text: &'a Expr,
    url: &'a Expr,
    opts: Option<&'a Expr>,
) -> Vec<Expr> {
    let mut out = vec![text.clone(), url.clone()];
    if let Some(o) = opts {
        out.push(o.clone());
    }
    out
}

fn try_emit_link_or_button(
    method: &str,
    args: &[Expr],
    ctx: &TsViewCtx,
) -> Option<String> {
    let js_helper = match method {
        "link_to" => "linkTo",
        "button_to" => "buttonTo",
        _ => return None,
    };

    // Text arg — first positional. Must be simple (literal or local
    // attribute access) so we can embed it.
    if !is_ts_simple_expr(&args[0], ctx) {
        return None;
    }
    let text = emit_ts_view_expr_raw(&args[0], ctx);

    // URL arg — second positional. Shapes:
    //   - literal string: use as-is
    //   - path helper call like `new_article_path` / `articles_path`
    //   - path helper with args: `article_path(@article)` → need to
    //     pass `.id` if the arg is a model
    //   - bare model reference: `@article` → resolve to the model's
    //     show path (`articlePath(article.id)`). Implemented by
    //     recognizing the local as a known model and mapping to the
    //     singular path helper.
    let url = if args.len() >= 2 {
        ts_emit_url_arg(&args[1], ctx)?
    } else {
        return None;
    };

    // Opts — last Hash arg (positional 2 for link_to with opts,
    // positional 2 for button_to with opts). Lower to a plain JS
    // object literal with data: subhash flattened into
    // `"data-key": value` entries (matching Rails' data-*
    // attribute convention).
    let opts = args.iter().skip(2).find_map(|a| match &*a.node {
        ExprNode::Hash { entries, .. } => Some(ts_link_opts_literal(entries, ctx)),
        _ => None,
    });
    // Rails allows opts to be passed without the positional URL
    // (link_to/button_to with model shorthand): `button_to "Text",
    // @article, method: :delete` — three positional args, third
    // being the opts hash in Ruby surface form that Prism ingests
    // as a KeywordHashNode or HashNode. Our split already handles
    // that above via iter.skip(2).
    let opts_s = opts.unwrap_or_else(|| "{}".to_string());

    Some(format!("Helpers.{js_helper}({text}, {url}, {opts_s})"))
}

/// Lower a single URL-position argument to a TS expression that
/// produces the URL string at render time.
fn ts_emit_url_arg(arg: &Expr, ctx: &TsViewCtx) -> Option<String> {
    match &*arg.node {
        // Bare path helper: `articles_path`, `new_article_path`.
        ExprNode::Send { recv: None, method, args, block: None, .. }
            if method.as_str().ends_with("_path") && args.is_empty() =>
        {
            let js = format!(
                "routeHelpers.{}()",
                lower_first_char(&crate::naming::camelize(method.as_str())),
            );
            Some(js)
        }
        // Path helper with args: `article_path(@article)`,
        // `edit_article_path(@article)` — args can be model
        // references (need `.id`) or ints.
        ExprNode::Send { recv: None, method, args, block: None, .. }
            if method.as_str().ends_with("_path") && !args.is_empty() =>
        {
            let fn_js = lower_first_char(&crate::naming::camelize(method.as_str()));
            let emitted_args: Vec<String> = args
                .iter()
                .map(|a| ts_emit_path_arg(a, ctx))
                .collect();
            Some(format!("routeHelpers.{fn_js}({})", emitted_args.join(", ")))
        }
        // Bare model reference — `@article` / `article`. Resolve
        // to the model's show path via its singular path helper.
        ExprNode::Var { name, .. } | ExprNode::Ivar { name } => {
            let local = name.as_str();
            if !ctx.is_local(local) {
                return None;
            }
            // Infer the path helper from the local's name. For
            // `article` this is `articlePath(article.id)`. We
            // trust the view-arg naming convention; a type-driven
            // check would be cleaner once typed IR is wired through
            // view emit.
            Some(format!("routeHelpers.{local}Path({local}.id)"))
        }
        // Same as above, but for partial-scope locals ingested as
        // bare `Send { recv: None, method: <local>, args: [] }`.
        ExprNode::Send {
            recv: None,
            method,
            args,
            block: None,
            ..
        } if args.is_empty() && ctx.is_local(method.as_str()) => {
            let local = method.as_str();
            Some(format!("routeHelpers.{local}Path({local}.id)"))
        }
        // String literal — just pass through.
        ExprNode::Lit { value: Literal::Str { .. } } => {
            Some(emit_ts_view_expr_raw(arg, ctx))
        }
        // Array form for nested resources: `[comment.article,
        // comment]` → `articleCommentPath(article_id, comment_id)`.
        // The path-helper name composes the singular of each
        // element's model; the positional args are each element's
        // id. For parent-model access (first element is `comment.
        // article`), use the FK on the owner (`comment.article_id`)
        // since the association may not be loaded.
        ExprNode::Array { elements, .. } if elements.len() >= 2 => {
            ts_emit_nested_path(elements, ctx)
        }
        _ => None,
    }
}

/// Lower a `[parent, child]` or `[parent.assoc, child]` array-URL
/// form to a nested-resource path-helper call. Returns None when
/// the element shapes aren't recognized.
fn ts_emit_nested_path(elements: &[Expr], ctx: &TsViewCtx) -> Option<String> {
    let mut singulars: Vec<String> = Vec::new();
    let mut args: Vec<String> = Vec::new();
    for el in elements {
        let (singular, id_expr) = ts_classify_nested_element(el, ctx)?;
        singulars.push(singular);
        args.push(id_expr);
    }
    // Compose the Rails-style nested path helper name:
    //   [article, comment] → articleCommentPath
    // The last segment stays singular; intermediate segments also
    // stay singular for the typical one-to-one-per-level nesting
    // (scaffold generator outputs this shape).
    let mut name = String::new();
    for (i, s) in singulars.iter().enumerate() {
        if i == 0 {
            name.push_str(s);
        } else {
            name.push_str(&crate::naming::camelize(s));
        }
    }
    name.push_str("Path");
    Some(format!("routeHelpers.{name}({})", args.join(", ")))
}

/// Classify one element of the nested-URL array. Thin adapter
/// around the shared `classify_nested_url_element` classifier —
/// renders the IR-level variant to `(singular, js_id_expr)` in TS
/// conventions.
fn ts_classify_nested_element(el: &Expr, ctx: &TsViewCtx) -> Option<(String, String)> {
    let is_local = |n: &str| ctx.is_local(n);
    let kind = crate::lower::classify_nested_url_element(el, &is_local)?;
    Some(match kind {
        crate::lower::NestedUrlElement::DirectLocal { name } => {
            (name.to_string(), format!("{name}.id"))
        }
        crate::lower::NestedUrlElement::Association { owner, assoc } => {
            (assoc.to_string(), format!("{owner}.{assoc}_id"))
        }
    })
}

/// Lower a positional argument inside a `*_path(...)` call. Model-
/// typed locals get `.id` appended; int-like args pass through.
fn ts_emit_path_arg(arg: &Expr, ctx: &TsViewCtx) -> String {
    match &*arg.node {
        ExprNode::Var { name, .. } | ExprNode::Ivar { name } => {
            if ctx.is_local(name.as_str()) {
                format!("{local}.id", local = name.as_str())
            } else {
                name.as_str().to_string()
            }
        }
        // Partial-bound locals are parsed as bare `Send { recv:
        // None, method: <name>, args: [] }` because the compiled
        // ERB wrapper doesn't formally introduce them as Ruby
        // locals before use. Treat them as local-reads when the
        // name matches a ctx local.
        ExprNode::Send {
            recv: None,
            method,
            args,
            block: None,
            ..
        } if args.is_empty() && ctx.is_local(method.as_str()) => {
            format!("{local}.id", local = method.as_str())
        }
        _ => emit_ts_view_expr_raw(arg, ctx),
    }
}

/// Emit a link_to/button_to opts hash as a TS object literal.
/// `data:` subhash flattens to `data-*` keys (Rails convention);
/// `method:` stays as-is (Helpers.buttonTo handles it).
fn ts_link_opts_literal(entries: &[(Expr, Expr)], ctx: &TsViewCtx) -> String {
    let mut parts: Vec<String> = Vec::new();
    for (k, v) in entries {
        let key = match &*k.node {
            ExprNode::Lit { value: Literal::Sym { value } } => value.as_str().to_string(),
            ExprNode::Lit { value: Literal::Str { value } } => value.clone(),
            _ => continue,
        };
        // `data: { turbo_confirm: "..." }` → flatten to
        // `"data-turbo-confirm": "..."`.
        if key == "data" {
            if let ExprNode::Hash { entries: data_entries, .. } = &*v.node {
                for (dk, dv) in data_entries {
                    let dk_str = match &*dk.node {
                        ExprNode::Lit { value: Literal::Sym { value } } => {
                            value.as_str().replace('_', "-")
                        }
                        ExprNode::Lit { value: Literal::Str { value } } => {
                            value.replace('_', "-")
                        }
                        _ => continue,
                    };
                    let dv_s = if is_ts_simple_expr(dv, ctx) {
                        emit_ts_view_expr_raw(dv, ctx)
                    } else {
                        continue;
                    };
                    parts.push(format!("\"data-{dk_str}\": {dv_s}"));
                }
                continue;
            }
        }
        // `method: :delete` or `method: "delete"` — normalize to
        // a string (the buttonTo helper compares string).
        let val = match &*v.node {
            ExprNode::Lit { value: Literal::Sym { value } } => format!("{:?}", value.as_str()),
            _ if is_ts_simple_expr(v, ctx) => emit_ts_view_expr_raw(v, ctx),
            _ => continue,
        };
        parts.push(format!("{key:?}: {val}"));
    }
    format!("{{ {} }}", parts.join(", "))
}

fn ts_hash_to_object_literal(
    entries: &[(Expr, Expr)],
    ctx: &TsViewCtx,
) -> String {
    let mut parts = Vec::new();
    for (k, v) in entries {
        let key = match &*k.node {
            ExprNode::Lit { value: Literal::Sym { value } } => value.as_str().to_string(),
            ExprNode::Lit { value: Literal::Str { value } } => value.clone(),
            _ => continue,
        };
        // Special-case `class:` value lowering — the scaffold uses
        // `class: [base_string, {cond_class: cond_expr, ...}]`
        // where cond_expr is one of `.errors[:field].none?` /
        // `.any?`. Fold the hash into a conditional JS expression
        // so the final class attribute matches Rails.
        let val = if key == "class" {
            ts_class_value(v, ctx)
        } else if is_ts_simple_expr(v, ctx) {
            emit_ts_view_expr_raw(v, ctx)
        } else if let ExprNode::Array { elements, .. } = &*v.node {
            match elements.first() {
                Some(first) if is_ts_simple_expr(first, ctx) => emit_ts_view_expr_raw(first, ctx),
                _ => "\"\"".to_string(),
            }
        } else {
            "\"\"".to_string()
        };
        parts.push(format!("{key:?}: {val}"));
    }
    format!("{{ {} }}", parts.join(", "))
}

/// Lower a `class:` option value. Handles three shapes:
///   - Simple (literal/interp): emit as-is
///   - `[base_string, {cond_class: cond_expr, ...}]`: emit a JS
///     conditional-string expression
///   - Anything else: `""` placeholder
fn ts_class_value(v: &Expr, ctx: &TsViewCtx) -> String {
    let is_local = |n: &str| ctx.is_local(n);
    let simple_as_js = |e: &Expr| -> Option<String> {
        if is_ts_simple_expr(e, ctx) {
            Some(emit_ts_view_expr_raw(e, ctx))
        } else {
            None
        }
    };
    match crate::lower::classify_class_value(v, &is_local) {
        crate::lower::ClassValueShape::Simple { expr } => {
            simple_as_js(expr).unwrap_or_else(|| "\"\"".to_string())
        }
        crate::lower::ClassValueShape::Conditional { base, clauses } => {
            let Some(base_js) = simple_as_js(base) else {
                return "\"\"".to_string();
            };
            if clauses.is_empty() {
                return base_js;
            }
            // Prepend a space so the combined class list stays
            // well-formed (base + " " + cond_class).
            let extras: Vec<String> = clauses
                .iter()
                .map(|(cls_text, pred)| {
                    let cond_js = ts_render_errors_field_predicate(pred);
                    format!(" + ({cond_js} ? \" {cls_text}\" : \"\")")
                })
                .collect();
            format!("({base_js}{})", extras.join(""))
        }
        crate::lower::ClassValueShape::Unknown => "\"\"".to_string(),
    }
}

/// Render a classified errors-field predicate to a TS boolean
/// expression via `Helpers.fieldHasError`.
fn ts_render_errors_field_predicate(pred: &crate::lower::ErrorsFieldPredicate<'_>) -> String {
    let call = format!(
        "Helpers.fieldHasError({}.errors, {:?})",
        pred.record, pred.field,
    );
    if pred.expect_present {
        call
    } else {
        format!("!{call}")
    }
}

fn emit_ts_captured_helper(
    method: &str,
    args: &[Expr],
    block: &Expr,
    ctx: &TsViewCtx,
) -> String {
    let ExprNode::Lambda { params, body, .. } = &*block.node else {
        return format!("_buf += \"\"; /* TODO ERB: {method} */");
    };
    let cls_expr = args
        .iter()
        .find_map(|a| ts_extract_kwarg(a, "class"))
        .filter(|e| is_ts_simple_expr(e, ctx))
        .map(|e| emit_ts_view_expr_raw(e, ctx))
        .unwrap_or_else(|| "\"\"".to_string());
    // `form_with(model: record, ...)` — extract the record
    // expression. It becomes the FormBuilder's record arg
    // (drives field values + persisted check) and feeds the
    // action URL derivation. When the extraction doesn't find a
    // simple Var-like reference (e.g. `form_with model: [parent,
    // Child.new]` for nested resources), fall back to the
    // view's primary arg — Rails' partial convention is that
    // the partial's first local IS the form record (`_form.html.
    // erb(article)` / `_form.html.erb(comment)`), so using
    // arg_name gives us correct values + persisted state for
    // every scaffold.
    // Model extraction — three shapes handled:
    //  1. Simple local / ivar: `model: @article` or `model:
    //     article` (partial convention)
    //  2. Array form for nested resources: `model: [@article,
    //     Comment.new]` — the LAST element is the form's record;
    //     the preceding elements are parent records that scope
    //     the action URL through a nested path helper
    //  3. No `model:` kwarg: fall back to the view's primary arg
    //     (Rails partial convention).
    let model_kwarg = args
        .iter()
        .find_map(|a| ts_extract_kwarg(a, "model"));
    let model_nested = model_kwarg.and_then(|e| match &*e.node {
        ExprNode::Array { elements, .. } if elements.len() >= 2 => Some(elements.clone()),
        _ => None,
    });
    let model_expr = if model_nested.is_some() {
        // Last element is the form's record; emit it directly.
        let elems = model_nested.as_ref().unwrap();
        Some(emit_ts_nested_form_record(&elems[elems.len() - 1]))
    } else {
        model_kwarg
            .filter(|e| is_ts_simple_expr(e, ctx))
            .map(|e| emit_ts_view_expr_raw(e, ctx))
            .or_else(|| {
                if !ctx.arg_name.is_empty() && ctx.is_local(&ctx.arg_name) {
                    Some(ctx.arg_name.clone())
                } else {
                    None
                }
            })
    };
    let inner_body = match method {
        "form_with" => {
            let pname = params.first().map(|p| p.as_str()).unwrap_or("form");
            let record_arg = model_expr.clone().unwrap_or_else(|| "null".to_string());
            // Prefix is the resource's singular name. For nested
            // array-form models, the prefix is the CHILD class
            // (last element of the array); otherwise it's the
            // view's resource-dir singularized.
            let prefix = if let Some(elems) = &model_nested {
                ts_nested_form_child_prefix(&elems[elems.len() - 1])
                    .unwrap_or_else(|| ctx.arg_name.clone())
            } else if !ctx.resource_dir.is_empty() {
                crate::naming::singularize(&ctx.resource_dir)
            } else {
                ctx.arg_name.clone()
            };
            let mut lines = vec![
                "{".to_string(),
                "  let _buf = \"\";".to_string(),
                format!(
                    "  const {pname} = new Helpers.FormBuilder({record_arg} as any, \"{prefix}\");",
                ),
                // Automatic validation-error display — renders the
                // scaffold error block when the record has errors,
                // empty otherwise. Replaces the scaffold's
                // `<% if record.errors.any? %>…<% end %>` block,
                // which currently stubs to `if (false) { … }` in
                // the generic view emit. Until the conditional +
                // iteration emission catches up, this helper gives
                // us the E2E-scenario error display for free.
                format!(
                    "  _buf += Helpers.errorMessagesFor({record_arg} as any, \"{prefix}\");",
                ),
            ];
            let inner_ctx = TsViewCtx {
                locals: {
                    let mut l = ctx.locals.clone();
                    l.push(pname.to_string());
                    l
                },
                arg_name: ctx.arg_name.clone(),
                arg_attrs: ctx.arg_attrs.clone(),
                resource_dir: ctx.resource_dir.clone(),
                stylesheets: ctx.stylesheets.clone(),
            };
            for line in emit_ts_view_body(body, &inner_ctx) {
                lines.push(format!("  {line}"));
            }
            // Action URL — computed at runtime from the record +
            // resource-path helper. For new (no id) it's the
            // collection path; for existing it's the member path.
            let plural = if !ctx.resource_dir.is_empty() {
                ctx.resource_dir.clone()
            } else {
                crate::naming::pluralize_snake(&prefix)
            };
            let plural_camel = lower_first_char(&crate::naming::camelize(&plural));
            let singular_camel = lower_first_char(&crate::naming::camelize(&prefix));
            let record_ref = model_expr.clone().unwrap_or_else(|| "null".to_string());
            let path_expr = if let Some(elems) = &model_nested {
                // Nested: `[parent, child]` → compose a nested
                // path helper like `articleCommentsPath(article.id)`
                // for a new child, or `articleCommentPath(
                // article.id, comment.id)` for an existing child.
                ts_nested_form_path_expr(elems, ctx, &record_ref, &prefix)
            } else {
                format!(
                    "(({record_ref} as any)?.id ? routeHelpers.{singular_camel}Path(({record_ref} as any).id) : routeHelpers.{plural_camel}Path())",
                )
            };
            lines.push(format!(
                "  return Helpers.formWrap({record_arg} as any, {path_expr}, {cls_expr}, _buf);",
            ));
            lines.push("}".to_string());
            lines.join("\n  ")
        }
        _ => "\"\"".to_string(),
    };
    match method {
        "form_with" => format!("_buf += (() => {inner_body})();"),
        "content_for" => {
            let _ = args;
            let _ = cls_expr;
            format!("/* content_for stashed */")
        }
        _ => format!("_buf += \"\";"),
    }
}

fn emit_ts_render_call(arg: &Expr, ctx: &TsViewCtx) -> String {
    match &*arg.node {
        ExprNode::Var { name, .. } | ExprNode::Ivar { name }
            if ctx.is_local(name.as_str()) =>
        {
            let plural_pascal = crate::naming::camelize(name.as_str());
            let singular_pascal =
                crate::naming::camelize(&crate::naming::singularize(name.as_str()));
            let partial_fn = format!("Views.render{plural_pascal}{singular_pascal}");
            let coll = name.to_string();
            format!(
                "_buf += {coll}.map((__r: any) => {partial_fn}(__r)).join(\"\");",
            )
        }
        ExprNode::Send { recv: Some(r), method, args, .. }
            if args.is_empty()
                && matches!(&*r.node, ExprNode::Var { .. } | ExprNode::Ivar { .. }) =>
        {
            let assoc_plural = method.as_str();
            let plural_pascal = crate::naming::camelize(assoc_plural);
            let singular_pascal =
                crate::naming::camelize(&crate::naming::singularize(assoc_plural));
            let partial_fn = format!("Views.render{plural_pascal}{singular_pascal}");
            let parent_name = match &*r.node {
                ExprNode::Var { name, .. } | ExprNode::Ivar { name } => name.to_string(),
                _ => unreachable!(),
            };
            // Use the Juntos CollectionProxy accessor (model getter).
            format!(
                "_buf += {parent_name}.{assoc_plural}.map((__c: any) => {partial_fn}(__c)).join(\"\");",
            )
        }
        _ => "_buf += \"\"; /* TODO ERB: render */".to_string(),
    }
}

/// Emit the record expression for the CHILD element of a nested
/// form_with's `model:` array (`[parent, Child.new]` or similar).
/// Dispatches on the shared `NestedFormChild` classifier.
fn emit_ts_nested_form_record(el: &Expr) -> String {
    match crate::lower::classify_nested_form_child(el) {
        Some(crate::lower::NestedFormChild::ClassNew { class }) => format!("new {class}()"),
        Some(crate::lower::NestedFormChild::Local { name }) => name.to_string(),
        None => "null".to_string(),
    }
}

/// Extract the singular prefix for a nested form_with's child
/// element — `Comment.new` → `"comment"`, bare `comment` local →
/// `"comment"`.
fn ts_nested_form_child_prefix(el: &Expr) -> Option<String> {
    crate::lower::classify_nested_form_child(el).map(|k| k.prefix())
}

/// Build the form action URL for a nested-resource form_with.
/// `elems` is the `[parent, child]` (or deeper-nested) array;
/// `record_ref` is the JS expression for the child. When the
/// child has an `id` (persisted), emit the member path; otherwise
/// the collection path.
fn ts_nested_form_path_expr(
    elems: &[Expr],
    ctx: &TsViewCtx,
    record_ref: &str,
    child_prefix: &str,
) -> String {
    // Parent ids — everything except the last element.
    let mut parent_ids: Vec<String> = Vec::new();
    let mut parent_singulars: Vec<String> = Vec::new();
    for parent in &elems[..elems.len() - 1] {
        let (singular, id_expr) = match ts_classify_nested_element(parent, ctx) {
            Some(x) => x,
            None => return format!("\"\" /* TODO: nested form parent */"),
        };
        parent_singulars.push(singular);
        parent_ids.push(id_expr);
    }
    // Compose two helper names: member (for persisted child) and
    // collection (for new child).
    let mut member_name = String::new();
    for (i, s) in parent_singulars.iter().enumerate() {
        if i == 0 {
            member_name.push_str(s);
        } else {
            member_name.push_str(&crate::naming::camelize(s));
        }
    }
    member_name.push_str(&crate::naming::camelize(child_prefix));
    member_name.push_str("Path");
    let mut collection_name = String::new();
    for (i, s) in parent_singulars.iter().enumerate() {
        if i == 0 {
            collection_name.push_str(s);
        } else {
            collection_name.push_str(&crate::naming::camelize(s));
        }
    }
    let child_plural = crate::naming::pluralize_snake(child_prefix);
    collection_name.push_str(&crate::naming::camelize(&child_plural));
    collection_name.push_str("Path");
    let parent_args = parent_ids.join(", ");
    format!(
        "(({record_ref} as any)?.id ? routeHelpers.{member_name}({parent_args}, ({record_ref} as any).id) : routeHelpers.{collection_name}({parent_args}))",
    )
}

fn ts_extract_kwarg<'a>(arg: &'a Expr, key: &str) -> Option<&'a Expr> {
    if let ExprNode::Hash { entries, .. } = &*arg.node {
        for (k, v) in entries {
            if let ExprNode::Lit { value: Literal::Sym { value } } = &*k.node {
                if value.as_str() == key {
                    return Some(v);
                }
            }
        }
    }
    None
}

fn unwrap_to_s_ts(expr: &Expr) -> &Expr {
    if let ExprNode::Send { recv: Some(inner), method, args, .. } = &*expr.node {
        if method.as_str() == "to_s" && args.is_empty() {
            return inner;
        }
    }
    expr
}

fn ts_string_literal(s: &str) -> String {
    // JSON-string encoding — safe for TS string literals.
    serde_json::to_string(s).unwrap_or_else(|_| format!("{s:?}"))
}

fn is_ts_simple_expr(expr: &Expr, ctx: &TsViewCtx) -> bool {
    match &*expr.node {
        ExprNode::Lit { .. } => true,
        ExprNode::Var { name, .. } | ExprNode::Ivar { name } => ctx.is_local(name.as_str()),
        // Partial-scope locals land as `Send { recv: None, method:
        // <name>, args: [] }` because the ERB wrapper doesn't
        // formally declare them before use. Treat them as simple
        // local-reads when the method name matches a ctx local.
        ExprNode::Send {
            recv: None,
            method,
            args,
            block: None,
            ..
        } if args.is_empty() && ctx.is_local(method.as_str()) => true,
        ExprNode::Send { recv: Some(r), method, args, block, .. } => {
            if !args.is_empty() || block.is_some() {
                return false;
            }
            let clean = method.as_str().trim_end_matches('?').trim_end_matches('!');
            if clean.is_empty() {
                return false;
            }
            let recv_local = match &*r.node {
                ExprNode::Var { name, .. } | ExprNode::Ivar { name } => Some(name.as_str()),
                ExprNode::Send {
                    recv: None,
                    method: m,
                    args: ra,
                    block: None,
                    ..
                } if ra.is_empty() && ctx.is_local(m.as_str()) => Some(m.as_str()),
                _ => None,
            };
            if let Some(local_name) = recv_local {
                if ctx.arg_has_attr(local_name, clean) {
                    return true;
                }
                if ctx.is_local(local_name)
                    && matches!(method.as_str(), "any?" | "none?" | "present?" | "empty?")
                {
                    return true;
                }
                // Allow association / computed reads on a known
                // local (`article.comments`). Can't know the exact
                // attribute set for associations without runtime
                // introspection, so accept any no-arg method as a
                // simple read. The emit produces `local.method`
                // which either resolves at runtime or errors
                // loudly — compare-tool divergence makes the gap
                // visible.
                if ctx.is_local(local_name) {
                    return true;
                }
            }
            // Chained simple Sends — `article.comments.size`. The
            // outer recv is itself a simple Send; recurse.
            if is_ts_simple_expr(r, ctx) {
                return true;
            }
            false
        }
        ExprNode::StringInterp { parts } => parts.iter().all(|p| match p {
            crate::expr::InterpPart::Text { .. } => true,
            crate::expr::InterpPart::Expr { expr } => is_ts_simple_expr(expr, ctx),
        }),
        _ => false,
    }
}

fn emit_ts_view_expr_raw(expr: &Expr, ctx: &TsViewCtx) -> String {
    match &*expr.node {
        ExprNode::Lit { value } => emit_literal(value),
        ExprNode::Var { name, .. } | ExprNode::Ivar { name } => name.to_string(),
        // Partial-scope locals parsed as bare-Send — emit as the
        // bare name. See the matching case in is_ts_simple_expr.
        ExprNode::Send {
            recv: None,
            method,
            args,
            block: None,
            ..
        } if args.is_empty() && ctx.is_local(method.as_str()) => method.to_string(),
        ExprNode::Send { recv: Some(r), method, args, .. } => {
            let method_s = method.as_str();
            // Collection predicates.
            if args.is_empty() {
                if let ExprNode::Var { name, .. } | ExprNode::Ivar { name } = &*r.node {
                    if ctx.is_local(name.as_str()) {
                        match method_s {
                            "any?" | "present?" => {
                                return format!("({name}.length > 0)");
                            }
                            "none?" | "empty?" => {
                                return format!("({name}.length === 0)");
                            }
                            _ => {}
                        }
                    }
                }
            }
            let recv_s = emit_ts_view_expr_raw(r, ctx);
            let clean = method_s.trim_end_matches('?').trim_end_matches('!');
            if args.is_empty() {
                format!("{recv_s}.{clean}")
            } else {
                let args_s: Vec<String> =
                    args.iter().map(|a| emit_ts_view_expr_raw(a, ctx)).collect();
                format!("{recv_s}.{clean}({})", args_s.join(", "))
            }
        }
        ExprNode::StringInterp { parts } => {
            use crate::expr::InterpPart;
            let mut out = String::from("`");
            for p in parts {
                match p {
                    InterpPart::Text { value } => {
                        for c in value.chars() {
                            if c == '`' || c == '\\' || c == '$' {
                                out.push('\\');
                            }
                            out.push(c);
                        }
                    }
                    InterpPart::Expr { expr } => {
                        out.push_str("${");
                        out.push_str(&emit_ts_view_expr_raw(expr, ctx));
                        out.push('}');
                    }
                }
            }
            out.push('`');
            out
        }
        _ => "\"\"".to_string(),
    }
}

/// Walk the view body and collect every ivar name referenced, in
/// source order, de-duplicated. These names are destructured out of
/// `locals` at the top of the view function so the rewritten body
/// can reference them as bare locals.
fn collect_ivar_names(expr: &Expr) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    collect_ivars_into(expr, &mut out);
    out
}

fn collect_ivars_into(expr: &Expr, out: &mut Vec<String>) {
    match &*expr.node {
        ExprNode::Ivar { name } => {
            let n = name.to_string();
            if !out.iter().any(|existing| existing == &n) {
                out.push(n);
            }
        }
        ExprNode::Assign { target, value } => {
            if let LValue::Ivar { name } = target {
                let n = name.to_string();
                if !out.iter().any(|existing| existing == &n) {
                    out.push(n);
                }
            }
            collect_ivars_into(value, out);
        }
        ExprNode::Send { recv, args, block, .. } => {
            if let Some(r) = recv {
                collect_ivars_into(r, out);
            }
            for a in args {
                collect_ivars_into(a, out);
            }
            if let Some(b) = block {
                collect_ivars_into(b, out);
            }
        }
        ExprNode::Seq { exprs } => {
            for e in exprs {
                collect_ivars_into(e, out);
            }
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            collect_ivars_into(cond, out);
            collect_ivars_into(then_branch, out);
            collect_ivars_into(else_branch, out);
        }
        ExprNode::BoolOp { left, right, .. } => {
            collect_ivars_into(left, out);
            collect_ivars_into(right, out);
        }
        ExprNode::Array { elements, .. } => {
            for e in elements {
                collect_ivars_into(e, out);
            }
        }
        ExprNode::Hash { entries, .. } => {
            for (k, v) in entries {
                collect_ivars_into(k, out);
                collect_ivars_into(v, out);
            }
        }
        ExprNode::Lambda { body, .. } => collect_ivars_into(body, out),
        ExprNode::Apply { fun, args, block } => {
            collect_ivars_into(fun, out);
            for a in args {
                collect_ivars_into(a, out);
            }
            if let Some(b) = block {
                collect_ivars_into(b, out);
            }
        }
        ExprNode::Let { value, body, .. } => {
            collect_ivars_into(value, out);
            collect_ivars_into(body, out);
        }
        ExprNode::Case { scrutinee, arms } => {
            collect_ivars_into(scrutinee, out);
            for arm in arms {
                if let Some(g) = &arm.guard {
                    collect_ivars_into(g, out);
                }
                collect_ivars_into(&arm.body, out);
            }
        }
        ExprNode::Yield { args } => {
            for a in args {
                collect_ivars_into(a, out);
            }
        }
        ExprNode::Raise { value } => collect_ivars_into(value, out),
        ExprNode::RescueModifier { expr, fallback } => {
            collect_ivars_into(expr, out);
            collect_ivars_into(fallback, out);
        }
        ExprNode::StringInterp { parts } => {
            for p in parts {
                if let crate::expr::InterpPart::Expr { expr } = p {
                    collect_ivars_into(expr, out);
                }
            }
        }
        // Leaves without subexpressions.
        ExprNode::Lit { .. }
        | ExprNode::Var { .. }
        | ExprNode::Const { .. } => {}
    }
}

/// `articles/index` → `renderArticlesIndex`. Slashes become PascalCase
/// separators; partial-prefix underscores stay lowercased at the head
/// of the segment (`_article` → `Article` — underscore dropped).
fn view_function_name(name: &str) -> String {
    let mut out = String::from("render");
    for seg in name.split('/') {
        let trimmed = seg.strip_prefix('_').unwrap_or(seg);
        for word in trimmed.split('_') {
            let mut chars = word.chars();
            if let Some(first) = chars.next() {
                out.push(first.to_ascii_uppercase());
                out.push_str(chars.as_str());
            }
        }
    }
    out
}


// Routes ---------------------------------------------------------------

/// Emit the Juntos-shaped routes file: `Router.root(...)` for any
/// `root "c#a"` entries and `Router.resources("name", XController,
/// { nested: [...] })` for each `resources :name` block. Per-
/// controller imports go at the top. Matches ruby2js's rails routes
/// filter (`spec/rails_routes_spec.rb`).
fn emit_routes(app: &App) -> EmittedFile {
    // Collect every controller class-name the routes reference, so
    // we can emit one import per controller. Uses a Vec (preserving
    // source order) with de-duplication; a HashSet would lose
    // ordering and the deterministic import layout is nicer for
    // source-equivalence down the road.
    let mut controllers: Vec<String> = Vec::new();
    let mut push_controller = |name: String, out: &mut Vec<String>| {
        if !out.iter().any(|c| c == &name) {
            out.push(name);
        }
    };
    for entry in &app.routes.entries {
        collect_controller_refs(entry, &mut controllers, &mut push_controller);
    }

    let mut s = String::new();
    writeln!(s, "// Generated by Roundhouse.").unwrap();
    writeln!(s, "import {{ Router }} from \"juntos\";").unwrap();
    for controller in &controllers {
        let file_stem = crate::naming::snake_case(controller);
        writeln!(
            s,
            "import {{ {controller} }} from \"../app/controllers/{file_stem}.js\";"
        )
        .unwrap();
    }
    writeln!(s).unwrap();
    for entry in &app.routes.entries {
        emit_route_spec(&mut s, entry);
    }
    EmittedFile { path: PathBuf::from("src/routes.ts"), content: s }
}

/// Walk the route spec tree and push every controller class-name
/// into `out` (via the dedup closure). Follows nested resources.
fn collect_controller_refs(
    spec: &RouteSpec,
    out: &mut Vec<String>,
    push: &mut impl FnMut(String, &mut Vec<String>),
) {
    match spec {
        RouteSpec::Explicit { controller, .. } => {
            push(controller.0.to_string(), out);
        }
        RouteSpec::Root { target } => {
            if let Some((c, _)) = target.split_once('#') {
                push(controller_class_name(c), out);
            }
        }
        RouteSpec::Resources { name, nested, .. } => {
            push(controller_class_name(name.as_str()), out);
            for child in nested {
                collect_controller_refs(child, out, push);
            }
        }
    }
}

/// Emit one route spec as a Router.* call. Nested resources appear
/// as a `{ nested: [...] }` option list on the outer resources call.
fn emit_route_spec(out: &mut String, spec: &RouteSpec) {
    match spec {
        RouteSpec::Explicit { method, path, controller, action, .. } => {
            // Ruby2js doesn't have a direct Router shortcut for raw
            // verb routes; emit a low-level `Router.add` call so the
            // route still registers. No fixture exercises this path
            // (tiny-blog has explicit routes; real-blog uses
            // resources) — sharpen when one does.
            let verb = match method {
                crate::dialect::HttpMethod::Get => "get",
                crate::dialect::HttpMethod::Post => "post",
                crate::dialect::HttpMethod::Put => "put",
                crate::dialect::HttpMethod::Patch => "patch",
                crate::dialect::HttpMethod::Delete => "delete",
                crate::dialect::HttpMethod::Head => "head",
                crate::dialect::HttpMethod::Options => "options",
                crate::dialect::HttpMethod::Any => "match",
            };
            writeln!(
                out,
                "Router.{verb}({:?}, {}, {:?});",
                path,
                controller.0,
                action.as_str(),
            )
            .unwrap();
        }
        RouteSpec::Root { target } => {
            let (controller, action) = target
                .split_once('#')
                .map(|(c, a)| (controller_class_name(c), a.to_string()))
                .unwrap_or_else(|| (target.clone(), "index".to_string()));
            writeln!(out, "Router.root(\"/\", {controller}, {action:?});").unwrap();
        }
        RouteSpec::Resources { name, only, except: _, nested } => {
            let controller = controller_class_name(name.as_str());
            let mut opts: Vec<String> = Vec::new();
            if !only.is_empty() {
                let parts: Vec<String> =
                    only.iter().map(|s| format!("{:?}", s.as_str())).collect();
                opts.push(format!("only: [{}]", parts.join(", ")));
            }
            // except: is handled at build time per ruby2js's filter —
            // not passed to Router.resources at runtime. We mirror.
            if !nested.is_empty() {
                let mut nested_parts: Vec<String> = Vec::new();
                for child in nested {
                    if let Some(part) = nested_spec_entry(child) {
                        nested_parts.push(part);
                    }
                }
                if !nested_parts.is_empty() {
                    opts.push(format!("nested: [{}]", nested_parts.join(", ")));
                }
            }
            if opts.is_empty() {
                writeln!(out, "Router.resources({:?}, {});", name.as_str(), controller).unwrap();
            } else {
                writeln!(
                    out,
                    "Router.resources({:?}, {}, {{{}}});",
                    name.as_str(),
                    controller,
                    opts.join(", "),
                )
                .unwrap();
            }
        }
    }
}

/// Turn a nested child (from `resources :x do ... end`) into the
/// object-literal entry Juntos expects inside the `nested: [...]`
/// array: `{ name: "comments", controller: CommentsController, only: [...] }`.
fn nested_spec_entry(spec: &RouteSpec) -> Option<String> {
    let RouteSpec::Resources { name, only, except: _, nested, .. } = spec else {
        // Non-resources entries inside a nested block (e.g. raw
        // `get "/x"`) don't have a direct Router.resources nested
        // shape; skip for now — no fixture uses this pattern.
        return None;
    };
    let controller = controller_class_name(name.as_str());
    let mut parts: Vec<String> = Vec::new();
    parts.push(format!("name: {:?}", name.as_str()));
    parts.push(format!("controller: {controller}"));
    if !only.is_empty() {
        let os: Vec<String> = only.iter().map(|s| format!("{:?}", s.as_str())).collect();
        parts.push(format!("only: [{}]", os.join(", ")));
    }
    if !nested.is_empty() {
        let mut inner: Vec<String> = Vec::new();
        for child in nested {
            if let Some(p) = nested_spec_entry(child) {
                inner.push(p);
            }
        }
        if !inner.is_empty() {
            parts.push(format!("nested: [{}]", inner.join(", ")));
        }
    }
    Some(format!("{{{}}}", parts.join(", ")))
}

fn controller_class_name(short: &str) -> String {
    let mut s = crate::naming::camelize(short);
    s.push_str("Controller");
    s
}

// Body + expressions ---------------------------------------------------

fn emit_body(body: &Expr, return_ty: &Ty) -> String {
    let is_void = matches!(return_ty, Ty::Nil);
    match &*body.node {
        ExprNode::Assign { target: LValue::Ivar { .. }, value } => {
            if is_void {
                format!("{};", emit_expr(value))
            } else {
                format!("return {};", emit_expr(value))
            }
        }
        ExprNode::Seq { exprs } if !exprs.is_empty() => {
            let mut lines: Vec<String> = Vec::new();
            for (i, e) in exprs.iter().enumerate() {
                lines.push(emit_stmt(e, i == exprs.len() - 1, is_void));
            }
            lines.join("\n")
        }
        _ => {
            if is_void {
                format!("{};", emit_expr(body))
            } else {
                format!("return {};", emit_expr(body))
            }
        }
    }
}

fn emit_stmt(e: &Expr, is_last: bool, void_return: bool) -> String {
    match &*e.node {
        ExprNode::Assign { target: LValue::Var { name, .. }, value } => {
            format!("const {} = {};", name, emit_expr(value))
        }
        ExprNode::Assign { target: LValue::Ivar { name }, value } => {
            format!("this.{} = {};", ts_field_name(name.as_str()), emit_expr(value))
        }
        _ => {
            if is_last && !void_return {
                format!("return {};", emit_expr(e))
            } else {
                format!("{};", emit_expr(e))
            }
        }
    }
}

fn emit_expr(e: &Expr) -> String {
    match &*e.node {
        ExprNode::Lit { value } => emit_literal(value),
        ExprNode::Const { path } => {
            path.iter().map(|s| s.to_string()).collect::<Vec<_>>().join(".")
        }
        ExprNode::Var { name, .. } => name.to_string(),
        ExprNode::Ivar { name } => format!("this.{}", ts_field_name(name.as_str())),
        ExprNode::Send { recv, method, args, parenthesized, .. } => {
            emit_send_with_parens(recv.as_ref(), method.as_str(), args, *parenthesized)
        }
        ExprNode::Assign { target: _, value } => emit_expr(value),
        ExprNode::Seq { exprs } => {
            exprs.iter().map(emit_expr).collect::<Vec<_>>().join("; ")
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            let cond_s = emit_expr(cond);
            let then_s = indent_lines(&emit_expr(then_branch), 1);
            let else_s = indent_lines(&emit_expr(else_branch), 1);
            format!("if ({cond_s}) {{\n{then_s}\n}} else {{\n{else_s}\n}}")
        }
        ExprNode::BoolOp { op, left, right, .. } => {
            use crate::expr::BoolOpKind;
            let op_s = match op {
                BoolOpKind::Or => "||",
                BoolOpKind::And => "&&",
            };
            format!("{} {} {}", emit_expr(left), op_s, emit_expr(right))
        }
        ExprNode::Array { elements, .. } => {
            let parts: Vec<String> = elements.iter().map(emit_expr).collect();
            format!("[{}]", parts.join(", "))
        }
        ExprNode::Hash { entries, .. } => {
            let parts: Vec<String> = entries
                .iter()
                .map(|(k, v)| format!("{}: {}", emit_expr(k), emit_expr(v)))
                .collect();
            format!("{{ {} }}", parts.join(", "))
        }
        ExprNode::StringInterp { parts } => {
            use crate::expr::InterpPart;
            let mut out = String::from("`");
            for p in parts {
                match p {
                    InterpPart::Text { value } => {
                        for c in value.chars() {
                            if c == '`' || c == '\\' {
                                out.push('\\');
                                out.push(c);
                            } else if c == '$' {
                                out.push_str("\\$");
                            } else {
                                out.push(c);
                            }
                        }
                    }
                    InterpPart::Expr { expr } => {
                        out.push_str("${");
                        out.push_str(&emit_expr(expr));
                        out.push('}');
                    }
                }
            }
            out.push('`');
            out
        }
        other => format!("/* TODO: emit {:?} */", std::mem::discriminant(other)),
    }
}

/// Core send emission. `parenthesized` reflects whether the Ruby
/// source wrapped args in explicit parens — for 0-arg explicit-
/// receiver calls we use it to decide between `recv.name` (Ruby
/// reader convention, JS property access) and `recv.name()` (method
/// call). Always emits parens when args are present.
fn emit_send_with_parens(
    recv: Option<&Expr>,
    method: &str,
    args: &[Expr],
    parenthesized: bool,
) -> String {
    let args_s: Vec<String> = args.iter().map(emit_expr).collect();
    if method == "[]" && recv.is_some() {
        return format!("{}[{}]", emit_expr(recv.unwrap()), args_s.join(", "));
    }
    // Ruby stdlib method → TS equivalent, when the Ruby name collides
    // with a nonexistent TS property. Keyed on name only today; a
    // receiver-typed dispatch would replace this when per-type
    // mappings diverge.
    let (mapped_name, force_parens) = match method {
        "strip" => ("trim", true),
        _ => (method, false),
    };
    let ts_m = ts_method_name(mapped_name);
    match recv {
        None => {
            if args_s.is_empty() {
                ts_m
            } else {
                format!("{}({})", ts_m, args_s.join(", "))
            }
        }
        Some(r) => {
            let recv_s = emit_expr(r);
            if args_s.is_empty() && !parenthesized && !force_parens {
                // Ruby's `obj.name` without parens is typically a
                // reader; Juntos mirrors that with a property
                // accessor / getter, so emit without parens.
                format!("{recv_s}.{ts_m}")
            } else {
                format!("{recv_s}.{ts_m}({})", args_s.join(", "))
            }
        }
    }
}

fn emit_literal(lit: &Literal) -> String {
    match lit {
        Literal::Nil => "null".to_string(),
        Literal::Bool { value } => value.to_string(),
        Literal::Int { value } => value.to_string(),
        Literal::Float { value } => {
            let s = value.to_string();
            if s.contains('.') { s } else { format!("{s}.0") }
        }
        Literal::Str { value } => format!("{value:?}"),
        // Ruby symbols map to string literals — the typed analyzer may
        // refine this into a discriminated-union enum later, but for
        // the scaffold a string is unambiguous and round-trips through
        // comparison as expected.
        Literal::Sym { value } => format!("{:?}", value.as_str()),
    }
}

fn indent_lines(text: &str, depth: usize) -> String {
    let pad = "  ".repeat(depth);
    text.lines()
        .map(|l| format!("{pad}{l}"))
        .collect::<Vec<_>>()
        .join("\n")
}

// Types ----------------------------------------------------------------

pub fn ts_ty(ty: &Ty) -> String {
    match ty {
        Ty::Int | Ty::Float => "number".to_string(),
        Ty::Bool => "boolean".to_string(),
        // Symbols model as string for now. When a pass identifies a
        // closed set of symbols at a given position (enum detection),
        // emit it as a union-of-string-literals instead.
        Ty::Str | Ty::Sym => "string".to_string(),
        Ty::Nil => "null".to_string(),
        Ty::Array { elem } => format!("{}[]", ts_ty(elem)),
        Ty::Hash { key, value } => {
            format!("Record<{}, {}>", ts_ty(key), ts_ty(value))
        }
        Ty::Tuple { elems } => {
            let parts: Vec<String> = elems.iter().map(ts_ty).collect();
            format!("[{}]", parts.join(", "))
        }
        Ty::Record { .. } => "Record<string, unknown>".to_string(),
        Ty::Union { variants } => {
            let parts: Vec<String> = variants.iter().map(ts_ty).collect();
            parts.join(" | ")
        }
        Ty::Class { id, .. } => id.0.to_string(),
        Ty::Fn { .. } => "(...args: unknown[]) => unknown".to_string(),
        Ty::Var { .. } => "unknown".to_string(),
    }
}

// Naming ---------------------------------------------------------------

/// Instance-field name: preserves snake_case. Juntos's ActiveRecord
/// accessors match the Rails column names exactly (`article_id`, not
/// `articleId`), and ruby2js's rails model filter does the same.
/// Single-word idiomatic JS (`title`) is the same either way; the
/// difference is only visible on multi-word names.
fn ts_field_name(ruby_name: &str) -> String {
    ruby_name.to_string()
}

/// Method name: same snake_case preservation as fields. Method calls
/// that should resolve to JS-native APIs (e.g. `findBy` vs Ruby's
/// `find_by`) will need a per-method translation table later; until
/// then, the Rails-side name survives and Juntos maps at runtime.
fn ts_method_name(ruby_name: &str) -> String {
    ruby_name.to_string()
}

// Fixtures + specs ----------------------------------------------------

/// Emit one fixture file as `spec/fixtures/<table>.ts` — a set of
/// named exports, one per fixture record. IDs assigned in insertion
/// order (1..N), matching Rust/Crystal emit.
fn emit_ts_fixture(lowered: &crate::lower::LoweredFixture) -> EmittedFile {
    let fixture_name = lowered.name.as_str();
    let class_name = lowered.class.0.as_str();
    let model_file = crate::naming::snake_case(class_name);

    let mut s = String::new();
    writeln!(s, "// Generated by Roundhouse.").unwrap();
    writeln!(
        s,
        "import {{ {class_name} }} from \"../../app/models/{model_file}.js\";"
    )
    .unwrap();
    writeln!(s, "import {{ FIXTURE_IDS, fixtureId }} from \"../fixtures.js\";").unwrap();

    // `_loadAll` — called from Fixtures.setup inside beforeEach. Each
    // record runs through the model's `save` getter (validations +
    // INSERT via the juntos runtime); the AUTOINCREMENT id lands in
    // FIXTURE_IDS so the named getters and cross-fixture FK refs can
    // find it.
    writeln!(s).unwrap();
    writeln!(s, "export function _loadAll(): void {{").unwrap();
    for record in &lowered.records {
        let label = record.label.as_str();
        writeln!(s, "  {{").unwrap();
        writeln!(s, "    const record = new {class_name}();").unwrap();
        for field in &record.fields {
            let col = field.column.as_str();
            let val = match &field.value {
                crate::lower::LoweredFixtureValue::Literal { ty, raw } => {
                    ts_literal_for(raw, ty)
                }
                crate::lower::LoweredFixtureValue::FkLookup {
                    target_fixture,
                    target_label,
                } => format!(
                    "fixtureId({:?}, {:?})",
                    target_fixture.as_str(),
                    target_label.as_str(),
                ),
            };
            writeln!(s, "    record.{col} = {val};").unwrap();
        }
        writeln!(
            s,
            "    if (!record.save) throw new Error(\"fixture {fixture_name}/{label} failed to save\");",
        )
        .unwrap();
        writeln!(
            s,
            "    FIXTURE_IDS.set(\"{fixture_name}:{label}\", record.id);",
        )
        .unwrap();
        writeln!(s, "  }}").unwrap();
    }
    writeln!(s, "}}").unwrap();

    for record in &lowered.records {
        let label = record.label.as_str();
        writeln!(s).unwrap();
        writeln!(
            s,
            "export function {label}(): {class_name} {{",
        )
        .unwrap();
        writeln!(
            s,
            "  const id = fixtureId({fixture_name:?}, {label:?});"
        )
        .unwrap();
        writeln!(
            s,
            "  const record = {class_name}.find(id);"
        )
        .unwrap();
        writeln!(
            s,
            "  if (!record) throw new Error(\"fixture {fixture_name}/{label} not loaded\");",
        )
        .unwrap();
        writeln!(s, "  return record;").unwrap();
        writeln!(s, "}}").unwrap();
    }

    EmittedFile {
        path: PathBuf::from(format!("spec/fixtures/{fixture_name}.ts")),
        content: s,
    }
}

/// `spec/fixtures.ts` — top-level fixture harness: the shared
/// FIXTURE_IDS map, the `setup()` entrypoint that every spec's
/// `beforeEach` runs, and the `fixtureId` lookup helper.
fn emit_ts_fixtures_helper(lowered: &crate::lower::LoweredFixtureSet) -> EmittedFile {
    let mut s = String::new();
    writeln!(s, "// Generated by Roundhouse.").unwrap();
    writeln!(s, "import {{ setupTestDb }} from \"juntos\";").unwrap();
    writeln!(s, "import {{ CREATE_TABLES }} from \"../src/schema_sql.js\";").unwrap();
    for f in &lowered.fixtures {
        writeln!(
            s,
            "import * as {}Fixtures from \"./fixtures/{}.js\";",
            crate::naming::camelize(f.name.as_str()),
            f.name.as_str(),
        )
        .unwrap();
    }
    writeln!(s).unwrap();
    writeln!(s, "export const FIXTURE_IDS = new Map<string, number>();").unwrap();
    writeln!(s).unwrap();
    writeln!(s, "/** Per-test entry point. Opens a fresh :memory: DB, runs schema").unwrap();
    writeln!(s, " *  DDL, and reloads every fixture in declaration order. */").unwrap();
    writeln!(s, "export function setup(): void {{").unwrap();
    writeln!(s, "  setupTestDb(CREATE_TABLES);").unwrap();
    writeln!(s, "  FIXTURE_IDS.clear();").unwrap();
    for f in &lowered.fixtures {
        writeln!(
            s,
            "  {}Fixtures._loadAll();",
            crate::naming::camelize(f.name.as_str())
        )
        .unwrap();
    }
    writeln!(s, "}}").unwrap();
    writeln!(s).unwrap();
    writeln!(s, "export function fixtureId(fixture: string, label: string): number {{").unwrap();
    writeln!(s, "  const v = FIXTURE_IDS.get(`${{fixture}}:${{label}}`);").unwrap();
    writeln!(
        s,
        "  if (v === undefined) throw new Error(`fixture ${{fixture}}/${{label}} not loaded`);"
    )
    .unwrap();
    writeln!(s, "  return v;").unwrap();
    writeln!(s, "}}").unwrap();
    EmittedFile {
        path: PathBuf::from("spec/fixtures.ts"),
        content: s,
    }
}

/// Map a Roundhouse `Ty` to (type annotation, default expression) for
/// a TS instance field declaration. Keeping both Time and DateTime as
/// `string` matches the Rust/Crystal convention — SQLite stores ISO
/// strings; generated tests compare them as opaque values.
fn ts_field_type_and_default(ty: &Ty) -> (String, String) {
    match ty {
        Ty::Int => ("number".into(), "0".into()),
        Ty::Float => ("number".into(), "0".into()),
        Ty::Bool => ("boolean".into(), "false".into()),
        Ty::Str | Ty::Sym => ("string".into(), "\"\"".into()),
        Ty::Class { id, .. } if id.0.as_str() == "Time" => ("string".into(), "\"\"".into()),
        _ => ("any".into(), "null".into()),
    }
}

fn ts_literal_for(value: &str, ty: &Ty) -> String {
    match ty {
        Ty::Str | Ty::Sym => format!("{value:?}"),
        Ty::Int => {
            if value.parse::<i64>().is_ok() {
                value.to_string()
            } else {
                format!("0 /* TODO: coerce {value:?} */")
            }
        }
        Ty::Float => {
            if value.parse::<f64>().is_ok() {
                value.to_string()
            } else {
                format!("0 /* TODO: coerce {value:?} */")
            }
        }
        Ty::Bool => match value {
            "true" | "1" => "true".into(),
            "false" | "0" => "false".into(),
            _ => format!("false /* TODO: coerce {value:?} */"),
        },
        Ty::Class { id, .. } if id.0.as_str() == "Time" => format!("{value:?}"),
        _ => format!("{value:?}"),
    }
}

fn emit_ts_spec(tm: &TestModule, app: &App) -> EmittedFile {
    let fixture_names: Vec<Symbol> =
        app.fixtures.iter().map(|f| f.name.clone()).collect();
    let known_models: Vec<Symbol> =
        app.models.iter().map(|m| m.name.0.clone()).collect();
    let mut attrs_set: std::collections::BTreeSet<Symbol> =
        std::collections::BTreeSet::new();
    for m in &app.models {
        for attr in m.attributes.fields.keys() {
            attrs_set.insert(attr.clone());
        }
    }
    let model_attrs: Vec<Symbol> = attrs_set.into_iter().collect();

    let ctx = SpecCtx {
        app,
        fixture_names: &fixture_names,
        known_models: &known_models,
        model_attrs: &model_attrs,
    };

    let is_controller_test = tm.name.0.as_str().ends_with("ControllerTest");

    let mut s = String::new();
    writeln!(s, "// Generated by Roundhouse.").unwrap();
    writeln!(s, "import {{ test, beforeEach }} from \"node:test\";").unwrap();
    writeln!(s, "import assert from \"node:assert/strict\";").unwrap();
    if is_controller_test {
        // Side-effect import: routes.ts pushes into the Router match
        // table at module load. Needed before any dispatch.
        writeln!(s, "import \"../../src/routes.js\";").unwrap();
        writeln!(s, "import {{ TestClient }} from \"../../src/test_support.js\";").unwrap();
        writeln!(s, "import * as routeHelpers from \"../../src/route_helpers.js\";").unwrap();
    }
    // Per-model class imports — one per known model, so `new Article()`
    // etc. in test bodies resolve. Models are siblings under
    // app/models/.
    for model_name in &known_models {
        let file = crate::naming::snake_case(model_name.as_str());
        writeln!(
            s,
            "import {{ {} }} from \"../../app/models/{}.js\";",
            model_name, file,
        )
        .unwrap();
    }
    for fixture in &app.fixtures {
        writeln!(
            s,
            "import * as {}Fixtures from \"../fixtures/{}.js\";",
            crate::naming::camelize(fixture.name.as_str()),
            fixture.name,
        )
        .unwrap();
    }
    // Every test starts on a fresh :memory: DB + fully-loaded
    // fixtures. `beforeEach` runs across the whole spec file, matching
    // Rails' transactional-fixture isolation.
    if !app.fixtures.is_empty() {
        writeln!(s, "import {{ setup }} from \"../fixtures.js\";").unwrap();
        writeln!(s).unwrap();
        writeln!(s, "beforeEach(() => setup());").unwrap();
    }

    for test in &tm.tests {
        writeln!(s).unwrap();
        let test_name = &test.name;
        if is_controller_test {
            emit_ts_controller_test(&mut s, test, app);
        } else if test_needs_runtime_unsupported_ts(test) {
            writeln!(
                s,
                "test.skip({test_name:?}, () => {{"
            )
            .unwrap();
            writeln!(s, "  // Phase 3: needs persistence runtime").unwrap();
            writeln!(s, "}});").unwrap();
        } else {
            writeln!(s, "test({test_name:?}, () => {{").unwrap();
            let body = emit_spec_body_ts(&test.body, ctx);
            for line in body.lines() {
                writeln!(s, "  {line}").unwrap();
            }
            writeln!(s, "}});").unwrap();
        }
    }

    let filename = crate::naming::snake_case(tm.name.0.as_str());
    let filename = filename.replace("_test", ".test");
    EmittedFile {
        path: PathBuf::from(format!("spec/models/{filename}.ts")),
        content: s,
    }
}

#[derive(Clone, Copy)]
struct SpecCtx<'a> {
    app: &'a App,
    fixture_names: &'a [Symbol],
    known_models: &'a [Symbol],
    model_attrs: &'a [Symbol],
}

fn emit_spec_body_ts(body: &Expr, ctx: SpecCtx) -> String {
    match &*body.node {
        ExprNode::Seq { exprs } if !exprs.is_empty() => exprs
            .iter()
            .map(|e| emit_spec_stmt_ts(e, ctx))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => emit_spec_stmt_ts(body, ctx),
    }
}

fn emit_spec_stmt_ts(e: &Expr, ctx: SpecCtx) -> String {
    match &*e.node {
        ExprNode::Assign { target: LValue::Var { name, .. }, value } => {
            format!("const {} = {};", name, emit_spec_expr_ts(value, ctx))
        }
        _ => format!("{};", emit_spec_expr_ts(e, ctx)),
    }
}

fn emit_spec_expr_ts(e: &Expr, ctx: SpecCtx) -> String {
    use crate::expr::InterpPart;
    match &*e.node {
        ExprNode::Lit { value } => emit_literal(value),
        ExprNode::Var { name, .. } => name.to_string(),
        ExprNode::Const { path } => {
            path.iter().map(|s| s.to_string()).collect::<Vec<_>>().join(".")
        }
        ExprNode::Hash { entries, .. } => {
            let parts: Vec<String> = entries
                .iter()
                .map(|(k, v)| {
                    let key = match &*k.node {
                        ExprNode::Lit { value: Literal::Sym { value } } => {
                            value.as_str().to_string()
                        }
                        _ => emit_spec_expr_ts(k, ctx),
                    };
                    format!("{}: {}", key, emit_spec_expr_ts(v, ctx))
                })
                .collect();
            format!("{{ {} }}", parts.join(", "))
        }
        ExprNode::Array { elements, .. } => {
            let parts: Vec<String> =
                elements.iter().map(|e| emit_spec_expr_ts(e, ctx)).collect();
            format!("[{}]", parts.join(", "))
        }
        ExprNode::StringInterp { parts } => {
            let mut out = String::from("`");
            for p in parts {
                match p {
                    InterpPart::Text { value } => out.push_str(value),
                    InterpPart::Expr { expr } => {
                        out.push_str("${");
                        out.push_str(&emit_spec_expr_ts(expr, ctx));
                        out.push('}');
                    }
                }
            }
            out.push('`');
            out
        }
        ExprNode::Send { recv, method, args, block, .. } => {
            emit_spec_send_ts(recv.as_ref(), method.as_str(), args, block.as_ref(), ctx)
        }
        ExprNode::BoolOp { op, left, right, .. } => {
            use crate::expr::BoolOpKind;
            let op_s = match op {
                BoolOpKind::Or => "||",
                BoolOpKind::And => "&&",
            };
            format!(
                "{} {} {}",
                emit_spec_expr_ts(left, ctx),
                op_s,
                emit_spec_expr_ts(right, ctx)
            )
        }
        _ => format!("/* TODO: spec emit for {:?} */", std::mem::discriminant(&*e.node)),
    }
}

fn emit_spec_send_ts(
    recv: Option<&Expr>,
    method: &str,
    args: &[Expr],
    block: Option<&Expr>,
    ctx: SpecCtx,
) -> String {
    let args_s: Vec<String> = args.iter().map(|a| emit_spec_expr_ts(a, ctx)).collect();

    // Fixture accessor: articles(:one) → articlesFixtures.one()
    if recv.is_none()
        && args.len() == 1
        && ctx.fixture_names.iter().any(|s| s.as_str() == method)
    {
        if let ExprNode::Lit { value: Literal::Sym { value: sym } } = &*args[0].node {
            let ns = crate::naming::camelize(method);
            return format!("{ns}Fixtures.{}()", sym.as_str());
        }
    }

    // assert_difference("Class.count", delta) do ... end → inline
    // before/after capture + assert.strictEqual on the delta.
    if recv.is_none() && method == "assert_difference" {
        if let Some(body) = block {
            if let Some(count_expr) = args
                .first()
                .and_then(|a| match &*a.node {
                    ExprNode::Lit { value: Literal::Str { value } } => {
                        rewrite_ruby_dot_call_ts(value)
                    }
                    _ => None,
                })
            {
                let delta = args_s.get(1).cloned().unwrap_or_else(|| "1".into());
                let body_s = emit_block_body_ts(body, ctx);
                return format!(
                    "(() => {{\n  const _before = {count_expr};\n  {body_s}\n  const _after = {count_expr};\n  assert.strictEqual(_after - _before, {delta});\n}})()"
                );
            }
        }
    }

    // owner.<assoc>.create(hash) / .build(hash) — HasMany rewrite.
    if (method == "create" || method == "build") && args.len() == 1 {
        if let Some(outer_recv) = recv {
            if let ExprNode::Send {
                recv: Some(assoc_recv),
                method: assoc_method,
                args: inner_args,
                ..
            } = &*outer_recv.node
            {
                if inner_args.is_empty() {
                    if let Some(s) = try_emit_assoc_create_ts(
                        assoc_recv,
                        assoc_method.as_str(),
                        args,
                        method,
                        ctx,
                    ) {
                        return s;
                    }
                }
            }
        }
    }

    // Assertion mapping. node:test uses `node:assert/strict` which
    // provides `assert.strictEqual`, `assert.notStrictEqual`,
    // `assert.ok`, etc. Ruby's `assert_equal expected, actual`
    // argument order flips to actual-first for Node.
    if recv.is_none() {
        match (method, args_s.len()) {
            ("assert_equal", 2) => {
                return format!("assert.strictEqual({}, {})", args_s[1], args_s[0]);
            }
            ("assert_not_equal", 2) => {
                return format!("assert.notStrictEqual({}, {})", args_s[1], args_s[0]);
            }
            ("assert_not", 1) => {
                return format!("assert.ok(!({}))", args_s[0]);
            }
            ("assert", 1) => {
                return format!("assert.ok({})", args_s[0]);
            }
            ("assert_nil", 1) => {
                return format!("assert.strictEqual({}, null)", args_s[0]);
            }
            ("assert_not_nil", 1) => {
                return format!("assert.notStrictEqual({}, null)", args_s[0]);
            }
            _ => {}
        }
    }

    // `Class.new(hash)` → Object.assign(new Class(), { k: v, ... })
    if let Some(r) = recv {
        if method == "new" && args.len() == 1 {
            if let ExprNode::Const { path } = &*r.node {
                if let Some(class_name) = path.last() {
                    if ctx.known_models.iter().any(|s| s == class_name) {
                        if let ExprNode::Hash { entries, .. } = &*args[0].node {
                            let pairs: Vec<String> = entries
                                .iter()
                                .map(|(k, v)| {
                                    let key = match &*k.node {
                                        ExprNode::Lit {
                                            value: Literal::Sym { value: f },
                                        } => f.as_str().to_string(),
                                        _ => emit_spec_expr_ts(k, ctx),
                                    };
                                    format!("{}: {}", key, emit_spec_expr_ts(v, ctx))
                                })
                                .collect();
                            return format!(
                                "Object.assign(new {}(), {{ {} }})",
                                class_name,
                                pairs.join(", ")
                            );
                        }
                    }
                }
            }
        }
    }

    match recv {
        None => {
            if args_s.is_empty() {
                method.to_string()
            } else {
                format!("{}({})", method, args_s.join(", "))
            }
        }
        Some(r) => {
            let recv_s = emit_spec_expr_ts(r, ctx);
            let is_class_call = matches!(&*r.node, ExprNode::Const { .. });
            // Class-level calls are always methods (e.g. `Comment.count`).
            if is_class_call {
                if args_s.is_empty() {
                    return format!("{recv_s}.{method}()");
                } else {
                    return format!("{recv_s}.{method}({})", args_s.join(", "));
                }
            }
            // Instance-level calls: attribute reads + Juntos getters
            // (save, destroy) render without parens; other zero-arg
            // calls go method-style.
            let is_attr_read = args_s.is_empty()
                && ctx.model_attrs.iter().any(|s| s.as_str() == method);
            let is_juntos_getter =
                args_s.is_empty() && matches!(method, "save" | "destroy");
            if is_attr_read || is_juntos_getter {
                format!("{recv_s}.{method}")
            } else if args_s.is_empty() {
                format!("{recv_s}.{method}()")
            } else {
                format!("{recv_s}.{method}({})", args_s.join(", "))
            }
        }
    }
}

/// Parse a Ruby-style `"Class.method"` expression string into TS
/// `Class.method()` syntax. Used by `assert_difference` to re-
/// evaluate the captured count expression before and after the block.
fn rewrite_ruby_dot_call_ts(expr: &str) -> Option<String> {
    let trimmed = expr.trim();
    let (lhs, rhs) = trimmed.split_once('.')?;
    let is_ident = |s: &str| {
        !s.is_empty()
            && s.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_')
            && s.chars().all(|c| c.is_alphanumeric() || c == '_')
    };
    if !is_ident(lhs) || !is_ident(rhs) {
        return None;
    }
    // Uppercase LHS → class-level call (`Comment.count()`).
    // Lowercase LHS → instance attribute read (`article.count`).
    let is_class = lhs.chars().next().is_some_and(|c| c.is_uppercase());
    if is_class {
        Some(format!("{lhs}.{rhs}()"))
    } else {
        Some(format!("{lhs}.{rhs}"))
    }
}

/// Render a Ruby block body as TS statements, peeling one Lambda
/// layer. Ruby `do ... end` lowers to `ExprNode::Lambda`.
fn emit_block_body_ts(e: &Expr, ctx: SpecCtx) -> String {
    let inner = match &*e.node {
        ExprNode::Lambda { body, .. } => body,
        _ => e,
    };
    match &*inner.node {
        ExprNode::Seq { exprs } if !exprs.is_empty() => exprs
            .iter()
            .map(|s| emit_spec_stmt_ts(s, ctx))
            .collect::<Vec<_>>()
            .join("\n  "),
        _ => emit_spec_stmt_ts(inner, ctx),
    }
}

fn try_emit_assoc_create_ts(
    owner: &Expr,
    assoc_name: &str,
    args: &[Expr],
    outer_method: &str,
    ctx: SpecCtx,
) -> Option<String> {
    let resolved = crate::lower::resolve_has_many(
        &Symbol::from(assoc_name),
        owner.ty.as_ref(),
        ctx.app,
    )?;
    let target_class = resolved.target_class.0.as_str();
    let foreign_key = resolved.foreign_key.as_str();

    let owner_s = emit_spec_expr_ts(owner, ctx);
    let hash_entries = match &args.first()?.node.as_ref() {
        ExprNode::Hash { entries, .. } => entries.clone(),
        _ => return None,
    };

    let mut pairs: Vec<String> =
        vec![format!("{foreign_key}: {owner_s}.id")];
    for (k, v) in &hash_entries {
        if let ExprNode::Lit { value: Literal::Sym { value: field_name } } = &*k.node {
            pairs.push(format!("{}: {}", field_name.as_str(), emit_spec_expr_ts(v, ctx)));
        }
    }
    // `.build` returns the unsaved record; `.create` runs save first.
    // Both evaluate to the record.
    let construct = format!(
        "Object.assign(new {target_class}(), {{ {} }})",
        pairs.join(", "),
    );
    if outer_method == "create" {
        Some(format!("(() => {{ const _r = {construct}; _r.save; return _r; }})()"))
    } else {
        Some(construct)
    }
}

fn test_needs_runtime_unsupported_ts(_test: &Test) -> bool {
    // Phase 3 rounded out the TS emitter's handling of the real-blog
    // test shapes. Keep as future-guard; no current pattern forces a
    // skip.
    false
}

#[allow(dead_code)]

fn test_body_uses_unsupported_ts(e: &Expr) -> bool {
    use crate::expr::InterpPart;
    let self_hit = match &*e.node {
        ExprNode::Send { recv, method, .. } => {
            let m = method.as_str();
            matches!(
                m,
                "assert_difference"
                    | "destroy"
                    | "destroy!"
                    | "build"
                    | "create"
                    | "create!"
            ) || (m == "count"
                && recv.as_ref().is_some_and(|r| matches!(&*r.node, ExprNode::Const { .. })))
        }
        _ => false,
    };
    if self_hit {
        return true;
    }
    match &*e.node {
        ExprNode::Send { recv, args, block, .. } => {
            if let Some(r) = recv {
                if test_body_uses_unsupported_ts(r) {
                    return true;
                }
            }
            for a in args {
                if test_body_uses_unsupported_ts(a) {
                    return true;
                }
            }
            if let Some(b) = block {
                if test_body_uses_unsupported_ts(b) {
                    return true;
                }
            }
        }
        ExprNode::Seq { exprs } | ExprNode::Array { elements: exprs, .. } => {
            for e in exprs {
                if test_body_uses_unsupported_ts(e) {
                    return true;
                }
            }
        }
        ExprNode::Hash { entries, .. } => {
            for (k, v) in entries {
                if test_body_uses_unsupported_ts(k) || test_body_uses_unsupported_ts(v) {
                    return true;
                }
            }
        }
        ExprNode::StringInterp { parts } => {
            for p in parts {
                if let InterpPart::Expr { expr } = p {
                    if test_body_uses_unsupported_ts(expr) {
                        return true;
                    }
                }
            }
        }
        ExprNode::BoolOp { left, right, .. }
        | ExprNode::RescueModifier { expr: left, fallback: right } => {
            if test_body_uses_unsupported_ts(left) || test_body_uses_unsupported_ts(right) {
                return true;
            }
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            if test_body_uses_unsupported_ts(cond)
                || test_body_uses_unsupported_ts(then_branch)
                || test_body_uses_unsupported_ts(else_branch)
            {
                return true;
            }
        }
        ExprNode::Let { value, body, .. } => {
            if test_body_uses_unsupported_ts(value) || test_body_uses_unsupported_ts(body) {
                return true;
            }
        }
        ExprNode::Lambda { body, .. } => {
            if test_body_uses_unsupported_ts(body) {
                return true;
            }
        }
        ExprNode::Assign { value, .. } => {
            if test_body_uses_unsupported_ts(value) {
                return true;
            }
        }
        _ => {}
    }
    false
}

/// Emit a single controller test (`test("name", async () => { ... })`).
/// Mirrors `emit_rust_controller_test`. Primes ivars from fixtures,
/// opens a fresh `TestClient`, then walks test-body statements.
fn emit_ts_controller_test(out: &mut String, test: &Test, app: &App) {
    writeln!(out, "test({:?}, async () => {{", test.name.as_str()).unwrap();
    writeln!(out, "  const client = new TestClient();").unwrap();

    let walked = crate::lower::walk_controller_ivars(&test.body);
    for ivar in walked.ivars_read_without_assign() {
        let plural = crate::naming::pluralize_snake(&crate::naming::camelize(ivar.as_str()));
        let fixt_ns = crate::naming::camelize(&plural);
        writeln!(
            out,
            "  const {} = {}Fixtures.one();",
            ivar.as_str(),
            fixt_ns,
        )
        .unwrap();
    }

    let stmts = ts_ctrl_test_body_stmts(&test.body);
    for stmt in stmts {
        let rendered = emit_ts_ctrl_test_stmt(stmt, app);
        for line in rendered.lines() {
            writeln!(out, "  {line}").unwrap();
        }
    }

    writeln!(out, "}});").unwrap();
}

fn ts_ctrl_test_body_stmts(body: &Expr) -> Vec<&Expr> {
    crate::lower::test_body_stmts(body)
}

fn emit_ts_ctrl_test_stmt(stmt: &Expr, app: &App) -> String {
    match &*stmt.node {
        ExprNode::Send { recv: None, method, args, block, .. } => {
            emit_ts_ctrl_test_send(method.as_str(), args, block.as_ref(), app)
        }
        ExprNode::Send { recv: Some(r), method, args, .. } => {
            if method.as_str() == "reload" {
                let recv_s = match &*r.node {
                    ExprNode::Ivar { name } | ExprNode::Var { name, .. } => name.to_string(),
                    _ => emit_ts_ctrl_test_expr(r, app),
                };
                return format!("{recv_s}.reload();");
            }
            let recv_s = emit_ts_ctrl_test_expr(r, app);
            let args_s: Vec<String> =
                args.iter().map(|a| emit_ts_ctrl_test_expr(a, app)).collect();
            if args_s.is_empty() {
                format!("{recv_s}.{method};")
            } else {
                format!("{recv_s}.{method}({});", args_s.join(", "))
            }
        }
        ExprNode::Assign { target: LValue::Var { name, .. }, value } => {
            format!("let {name} = {};", emit_ts_ctrl_test_expr(value, app))
        }
        ExprNode::Assign { target: LValue::Ivar { name }, value } => {
            format!("let {name} = {};", emit_ts_ctrl_test_expr(value, app))
        }
        _ => format!("{};", emit_ts_ctrl_test_expr(stmt, app)),
    }
}

fn emit_ts_ctrl_test_send(
    method: &str,
    args: &[Expr],
    block: Option<&Expr>,
    app: &App,
) -> String {
    use crate::lower::ControllerTestSend;
    match crate::lower::classify_controller_test_send(method, args, block) {
        Some(ControllerTestSend::HttpGet { url }) => {
            let u = emit_ts_url_expr(url, app);
            format!("const resp = await client.get({u});")
        }
        Some(ControllerTestSend::HttpWrite { method, url, params }) => {
            let u = emit_ts_url_expr(url, app);
            let body = params
                .map(|h| flatten_ts_params_to_form(h, None, app))
                .unwrap_or_else(|| "{}".to_string());
            format!("const resp = await client.{method}({u}, {body});")
        }
        Some(ControllerTestSend::HttpDelete { url }) => {
            let u = emit_ts_url_expr(url, app);
            format!("const resp = await client.delete({u});")
        }
        Some(ControllerTestSend::AssertResponse { sym }) => match sym.as_str() {
            "success" => "resp.assertOk();".to_string(),
            "unprocessable_entity" => "resp.assertUnprocessable();".to_string(),
            other => format!("resp.assertStatus(/* {other:?} */ 200);"),
        },
        Some(ControllerTestSend::AssertRedirectedTo { url }) => {
            let u = emit_ts_url_expr(url, app);
            format!("resp.assertRedirectedTo({u});")
        }
        Some(ControllerTestSend::AssertSelect { selector, kind }) => {
            emit_ts_assert_select_classified(selector, kind, app)
        }
        Some(ControllerTestSend::AssertDifference { method, count_expr, delta, block }) => {
            let _ = method;
            emit_ts_assert_difference_classified(count_expr, delta, block, app)
        }
        Some(ControllerTestSend::AssertEqual { expected, actual }) => {
            let e = emit_ts_ctrl_test_expr(expected, app);
            let a = emit_ts_ctrl_test_expr(actual, app);
            // Rails calls assert_equal(expected, actual); node:test
            // assert.equal takes (actual, expected) — swap order.
            format!("assert.equal({a}, {e});")
        }
        None => {
            let args_s: Vec<String> =
                args.iter().map(|a| emit_ts_ctrl_test_expr(a, app)).collect();
            if args_s.is_empty() {
                format!("{method}();")
            } else {
                format!("{method}({});", args_s.join(", "))
            }
        }
    }
}

/// `articles_url`, `article_url(@article)` → `routeHelpers.articlesPath()`,
/// `routeHelpers.articlePath(article.id)`. Uses the shared URL-helper
/// classifier; target-specific pieces are (a) helper name casing and
/// (b) the optional-unwrap syntax for `Model.last`.
fn emit_ts_url_expr(expr: &Expr, app: &App) -> String {
    use crate::lower::UrlArg;
    let Some(helper) = crate::lower::classify_url_expr(expr) else {
        return emit_ts_ctrl_test_expr(expr, app);
    };
    let camel = crate::naming::camelize(&helper.helper_base);
    let mut chars = camel.chars();
    let helper_name = match chars.next() {
        Some(c) => format!("{}{}Path", c.to_lowercase(), chars.as_str()),
        None => format!("{}Path", helper.helper_base),
    };
    let args_s: Vec<String> = helper
        .args
        .iter()
        .map(|a| match a {
            UrlArg::IvarOrVarId(name) => format!("{name}.id"),
            UrlArg::ModelLast(class) => format!("{}.last()!.id", class.as_str()),
            UrlArg::Raw(e) => emit_ts_ctrl_test_expr(e, app),
        })
        .collect();
    format!("routeHelpers.{helper_name}({})", args_s.join(", "))
}

fn emit_ts_assert_select_classified(
    selector_expr: &Expr,
    kind: crate::lower::AssertSelectKind<'_>,
    app: &App,
) -> String {
    use crate::lower::AssertSelectKind;
    let ExprNode::Lit { value: Literal::Str { value: selector } } = &*selector_expr.node
    else {
        return format!(
            "/* TODO: dynamic selector */ resp.assertSelect({});",
            emit_ts_ctrl_test_expr(selector_expr, app),
        );
    };
    match kind {
        AssertSelectKind::Text(expr) => {
            let text = emit_ts_ctrl_test_expr(expr, app);
            format!("resp.assertSelectText({selector:?}, {text});")
        }
        AssertSelectKind::Minimum(expr) => {
            let n = emit_ts_ctrl_test_expr(expr, app);
            format!("resp.assertSelectMin({selector:?}, {n});")
        }
        AssertSelectKind::SelectorBlock(b) => {
            let mut out = String::new();
            out.push_str(&format!("resp.assertSelect({selector:?});\n"));
            let inner_body = match &*b.node {
                ExprNode::Lambda { body, .. } => body,
                _ => b,
            };
            for stmt in ts_ctrl_test_body_stmts(inner_body) {
                out.push_str(&emit_ts_ctrl_test_stmt(stmt, app));
                out.push('\n');
            }
            out.trim_end().to_string()
        }
        AssertSelectKind::SelectorOnly => {
            format!("resp.assertSelect({selector:?});")
        }
    }
}

fn emit_ts_assert_difference_classified(
    count_expr_str: String,
    expected_delta: i64,
    block: Option<&Expr>,
    app: &App,
) -> String {
    // `Article.count` → `Article.count()`.
    let count_expr = count_expr_str
        .split_once('.')
        .map(|(cls, m)| format!("{cls}.{m}()"))
        .unwrap_or_else(|| count_expr_str.clone());

    let mut out = String::new();
    out.push_str(&format!("const _before = {count_expr};\n"));
    if let Some(b) = block {
        let inner_body = match &*b.node {
            ExprNode::Lambda { body, .. } => body,
            _ => b,
        };
        for stmt in ts_ctrl_test_body_stmts(inner_body) {
            out.push_str(&emit_ts_ctrl_test_stmt(stmt, app));
            out.push('\n');
        }
    }
    out.push_str(&format!("const _after = {count_expr};\n"));
    out.push_str(&format!("assert.equal(_after - _before, {expected_delta});"));
    out
}

fn emit_ts_ctrl_test_expr(expr: &Expr, app: &App) -> String {
    match &*expr.node {
        ExprNode::Lit { value } => emit_ts_literal(value),
        ExprNode::Ivar { name } => name.to_string(),
        ExprNode::Var { name, .. } => name.to_string(),
        ExprNode::Const { path } => {
            path.iter().map(|s| s.to_string()).collect::<Vec<_>>().join(".")
        }
        ExprNode::Send { recv: Some(r), method, args, .. } => {
            let m = method.as_str();
            if m == "last" && args.is_empty() {
                if let ExprNode::Const { path } = &*r.node {
                    let class = path.last().map(|s| s.as_str().to_string()).unwrap_or_default();
                    return format!("{class}.last()!");
                }
            }
            if m == "count" && args.is_empty() {
                if let ExprNode::Const { path } = &*r.node {
                    let class = path.last().map(|s| s.as_str().to_string()).unwrap_or_default();
                    return format!("{class}.count()");
                }
            }
            if args.is_empty() {
                let recv_s = match &*r.node {
                    ExprNode::Ivar { name } | ExprNode::Var { name, .. } => name.to_string(),
                    _ => emit_ts_ctrl_test_expr(r, app),
                };
                return format!("{recv_s}.{m}");
            }
            let recv_s = emit_ts_ctrl_test_expr(r, app);
            let args_s: Vec<String> =
                args.iter().map(|a| emit_ts_ctrl_test_expr(a, app)).collect();
            format!("{recv_s}.{m}({})", args_s.join(", "))
        }
        ExprNode::Send { recv: None, method, args, .. } => {
            if method.as_str().ends_with("_url") || method.as_str().ends_with("_path") {
                return emit_ts_url_expr(expr, app);
            }
            let args_s: Vec<String> =
                args.iter().map(|a| emit_ts_ctrl_test_expr(a, app)).collect();
            if args_s.is_empty() {
                method.to_string()
            } else {
                format!("{method}({})", args_s.join(", "))
            }
        }
        _ => format!("/* TODO expr {:?} */", std::mem::discriminant(&*expr.node)),
    }
}

fn emit_ts_literal(v: &Literal) -> String {
    match v {
        Literal::Str { value } => format!("{value:?}"),
        Literal::Int { value } => value.to_string(),
        Literal::Float { value } => value.to_string(),
        Literal::Bool { value } => value.to_string(),
        Literal::Nil => "null".to_string(),
        Literal::Sym { value } => format!("{:?}", value.as_str()),
    }
}

/// Flatten `{ article: { title: "X", body: "Y" } }` into a TS object
/// literal `{ "article[title]": "X", "article[body]": "Y" }` matching
/// the TestClient's form-body shape (which the controller template
/// destructures via `context.params["article[title]"]`). The key
/// flattening lives in `crate::lower::flatten_params_pairs`; this
/// function is just the TS-side value render.
fn flatten_ts_params_to_form(expr: &Expr, scope: Option<&str>, app: &App) -> String {
    let pairs: Vec<String> = crate::lower::flatten_params_pairs(expr, scope)
        .into_iter()
        .map(|(key, value)| {
            let val = emit_ts_ctrl_test_expr(value, app);
            format!("{key:?}: String({val})")
        })
        .collect();
    format!("{{ {} }}", pairs.join(", "))
}
