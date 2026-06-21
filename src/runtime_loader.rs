//! Inline transpile of `runtime/ruby/*.rb` → TS at app emit time.
//!
//! Replaces the old "runtime_transpile_ts bin writes to checked-in
//! `runtime/typescript/*.ts` files" workflow. The Ruby + RBS sources
//! are baked into the binary via `include_str!`; at emit time we
//! parse them and produce the TS classes inline. No drift possible —
//! the Ruby source IS the source of truth, no checked-in artifact
//! to fall out of sync.
//!
//! Hand-written runtime files (`juntos.ts`, `minitest.ts`,
//! etc.) stay as `include_str!` of `.ts` content in `src/emit/typescript.rs`.
//! This module covers ONLY the generated set.
//!
//! The mapping table here is the lift-and-shift of
//! `src/bin/runtime_transpile_ts.rs`'s PAIRS, with a single new
//! function (`typescript_units`) that returns the parsed-and-emitted
//! result. Tree-shake (a follow-up) operates on the parsed
//! `LibraryClass`es before final emit, dropping methods nothing in
//! the app references.
//!
//! This module is target-specific (TypeScript). When other targets
//! catch up to the level where they need transpiled framework Ruby,
//! they get sibling functions (`crystal_units`, `rust_units`, …)
//! that consume the same source set with target-specific imports
//! and preludes.

use crate::dialect::{LibraryClass, MethodDef};
use crate::expr::Expr;
use crate::runtime_src::{
    parse_library_with_rbs, parse_methods_with_rbs, parse_module_constant_exprs,
    parse_module_ivar_exprs,
};
use std::path::PathBuf;

/// Per-target emission hooks. Each target plugs in its own
/// `Module`-mode + `Library`-mode + per-expression renderer plus its
/// import syntax. The `transpile_entry` driver and the `RuntimeEntry`
/// table shape are shared.
///
/// `format_import` formats one `(name_clause, source_module)` pair as
/// the target's import line — `import { X } from "Y";\n` for TS,
/// `require "Y"\n` for Crystal, etc.
pub struct TargetEmit {
    pub emit_module: fn(&[MethodDef]) -> Result<String, String>,
    pub emit_library_class: fn(&LibraryClass) -> Result<String, String>,
    pub format_import: fn(name: &str, source: &str) -> String,
    /// Format a top-level constant declaration. TS: `const NAME = VALUE;`.
    /// Crystal: `NAME = VALUE` (no `const` keyword, no terminator).
    /// Rust picks `pub const`, `pub static`, or `static LazyLock` based
    /// on the value's IR shape — passing the Expr itself lets each
    /// target decide internally instead of pattern-matching a
    /// pre-rendered string.
    pub format_constant: fn(name: &str, value: &Expr) -> String,
    /// Format a module-level `@ivar = value` assignment as a target
    /// declaration. `None` skips ivar emission (TS/Crystal/Rust all
    /// model module state through other means and don't surface
    /// module-level ivars in transpiled output today). `Some(fn)`
    /// opts the target in — Go emits these as package-level `var`s
    /// so module-singleton state (e.g. ViewHelpers' `@slots`)
    /// resolves at use sites.
    pub format_module_ivar: Option<fn(owner: &str, name: &str, value: &Expr) -> String>,
    /// Wrap a body in the target's namespace syntax. TS resolves
    /// namespaces through imports + qualified-name registration in
    /// treeshake, so this is a no-op (`namespace` arg ignored). Crystal
    /// requires explicit `module X ... end` wrapping for refs of the
    /// form `X::Y` to resolve.
    pub wrap_namespace: fn(namespace: &str, body: &str) -> String,
    /// Text inserted after the generated-file header comment and before
    /// any imports. Python uses it for `from __future__ import
    /// annotations` so cross-file type annotations (e.g. `errors.py`'s
    /// `record: Base`) are evaluated lazily — both to avoid import cycles
    /// and because annotations are eager before Python 3.14 (PEP 649), so
    /// a bare forward-ref annotation would `NameError` at import on 3.12.
    /// Empty for targets that don't need it.
    pub module_prelude: &'static str,
}

const TS_TARGET: TargetEmit = TargetEmit {
    emit_module: crate::emit::typescript::emit_module,
    emit_library_class: crate::emit::typescript::emit_library_class,
    format_import: ts_format_import,
    format_constant: ts_format_constant,
    format_module_ivar: None,
    wrap_namespace: ts_wrap_namespace,
    module_prelude: "",
};

fn ts_wrap_namespace(_namespace: &str, body: &str) -> String {
    // TS resolves namespaces through imports + treeshake's qualified
    // class registry; the per-file body emits flat at module top.
    body.to_string()
}

fn ts_format_import(name: &str, source: &str) -> String {
    format!("import {{ {name} }} from \"{source}\";\n")
}

fn ts_format_constant(name: &str, value: &Expr) -> String {
    format!(
        "const {name} = {};",
        crate::emit::typescript::emit_expr_for_runtime(value)
    )
}

const CRYSTAL_TARGET: TargetEmit = TargetEmit {
    emit_module: crate::emit::crystal::emit_module,
    emit_library_class: crate::emit::crystal::emit_library_class,
    format_import: crystal_format_import,
    format_constant: crystal_format_constant,
    format_module_ivar: None,
    wrap_namespace: crystal_wrap_namespace,
    module_prelude: "",
};

/// Wrap `body` in `module Foo ... end` (or nested forms for compound
/// names like `A::B`). Empty namespace returns the body unchanged.
///
/// As of the RBS scope-tracking change, LibraryClass names carry the
/// fully-qualified path (`ActiveRecord::Base`) and `render_class`
/// wraps each class in its own `module Namespace` chain. Wrapping
/// HERE on top of that produced double-nested modules. Detect
/// pre-wrapped bodies (any line starts with `module <first-segment>`)
/// and skip the outer wrap; constants emit at indent 0 outside any
/// module, so we re-wrap them via `module Namespace ... end` and
/// concatenate the (already-wrapped) class bodies after.
fn crystal_wrap_namespace(namespace: &str, body: &str) -> String {
    if namespace.is_empty() {
        return body.to_string();
    }
    let first_seg = namespace.split("::").next().unwrap_or(namespace);
    let already_wraps = body
        .lines()
        .any(|l| l.starts_with(&format!("module {first_seg}")));
    if already_wraps {
        return body.to_string();
    }
    let segments: Vec<&str> = namespace.split("::").collect();
    let mut out = String::new();
    for (i, seg) in segments.iter().enumerate() {
        out.push_str(&"  ".repeat(i));
        out.push_str(&format!("module {seg}\n"));
    }
    let pad = "  ".repeat(segments.len());
    for line in body.lines() {
        if line.is_empty() {
            out.push('\n');
        } else {
            out.push_str(&format!("{pad}{line}\n"));
        }
    }
    for i in (0..segments.len()).rev() {
        out.push_str(&"  ".repeat(i));
        out.push_str("end\n");
    }
    out
}

/// Crystal's `require` is path-based (no named-import clause). The
/// `name` parameter from `RuntimeEntry.imports` is informational —
/// Crystal pulls everything publicly visible from the required file.
fn crystal_format_import(_name: &str, source: &str) -> String {
    format!("require {source:?}\n")
}

/// Crystal: top-level constant `NAME = VALUE` (no `const` keyword,
/// no statement terminator).
fn crystal_format_constant(name: &str, value: &Expr) -> String {
    format!("{name} = {}", crate::emit::crystal::emit_expr_for_runtime(value))
}

const RUST_TARGET: TargetEmit = TargetEmit {
    emit_module: crate::emit::rust2::library::emit_module,
    emit_library_class: crate::emit::rust2::library::emit_library_class,
    format_import: rust_format_import,
    format_constant: crate::emit::rust2::library::format_constant,
    format_module_ivar: None,
    wrap_namespace: rust_wrap_namespace,
    module_prelude: "",
};

/// Rust uses the file-as-module convention — `src/inflector.rs` IS
/// the `inflector` module. No `mod NAME { ... }` wrapping needed at
/// emit time; consumers reference via `crate::inflector::...`.
fn rust_wrap_namespace(_namespace: &str, body: &str) -> String {
    body.to_string()
}

/// Rust imports follow `use crate::module::Item;` form. The `name`
/// parameter is the imported item, `source` is the module path.
fn rust_format_import(name: &str, source: &str) -> String {
    format!("use crate::{source}::{name};\n")
}


/// Strategy: each entry picks one of two pipelines.
///
/// `Module` — flat list of `def self.*` helpers (e.g. inflector.rb).
/// Each method becomes a standalone `export function`.
///
/// `Library` — file with one or more class/module definitions
/// preserving parent/includes (e.g. errors.rb). Output is a
/// concatenation of the per-class TS class declarations.
enum Mode {
    Module,
    Library,
}

/// Cross-runtime imports each transpiled file needs. Each entry:
/// (named_import_clause, source_module). The runtime files all
/// emit under `src/<name>.ts`, so cross-references use `./<name>.js`.
type ImportSpec = &'static [(&'static str, &'static str)];
const NO_IMPORTS: ImportSpec = &[];

/// Hand-written TS prepended after imports and before the
/// transpiled class bodies. Used for module-scope constants
/// (`STATUS_CODES = {...}.freeze`) the lowerer doesn't yet
/// recognize.
const NO_PRELUDE: &str = "";

/// `(class_name, method_name)` pairs that hand-written runtime files
/// (server.ts, test_support.ts, broadcasts.ts) call into directly.
/// Tree-shake walks app-side bodies for reachability; calls that
/// only appear in hand-written runtime code aren't visible to that
/// walk and would otherwise drop. Listing them here keeps them.
type ExtraRoots = &'static [(&'static str, &'static str)];
const NO_EXTRA_ROOTS: ExtraRoots = &[];

/// One entry in the runtime transpile table. Maps a Ruby+RBS pair
/// to its emit shape (target path, mode, imports, prelude).
struct RuntimeEntry {
    rb_src: &'static str,
    rbs_src: &'static str,
    rb_path: &'static str,  // for error messages only
    /// Enclosing module name as written in the Ruby source (e.g.
    /// `"ActiveRecord"` for `module ActiveRecord; class Base; end; end`).
    /// Tree-shake uses this to register parsed classes under their
    /// qualified name (`ActiveRecord::Base`) so app-side parent
    /// references like `class ApplicationRecord < ActiveRecord::Base`
    /// resolve in the registry. Empty when no module wrapper.
    namespace: &'static str,
    out_path: &'static str,
    mode: Mode,
    imports: ImportSpec,
    prelude: &'static str,
    /// `(class, method)` pairs that the per-target hand-written
    /// runtime references but that wouldn't be discovered by the
    /// reachability walk over app-side bodies.
    extra_roots: ExtraRoots,
}

