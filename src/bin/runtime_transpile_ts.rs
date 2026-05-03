//! Phase 1 runtime-runtime transpile: read every `runtime/ruby/*` file
//! that the TS pipeline can handle today, emit it to
//! `runtime/typescript/<name>.ts`, and print a status line per file.
//!
//! Bin form (option (a) per the Phase 1 brief): runs once per build,
//! writes to checked-in artifacts, diff-able in PRs. Future
//! refactoring may inline the transpile into `typescript::emit_with_adapter`,
//! but for Phase 1 we want the artifacts visible.
//!
//! Coverage today: only modules whose methods are all `def self.*` —
//! emitted as standalone exported TS functions. Modules with instance
//! methods (validations, base, view_helpers, …) error and print the
//! gap reason; those entries close one by one as `emit_module`
//! gains class-form support.

use roundhouse::emit::typescript::{emit_library_class, emit_module};
use roundhouse::runtime_src::{parse_library_with_rbs, parse_methods_with_rbs};
use std::path::PathBuf;

/// Strategy: each entry picks one of two pipelines.
///
/// `Module` — flat list of `def self.*` helpers (e.g. inflector.rb).
/// Goes through `parse_methods_with_rbs` + `emit_module`. Each method
/// becomes a standalone `export function`.
///
/// `Library` — file with one or more class/module definitions
/// preserving parent/includes (e.g. errors.rb). Goes through
/// `parse_library_with_rbs` + `emit_library_class`. Output is a
/// concatenation of the per-class TS class declarations.
enum Mode {
    Module,
    Library,
}

/// Cross-runtime imports each transpiled file needs. The class-body
/// emitter doesn't have global knowledge of sibling runtime files —
/// it just renders a class — so the bin attaches imports here. Each
/// entry: (named_import, source_module). The runtime files all live
/// in `runtime/typescript/` (mirrored to `src/` in the emitted app),
/// so module paths use `./<name>.js` exclusively.
type ImportSpec = &'static [(&'static str, &'static str)];
const NO_IMPORTS: ImportSpec = &[];

/// Hand-written TS prepended after the imports and before the
/// transpiled class bodies. Pragmatic shortcut for module-scope
/// constants (e.g. `STATUS_CODES = {...}.freeze` in
/// `action_controller/base.rb`) that the lowerer doesn't yet
/// recognize — `walk_decl_body` only handles `def`, `include`,
/// `attr_*`, `class << self`, and `module_function`. Refactor to
/// a real lowerer pass when constants surface in more files.
const NO_PRELUDE: &str = "";

