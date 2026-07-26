//! Every `Db` shim implements the same contract.
//!
//! The primitive `Db` surface is hand-written once per target (the
//! lower half of the two-layer runtime), and the lowerer emits calls
//! against it blind — nothing at emit time proves the method it calls
//! actually exists in the shim it will run on. A missing method is a
//! per-target build or runtime failure, discovered only when that
//! lane's toolchain job runs.
//!
//! That is how the nullable-column seam (`*_opt`) shipped half-done:
//! TypeScript has FOUR shims and only one of them got the new methods,
//! which surfaced as `Property 'column_text_opt' does not exist` in CI
//! two cycles later. This test enumerates the implementations in ONE
//! place so adding a method to the contract means adding it everywhere
//! or explaining why not.
//!
//! Naming: each target renames on its own convention (Go prefixes
//! `Db_`, Swift/Kotlin camelCase, C# PascalCase) and Ruby's `?` suffix
//! is unspellable outside the Ruby family, so predicates carry a
//! per-target spelling. Both live in the tables below — the contract
//! itself stays in canonical snake_case.

/// How a target spells a contract method.
#[derive(Clone, Copy, Debug)]
enum Naming {
    /// `column_int_opt` — Ruby family, TypeScript (its export map keys
    /// keep the Ruby names), Crystal, Rust, Python, Elixir.
    Snake,
    /// `Db_column_int_opt` — Go has no namespace for these, so the
    /// package-level functions carry the module as a prefix.
    GoPrefixed,
    /// `columnIntOpt` — Swift, Kotlin.
    Camel,
    /// `ColumnIntOpt` — C#.
    Pascal,
}

/// A shim, its naming convention, and how it spells `step?`. Ruby's
/// `?` suffix has no cross-language spelling, so each target picked
/// one (`step_pred` / `is_step` / `step_p` / `StepPred`); the
/// predicate is listed explicitly rather than derived.
struct Shim {
    path: &'static str,
    naming: Naming,
    step_pred: &'static str,
    /// Literal prefixes that introduce a definition in this target's
    /// syntax. A bare name search is not enough: every shim MENTIONS
    /// its method names in comments and in error strings
    /// (`throw new Error(\"Db: column_text_opt called ...\")`), so a
    /// shim that merely talks about a method would pass. Requiring a
    /// definition prefix is what makes the check mean something —
    /// verified by deleting a method and watching this fail.
    def_forms: &'static [&'static str],
    /// TypeScript's `db.ts` names its functions camelCase and exposes
    /// the contract through an exported object whose KEYS are the
    /// contract names (`column_int_opt: columnIntOpt,`). Those entries
    /// are definitions for our purposes, so accept a bare
    /// `name,` / `name:` on an otherwise-blank line prefix.
    map_entry: bool,
    /// C# declares the return type between the keywords and the name
    /// (`public static long? ColumnIntOpt(...)`), so the definition
    /// can't be recognised by the text immediately before the name.
    /// For those, the LINE must start with one of `def_forms` and the
    /// name must be the identifier introducing the parameter list.
    type_before_name: bool,
}