/// One emitted runtime file ready to be written into the target
/// project's `src/` tree.
pub struct RuntimeUnit {
    pub out_path: PathBuf,
    pub content: String,
    /// The parsed `LibraryClass`es (for `Mode::Library`). Empty
    /// for `Mode::Module`. Tree-shake operates on these *before*
    /// they're emitted; today the parse-and-emit happens together,
    /// but the data is preserved so a future tree-shake pass can
    /// filter `methods` before re-emit.
    pub classes: Vec<LibraryClass>,
    pub functions: Vec<MethodDef>,
    /// Enclosing module name (e.g. `"ActiveRecord"`). See
    /// `RuntimeEntry.namespace`.
    pub namespace: &'static str,
    /// Hand-written runtime call sites, propagated from
    /// `RuntimeEntry.extra_roots`.
    pub extra_roots: ExtraRoots,
}

/// The TypeScript-target transpile table. Each entry corresponds
/// to one `runtime/ruby/<x>.rb` file we want to emit as a TS file
/// in the output project.
const TYPESCRIPT_RUNTIME: &[RuntimeEntry] = &[
    // HashWithIndifferentAccess no longer transpiled to typed targets
    // per Phase 2.5(b). @flash / @session moved to per-app
    // ActionDispatch::Flash / ActionDispatch::Session structs with
    // typed fields + HWIA-shape shims; HWIA stays in runtime/ruby/
    // as a CRuby/Spinel helper for parity.
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/action_dispatch/flash.rb"),
        rbs_src: include_str!("../runtime/ruby/action_dispatch/flash.rbs"),
        rb_path: "runtime/ruby/action_dispatch/flash.rb",
        namespace: "ActionDispatch",
        out_path: "src/flash.ts",
        mode: Mode::Library,
        imports: NO_IMPORTS,
        prelude: NO_PRELUDE,
        // Hand-written server{,-worker,-libsql}.ts construct
        // `new Flash(flashStore)` and persist `flash.to_persisted()`
        // between requests (`to_h` is still used by the test harness) —
        // none visible to the app-side reachability walk, so seed them.
        extra_roots: &[("Flash", "to_h"), ("Flash", "to_persisted")],
    },
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/action_dispatch/session.rb"),
        rbs_src: include_str!("../runtime/ruby/action_dispatch/session.rbs"),
        rb_path: "runtime/ruby/action_dispatch/session.rb",
        namespace: "ActionDispatch",
        out_path: "src/session.ts",
        mode: Mode::Library,
        imports: NO_IMPORTS,
        prelude: NO_PRELUDE,
        extra_roots: NO_EXTRA_ROOTS,
    },
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/inflector.rb"),
        rbs_src: include_str!("../runtime/ruby/inflector.rbs"),
        rb_path: "runtime/ruby/inflector.rb",
        namespace: "",
        out_path: "src/inflector.ts",
        mode: Mode::Module,
        imports: NO_IMPORTS,
        prelude: NO_PRELUDE,
        extra_roots: NO_EXTRA_ROOTS,
    },
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/json_builder.rb"),
        rbs_src: include_str!("../runtime/ruby/json_builder.rbs"),
        rb_path: "runtime/ruby/json_builder.rb",
        namespace: "",
        out_path: "src/json_builder.ts",
        mode: Mode::Module,
        imports: NO_IMPORTS,
        prelude: NO_PRELUDE,
        extra_roots: NO_EXTRA_ROOTS,
    },
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/active_record/errors.rb"),
        rbs_src: include_str!("../runtime/ruby/active_record/errors.rbs"),
        rb_path: "runtime/ruby/active_record/errors.rb",
        namespace: "ActiveRecord",
        out_path: "src/errors.ts",
        mode: Mode::Library,
        imports: &[("type Base", "./active_record_base.js")],
        prelude: NO_PRELUDE,
        extra_roots: NO_EXTRA_ROOTS,
    },
    // `validations.rb` intentionally NOT transpiled — Phase 2.5(a)
    // inlines every `validates :x, …` declaration at lower time (see
    // `src/lower/model_to_library/validations.rs`). No transpiled
    // model dispatches into the Validations module any more.
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/active_record/base.rb"),
        rbs_src: include_str!("../runtime/ruby/active_record/base.rbs"),
        rb_path: "runtime/ruby/active_record/base.rb",
        namespace: "ActiveRecord",
        out_path: "src/active_record_base.ts",
        mode: Mode::Library,
        // `AdapterInterface` is the phantom class the analyzer registers
        // for the adapter slot type. TS aliases it to ActiveRecordAdapter
        // in juntos.ts; this import wires that alias into every call site.
        imports: &[
            ("RecordNotFound, RecordInvalid", "./errors.js"),
            ("type AdapterInterface", "./juntos.js"),
        ],
        prelude: NO_PRELUDE,
        extra_roots: NO_EXTRA_ROOTS,
    },
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/action_controller/base.rb"),
        rbs_src: include_str!("../runtime/ruby/action_controller/base.rbs"),
        rb_path: "runtime/ruby/action_controller/base.rb",
        namespace: "ActionController",
        out_path: "src/action_controller_base.ts",
        mode: Mode::Library,
        imports: &[
            ("Flash", "./flash.js"),
            ("Session", "./session.js"),
            // `ParamValue` is the recursive params type referenced
            // by `@params`'s RBS signature. The TS runtime declares
            // it in `runtime/typescript/param_value.ts`; the type-
            // only import keeps it in scope for the `Record<string,
            // ParamValue>` annotation. See cross-target rationale
            // in `runtime/crystal/param_value.cr`.
            ("type ParamValue", "./param_value.js"),
        ],
        // Module-scope `STATUS_CODES` is now picked up by
        // `parse_module_constant_exprs` and emitted as a
        // top-level `const STATUS_CODES = ...;` automatically;
        // no hand-written prelude needed.
        prelude: NO_PRELUDE,
        extra_roots: NO_EXTRA_ROOTS,
    },
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/action_dispatch/router.rb"),
        rbs_src: include_str!("../runtime/ruby/action_dispatch/router.rbs"),
        rb_path: "runtime/ruby/action_dispatch/router.rb",
        namespace: "",
        out_path: "src/router.ts",
        mode: Mode::Library,
        // Router historically imported HWIA for path-param storage; the
        // class is now self-contained (plain String-keyed Hash) and
        // doesn't need it.
        imports: NO_IMPORTS,
        prelude: NO_PRELUDE,
        // Hand-written `server.ts` and `test_support.ts` call
        // `Router.match(method, path, table)` directly. Treeshake's
        // app-side walk doesn't see those callsites, so without
        // these roots `Router.match` would drop and `src/router.ts`
        // would emit empty.
        extra_roots: &[("Router", "match")],
    },
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/action_view/view_helpers.rb"),
        rbs_src: include_str!("../runtime/ruby/action_view/view_helpers.rbs"),
        rb_path: "runtime/ruby/action_view/view_helpers.rb",
        namespace: "",
        out_path: "src/view_helpers.ts",
        mode: Mode::Library,
        // FormBuilder.model is RBS-typed `ActiveRecord::Base`; the
        // emit surfaces `model: Base` on the field + constructor.
        // Type-only — runtime never instantiates Base directly.
        imports: &[("type Base", "./active_record_base.js")],
        prelude: NO_PRELUDE,
        // Roots for the hand-written server.ts that calls into
        // ViewHelpers directly. Suffix-renames apply (`reset_slots!`
        // → `reset_slots_bang`).
        extra_roots: &[
            ("ViewHelpers", "reset_slots!"),
            ("ViewHelpers", "set_yield"),
        ],
    },
];

/// The Crystal-target transpile table. Mirrors TYPESCRIPT_RUNTIME
/// shape — each entry maps one `runtime/ruby/<x>.rb` file to a
/// transpiled `src/<x>.cr` output. Initial scope is just `inflector.rb`
/// (Module mode, no dependencies); expanded as the Crystal target
/// proves out additional surface area.
const CRYSTAL_RUNTIME: &[RuntimeEntry] = &[
    // HWIA dropped per Phase 2.5(b); flash/session moved to typed
    // ActionDispatch structs.
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/action_dispatch/flash.rb"),
        rbs_src: include_str!("../runtime/ruby/action_dispatch/flash.rbs"),
        rb_path: "runtime/ruby/action_dispatch/flash.rb",
        namespace: "ActionDispatch",
        out_path: "src/flash.cr",
        mode: Mode::Library,
        imports: NO_IMPORTS,
        prelude: NO_PRELUDE,
        // Hand-written server.cr persists `flash.to_persisted` between
        // requests — invisible to the app-side reachability walk.
        extra_roots: &[("Flash", "to_persisted")],
    },
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/action_dispatch/session.rb"),
        rbs_src: include_str!("../runtime/ruby/action_dispatch/session.rbs"),
        rb_path: "runtime/ruby/action_dispatch/session.rb",
        namespace: "ActionDispatch",
        out_path: "src/session.cr",
        mode: Mode::Library,
        imports: NO_IMPORTS,
        prelude: NO_PRELUDE,
        extra_roots: NO_EXTRA_ROOTS,
    },
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/inflector.rb"),
        rbs_src: include_str!("../runtime/ruby/inflector.rbs"),
        rb_path: "runtime/ruby/inflector.rb",
        namespace: "",
        out_path: "src/inflector.cr",
        mode: Mode::Module,
        imports: NO_IMPORTS,
        prelude: NO_PRELUDE,
        extra_roots: NO_EXTRA_ROOTS,
    },
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/json_builder.rb"),
        rbs_src: include_str!("../runtime/ruby/json_builder.rbs"),
        rb_path: "runtime/ruby/json_builder.rb",
        namespace: "",
        out_path: "src/json_builder.cr",
        mode: Mode::Module,
        imports: NO_IMPORTS,
        prelude: NO_PRELUDE,
        extra_roots: NO_EXTRA_ROOTS,
    },
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/active_record/errors.rb"),
        rbs_src: include_str!("../runtime/ruby/active_record/errors.rbs"),
        rb_path: "runtime/ruby/active_record/errors.rb",
        namespace: "ActiveRecord",
        out_path: "src/errors.cr",
        mode: Mode::Library,
        imports: NO_IMPORTS,
        prelude: NO_PRELUDE,
        extra_roots: NO_EXTRA_ROOTS,
    },
    // `validations.rb` intentionally NOT transpiled — Phase 2.5(a)
    // inlines every `validates :x, …` declaration at lower time (see
    // `src/lower/model_to_library/validations.rs`). No transpiled
    // model dispatches into the Validations module any more.
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/active_record/base.rb"),
        rbs_src: include_str!("../runtime/ruby/active_record/base.rbs"),
        rb_path: "runtime/ruby/active_record/base.rb",
        namespace: "ActiveRecord",
        out_path: "src/active_record_base.cr",
        mode: Mode::Library,
        imports: NO_IMPORTS,
        prelude: NO_PRELUDE,
        extra_roots: NO_EXTRA_ROOTS,
    },
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/action_controller/base.rb"),
        rbs_src: include_str!("../runtime/ruby/action_controller/base.rbs"),
        rb_path: "runtime/ruby/action_controller/base.rb",
        namespace: "ActionController",
        out_path: "src/action_controller_base.cr",
        mode: Mode::Library,
        imports: NO_IMPORTS,
        prelude: NO_PRELUDE,
        extra_roots: NO_EXTRA_ROOTS,
    },
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/action_dispatch/router.rb"),
        rbs_src: include_str!("../runtime/ruby/action_dispatch/router.rbs"),
        rb_path: "runtime/ruby/action_dispatch/router.rb",
        namespace: "",
        out_path: "src/router.cr",
        mode: Mode::Library,
        imports: NO_IMPORTS,
        prelude: NO_PRELUDE,
        extra_roots: NO_EXTRA_ROOTS,
    },
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/action_view/view_helpers.rb"),
        rbs_src: include_str!("../runtime/ruby/action_view/view_helpers.rbs"),
        rb_path: "runtime/ruby/action_view/view_helpers.rb",
        namespace: "",
        out_path: "src/view_helpers.cr",
        mode: Mode::Library,
        imports: NO_IMPORTS,
        // `html_escape` emits Crystal's stdlib `HTML.escape` (see
        // src/emit/crystal/expr.rs); pull in the `html` module.
        prelude: "require \"html\"\n\n",
        extra_roots: NO_EXTRA_ROOTS,
    },
];

