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
    pub emit_expr_for_runtime: fn(&Expr) -> String,
    pub format_import: fn(name: &str, source: &str) -> String,
    /// Format a top-level constant declaration. TS: `const NAME = VALUE;`.
    /// Crystal: `NAME = VALUE` (no `const` keyword, no terminator).
    pub format_constant: fn(name: &str, value_expr: &str) -> String,
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
    emit_expr_for_runtime: crate::emit::typescript::emit_expr_for_runtime,
    format_import: ts_format_import,
    format_constant: ts_format_constant,
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

fn ts_format_constant(name: &str, value: &str) -> String {
    format!("const {name} = {value};")
}

const CRYSTAL_TARGET: TargetEmit = TargetEmit {
    emit_module: crate::emit::crystal::emit_module,
    emit_library_class: crate::emit::crystal::emit_library_class,
    emit_expr_for_runtime: crate::emit::crystal::emit_expr_for_runtime,
    format_import: crystal_format_import,
    format_constant: crystal_format_constant,
    wrap_namespace: crystal_wrap_namespace,
};

/// Wrap `body` in `module Foo ... end` (or nested forms for compound
/// names like `A::B`). Empty namespace returns the body unchanged.
fn crystal_wrap_namespace(namespace: &str, body: &str) -> String {
    if namespace.is_empty() {
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
fn crystal_format_constant(name: &str, value: &str) -> String {
    format!("{name} = {value}")
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
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/active_support/hash_with_indifferent_access.rb"),
        rbs_src: include_str!("../runtime/ruby/active_support/hash_with_indifferent_access.rbs"),
        rb_path: "runtime/ruby/active_support/hash_with_indifferent_access.rb",
        namespace: "ActiveSupport",
        out_path: "src/hash_with_indifferent_access.ts",
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
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/active_record/validations.rb"),
        rbs_src: include_str!("../runtime/ruby/active_record/validations.rbs"),
        rb_path: "runtime/ruby/active_record/validations.rb",
        namespace: "ActiveRecord",
        out_path: "src/validations.ts",
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
        out_path: "src/active_record_base.ts",
        mode: Mode::Library,
        imports: &[
            ("Validations", "./validations.js"),
            ("RecordNotFound, RecordInvalid", "./errors.js"),
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
        imports: &[("Parameters", "./parameters.js")],
        // Module-scope `STATUS_CODES` is now picked up by
        // `parse_module_constant_exprs` and emitted as a
        // top-level `const STATUS_CODES = ...;` automatically;
        // no hand-written prelude needed.
        prelude: NO_PRELUDE,
        extra_roots: NO_EXTRA_ROOTS,
    },
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/action_controller/parameters.rb"),
        rbs_src: include_str!("../runtime/ruby/action_controller/parameters.rbs"),
        rb_path: "runtime/ruby/action_controller/parameters.rb",
        namespace: "ActionController",
        out_path: "src/parameters.ts",
        mode: Mode::Library,
        imports: &[("HashWithIndifferentAccess", "./hash_with_indifferent_access.js")],
        prelude: NO_PRELUDE,
        extra_roots: NO_EXTRA_ROOTS,
    },
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/action_dispatch/router.rb"),
        rbs_src: include_str!("../runtime/ruby/action_dispatch/router.rbs"),
        rb_path: "runtime/ruby/action_dispatch/router.rb",
        namespace: "ActionDispatch",
        out_path: "src/router.ts",
        mode: Mode::Library,
        imports: &[("HashWithIndifferentAccess", "./hash_with_indifferent_access.js")],
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
        namespace: "ActionView",
        out_path: "src/view_helpers.ts",
        mode: Mode::Library,
        imports: NO_IMPORTS,
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
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/active_support/hash_with_indifferent_access.rb"),
        rbs_src: include_str!("../runtime/ruby/active_support/hash_with_indifferent_access.rbs"),
        rb_path: "runtime/ruby/active_support/hash_with_indifferent_access.rb",
        namespace: "ActiveSupport",
        out_path: "src/hash_with_indifferent_access.cr",
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
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/active_record/validations.rb"),
        rbs_src: include_str!("../runtime/ruby/active_record/validations.rbs"),
        rb_path: "runtime/ruby/active_record/validations.rb",
        namespace: "ActiveRecord",
        out_path: "src/validations.cr",
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
        out_path: "src/active_record_base.cr",
        mode: Mode::Library,
        imports: NO_IMPORTS,
        prelude: NO_PRELUDE,
        extra_roots: NO_EXTRA_ROOTS,
    },
    RuntimeEntry {
        rb_src: include_str!("../runtime/ruby/action_controller/parameters.rb"),
        rbs_src: include_str!("../runtime/ruby/action_controller/parameters.rbs"),
        rb_path: "runtime/ruby/action_controller/parameters.rb",
        namespace: "ActionController",
        out_path: "src/parameters.cr",
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
        namespace: "ActionDispatch",
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
        namespace: "ActionView",
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
            let body = (target.emit_module)(&methods)?;
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
                    (target.format_constant)(name.as_str(), &(target.emit_expr_for_runtime)(value)),
                ));
            }
            if !constants.is_empty() {
                body.push('\n');
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