const PAIRS: &[(&str, &str, &str, Mode, ImportSpec, &str)] = &[
    // (rb_path, rbs_path, ts_out_path, mode, imports, prelude)
    (
        "runtime/ruby/inflector.rb",
        "runtime/ruby/inflector.rbs",
        "runtime/typescript/inflector.ts",
        Mode::Module,
        NO_IMPORTS,
        NO_PRELUDE,
    ),
    (
        "runtime/ruby/active_record/errors.rb",
        "runtime/ruby/active_record/errors.rbs",
        "runtime/typescript/errors.ts",
        Mode::Library,
        // RecordInvalid's constructor takes a Base instance —
        // forward-declare via type-only import so errors.ts compiles
        // without a circular module load (Base imports
        // RecordNotFound from this same file).
        &[("type Base", "./active_record_base.js")],
        NO_PRELUDE,
    ),
    (
        "runtime/ruby/active_record/validations.rb",
        "runtime/ruby/active_record/validations.rbs",
        "runtime/typescript/validations.ts",
        Mode::Library,
        NO_IMPORTS,
        NO_PRELUDE,
    ),
    (
        "runtime/ruby/active_record/base.rb",
        "runtime/ruby/active_record/base.rbs",
        "runtime/typescript/active_record_base.ts",
        Mode::Library,
        // Base extends Validations (via include lowering) and
        // throws RecordNotFound from `find` / RecordInvalid from
        // `save!` / `create!`.
        &[
            ("Validations", "./validations.js"),
            ("RecordNotFound, RecordInvalid", "./errors.js"),
        ],
        NO_PRELUDE,
    ),
    (
        "runtime/ruby/action_view/view_helpers.rb",
        "runtime/ruby/action_view/view_helpers.rbs",
        "runtime/typescript/view_helpers_generated.ts",
        Mode::Library,
        NO_IMPORTS,
        NO_PRELUDE,
    ),
    (
        "runtime/ruby/action_view/route_helpers.rb",
        "runtime/ruby/action_view/route_helpers.rbs",
        "runtime/typescript/route_helpers.ts",
        Mode::Library,
        NO_IMPORTS,
        NO_PRELUDE,
    ),
    (
        "runtime/ruby/action_controller/base.rb",
        "runtime/ruby/action_controller/base.rbs",
        "runtime/typescript/action_controller_base.ts",
        Mode::Library,
        // Body uses ActionController.Parameters.new({}) — the
        // Parameters class lives in parameters.ts.
        &[("Parameters", "./parameters.js")],
        // Module-scope `STATUS_CODES = {...}.freeze` in base.rb
        // isn't recognized by the lowerer yet; hand-mirror it
        // here. Sync risk if base.rb's table changes — keep the
        // values in sync until the lowerer learns module
        // constants.
        "const STATUS_CODES: Record<string, number> = {\n\
        \x20 ok: 200, created: 201, accepted: 202, no_content: 204,\n\
        \x20 moved_permanently: 301, found: 302, see_other: 303, not_modified: 304,\n\
        \x20 bad_request: 400, unauthorized: 401, forbidden: 403, not_found: 404,\n\
        \x20 unprocessable_entity: 422, internal_server_error: 500,\n\
        };\n\n",
    ),
    (
        "runtime/ruby/action_controller/parameters.rb",
        "runtime/ruby/action_controller/parameters.rbs",
        "runtime/typescript/parameters.ts",
        Mode::Library,
        NO_IMPORTS,
        NO_PRELUDE,
    ),
    (
        "runtime/ruby/action_dispatch/router.rb",
        "runtime/ruby/action_dispatch/router.rbs",
        "runtime/typescript/router.ts",
        Mode::Library,
        NO_IMPORTS,
        NO_PRELUDE,
    ),
];

fn main() {
    let mut had_errors = false;
    for (rb_path, rbs_path, ts_out, mode, imports, prelude) in PAIRS {
        match transpile_one(rb_path, rbs_path, ts_out, mode, imports, prelude) {
            Ok(n) => {
                println!("OK   {rb_path} → {ts_out} ({n} unit(s))");
            }
            Err(e) => {
                eprintln!("ERR  {rb_path}: {e}");
                had_errors = true;
            }
        }
    }
    if had_errors {
        std::process::exit(1);
    }
}

fn transpile_one(
    rb_path: &str,
    rbs_path: &str,
    ts_out: &str,
    mode: &Mode,
    imports: &[(&str, &str)],
    prelude: &str,
) -> Result<usize, String> {
    let ruby_bytes = std::fs::read(rb_path).map_err(|e| format!("read {rb_path}: {e}"))?;
    let rbs = std::fs::read_to_string(rbs_path)
        .map_err(|e| format!("read {rbs_path}: {e}"))?;

    let (emitted, units) = match mode {
        Mode::Module => {
            let ruby = std::str::from_utf8(&ruby_bytes)
                .map_err(|e| format!("{rb_path} is not UTF-8: {e}"))?;
            let methods = parse_methods_with_rbs(ruby, &rbs)?;
            let n = methods.len();
            (emit_module(&methods)?, n)
        }
        Mode::Library => {
            let classes = parse_library_with_rbs(&ruby_bytes, &rbs, rb_path)?;
            let mut emitted = String::new();
            for (i, c) in classes.iter().enumerate() {
                if i > 0 {
                    emitted.push('\n');
                }
                emitted.push_str(&emit_library_class(c)?);
            }
            (emitted, classes.len())
        }
    };

    let mut import_block = String::new();
    for (name, source) in imports {
        import_block.push_str(&format!("import {{ {name} }} from \"{source}\";\n"));
    }
    if !import_block.is_empty() {
        import_block.push('\n');
    }

    let header = format!(
        "// Generated by `cargo run --bin runtime_transpile_ts` from {}.\n\
         // Do not edit by hand — edit the source `.rb` and re-run.\n\
         \n",
        rb_path,
    );

    let path = PathBuf::from(ts_out);
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p).map_err(|e| format!("mkdir {}: {e}", p.display()))?;
    }
    std::fs::write(&path, format!("{header}{import_block}{prelude}{emitted}"))
        .map_err(|e| format!("write {ts_out}: {e}"))?;

    Ok(units)
}