/// Parse + emit the Crystal runtime files. Mirrors `typescript_units`
/// — same driver, same `RuntimeEntry` shape, plus the Crystal-specific
/// `TargetEmit` and `#` comment prefix.
pub fn crystal_units<F>(mut transform: F) -> Result<Vec<RuntimeUnit>, String>
where
    F: FnMut(&str, Vec<LibraryClass>) -> Vec<LibraryClass>,
{
    let mut out = Vec::with_capacity(CRYSTAL_RUNTIME.len());
    for entry in CRYSTAL_RUNTIME {
        let unit = transpile_entry(entry, &CRYSTAL_TARGET, "#", &mut transform)?;
        out.push(unit);
    }
    Ok(out)
}

/// Rust runtime entries — Phase 2 of the rust migration (see
/// `docs/rust-migration-plan.md`). Populated file-by-file in
/// dependency order matching Crystal's RUNTIME_ORDER. Phase 3 layers
/// hand-written primitive runtime (`runtime/rust/`) on top.
const RUST_RUNTIME: &[RuntimeEntry] = &[
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/inflector.rb"),
        rbs_src: include_str!("../runtime/ruby/inflector.rbs"),
        rb_path: "runtime/ruby/inflector.rb",
        namespace: "",
        out_path: "src/inflector.rs",
        // Library (not Module) so the emit produces `pub struct
        // Inflector; impl Inflector { pub fn pluralize(...) }` —
        // matches the view-lowered IR's qualified call shape
        // `Inflector::pluralize(...)`. TS resolves the same shape via
        // `import * as Inflector`; Rust needs the struct.
        mode: Mode::Library,
        imports: NO_IMPORTS,
        prelude: NO_PRELUDE,
        extra_roots: NO_EXTRA_ROOTS,
    },
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/json_builder.rb"),
        rbs_src: include_str!("../runtime/ruby/json_builder.rbs"),
        rb_path: "runtime/ruby/json_builder.rb",
        namespace: "",
        out_path: "src/json_builder.rs",
        // Library mode parallel to inflector — emit `pub struct
        // JsonBuilder; impl JsonBuilder { pub fn encode_string ... }`
        // so the jbuilder view lowerer's `JsonBuilder::encode_string(...)`
        // qualified-call resolves. `RubyToS` covers `v.to_s` Sends on
        // untyped JSON values — the trait method needs to be in scope.
        mode: Mode::Library,
        imports: &[("RubyToS", "http")],
        prelude: NO_PRELUDE,
        extra_roots: NO_EXTRA_ROOTS,
    },
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/action_dispatch/router.rb"),
        rbs_src: include_str!("../runtime/ruby/action_dispatch/router.rbs"),
        rb_path: "runtime/ruby/action_dispatch/router.rb",
        namespace: "",
        out_path: "src/router.rs",
        // Library mode (not Module): router.rb now carries typed
        // `Route` and `MatchResult` classes alongside the
        // `Router.match` / `Router.match_pattern` class methods.
        mode: Mode::Library,
        imports: NO_IMPORTS,
        prelude: NO_PRELUDE,
        extra_roots: NO_EXTRA_ROOTS,
    },
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/action_view/view_helpers.rb"),
        rbs_src: include_str!("../runtime/ruby/action_view/view_helpers.rbs"),
        rb_path: "runtime/ruby/action_view/view_helpers.rb",
        namespace: "ActionView",
        out_path: "src/view_helpers.rs",
        mode: Mode::Library,
        imports: &[
            ("Base", "active_record_base"),
            ("merge_attrs", "hash_ext"),
            // `RubyToS` trait — the `inner_v.to_s` / `v.to_s` sends
            // in `render_attrs` lower to `(recv).ruby_to_s()` via
            // the `Ty::Untyped` arm in
            // `src/emit/rust2/expr/send/dispatch.rs`; the trait
            // needs to be in scope for those method calls to
            // resolve. Trait lives in `runtime/rust/http.rs` so it
            // ships alongside the hand-written response shape.
            ("RubyToS", "http"),
        ],
        prelude: NO_PRELUDE,
        extra_roots: NO_EXTRA_ROOTS,
    },
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/active_record/base.rb"),
        rbs_src: include_str!("../runtime/ruby/active_record/base.rbs"),
        rb_path: "runtime/ruby/active_record/base.rb",
        namespace: "ActiveRecord",
        out_path: "src/active_record_base.rs",
        mode: Mode::Library,
        // Phase 3 hand-written runtime ships these — make the
        // transpiled file's bare references resolve via use-imports.
        // Bare-token names mirror the Ruby source's call sites
        // (`raise NotImplementedError, ...`); the emit pipeline
        // lowers them to `raise(NotImplementedError, ...)` Rust
        // call syntax. Each entry becomes a separate `use crate::X::Y;`
        // line.
        imports: &[
            ("ActiveRecordAdapter", "active_record_adapter"),
            ("AdapterInterface", "adapter_interface"),
            ("raise", "errors_ext"),
            ("name", "errors_ext"),
            ("NotImplementedError", "errors_ext"),
            ("RecordNotFound", "errors_ext"),
            ("RecordInvalid", "errors_ext"),
        ],
        prelude: NO_PRELUDE,
        extra_roots: NO_EXTRA_ROOTS,
    },
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/action_controller/base.rb"),
        rbs_src: include_str!("../runtime/ruby/action_controller/base.rbs"),
        rb_path: "runtime/ruby/action_controller/base.rb",
        namespace: "ActionController",
        out_path: "src/action_controller_base.rs",
        mode: Mode::Library,
        imports: &[
            ("Flash", "flash"),
            ("Session", "session"),
            ("ParamValue", "param_value"),
            ("raise", "errors_ext"),
            ("NotImplementedError", "errors_ext"),
        ],
        prelude: NO_PRELUDE,
        extra_roots: NO_EXTRA_ROOTS,
    },
    // errors.rb intentionally NOT transpiled — the Rust-natural
    // `class < StandardError` shape needs Display + Error synthesis
    // that the transpile pipeline doesn't yet support. Phase 3
    // hand-writes `runtime/rust/errors_ext.rs` providing a single
    // `FrameworkError` enum + `raise_error` macro; the transpiled
    // bare tokens `NotImplementedError`/`RecordNotFound`/
    // `RecordInvalid` reach those via emit-side mapping (deferred
    // emit fix).
    //
    // HWIA intentionally NOT transpiled to rust2 — per Phase 2.5(b),
    // `@flash` and `@session` move to per-app ActionDispatch::Flash /
    // ActionDispatch::Session structs. Phase 3 hand-writes those in
    // `runtime/rust/{flash,session}.rs` with the same HWIA-shape API.
    // HWIA stays in runtime/ruby/ as a CRuby/Spinel helper for test
    // parity.
    //
    // `validations.rb` similarly NOT transpiled (Phase 2.5(a)) —
    // every `validates :x, …` declaration expands inline at lower
    // time (see `src/lower/model_to_library/validations.rs`).
];

/// Parse + emit the Rust runtime files. Mirrors `crystal_units` /
/// `typescript_units` — same driver, same `RuntimeEntry` shape,
/// plus the rust-specific `TargetEmit` and `//` comment prefix.
pub fn rust_units<F>(mut transform: F) -> Result<Vec<RuntimeUnit>, String>
where
    F: FnMut(&str, Vec<LibraryClass>) -> Vec<LibraryClass>,
{
    let mut out = Vec::with_capacity(RUST_RUNTIME.len());
    for entry in RUST_RUNTIME {
        let unit = transpile_entry(entry, &RUST_TARGET, "//", &mut transform)?;
        out.push(unit);
    }
    Ok(out)
}

/// Parse + emit the TypeScript runtime files. Returns one `RuntimeUnit`
/// per entry, ready to be written to disk by the caller.
///
/// Caller passes a `transform` closure that gets the parsed
/// `Vec<LibraryClass>` before emission — this is the hook for
/// runtime tree-shake (filter `lc.methods` to a reachable set).
/// Callers that don't need tree-shake pass `|c| c` (identity).
pub fn typescript_units<F>(mut transform: F) -> Result<Vec<RuntimeUnit>, String>
where
    F: FnMut(&str, Vec<LibraryClass>) -> Vec<LibraryClass>,
{
    let mut out = Vec::with_capacity(TYPESCRIPT_RUNTIME.len());
    for entry in TYPESCRIPT_RUNTIME {
        let unit = transpile_entry(entry, &TS_TARGET, "//", &mut transform)?;
        out.push(unit);
    }
    Ok(out)
}