const SHIMS: &[Shim] = &[
    // Ruby family: the cruby gem shim, the JDBC shim, and the spinel
    // FFI shim. `project.rs::ruby_overlay` renames db_cruby.rb over
    // db.rb for the cruby target.
    Shim {
        path: "runtime/spinel/db_cruby.rb",
        naming: Naming::Snake,
        step_pred: "step?",
        def_forms: &["def self."],
        map_entry: false,
        type_before_name: false,
    },
    Shim {
        path: "runtime/spinel/db_jruby.rb",
        naming: Naming::Snake,
        step_pred: "step?",
        def_forms: &["def self."],
        map_entry: false,
        type_before_name: false,
    },
    Shim {
        path: "runtime/spinel/db.rb",
        naming: Naming::Snake,
        step_pred: "step?",
        def_forms: &["def self."],
        map_entry: false,
        type_before_name: false,
    },
    // TypeScript ships FOUR backends behind one export shape. Missing
    // three of them is the exact mistake this test exists to catch.
    // Two def forms: the plain `function name(...)` the libsql and
    // worker shims use, and the `contract_name: camelCaseImpl` entry
    // in db.ts's exported `Db` object (its functions are camelCase;
    // the export KEY is the contract name).
    Shim {
        path: "runtime/typescript/db.ts",
        naming: Naming::Snake,
        step_pred: "is_step",
        def_forms: &["function "],
        map_entry: true,
        type_before_name: false,
    },
    Shim {
        path: "runtime/typescript/db-libsql.ts",
        naming: Naming::Snake,
        step_pred: "is_step",
        def_forms: &["function "],
        map_entry: true,
        type_before_name: false,
    },
    Shim {
        path: "runtime/typescript/db-worker-proxy.ts",
        naming: Naming::Snake,
        step_pred: "is_step",
        def_forms: &["function "],
        map_entry: true,
        type_before_name: false,
    },
    Shim {
        path: "runtime/crystal/db.cr",
        naming: Naming::Snake,
        step_pred: "step?",
        def_forms: &["def self."],
        map_entry: false,
        type_before_name: false,
    },
    Shim {
        path: "runtime/rust/db.rs",
        naming: Naming::Snake,
        step_pred: "step_pred",
        def_forms: &["fn "],
        map_entry: false,
        type_before_name: false,
    },
    Shim {
        path: "runtime/go/v2/db.go",
        naming: Naming::GoPrefixed,
        step_pred: "Db_step_pred",
        def_forms: &["func "],
        map_entry: false,
        type_before_name: false,
    },
    Shim {
        path: "runtime/python/db.py",
        naming: Naming::Snake,
        step_pred: "step_p",
        def_forms: &["def "],
        map_entry: false,
        type_before_name: false,
    },
    Shim {
        path: "runtime/elixir/v2/db.ex",
        naming: Naming::Snake,
        step_pred: "step?",
        def_forms: &["def "],
        map_entry: false,
        type_before_name: false,
    },
    Shim {
        path: "runtime/swift/db.swift",
        naming: Naming::Camel,
        step_pred: "stepPred",
        def_forms: &["func "],
        map_entry: false,
        type_before_name: false,
    },
    Shim {
        path: "runtime/kotlin/db.kt",
        naming: Naming::Camel,
        step_pred: "stepPred",
        def_forms: &["fun "],
        map_entry: false,
        type_before_name: false,
    },
    // C# declares the return type before the name, so the definition
    // prefix is the visibility keyword chain rather than a keyword.
    Shim {
        path: "runtime/csharp/Db.cs",
        naming: Naming::Pascal,
        step_pred: "StepPred",
        def_forms: &["public static "],
        map_entry: false,
        type_before_name: true,
    },
];

/// Methods EVERY shim must implement. Deliberately the universal core
/// — statement lifecycle, the two scalar reads every hydration path
/// uses, and the two escapes every INSERT/UPDATE composes. Methods
/// only some shims carry (`configure`, `close`, `changes`,
/// `column_bool`, `column_float`, `escape_bool`, `escape_int_list`)
/// are NOT pinned here: they're real gaps, but pre-existing ones, and
/// pinning them would make this test a to-do list rather than a
/// regression guard. Add one here when it lands everywhere.
const CORE: &[&str] = &[
    "exec",
    "prepare",
    "finalize",
    "last_insert_rowid",
    "column_int",
    "column_text",
    "escape_string",
    "escape_int",
];

/// The nullable-column seam. A column the schema declares nullable
/// holds NULL until something sets it, and NULL is not the type's
/// zero: `column_text` collapsing it to "" makes a nullable UNIQUE
/// column collide row-to-row, and `column_int`'s 0 makes
/// `where(fk: nil)` match nothing. The lowerer emits these for exactly
/// those columns (`ty_of_column_slot` / `column_read_method_for`), so
/// a shim missing one breaks that target the moment an app has a
/// nullable column — which is every app.
const NULLABLE_SEAM: &[&str] = &[
    "column_int_opt",
    "column_float_opt",
    "column_text_opt",
    "column_bool_opt",
    "escape_string_opt",
    "escape_int_opt",
    "escape_float_opt",
    "escape_bool_opt",
];

fn spelling(method: &str, naming: Naming) -> String {
    match naming {
        Naming::Snake => method.to_string(),
        Naming::GoPrefixed => format!("Db_{method}"),
        Naming::Camel => {
            let mut parts = method.split('_');
            let first = parts.next().unwrap_or_default().to_string();
            first + &parts.map(capitalize).collect::<String>()
        }
        Naming::Pascal => method.split('_').map(capitalize).collect(),
    }
}

