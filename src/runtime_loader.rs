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
}

const TS_TARGET: TargetEmit = TargetEmit {
    emit_module: crate::emit::typescript::emit_module,
    emit_library_class: crate::emit::typescript::emit_library_class,
    format_import: ts_format_import,
    format_constant: ts_format_constant,
    format_module_ivar: None,
    wrap_namespace: ts_wrap_namespace,
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
        // Hand-written server.ts constructs `new Flash(flashStore)`
        // and reads `flash.to_h()` between requests — neither is
        // visible to the app-side reachability walk.
        extra_roots: &[("Flash", "to_h")],
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
        extra_roots: NO_EXTRA_ROOTS,
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
        prelude: NO_PRELUDE,
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

    let content = format!("{header}{import_block}{}{}", entry.prelude, emitted);

    Ok(RuntimeUnit {
        out_path: PathBuf::from(entry.out_path),
        content,
        classes,
        functions,
        namespace: entry.namespace,
        extra_roots: entry.extra_roots,
    })
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
        extra_roots: NO_EXTRA_ROOTS,
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