/// `comment_prefix` selects the target's line-comment marker (TS:
/// `//`, Crystal: `#`). The header lines are prefixed with this so
/// the generated file's first lines are valid in the target language.
fn transpile_entry<F>(
    entry: &RuntimeEntry,
    target: &TargetEmit,
    comment_prefix: &str,
    transform: &mut F,
) -> Result<RuntimeUnit, String>
where
    F: FnMut(&str, Vec<LibraryClass>) -> Vec<LibraryClass>,
{
    let (emitted, classes, functions) = match entry.mode {
        Mode::Module => {
            let methods = parse_methods_with_rbs(entry.rb_src, entry.rbs_src)?;
            // Module-level constants (`ESCAPES = { ... }.freeze` in
            // `json_builder.rb`) prepend the method emits, same as in
            // Library mode below. Without this the methods reference
            // undefined constants and trigger "Error: undefined
            // constant" at the next compiler stage (Crystal
            // semantic analysis, Rust borrow-check, etc.).
            let constants = parse_module_constant_exprs(entry.rb_src)
                .unwrap_or_default();
            let mut body = String::new();
            for (name, value) in &constants {
                body.push_str(&format!(
                    "{}\n",
                    (target.format_constant)(name.as_str(), value),
                ));
            }
            if !constants.is_empty() {
                body.push('\n');
            }
            body.push_str(&(target.emit_module)(&methods)?);
            (body, Vec::new(), methods)
        }
        Mode::Library => {
            let classes = parse_library_with_rbs(
                entry.rb_src.as_bytes(),
                entry.rbs_src,
                entry.rb_path,
            )?;
            let classes = transform(entry.out_path, classes);
            // Module-level constants (`HTML_ESCAPES = { ... }.freeze`
            // in `view_helpers.rb`, etc.) emit as top-level
            // `const NAME = ...;` declarations BEFORE class bodies,
            // so methods that reference them resolve. Same source
            // walked by `parse_module_constants` for typing — these
            // two views need to stay in sync.
            //
            // Constant declaration syntax is target-specific (TS:
            // `const NAME = ...;`, Crystal: `NAME = ...`). Targets
            // that don't have a syntax for top-level constants in
            // their library file would need a different approach;
            // both currently-supported targets accept the form.
            let constants = parse_module_constant_exprs(entry.rb_src)
                .unwrap_or_default();
            let mut body = String::new();
            for (name, value) in &constants {
                body.push_str(&format!(
                    "{}\n",
                    (target.format_constant)(name.as_str(), value),
                ));
            }
            if !constants.is_empty() {
                body.push('\n');
            }
            // Module-level `@ivar = value` declarations (e.g.
            // `ViewHelpers @slots = {}`). Targets that don't model
            // module state at the source level leave
            // `format_module_ivar` as `None`; Go emits them as
            // package vars so class methods can reference them.
            if let Some(format_ivar) = target.format_module_ivar {
                let ivars = parse_module_ivar_exprs(entry.rb_src)
                    .unwrap_or_default();
                for (owner, name, value) in &ivars {
                    body.push_str(&format!(
                        "{}\n",
                        format_ivar(owner.as_str(), name.as_str(), value)
                    ));
                }
                if !ivars.is_empty() {
                    body.push('\n');
                }
            }
            for (i, c) in classes.iter().enumerate() {
                if i > 0 {
                    body.push('\n');
                }
                body.push_str(&(target.emit_library_class)(c)?);
            }
            // Wrap in the enclosing namespace, if any. TS no-ops here
            // (its target collapses namespaces through imports + the
            // treeshake registry); Crystal wraps in `module X ... end`
            // so refs of the form `X::Y` resolve at the call site.
            let body = (target.wrap_namespace)(entry.namespace, &body);
            (body, classes, Vec::new())
        }
    };

    let mut import_block = String::new();
    for (name, source) in entry.imports {
        import_block.push_str(&(target.format_import)(name, source));
    }
    if !import_block.is_empty() {
        import_block.push('\n');
    }

    let header = format!(
        "{cp} Generated from {} at app emit time.\n\
         {cp} Do not edit by hand — edit the source `.rb` and re-run emit.\n\n",
        entry.rb_path,
        cp = comment_prefix,
    );

    let content = format!(
        "{header}{}{import_block}{}{}",
        target.module_prelude, entry.prelude, emitted
    );

    Ok(RuntimeUnit {
        out_path: PathBuf::from(entry.out_path),
        content,
        classes,
        functions,
        namespace: entry.namespace,
        extra_roots: entry.extra_roots,
    })
}

// -------- Kotlin target --------
//
// Transpiles the framework runtime (`runtime/ruby/*.rb`) to Kotlin under
// the emitted project's `src/main/kotlin/`. All runtime files live in a
// single `roundhouse` package, so imports are unnecessary (same-package
// resolution) and namespaces collapse — `wrap_namespace` is a no-op like
// TypeScript's. The runtime is grown one file at a time (inflector →
// json_builder → … → action_controller/base), mirroring elixir2/go2.

const KOTLIN_TARGET: TargetEmit = TargetEmit {
    emit_module: crate::emit::kotlin::emit_module,
    emit_library_class: crate::emit::kotlin::emit_library_class_result,
    format_import: kotlin_format_import,
    format_constant: kotlin_format_constant,
    format_module_ivar: None,
    wrap_namespace: kotlin_wrap_namespace,
    // Every runtime file declares the shared package up front.
    module_prelude: "package roundhouse\n\n",
};

/// Same-package resolution → no per-symbol imports needed.
fn kotlin_format_import(_name: &str, _source: &str) -> String {
    String::new()
}

/// Top-level `val NAME = VALUE` (Kotlin allows file-level properties).
fn kotlin_format_constant(name: &str, value: &Expr) -> String {
    format!("val {name} = {}", crate::emit::kotlin::emit_constant_for_runtime(value))
}

/// Kotlin uses packages, not nested namespace blocks; the package decl in
/// `module_prelude` covers it, so this is a no-op.
fn kotlin_wrap_namespace(_namespace: &str, body: &str) -> String {
    body.to_string()
}

const KOTLIN_RUNTIME: &[RuntimeEntry] = &[
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/inflector.rb"),
        rbs_src: include_str!("../runtime/ruby/inflector.rbs"),
        rb_path: "runtime/ruby/inflector.rb",
        namespace: "",
        out_path: "src/main/kotlin/Inflector.kt",
        mode: Mode::Module,
        imports: NO_IMPORTS,
        prelude: NO_PRELUDE,
        extra_roots: NO_EXTRA_ROOTS,
    },
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/json_builder.rb"),
        rbs_src: include_str!("../runtime/ruby/json_builder.rbs"),
        rb_path: "runtime/ruby/json_builder.rb",
        namespace: "",
        out_path: "src/main/kotlin/JsonBuilder.kt",
        mode: Mode::Module,
        imports: NO_IMPORTS,
        prelude: NO_PRELUDE,
        extra_roots: NO_EXTRA_ROOTS,
    },
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/action_dispatch/router.rb"),
        rbs_src: include_str!("../runtime/ruby/action_dispatch/router.rbs"),
        rb_path: "runtime/ruby/action_dispatch/router.rb",
        namespace: "",
        out_path: "src/main/kotlin/Router.kt",
        mode: Mode::Library,
        imports: NO_IMPORTS,
        prelude: NO_PRELUDE,
        extra_roots: &[("Router", "match"), ("Router", "match_pattern")],
    },
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/active_record/errors.rb"),
        rbs_src: include_str!("../runtime/ruby/active_record/errors.rbs"),
        rb_path: "runtime/ruby/active_record/errors.rb",
        namespace: "",
        out_path: "src/main/kotlin/Errors.kt",
        mode: Mode::Library,
        imports: NO_IMPORTS,
        prelude: NO_PRELUDE,
        extra_roots: NO_EXTRA_ROOTS,
    },
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/active_record/base.rb"),
        rbs_src: include_str!("../runtime/ruby/active_record/base.rbs"),
        rb_path: "runtime/ruby/active_record/base.rb",
        namespace: "",
        out_path: "src/main/kotlin/ActiveRecordBase.kt",
        mode: Mode::Library,
        imports: NO_IMPORTS,
        prelude: NO_PRELUDE,
        extra_roots: NO_EXTRA_ROOTS,
    },
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/action_view/view_helpers.rb"),
        rbs_src: include_str!("../runtime/ruby/action_view/view_helpers.rbs"),
        rb_path: "runtime/ruby/action_view/view_helpers.rb",
        namespace: "",
        out_path: "src/main/kotlin/ViewHelpers.kt",
        mode: Mode::Module,
        imports: NO_IMPORTS,
        prelude: NO_PRELUDE,
        extra_roots: NO_EXTRA_ROOTS,
    },
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/action_dispatch/flash.rb"),
        rbs_src: include_str!("../runtime/ruby/action_dispatch/flash.rbs"),
        rb_path: "runtime/ruby/action_dispatch/flash.rb",
        namespace: "",
        out_path: "src/main/kotlin/Flash.kt",
        mode: Mode::Library,
        imports: NO_IMPORTS,
        prelude: NO_PRELUDE,
        // Hand-written Server.dispatch persists `flash.toPersisted` between
        // requests (cookie-backed) — invisible to the app-side reachability
        // walk, so seed it. (Inert today — kotlin doesn't treeshake — but
        // documents the dep + matches go/crystal/ts.)
        extra_roots: &[("Flash", "to_persisted")],
    },
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/action_dispatch/session.rb"),
        rbs_src: include_str!("../runtime/ruby/action_dispatch/session.rbs"),
        rb_path: "runtime/ruby/action_dispatch/session.rb",
        namespace: "",
        out_path: "src/main/kotlin/Session.kt",
        mode: Mode::Library,
        imports: NO_IMPORTS,
        prelude: NO_PRELUDE,
        extra_roots: NO_EXTRA_ROOTS,
    },
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/action_controller/base.rb"),
        rbs_src: include_str!("../runtime/ruby/action_controller/base.rbs"),
        rb_path: "runtime/ruby/action_controller/base.rb",
        namespace: "",
        out_path: "src/main/kotlin/ActionControllerBase.kt",
        mode: Mode::Library,
        imports: NO_IMPORTS,
        prelude: NO_PRELUDE,
        extra_roots: NO_EXTRA_ROOTS,
    },
    // errors.rb + base.rb WIRED above (this pass). The transpiled
    // Errors.kt + ActiveRecordBase.kt compile clean kotlinc alongside the
    // primitives (Time/Db/ParamValue/AdapterInterface) + the rest of the
    // runtime (Inflector/JsonBuilder/Router). The legacy *functional*
    // adapter path is DROPPED for Kotlin: AdapterInterface.kt (a hand-
    // written primitive) is the compile-time contract Base's class-level
    // CRUD defaults type-check against, but no implementation is provided
    // and `ActiveRecord.adapter` is never assigned — all real CRUD is
    // Db-direct via the Level-3 per-model `_adapter_*` overrides (Kotlin
    // companions aren't inherited, so Base's defaults are unreached). The
    // only callers without a per-model override are `where`/`find_by`,
    // which real-blog never invokes.
    //
    // flash.rb / session.rb deferred: they need non-accessor ivar types
    // (rbs `@data: Hash[...]`) plumbed to emit, Map shim methods
    // (delete→remove), and `!!` for mutable-property smart-casts. The
    // emitter groundwork (yield→block, init-block run-wrapper, `!`/push)
    // is in place; wiring the entries is the next step.
    //
    // MODEL emit punch-list — LANDED (83→6). With base wired, the emitted
    // models (Article/Comment/ApplicationRecord) surfaced ~83 errors;
    // resolved across these clusters (all in src/emit/kotlin/):
    //  (1) `open class` + `override` modifiers — class-hierarchy registry
    //      (expr CLASS_HIERARCHY) drives `open`/`override` per member.
    //  (2) untyped-Map→typed-property Cast — `self.<col> = row[k]`/`attrs[k]`
    //      coerced to the column scalar (INSTANCE_PROP_TYPES + emit_cast),
    //      self-receiver-gated so `from_row` (already Cast) is untouched.
    //  (3) has_many lazy loader — wrap_return recurses into a nested Seq
    //      (fixing `return val stmt = …`); body-ivar typing now reads the
    //      `Ty` on ivar nodes (`@comments_cache` → `MutableList<Comment>`).
    //  (4) per-model companion finders (all/find/count/exists/last/
    //      destroyAll/create/createBang) synthesized delegating to the
    //      model's `_adapter_*` (companions aren't inherited); `last` uses
    //      `size-1` (fixes the negative-index latent bug).
    //  (5) Broadcasts no-op primitive (primitives.rs); `return nil` in a
    //      Unit callback → bare `return` (RETURNS_UNIT).
    //
    // PHASE 4 VIEW EMIT — STARTED (48→11 kotlinc errors). The model↔view 6
    // are CLOSED: each `Views::<Plural>` template-LC is now lowered with the
    // model registry and emitted as a merged `object` (app/views/*.kt) —
    // Kotlin objects can't be reopened, so templates sharing a module
    // collapse into one object named for the call site (`Articles.article`,
    // last-segment of `Views::Articles`). RouteHelpers emits clean (object
    // from lower_routes_to_library_functions). view_helpers.rb WIRED above →
    // ViewHelpers.kt. Emit support added (all in src/emit/kotlin/): StringBuilder
    // IrHints (io=StringBuilder/append/toString), param-as-local (a bare
    // no-recv 0-arg send naming a param → identifier, since the view lowerer
    // renders a partial local as a Send in arg position but a Var as a
    // receiver), `!x` not (Send{None,"!",[x]} was dropped — inverted
    // any?/present?), typed-receiver instance-method registry (article.comments()
    // keeps parens vs article.title property), collection `count`→size, map
    // fetch/delete/merge + tr/join + is_a?(Hash/Array), object-level @ivar
    // decls (ViewHelpers @slots → `private var slots: MutableMap<String,String>`).
    // PHASE 4 NOW kotlinc-CLEAN (0 errors across all 21 emitted .kt). Closed
    // 48→0 via: kwarg→named-arg (trailing IR-flagged `kwargs:true` hash splats
    // to Kotlin named args ONLY when the callee is registered (Receiver.method
    // → param-names, METHOD_PARAMS) and keys ⊆ params — so `truncate(body,
    // length=100)` but `Broadcasts.append(map)` stays a map; gated so it never
    // catches a genuine sym-keyed map arg); Importmap emitted as a function
    // module (library::emit_function_module, shared w/ RouteHelpers; resolves
    // the layout's `Importmap.pins()/.entry()`); Base64.strict_encode64→
    // java.util.Base64, JSON.generate→JsonBuilder.encodeValue; Ty::Record→
    // MutableMap<String,Any?> (importmap pins; Router uses a named class not
    // Record so unaffected); `.each` on a nullable Array (ty_is peers through
    // Union{T,Nil}); and a var-HOIST pass (scan_hoist: a local first assigned
    // in a nested scope but assigned again at an outer level hoists a typed
    // `var x = default` to the method top so the outer write resolves —
    // `json` in javascript_importmap_tags). THEN Server.kt (Javalin) + Main.kt
    // + controllers (lower_controllers_with_arel_and_views) → boot + serve.
    // (Emit-polish later: the guard-return lowering emits `if (cond) { null }
    // else { return X }`, which warns "expression is unused" — correct, ugly.)
];