fn capitalize(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// Does `src` DEFINE `name`? Two conditions, both load-bearing:
///
/// - the occurrence sits at a word boundary, so `column_int` is not
///   satisfied by `column_int_opt` nor `escape_int` by
///   `escape_int_list`;
/// - and it is introduced by one of the target's definition forms, so
///   a mention in a comment or an error string doesn't count. (The
///   first draft of this test omitted that and passed a shim whose
///   method had been deleted — the name survived inside its own
///   `throw new Error("Db: column_text_opt called ...")`.)
fn defines_with(
    src: &str,
    name: &str,
    def_forms: &[&str],
    map_entry: bool,
    type_before_name: bool,
) -> bool {
    let bytes = src.as_bytes();
    let ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'?';
    src.match_indices(name).any(|(i, _)| {
        let after = i + name.len();
        let word_bounded = (i == 0 || !ident(bytes[i - 1]))
            && (after >= bytes.len() || !ident(bytes[after]));
        if !word_bounded {
            return false;
        }
        let line_start = src[..i].rfind('\n').map(|n| n + 1).unwrap_or(0);
        let prefix = &src[line_start..i];
        if type_before_name {
            // `public static long? ColumnIntOpt(` — the line opens with
            // the form, and the name is what precedes the parameter
            // list. Both halves matter: without the `(` check, the
            // return type of a DIFFERENT method would qualify.
            return def_forms.iter().any(|f| prefix.trim_start().starts_with(f))
                && matches!(bytes.get(after), Some(b'('));
        }
        if def_forms.iter().any(|f| prefix.ends_with(f)) {
            return true;
        }
        // Export-map entry: `  column_int_opt,` / `  column_int_opt:`.
        // The blank prefix is what keeps a comment (`// column_int_opt
        // reads …`) or a string from qualifying.
        map_entry
            && prefix.trim().is_empty()
            && matches!(bytes.get(after), Some(b',') | Some(b':'))
    })
}

/// Contract-declaration search for the RBS, which spells everything
/// `def self.name:`.
fn defines_in_rbs(src: &str, name: &str) -> bool {
    defines_with(src, name, &["def self."], false, false)
}

fn read_shim(path: &str) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {path}: {e}"))
}

#[test]
fn every_db_shim_implements_the_core_contract() {
    let mut missing: Vec<String> = Vec::new();
    for shim in SHIMS {
        let src = read_shim(shim.path);
        for method in CORE {
            let name = spelling(method, shim.naming);
            if !defines_with(&src, &name, shim.def_forms, shim.map_entry, shim.type_before_name) {
                missing.push(format!("{} is missing `{name}`", shim.path));
            }
        }
        if !defines_with(&src, shim.step_pred, shim.def_forms, shim.map_entry, shim.type_before_name) {
            missing.push(format!(
                "{} is missing `{}` (the `step?` predicate)",
                shim.path, shim.step_pred
            ));
        }
    }
    assert!(
        missing.is_empty(),
        "Db shims out of contract:\n  {}\n\nThe lowerer emits these calls against every \
         target's shim. Add the method to the shim, or — if the target genuinely can't \
         provide it — drop it from CORE with a comment saying why.",
        missing.join("\n  ")
    );
}

#[test]
fn every_db_shim_implements_the_nullable_seam() {
    let mut missing: Vec<String> = Vec::new();
    for shim in SHIMS {
        let src = read_shim(shim.path);
        for method in NULLABLE_SEAM {
            let name = spelling(method, shim.naming);
            if !defines_with(&src, &name, shim.def_forms, shim.map_entry, shim.type_before_name) {
                missing.push(format!("{} is missing `{name}`", shim.path));
            }
        }
    }
    assert!(
        missing.is_empty(),
        "Db shims missing nullable-column methods:\n  {}\n\nEvery app has nullable \
         columns, so a shim without these breaks its target as soon as one is read or \
         written. See `ty_of_column_slot`.",
        missing.join("\n  ")
    );
}

/// The ruby-family RBS is the only written form of this contract, and
/// the strict targets consume it when they transpile framework Ruby —
/// a method implemented in the shims but absent from the RBS types as
/// untyped at every call site.
#[test]
fn the_rbs_contract_declares_the_nullable_seam() {
    let rbs = read_shim("runtime/ruby/db.rbs");
    let missing: Vec<&str> = NULLABLE_SEAM
        .iter()
        .copied()
        .filter(|m| !defines_in_rbs(&rbs, m))
        .collect();
    assert!(
        missing.is_empty(),
        "runtime/ruby/db.rbs does not declare: {}\n\nThe shims implement these; the \
         declared contract has to keep up or the calls type as untyped.",
        missing.join(", ")
    );
}