pub fn kotlin_units<F>(mut transform: F) -> Result<Vec<RuntimeUnit>, String>
where
    F: FnMut(&str, Vec<LibraryClass>) -> Vec<LibraryClass>,
{
    let mut out = Vec::with_capacity(KOTLIN_RUNTIME.len());
    for entry in KOTLIN_RUNTIME {
        out.push(transpile_entry(entry, &KOTLIN_TARGET, "//", &mut transform)?);
    }
    Ok(out)
}

// -------- C# target --------
//
// Transpiles the framework runtime (`runtime/ruby/*.rb`) to C# under the
// emitted project's `app/runtime/`. The whole emit is a single `Roundhouse`
// namespace, so imports are unnecessary (same-namespace resolution) and
// namespaces collapse — `wrap_namespace` is a no-op like Kotlin's. The
// runtime is grown one file at a time (inflector → json_builder → …),
// mirroring the Kotlin/Swift arc (see `docs/csharp-migration-plan.md`).

const CSHARP_TARGET: TargetEmit = TargetEmit {
    emit_module: crate::emit::csharp::emit_module,
    emit_library_class: crate::emit::csharp::emit_library_class_result,
    format_import: csharp_format_import,
    format_constant: csharp_format_constant,
    format_module_ivar: None,
    wrap_namespace: csharp_wrap_namespace,
    // Each runtime file opens with the usings + file-scoped namespace. The
    // `using static` brings the shared `RuntimeConstants` members into bare
    // scope (C# has no top-level constant — see `csharp_format_constant`).
    module_prelude: "using System;\n\
                     using System.Collections.Generic;\n\
                     using System.Linq;\n\
                     using System.Text;\n\
                     using System.Text.RegularExpressions;\n\
                     using static Roundhouse.RuntimeConstants;\n\n\
                     namespace Roundhouse;\n\n",
};

/// Same-namespace resolution → no per-symbol imports needed.
fn csharp_format_import(_name: &str, _source: &str) -> String {
    String::new()
}

/// Module-level constants have no top-level form in C# — they're emitted as
/// fragments of a shared `partial class RuntimeConstants`, reached via the
/// `using static` in `module_prelude`.
fn csharp_format_constant(name: &str, value: &Expr) -> String {
    crate::emit::csharp::emit_module_constant(name, value)
}

/// C# uses the file-scoped `namespace Roundhouse;` from `module_prelude`; no
/// nested namespace wrapping needed.
fn csharp_wrap_namespace(_namespace: &str, body: &str) -> String {
    body.to_string()
}

const CSHARP_RUNTIME: &[RuntimeEntry] = &[
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/inflector.rb"),
        rbs_src: include_str!("../runtime/ruby/inflector.rbs"),
        rb_path: "runtime/ruby/inflector.rb",
        namespace: "",
        out_path: "app/runtime/Inflector.cs",
        mode: Mode::Module,
        imports: NO_IMPORTS,
        prelude: NO_PRELUDE,
        extra_roots: NO_EXTRA_ROOTS,
    },
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/json_builder.rb"),
        rbs_src: include_str!("../runtime/ruby/json_builder.rbs"),
        rb_path: "runtime/ruby/json_builder.rb",
        namespace: "",
        out_path: "app/runtime/JsonBuilder.cs",
        mode: Mode::Module,
        imports: NO_IMPORTS,
        prelude: NO_PRELUDE,
        extra_roots: NO_EXTRA_ROOTS,
    },
    // `base` before `errors`: errors.rb's `RecordInvalid#initialize` calls
    // `record.errors` (a method on Base), which only resolves as a call once
    // `ActiveRecordBase` is registered. base.rb only *raises* RecordNotFound/
    // RecordInvalid (name-only), so it needs nothing from errors at emit time.
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/active_record/base.rb"),
        rbs_src: include_str!("../runtime/ruby/active_record/base.rbs"),
        rb_path: "runtime/ruby/active_record/base.rb",
        namespace: "ActiveRecord",
        out_path: "app/runtime/ActiveRecordBase.cs",
        mode: Mode::Library,
        imports: NO_IMPORTS,
        prelude: NO_PRELUDE,
        extra_roots: NO_EXTRA_ROOTS,
    },
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/active_record/errors.rb"),
        rbs_src: include_str!("../runtime/ruby/active_record/errors.rbs"),
        rb_path: "runtime/ruby/active_record/errors.rb",
        namespace: "ActiveRecord",
        out_path: "app/runtime/Errors.cs",
        mode: Mode::Library,
        imports: NO_IMPORTS,
        prelude: NO_PRELUDE,
        extra_roots: NO_EXTRA_ROOTS,
    },
    // Flash + Session: the per-request state ActionController::Base holds.
    // Independent of the AR layer; needed before AC::Base (it instantiates
    // both and uses `@flash[:notice] = …`).
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/action_dispatch/flash.rb"),
        rbs_src: include_str!("../runtime/ruby/action_dispatch/flash.rbs"),
        rb_path: "runtime/ruby/action_dispatch/flash.rb",
        namespace: "ActionDispatch",
        out_path: "app/runtime/Flash.cs",
        mode: Mode::Library,
        imports: NO_IMPORTS,
        prelude: NO_PRELUDE,
        extra_roots: NO_EXTRA_ROOTS,
    },
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/action_dispatch/session.rb"),
        rbs_src: include_str!("../runtime/ruby/action_dispatch/session.rbs"),
        rb_path: "runtime/ruby/action_dispatch/session.rb",
        namespace: "ActionDispatch",
        out_path: "app/runtime/Session.cs",
        mode: Mode::Library,
        imports: NO_IMPORTS,
        prelude: NO_PRELUDE,
        extra_roots: NO_EXTRA_ROOTS,
    },
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/action_controller/base.rb"),
        rbs_src: include_str!("../runtime/ruby/action_controller/base.rbs"),
        rb_path: "runtime/ruby/action_controller/base.rb",
        namespace: "ActionController",
        out_path: "app/runtime/ActionControllerBase.cs",
        mode: Mode::Library,
        imports: NO_IMPORTS,
        prelude: NO_PRELUDE,
        extra_roots: NO_EXTRA_ROOTS,
    },
];

pub fn csharp_units<F>(mut transform: F) -> Result<Vec<RuntimeUnit>, String>
where
    F: FnMut(&str, Vec<LibraryClass>) -> Vec<LibraryClass>,
{
    let mut out = Vec::with_capacity(CSHARP_RUNTIME.len());
    for entry in CSHARP_RUNTIME {
        out.push(transpile_entry(entry, &CSHARP_TARGET, "//", &mut transform)?);
    }
    Ok(out)
}

// -------- Swift target --------
//
// Transpiles the framework runtime (`runtime/ruby/*.rb`) to Swift under
// the emitted project's `Sources/App/`. The whole emit is a single Swift
// module, so imports are unnecessary (same-module resolution) and
// namespaces collapse — `wrap_namespace` is a no-op like Kotlin's. The
// runtime is grown one file at a time (inflector → json_builder → …),
// mirroring the Kotlin arc (see `docs/swift-migration-plan.md`).

const SWIFT_TARGET: TargetEmit = TargetEmit {
    emit_module: crate::emit::swift::emit_module,
    emit_library_class: crate::emit::swift::emit_library_class_result,
    format_import: swift_format_import,
    format_constant: swift_format_constant,
    format_module_ivar: None,
    wrap_namespace: swift_wrap_namespace,
    // No package decl in Swift; Foundation covers the string/format APIs
    // the transpiled runtime leans on (replacingOccurrences, …).
    module_prelude: "import Foundation\n\n",
};

/// Same-module resolution → no per-symbol imports needed.
fn swift_format_import(_name: &str, _source: &str) -> String {
    String::new()
}

/// Top-level `let NAME = VALUE` (Swift allows file-level constants).
fn swift_format_constant(name: &str, value: &Expr) -> String {
    format!("let {name} = {}", crate::emit::swift::emit_constant_for_runtime(value))
}

/// Single flat module; no namespace blocks.
fn swift_wrap_namespace(_namespace: &str, body: &str) -> String {
    body.to_string()
}

const SWIFT_RUNTIME: &[RuntimeEntry] = &[
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/inflector.rb"),
        rbs_src: include_str!("../runtime/ruby/inflector.rbs"),
        rb_path: "runtime/ruby/inflector.rb",
        namespace: "",
        out_path: "Sources/App/Inflector.swift",
        mode: Mode::Module,
        imports: NO_IMPORTS,
        prelude: NO_PRELUDE,
        extra_roots: NO_EXTRA_ROOTS,
    },
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/json_builder.rb"),
        rbs_src: include_str!("../runtime/ruby/json_builder.rbs"),
        rb_path: "runtime/ruby/json_builder.rb",
        namespace: "",
        out_path: "Sources/App/JsonBuilder.swift",
        mode: Mode::Module,
        imports: NO_IMPORTS,
        prelude: NO_PRELUDE,
        extra_roots: NO_EXTRA_ROOTS,
    },
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/action_dispatch/router.rb"),
        rbs_src: include_str!("../runtime/ruby/action_dispatch/router.rbs"),
        rb_path: "runtime/ruby/action_dispatch/router.rb",
        namespace: "",
        out_path: "Sources/App/Router.swift",
        mode: Mode::Library,
        imports: NO_IMPORTS,
        prelude: NO_PRELUDE,
        extra_roots: &[("Router", "match"), ("Router", "match_pattern")],
    },
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/active_record/errors.rb"),
        rbs_src: include_str!("../runtime/ruby/active_record/errors.rbs"),
        rb_path: "runtime/ruby/active_record/errors.rb",
        namespace: "",
        out_path: "Sources/App/Errors.swift",
        mode: Mode::Library,
        imports: NO_IMPORTS,
        prelude: NO_PRELUDE,
        extra_roots: NO_EXTRA_ROOTS,
    },
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/active_record/base.rb"),
        rbs_src: include_str!("../runtime/ruby/active_record/base.rbs"),
        rb_path: "runtime/ruby/active_record/base.rb",
        namespace: "",
        out_path: "Sources/App/ActiveRecordBase.swift",
        mode: Mode::Library,
        imports: NO_IMPORTS,
        prelude: NO_PRELUDE,
        extra_roots: NO_EXTRA_ROOTS,
    },
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/action_view/view_helpers.rb"),
        rbs_src: include_str!("../runtime/ruby/action_view/view_helpers.rbs"),
        rb_path: "runtime/ruby/action_view/view_helpers.rb",
        namespace: "",
        out_path: "Sources/App/ViewHelpers.swift",
        mode: Mode::Module,
        imports: NO_IMPORTS,
        prelude: NO_PRELUDE,
        extra_roots: NO_EXTRA_ROOTS,
    },
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/action_dispatch/flash.rb"),
        rbs_src: include_str!("../runtime/ruby/action_dispatch/flash.rbs"),
        rb_path: "runtime/ruby/action_dispatch/flash.rb",
        namespace: "",
        out_path: "Sources/App/Flash.swift",
        mode: Mode::Library,
        imports: NO_IMPORTS,
        prelude: NO_PRELUDE,
        // Hand-written Server.dispatch persists `flash.toPersisted` between
        // requests (cookie-backed) — invisible to the app-side reachability
        // walk, so seed it. (Inert today — swift doesn't treeshake — but
        // documents the dep + matches go/kotlin/crystal/ts.)
        extra_roots: &[("Flash", "to_persisted")],
    },
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/action_dispatch/session.rb"),
        rbs_src: include_str!("../runtime/ruby/action_dispatch/session.rbs"),
        rb_path: "runtime/ruby/action_dispatch/session.rb",
        namespace: "",
        out_path: "Sources/App/Session.swift",
        mode: Mode::Library,
        imports: NO_IMPORTS,
        prelude: NO_PRELUDE,
        extra_roots: NO_EXTRA_ROOTS,
    },
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/action_controller/base.rb"),
        rbs_src: include_str!("../runtime/ruby/action_controller/base.rbs"),
        rb_path: "runtime/ruby/action_controller/base.rb",
        namespace: "",
        out_path: "Sources/App/ActionControllerBase.swift",
        mode: Mode::Library,
        imports: NO_IMPORTS,
        prelude: NO_PRELUDE,
        extra_roots: NO_EXTRA_ROOTS,
    },
];

pub fn swift_units<F>(mut transform: F) -> Result<Vec<RuntimeUnit>, String>
where
    F: FnMut(&str, Vec<LibraryClass>) -> Vec<LibraryClass>,
{
    let mut out = Vec::with_capacity(SWIFT_RUNTIME.len());
    for entry in SWIFT_RUNTIME {
        out.push(transpile_entry(entry, &SWIFT_TARGET, "//", &mut transform)?);
    }
    Ok(out)
}

// -------- Go target (Phase 1: scaffolded, stubs only) --------
//
// Mirrors the Rust target wiring above. The `GO_TARGET` callbacks
// dispatch into `src/emit/go2/library.rs`, which emits
// SYNTACTICALLY-valid Go for every method shape but populates
// bodies with `panic("go2 stub")`. The point of Phase 1 is to make
// end-to-end emit run, not to produce correct Go semantics —
// `go build ./...` will surface a real error inventory we can drive
// subsequent sessions against. See `src/emit/go2.rs` for the
// overlay strategy.

const GO_TARGET: TargetEmit = TargetEmit {
    emit_module: crate::emit::go2::emit_module,
    emit_library_class: crate::emit::go2::emit_library_class,
    format_import: go_format_import,
    format_constant: go_format_constant_thunk,
    format_module_ivar: Some(go_format_module_ivar_thunk),
    wrap_namespace: go_wrap_namespace,
    module_prelude: "",
};

/// Go uses path-based imports. `name` is informational (Go can't
/// import individual symbols), `source` is the import path.
fn go_format_import(_name: &str, source: &str) -> String {
    format!("import {source:?}\n")
}

/// Forwards to the go2 stub. Crystal/Rust have a dedicated emit-side
/// constant renderer; go2 doesn't yet, so the stub emits `var NAME
/// interface{} = nil` placeholders.
fn go_format_constant_thunk(name: &str, value: &Expr) -> String {
    crate::emit::go2::format_constant(name, value)
}

/// Module-level `@ivar = value` → Go `var <Owner>_<ivar>_slot = <value>`.
/// `owner` is the qualified Ruby module/class path the ivar was
/// declared inside (e.g. `"ActionView::ViewHelpers"`); the formatter
/// strips `::` to produce the Go-legal identifier. The `_slot` suffix
/// and the namespacing rule mirror the read-side emit in
/// `src/emit/go2/expr.rs::ExprNode::Ivar`, so `@slots` writes here
/// and `@slots` reads there resolve to the same package var.
fn go_format_module_ivar_thunk(owner: &str, name: &str, value: &Expr) -> String {
    crate::emit::go2::format_module_ivar(owner, name, value)
}

/// Go's package-per-directory model means namespaces collapse into
/// the file's `package` declaration. The overlay rewrites this to
/// `package v2` at file-emit time; here we just return the body
/// unchanged (the per-unit content gets a `package app` prelude
/// prepended by the GO_RUNTIME entries — see `prelude`).
fn go_wrap_namespace(_namespace: &str, body: &str) -> String {
    body.to_string()
}

/// Go runtime transpile table. Mirrors `RUST_RUNTIME` 1:1 by source
/// file. Out paths flatten under `app/` (the go2 overlay then
/// relocates each to `app/v2/<name>.go`). The `prelude` line emits
/// `package app\n` so the file file-parses before the overlay
/// rewrites the package to `v2`.
const GO_PRELUDE: &str = "package app\n\n";

const GO_RUNTIME: &[RuntimeEntry] = &[
    // Smallest runtime file (1 method, no cross-class deps) — drives
    // the per-variant walker widening cycle. Additional entries
    // (json_builder, router, view_helpers, active_record/base,
    // action_controller/base) land as `super::expr` grows coverage
    // for the variants their bodies hit. The list stays narrow on
    // purpose so the v2/ overlay keeps go-build clean — the inventory
    // signal we want is whether the CURRENT scope still passes.
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/inflector.rb"),
        rbs_src: include_str!("../runtime/ruby/inflector.rbs"),
        rb_path: "runtime/ruby/inflector.rb",
        namespace: "",
        out_path: "app/inflector.go",
        mode: Mode::Library,
        imports: NO_IMPORTS,
        prelude: GO_PRELUDE,
        extra_roots: NO_EXTRA_ROOTS,
    },
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/json_builder.rb"),
        rbs_src: include_str!("../runtime/ruby/json_builder.rbs"),
        rb_path: "runtime/ruby/json_builder.rb",
        namespace: "",
        out_path: "app/json_builder.go",
        mode: Mode::Library,
        imports: NO_IMPORTS,
        prelude: GO_PRELUDE,
        extra_roots: NO_EXTRA_ROOTS,
    },
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/action_dispatch/router.rb"),
        rbs_src: include_str!("../runtime/ruby/action_dispatch/router.rbs"),
        rb_path: "runtime/ruby/action_dispatch/router.rb",
        namespace: "",
        out_path: "app/router.go",
        mode: Mode::Library,
        imports: NO_IMPORTS,
        prelude: GO_PRELUDE,
        extra_roots: NO_EXTRA_ROOTS,
    },
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/active_record/base.rb"),
        rbs_src: include_str!("../runtime/ruby/active_record/base.rbs"),
        rb_path: "runtime/ruby/active_record/base.rb",
        namespace: "",
        out_path: "app/active_record_base.go",
        mode: Mode::Library,
        imports: NO_IMPORTS,
        prelude: GO_PRELUDE,
        extra_roots: NO_EXTRA_ROOTS,
    },
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/action_dispatch/session.rb"),
        rbs_src: include_str!("../runtime/ruby/action_dispatch/session.rbs"),
        rb_path: "runtime/ruby/action_dispatch/session.rb",
        namespace: "",
        out_path: "app/session.go",
        mode: Mode::Library,
        imports: NO_IMPORTS,
        prelude: GO_PRELUDE,
        extra_roots: NO_EXTRA_ROOTS,
    },
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/action_dispatch/flash.rb"),
        rbs_src: include_str!("../runtime/ruby/action_dispatch/flash.rbs"),
        rb_path: "runtime/ruby/action_dispatch/flash.rb",
        namespace: "",
        out_path: "app/flash.go",
        mode: Mode::Library,
        imports: NO_IMPORTS,
        prelude: GO_PRELUDE,
        // Hand-written server.go + emitted dispatch.go persist
        // `flash.to_persisted` between requests (cookie-backed) —
        // invisible to the app-side reachability walk, so seed it.
        extra_roots: &[("Flash", "to_persisted")],
    },
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/action_view/view_helpers.rb"),
        rbs_src: include_str!("../runtime/ruby/action_view/view_helpers.rbs"),
        rb_path: "runtime/ruby/action_view/view_helpers.rb",
        namespace: "",
        out_path: "app/view_helpers.go",
        mode: Mode::Library,
        imports: NO_IMPORTS,
        prelude: GO_PRELUDE,
        extra_roots: NO_EXTRA_ROOTS,
    },
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/action_controller/base.rb"),
        rbs_src: include_str!("../runtime/ruby/action_controller/base.rbs"),
        rb_path: "runtime/ruby/action_controller/base.rb",
        namespace: "",
        out_path: "app/action_controller_base.go",
        mode: Mode::Library,
        imports: NO_IMPORTS,
        prelude: GO_PRELUDE,
        extra_roots: NO_EXTRA_ROOTS,
    },
];

/// Parse + emit the Go runtime files. Phase 1 scaffold — emit shape
/// is stubbed (see `src/emit/go2/library.rs`).
pub fn go_units<F>(mut transform: F) -> Result<Vec<RuntimeUnit>, String>
where
    F: FnMut(&str, Vec<LibraryClass>) -> Vec<LibraryClass>,
{
    let mut out = Vec::with_capacity(GO_RUNTIME.len());
    for entry in GO_RUNTIME {
        let unit = transpile_entry(entry, &GO_TARGET, "//", &mut transform)?;
        out.push(unit);
    }
    Ok(out)
}

// -------- Elixir target (Phase 1: scaffolded, stubs only) --------
//
// Mirrors the Go/Rust target wiring above. The `ELIXIR_TARGET`
// callbacks dispatch into `src/emit/elixir2/library.rs`, which emits
// syntactically-valid Elixir for every method shape but stubs bodies
// with `raise "elixir2 stub"`. The point of Phase 1 is end-to-end
// emit, not correct semantics — `mix compile --warnings-as-errors`
// over the `lib/v2/` overlay surfaces a real error inventory for
// subsequent sessions. See `src/emit/elixir2.rs` for the overlay
// strategy.

const ELIXIR_TARGET: TargetEmit = TargetEmit {
    emit_module: crate::emit::elixir2::emit_module,
    emit_library_class: crate::emit::elixir2::emit_library_class,
    format_import: elixir_format_import,
    format_constant: elixir_format_constant,
    // Elixir has no module-level mutable state (modules are static).
    // `ActionView::ViewHelpers`'s `@slots` and friends defer to a
    // hand-written process/Agent-based holder in a later phase, the
    // same way TS/Crystal/Rust opt out here.
    format_module_ivar: None,
    wrap_namespace: elixir_wrap_namespace,
    module_prelude: "",
};

/// Elixir `alias`. `name` is the module; the source path is implicit
/// (Mix compiles the project together). Informational for Phase 1 —
/// the `ELIXIR_RUNTIME` slice declares no imports yet.
fn elixir_format_import(name: &str, _source: &str) -> String {
    format!("alias {name}\n")
}

/// Module-level constant → Elixir module attribute (delegated to
/// `elixir2`, which renders the value and strips `.freeze`). Emitted
/// just ahead of the class body so it sits INSIDE the module the
/// namespace wrapper supplies.
fn elixir_format_constant(name: &str, value: &Expr) -> String {
    crate::emit::elixir2::format_constant(name, value)
}

/// Each class already emits its own `defmodule V2.<Name>` (see
/// `emit_library_class`), so this hook's job for Elixir is just to
/// place module-level constants INSIDE their module. `transpile_entry`
/// emits constants (via `format_constant`) as lines ahead of the class
/// bodies, but Elixir has no file-level constants and module attributes
/// don't cross module boundaries — so move any leading constant lines
/// into the first `defmodule`. (Current const-bearing files are single-
/// module — `json_builder`, `action_controller/base`; a multi-module
/// file with constants would need owner-aware routing, revisit then.)
/// The `namespace` arg is unused: V2-prefixing + naming happen in
/// `emit_library_class`.
fn elixir_wrap_namespace(_namespace: &str, body: &str) -> String {
    let lines: Vec<&str> = body.lines().collect();
    let Some(first_mod) = lines.iter().position(|l| l.trim_start().starts_with("defmodule "))
    else {
        return body.to_string();
    };
    let consts: Vec<&str> = lines[..first_mod]
        .iter()
        .copied()
        .filter(|l| !l.trim().is_empty())
        .collect();
    if consts.is_empty() {
        return body.to_string();
    }
    let mut out = String::new();
    out.push_str(lines[first_mod]); // `defmodule V2.X do`
    out.push('\n');
    for c in &consts {
        out.push_str(c);
        out.push('\n');
    }
    for l in &lines[first_mod + 1..] {
        out.push_str(l);
        out.push('\n');
    }
    out
}

/// Elixir runtime transpile table. Widened one file at a time as
/// `elixir2`'s body walker grows, keeping the `lib/v2/` overlay
/// compile-clean so the inventory signal is whether the CURRENT scope
/// passes. Each class self-names as `V2.<Module>` in
/// `emit_library_class`, so the `namespace` field is unused for Elixir
/// (left empty). Remaining files (active_record/base, view_helpers) land
/// as the body walker grows to cover while-loop→recursion and instance
/// mutation-threading.
const ELIXIR_RUNTIME: &[RuntimeEntry] = &[
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/inflector.rb"),
        rbs_src: include_str!("../runtime/ruby/inflector.rbs"),
        rb_path: "runtime/ruby/inflector.rb",
        namespace: "",
        out_path: "lib/v2/inflector.ex",
        mode: Mode::Library,
        imports: NO_IMPORTS,
        prelude: NO_PRELUDE,
        extra_roots: NO_EXTRA_ROOTS,
    },
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/json_builder.rb"),
        rbs_src: include_str!("../runtime/ruby/json_builder.rbs"),
        rb_path: "runtime/ruby/json_builder.rb",
        namespace: "",
        out_path: "lib/v2/json_builder.ex",
        mode: Mode::Library,
        imports: NO_IMPORTS,
        prelude: NO_PRELUDE,
        extra_roots: NO_EXTRA_ROOTS,
    },
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/action_dispatch/router.rb"),
        rbs_src: include_str!("../runtime/ruby/action_dispatch/router.rbs"),
        rb_path: "runtime/ruby/action_dispatch/router.rb",
        namespace: "",
        out_path: "lib/v2/router.ex",
        mode: Mode::Library,
        imports: NO_IMPORTS,
        prelude: NO_PRELUDE,
        extra_roots: NO_EXTRA_ROOTS,
    },
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/action_dispatch/flash.rb"),
        rbs_src: include_str!("../runtime/ruby/action_dispatch/flash.rbs"),
        rb_path: "runtime/ruby/action_dispatch/flash.rb",
        namespace: "",
        out_path: "lib/v2/flash.ex",
        mode: Mode::Library,
        imports: NO_IMPORTS,
        prelude: NO_PRELUDE,
        // Hand-written server.ex + emitted dispatch.ex persist
        // `flash.to_persisted` between requests (cookie-backed) — invisible
        // to the app-side reachability walk, so seed it. (Inert today —
        // elixir doesn't treeshake — but documents the dep.)
        extra_roots: &[("Flash", "to_persisted")],
    },
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/action_dispatch/session.rb"),
        rbs_src: include_str!("../runtime/ruby/action_dispatch/session.rbs"),
        rb_path: "runtime/ruby/action_dispatch/session.rb",
        namespace: "",
        out_path: "lib/v2/session.ex",
        mode: Mode::Library,
        imports: NO_IMPORTS,
        prelude: NO_PRELUDE,
        extra_roots: NO_EXTRA_ROOTS,
    },
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/action_controller/base.rb"),
        rbs_src: include_str!("../runtime/ruby/action_controller/base.rbs"),
        rb_path: "runtime/ruby/action_controller/base.rb",
        namespace: "",
        out_path: "lib/v2/action_controller_base.ex",
        mode: Mode::Library,
        imports: NO_IMPORTS,
        prelude: NO_PRELUDE,
        extra_roots: NO_EXTRA_ROOTS,
    },
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/action_view/view_helpers.rb"),
        rbs_src: include_str!("../runtime/ruby/action_view/view_helpers.rbs"),
        rb_path: "runtime/ruby/action_view/view_helpers.rb",
        namespace: "ActionView",
        out_path: "lib/v2/view_helpers.ex",
        mode: Mode::Library,
        imports: NO_IMPORTS,
        prelude: NO_PRELUDE,
        extra_roots: NO_EXTRA_ROOTS,
    },
];

/// Parse + emit the Elixir runtime files. Phase 1 scaffold — emit shape
/// is stubbed (see `src/emit/elixir2/library.rs`).
pub fn elixir_units<F>(mut transform: F) -> Result<Vec<RuntimeUnit>, String>
where
    F: FnMut(&str, Vec<LibraryClass>) -> Vec<LibraryClass>,
{
    let mut out = Vec::with_capacity(ELIXIR_RUNTIME.len());
    for entry in ELIXIR_RUNTIME {
        let unit = transpile_entry(entry, &ELIXIR_TARGET, "#", &mut transform)?;
        out.push(unit);
    }
    Ok(out)
}

/// Parse every Library-mode Elixir runtime entry and hand its classes to
/// `f`. A pre-emit pass uses this to register ALL `V2.*` module names
/// globally before any file is emitted, so a constant reference that
/// crosses files (`ActionController::Base` referencing
/// `ActionDispatch::Session` from another unit) resolves — `elixir_units`
/// emits one file at a time and can't see modules it hasn't reached yet.
pub fn elixir_library_classes<F>(mut f: F) -> Result<(), String>
where
    F: FnMut(&[LibraryClass]),
{
    for entry in ELIXIR_RUNTIME {
        if matches!(entry.mode, Mode::Library) {
            let classes =
                parse_library_with_rbs(entry.rb_src.as_bytes(), entry.rbs_src, entry.rb_path)?;
            f(&classes);
        }
    }
    Ok(())
}

/// Hand every Elixir runtime entry's module-level constant NAMES (e.g.
/// `HTML_ESCAPES`, `ESCAPES`) to `f`. A pre-emit pass registers these so
/// `emit_const` only rewrites a SCREAMING_SNAKE reference to a module
/// attribute (`@html_escapes`) when it's a DECLARED constant — an
/// all-caps *module* reference like `JSON`/`IO` stays a module name.
pub fn elixir_constant_names<F>(mut f: F)
where
    F: FnMut(&str),
{
    for entry in ELIXIR_RUNTIME {
        for (name, _value) in parse_module_constant_exprs(entry.rb_src).unwrap_or_default() {
            f(name.as_str());
        }
    }
}

// -------- Python target (strangler scaffold) --------
//
// Modeled on `TS_TARGET` / `TYPESCRIPT_RUNTIME`, not Elixir: Python is
// mutable + imperative (no functionalize), and its import/constant
// syntax are structural twins of TS's (`from m import X`,
// `NAME = value`). The framework files land under `app/*.py`, replacing
// the hand-written `runtime/python/*.py` duplicates one entry at a time.
//
// This path is DORMANT: nothing in `emit::python::emit` consumes
// `python_units` yet, so the shipping Python target stays green. A test
// drives it to keep the inventory signal live; entries graduate from
// hand-written to transpiled as the body walker is confirmed per file.

const PYTHON_TARGET: TargetEmit = TargetEmit {
    emit_module: crate::emit::python::emit_module,
    emit_library_class: crate::emit::python::emit_library_class,
    format_import: py_format_import,
    format_constant: py_format_constant,
    // Python models module-singleton state (ViewHelpers' `@slots`) in a
    // hand-written holder, same opt-out as TS/Crystal/Rust.
    format_module_ivar: None,
    wrap_namespace: py_wrap_namespace,
    module_prelude: "from __future__ import annotations\n\n",
};

/// Python `from <module> import <name>`. The `source` is the dotted
/// module path (`app.flash`), matching the `app/*.py` out-paths.
fn py_format_import(name: &str, source: &str) -> String {
    // An empty source means a plain top-level `import <name>` (stdlib
    // modules like `datetime`, which the transpiled `Time.now` mapping
    // reaches into) rather than the cross-file `from app.x import Y` form.
    if source.is_empty() {
        format!("import {name}\n")
    } else {
        format!("from {source} import {name}\n")
    }
}

/// Module-level constant → `NAME = value` (no keyword, no terminator),
/// the direct analog of TS's `const NAME = value;`.
fn py_format_constant(name: &str, value: &Expr) -> String {
    format!("{name} = {}", crate::emit::python::emit_expr_for_runtime(value))
}

/// Python resolves cross-file refs through `from app.x import Y` (like
/// TS through imports); bodies emit flat at module top, so namespace
/// wrapping is a no-op.
fn py_wrap_namespace(_namespace: &str, body: &str) -> String {
    body.to_string()
}

/// Python runtime transpile table. Mirrors `TYPESCRIPT_RUNTIME` 1:1 by
/// source file; `out_path`s land under `app/` and imports use dotted
/// `app.<name>` module paths. `extra_roots` is empty for every entry —
/// Python has no tree-shake pass yet, so the reachability roots TS needs
/// don't apply.
const PYTHON_RUNTIME: &[RuntimeEntry] = &[
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/inflector.rb"),
        rbs_src: include_str!("../runtime/ruby/inflector.rbs"),
        rb_path: "runtime/ruby/inflector.rb",
        namespace: "",
        out_path: "app/inflector.py",
        mode: Mode::Module,
        imports: NO_IMPORTS,
        prelude: NO_PRELUDE,
        extra_roots: NO_EXTRA_ROOTS,
    },
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/json_builder.rb"),
        rbs_src: include_str!("../runtime/ruby/json_builder.rbs"),
        rb_path: "runtime/ruby/json_builder.rb",
        namespace: "",
        out_path: "app/json_builder.py",
        mode: Mode::Module,
        imports: NO_IMPORTS,
        // Module-level `ESCAPE_PATTERN = re.compile(...)` runs at import
        // time, so `re` must be in scope. (The `gsub` -> `re.sub` body
        // rewrite is a separate follow-up; it only fires when the
        // encoder is actually called.)
        prelude: "import re\n\n",
        extra_roots: NO_EXTRA_ROOTS,
    },
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/active_record/errors.rb"),
        rbs_src: include_str!("../runtime/ruby/active_record/errors.rbs"),
        rb_path: "runtime/ruby/active_record/errors.rb",
        namespace: "ActiveRecord",
        out_path: "app/errors.py",
        mode: Mode::Library,
        imports: NO_IMPORTS,
        prelude: NO_PRELUDE,
        extra_roots: NO_EXTRA_ROOTS,
    },
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/active_record/base.rb"),
        rbs_src: include_str!("../runtime/ruby/active_record/base.rbs"),
        rb_path: "runtime/ruby/active_record/base.rb",
        namespace: "ActiveRecord",
        out_path: "app/active_record_base.py",
        mode: Mode::Library,
        imports: &[
            ("RecordNotFound, RecordInvalid", "app.errors"),
            ("datetime", ""),
        ],
        prelude: NO_PRELUDE,
        extra_roots: NO_EXTRA_ROOTS,
    },
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/action_dispatch/flash.rb"),
        rbs_src: include_str!("../runtime/ruby/action_dispatch/flash.rbs"),
        rb_path: "runtime/ruby/action_dispatch/flash.rb",
        namespace: "ActionDispatch",
        out_path: "app/flash.py",
        mode: Mode::Library,
        imports: NO_IMPORTS,
        prelude: NO_PRELUDE,
        // Hand-written http.py/server.py construct `Flash(cookie_map)` for
        // context.flash and read `to_persisted`-shaped maps — invisible to
        // the app-side reachability walk, so seed it. (Inert today — python
        // doesn't treeshake — but documents the dep.)
        extra_roots: &[("Flash", "to_persisted")],
    },
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/action_dispatch/session.rb"),
        rbs_src: include_str!("../runtime/ruby/action_dispatch/session.rbs"),
        rb_path: "runtime/ruby/action_dispatch/session.rb",
        namespace: "ActionDispatch",
        out_path: "app/session.py",
        mode: Mode::Library,
        imports: NO_IMPORTS,
        prelude: NO_PRELUDE,
        extra_roots: NO_EXTRA_ROOTS,
    },
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/action_controller/base.rb"),
        rbs_src: include_str!("../runtime/ruby/action_controller/base.rbs"),
        rb_path: "runtime/ruby/action_controller/base.rb",
        namespace: "ActionController",
        out_path: "app/action_controller_base.py",
        mode: Mode::Library,
        imports: &[("Flash", "app.flash"), ("Session", "app.session")],
        prelude: NO_PRELUDE,
        extra_roots: NO_EXTRA_ROOTS,
    },
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/action_dispatch/router.rb"),
        rbs_src: include_str!("../runtime/ruby/action_dispatch/router.rbs"),
        rb_path: "runtime/ruby/action_dispatch/router.rb",
        namespace: "",
        out_path: "app/router.py",
        mode: Mode::Library,
        imports: NO_IMPORTS,
        prelude: NO_PRELUDE,
        extra_roots: NO_EXTRA_ROOTS,
    },
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/action_view/view_helpers.rb"),
        rbs_src: include_str!("../runtime/ruby/action_view/view_helpers.rbs"),
        rb_path: "runtime/ruby/action_view/view_helpers.rb",
        namespace: "",
        out_path: "app/view_helpers.py",
        mode: Mode::Library,
        imports: &[("Base", "app.active_record_base")],
        prelude: NO_PRELUDE,
        extra_roots: NO_EXTRA_ROOTS,
    },
];

/// Parse + emit the Python runtime files. Strangler scaffold — the
/// `transform` hook is the tree-shake/registration seam (identity for
/// now). Mirrors `elixir_units` / `typescript_units`.
pub fn python_units<F>(mut transform: F) -> Result<Vec<RuntimeUnit>, String>
where
    F: FnMut(&str, Vec<LibraryClass>) -> Vec<LibraryClass>,
{
    let mut out = Vec::with_capacity(PYTHON_RUNTIME.len());
    for entry in PYTHON_RUNTIME {
        out.push(transpile_entry(entry, &PYTHON_TARGET, "#", &mut transform)?);
    }
    Ok(out)
}

/// Parse + emit only the Python runtime entries whose `out_path` is in
/// `keep`. Used by the strangler switchover: emitting a degrade-heavy
/// dormant entry (e.g. `active_record_base`) would fire its
/// `report_unsupported` diagnostics into the shared sink and trip the
/// transpile fail-policy, even if its output were later dropped. Filter
/// at the source so only switched-over leaves are ever emitted.
pub fn python_units_subset<F>(keep: &[&str], mut transform: F) -> Result<Vec<RuntimeUnit>, String>
where
    F: FnMut(&str, Vec<LibraryClass>) -> Vec<LibraryClass>,
{
    let mut out = Vec::new();
    for entry in PYTHON_RUNTIME {
        if keep.contains(&entry.out_path) {
            out.push(transpile_entry(entry, &PYTHON_TARGET, "#", &mut transform)?);
        }
    }
    Ok(out)
}
