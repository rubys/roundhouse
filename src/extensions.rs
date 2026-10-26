//! Project extensions: facts about an app's gems and class-body DSLs that
//! the app keeps in its own tree, so the compiler does not have to carry
//! them.
//!
//! `roundhouse.yml` lists extension directories:
//!
//! ```yaml
//! extensions:
//!   - config/roundhouse
//! ```
//!
//! Each entry names a literal directory relative to the app root, under the
//! rules `test_paths` follows: no globs, no `..`, no absolute path, and no
//! symbolic link in the directory path. The directory holds `gems.yml` and
//! `macros.yml`. Both are optional and may be symbolic links, including links
//! to files outside the app root. Unreadable links are errors.
//!
//! ```yaml
//! # gems.yml
//! #
//! # (require path, lockfile gem). The Ruby and JRuby trees pin registry
//! # versions or preserve Git URLs and revisions, then require the path
//! # at boot after the compiler's own list. PATH gems remain source inputs.
//! lock_gated_requires:
//!   - {require: "widgets/all", gem: "widgets"}
//! # Gems whose load closure reaches Rails, with the evidence. Compiled
//! # projects do not require them natively. Their constants stay refused
//! # by owning gem, including constants whose RBIs supply known types.
//! rails_dependent:
//!   - {gem: "widget-engine", why: "requiring it loads Rails framework files"}
//! # (what an app/ body writes, gem that provides it). The emitted Gemfile
//! # declares the gem when an app/ file names the constant or contains any
//! # of the texts.
//! runtime_gems:
//!   - {constant: "Widget", gem: "widgets"}
//!   - {any_text: ["Widget::Cache."], gem: "widget-cache"}
//! # Gems whose installed lib/ is compiled with the app, as a PATH gem's is.
//! source_gems:
//!   - {gem: "widgets"}
//! ```
//!
//! These facts also select locked gems' RBIs, including infrastructure and
//! stdlib gems. Boot-load tracking belongs to the app's gem boundary; an
//! RBI or a dependency entry alone does not establish a runtime load.
//! Git-backed native gems and their Git dependencies activate from locked
//! Bundler checkouts. Missing checkouts fail instead of selecting a
//! published replacement. Guarded requires report internal load failures;
//! only an absent optional feature remains quiet.
//!
//! ```yaml
//! # macros.yml
//! #
//! # Class-body DSL calls that add attributes to a model. Each name the
//! # macro's symbol arguments give reads and writes untyped: the DSL's value
//! # object is not modeled. No target lowers the call itself, so analysis
//! # reports each one as unsupported at its declaration, in place of a
//! # dispatch failure at every use of the value object.
//! attribute_macros:
//!   - {name: price_column, positional: true}     # price_column :a, :b, opt: 1
//!   - {name: bag_columns, hash_key: attributes}  # bag_columns attributes: [:a, :b]
//! ```
//!
//! Rows append after the compiler's own, in listing order and then file
//! order. An extension never removes a compiler row. Unknown keys, a listed
//! directory that does not exist, a malformed row, and a gem fact that
//! contradicts another are refused by name rather than ignored. So are the
//! entries a later format revision gives meaning to (`types`, `overlay`):
//! a typo and an unimplemented feature both fail loudly.
//!
//! The facts belong to one project. Ingest loads them for the app it reads
//! and stores them on [`crate::App::extensions`]; analysis and emission read
//! them from the app in hand. Nothing here is process or thread state, so
//! apps with different extensions can be ingested, analyzed and emitted side
//! by side, in any order, on any thread.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::vfs::Vfs;

/// The facts every extension an app lists supplies, in listing order.
/// Empty for an app that lists none, which is every app's behavior before
/// extensions existed.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Extensions {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lock_gated_requires: Vec<LockGatedRequire>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rails_dependent: Vec<RailsDependentGem>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runtime_gems: Vec<RuntimeGem>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attribute_macros: Vec<AttributeMacro>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_gems: Vec<String>,
}

/// A `lock_gated_requires` row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockGatedRequire {
    /// What the emitted tree passes to `require`.
    pub require: String,
    /// The lockfile spec that gates it.
    pub gem: String,
}

/// A `rails_dependent` row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RailsDependentGem {
    pub gem: String,
    /// The evidence that requiring the gem reaches Rails.
    pub why: String,
}

/// A `runtime_gems` row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeGem {
    pub demand: Demand,
    pub gem: String,
}

/// What a `runtime_gems` row looks for in an `app/` file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Demand {
    /// A constant the body names, word-bounded.
    Constant(String),
    /// Any of these literal texts.
    AnyText(Vec<String>),
}

/// An `attribute_macros` row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttributeMacro {
    pub name: String,
    pub attributes: MacroAttributes,
}

/// Where a model-body macro names the attributes it defines.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MacroAttributes {
    /// The leading symbol arguments.
    Positional,
    /// The symbols in the array under this keyword argument.
    HashKey(String),
}

impl Extensions {
    pub fn is_empty(&self) -> bool {
        self.lock_gated_requires.is_empty()
            && self.rails_dependent.is_empty()
            && self.runtime_gems.is_empty()
            && self.attribute_macros.is_empty()
            && self.source_gems.is_empty()
    }

    /// Whether an extension says requiring `gem` reaches Rails.
    pub fn is_rails_dependent(&self, gem: &str) -> bool {
        self.rails_dependent.iter().any(|row| row.gem == gem)
    }

    /// The first attribute macro named `name`.
    pub fn attribute_macro(&self, name: &str) -> Option<&AttributeMacro> {
        self.attribute_macros.iter().find(|row| row.name == name)
    }
}

/// A refused extension: the file to fix and what is wrong with it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtensionError {
    pub file: String,
    pub message: String,
}

impl std::fmt::Display for ExtensionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "extension {}: {}", self.file, self.message)
    }
}

impl std::error::Error for ExtensionError {}

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct GemsFile {
    lock_gated_requires: Vec<LockGatedRow>,
    rails_dependent: Vec<RailsDependentRow>,
    runtime_gems: Vec<RuntimeGemRow>,
    source_gems: Vec<SourceGemRow>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceGemRow {
    gem: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LockGatedRow {
    require: String,
    gem: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RailsDependentRow {
    gem: String,
    why: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RuntimeGemRow {
    gem: String,
    constant: Option<String>,
    any_text: Option<Vec<String>>,
}

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct MacrosFile {
    attribute_macros: Vec<AttributeMacroRow>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AttributeMacroRow {
    name: String,
    #[serde(default)]
    positional: bool,
    hash_key: Option<String>,
}

/// The files an extension directory may hold. Anything else in it is the
/// app's own business (a README, say) and is not read.
pub const EXTENSION_FILES: &[&str] = &["gems.yml", "macros.yml"];

/// Directory entries a later format revision gives meaning to. Refused now
/// so a typo and an unimplemented feature both fail loudly.
const RESERVED_ENTRIES: &[&str] = &["types", "overlay"];

/// Where a gem fact came from, for naming both sides of a contradiction.
struct Origin {
    gem: String,
    key: &'static str,
    file: String,
}

/// Read the extension directories `dirs`, in order. Each must already be
/// resolved against the app root and checked against the `extensions:`
/// path rules (`ingest` does both, since the errors are `roundhouse.yml`'s).
pub fn load<V: Vfs + ?Sized>(vfs: &V, dirs: &[PathBuf]) -> Result<Extensions, ExtensionError> {
    let mut loaded = Extensions::default();
    let mut loads: Vec<Origin> = Vec::new();
    let mut refused: Vec<Origin> = Vec::new();
    for dir in dirs {
        if !vfs.is_dir(dir) {
            return Err(ExtensionError { file: display(dir), message: "not a directory".into() });
        }
        for reserved in RESERVED_ENTRIES {
            let path = dir.join(reserved);
            if vfs.exists(&path) {
                return Err(ExtensionError {
                    file: display(&path),
                    message: format!(
                        "{reserved} is not supported by this compiler (an extension may hold {})",
                        EXTENSION_FILES.join(" and ")
                    ),
                });
            }
        }
        if let Some((file, macros)) = read_yaml::<V, MacrosFile>(vfs, &dir.join("macros.yml"))? {
            load_macros(&file, macros, &mut loaded)?;
        }
        if let Some((file, gems)) = read_yaml::<V, GemsFile>(vfs, &dir.join("gems.yml"))? {
            load_gems(&file, gems, &mut loaded, &mut loads, &mut refused)?;
        }
    }
    validate(&loads, &refused)?;
    Ok(loaded)
}

fn display(path: &Path) -> String {
    path.display().to_string()
}

/// `path`'s YAML, or `None` when the file is absent. An empty file reads as
/// an empty table.
fn read_yaml<V: Vfs + ?Sized, T: for<'de> Deserialize<'de> + Default>(
    vfs: &V,
    path: &Path,
) -> Result<Option<(String, T)>, ExtensionError> {
    if !vfs.exists(path) && !vfs.is_symlink(path) {
        return Ok(None);
    }
    let file = display(path);
    let text = vfs.read_to_string(path).map_err(|e| ExtensionError { file: file.clone(), message: e.to_string() })?;
    let parsed: Option<T> =
        serde_yaml_ng::from_str(&text).map_err(|e| ExtensionError { file: file.clone(), message: e.to_string() })?;
    Ok(Some((file, parsed.unwrap_or_default())))
}

fn load_macros(file: &str, macros: MacrosFile, loaded: &mut Extensions) -> Result<(), ExtensionError> {
    let bad = |message: String| ExtensionError { file: file.to_string(), message };
    for row in macros.attribute_macros {
        if row.name.is_empty() {
            return Err(bad("attribute_macros rows need a name".into()));
        }
        let attributes = match (row.positional, row.hash_key) {
            (true, None) => MacroAttributes::Positional,
            (false, Some(key)) if !key.is_empty() => MacroAttributes::HashKey(key),
            _ => {
                return Err(bad(format!(
                    "attribute_macros row {:?} needs exactly one of positional and hash_key",
                    row.name
                )));
            }
        };
        loaded.attribute_macros.push(AttributeMacro { name: row.name, attributes });
    }
    Ok(())
}

fn load_gems(
    file: &str,
    gems: GemsFile,
    loaded: &mut Extensions,
    loads: &mut Vec<Origin>,
    refused: &mut Vec<Origin>,
) -> Result<(), ExtensionError> {
    let bad = |message: String| ExtensionError { file: file.to_string(), message };
    for row in gems.lock_gated_requires {
        if row.require.is_empty() || row.gem.is_empty() {
            return Err(bad("lock_gated_requires rows need a non-empty require and gem".into()));
        }
        loads.push(Origin { gem: row.gem.clone(), key: "lock_gated_requires", file: file.to_string() });
        loaded.lock_gated_requires.push(LockGatedRequire { require: row.require, gem: row.gem });
    }
    for row in gems.rails_dependent {
        if row.gem.is_empty() || row.why.is_empty() {
            return Err(bad("rails_dependent rows need a gem and the evidence (why)".into()));
        }
        refused.push(Origin { gem: row.gem.clone(), key: "rails_dependent", file: file.to_string() });
        loaded.rails_dependent.push(RailsDependentGem { gem: row.gem, why: row.why });
    }
    for row in gems.runtime_gems {
        if row.gem.is_empty() {
            return Err(bad("runtime_gems rows need a gem".into()));
        }
        let demand = match (row.constant, row.any_text) {
            (Some(constant), None) if !constant.is_empty() => Demand::Constant(constant),
            (None, Some(texts)) if !texts.is_empty() => {
                if texts.iter().any(String::is_empty) {
                    return Err(bad(format!("runtime_gems row for {:?} has an empty any_text entry", row.gem)));
                }
                Demand::AnyText(texts)
            }
            _ => {
                return Err(bad(format!(
                    "runtime_gems row for {:?} needs exactly one of constant and any_text",
                    row.gem
                )));
            }
        };
        loads.push(Origin { gem: row.gem.clone(), key: "runtime_gems", file: file.to_string() });
        loaded.runtime_gems.push(RuntimeGem { demand, gem: row.gem });
    }
    for row in gems.source_gems {
        if row.gem.is_empty() {
            return Err(bad("source_gems rows need a gem".into()));
        }
        if loaded.source_gems.contains(&row.gem) {
            return Err(bad(format!("source_gems lists {:?} twice", row.gem)));
        }
        refused.push(Origin { gem: row.gem.clone(), key: "source_gems", file: file.to_string() });
        loaded.source_gems.push(row.gem);
    }
    Ok(())
}

/// A gem that reaches Rails is never required natively, so no fact may also
/// say a tree loads it, and the compiler must not already stand in for it.
fn validate(loads: &[Origin], refused: &[Origin]) -> Result<(), ExtensionError> {
    for row in refused {
        let gem = &row.gem;
        let conflict = |message: String| ExtensionError { file: row.file.clone(), message };
        if let Some(other) = refused.iter().find(|other| other.gem == *gem && other.key != row.key) {
            return Err(conflict(format!("{gem:?} is {} but also {} in {}", row.key, other.key, other.file)));
        }
        if let Some(load) = loads.iter().find(|load| load.gem == *gem) {
            return Err(conflict(format!(
                "{gem:?} is {} but the tree also loads it ({} in {})",
                row.key, load.key, load.file
            )));
        }
        if crate::gems::compiler_tree_loads_gem(gem) {
            return Err(conflict(format!(
                "{gem:?} is {} but the tree also loads it (the compiler's own runtime gems)",
                row.key
            )));
        }
        let fate = crate::gems::fate_of(gem);
        if matches!(fate, crate::gems::GemFate::Framework | crate::gems::GemFate::Modeled) {
            return Err(conflict(format!(
                "{gem:?} is {} but the compiler stands in for it (a {} gem)",
                row.key,
                fate.label()
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vfs::MapVfs;

    fn vfs(files: &[(&str, &str)]) -> MapVfs {
        MapVfs::new(files.iter().map(|(path, text)| (PathBuf::from(path), text.as_bytes().to_vec())).collect())
    }

    fn load_one(files: &[(&str, &str)]) -> Result<Extensions, ExtensionError> {
        load(&vfs(files), &[PathBuf::from("ext")])
    }

    const TOY: &str = r#"
lock_gated_requires:
  - {require: "widgets/all", gem: "widgets"}
rails_dependent:
  - {gem: "widget-engine", why: "requiring it loads 3 Rails framework files"}
runtime_gems:
  - {constant: "Widget", gem: "widgets"}
  - {any_text: ["Widget::Cache.", "Widget::Store."], gem: "widget-cache"}
"#;

    #[test]
    fn gem_facts_load_in_file_order() {
        let loaded = load_one(&[("ext/gems.yml", TOY)]).unwrap();
        assert_eq!(
            loaded.lock_gated_requires,
            vec![LockGatedRequire { require: "widgets/all".into(), gem: "widgets".into() }]
        );
        assert_eq!(
            loaded.rails_dependent,
            vec![RailsDependentGem {
                gem: "widget-engine".into(),
                why: "requiring it loads 3 Rails framework files".into()
            }]
        );
        assert_eq!(
            loaded.runtime_gems,
            vec![
                RuntimeGem { demand: Demand::Constant("Widget".into()), gem: "widgets".into() },
                RuntimeGem {
                    demand: Demand::AnyText(vec!["Widget::Cache.".into(), "Widget::Store.".into()]),
                    gem: "widget-cache".into()
                },
            ]
        );
        assert!(loaded.is_rails_dependent("widget-engine"));
        assert!(!loaded.is_rails_dependent("widgets"));
    }

    #[test]
    fn extensions_append_in_listing_order() {
        let one = "lock_gated_requires:\n  - {require: a, gem: a}\n";
        let two = "lock_gated_requires:\n  - {require: b, gem: b}\n  - {require: c, gem: c}\n";
        let files = vfs(&[("one/gems.yml", one), ("two/gems.yml", two)]);
        let gems = |loaded: Extensions| loaded.lock_gated_requires.into_iter().map(|row| row.gem).collect::<Vec<_>>();
        let forward = load(&files, &[PathBuf::from("one"), PathBuf::from("two")]).unwrap();
        assert_eq!(gems(forward), ["a", "b", "c"]);
        let backward = load(&files, &[PathBuf::from("two"), PathBuf::from("one")]).unwrap();
        assert_eq!(gems(backward), ["b", "c", "a"]);
    }

    #[test]
    fn a_directory_without_extension_files_loads_empty() {
        let loaded = load_one(&[("ext/readme.txt", "hi")]).unwrap();
        assert_eq!(loaded, Extensions::default());
        assert!(loaded.is_empty());
        let loaded = load_one(&[("ext/gems.yml", ""), ("ext/macros.yml", "# nothing yet\n")]).unwrap();
        assert!(loaded.is_empty());
    }

    #[test]
    fn unknown_keys_are_refused() {
        let err = load_one(&[("ext/gems.yml", "gem_facts: []\n")]).unwrap_err();
        assert_eq!(err.file, "ext/gems.yml");
        assert!(err.message.contains("unknown field `gem_facts`"), "{err}");
        let err = load_one(&[("ext/gems.yml", "rails_dependent:\n  - {gem: g, why: w, extra: 1}\n")]).unwrap_err();
        assert!(err.message.contains("unknown field `extra`"), "{err}");
        let err = load_one(&[("ext/macros.yml", "macros: []\n")]).unwrap_err();
        assert_eq!(err.file, "ext/macros.yml");
        assert!(err.message.contains("unknown field `macros`"), "{err}");
    }

    #[test]
    fn a_wrongly_shaped_file_is_refused_with_its_name() {
        let err = load_one(&[("ext/gems.yml", "lock_gated_requires: widgets\n")]).unwrap_err();
        assert_eq!(err.file, "ext/gems.yml");
        let err = load_one(&[("ext/gems.yml", "lock_gated_requires: [\n")]).unwrap_err();
        assert_eq!(err.file, "ext/gems.yml");
        let err = load_one(&[("ext/gems.yml", "lock_gated_requires:\n  - {require: a}\n")]).unwrap_err();
        assert!(err.message.contains("missing field `gem`"), "{err}");
    }

    #[test]
    fn a_missing_directory_is_refused() {
        let err = load(&vfs(&[]), &[PathBuf::from("nowhere")]).unwrap_err();
        assert_eq!(err, ExtensionError { file: "nowhere".into(), message: "not a directory".into() });
        let err = load(&vfs(&[("file", "x")]), &[PathBuf::from("file")]).unwrap_err();
        assert_eq!(err.message, "not a directory");
    }

    #[test]
    fn formats_not_yet_implemented_are_refused_by_name() {
        let err = load_one(&[("ext/types/widget.rbs", "class Widget\nend\n")]).unwrap_err();
        assert_eq!(err.file, "ext/types");
        assert!(err.message.contains("types is not supported"), "{err}");
        let err = load_one(&[("ext/overlay/widget.rb", "class Widget; end\n")]).unwrap_err();
        assert_eq!(err.file, "ext/overlay");
        assert!(err.message.contains("overlay is not supported"), "{err}");
        assert!(err.message.contains("gems.yml and macros.yml"), "{err}");
    }

    #[test]
    fn attribute_macros_name_where_the_attributes_are() {
        let text = "attribute_macros:\n  - {name: price_column, positional: true}\n  - {name: bag_columns, hash_key: attributes}\n";
        let loaded = load_one(&[("ext/macros.yml", text)]).unwrap();
        assert_eq!(
            loaded.attribute_macros,
            vec![
                AttributeMacro { name: "price_column".into(), attributes: MacroAttributes::Positional },
                AttributeMacro { name: "bag_columns".into(), attributes: MacroAttributes::HashKey("attributes".into()) },
            ]
        );
        assert_eq!(loaded.attribute_macro("bag_columns").map(|m| &m.attributes), Some(&MacroAttributes::HashKey("attributes".into())));
        assert!(loaded.attribute_macro("serialize").is_none());
    }

    #[test]
    fn an_attribute_macro_names_exactly_one_argument_shape() {
        for row in ["{name: m}", "{name: m, positional: true, hash_key: k}", "{name: m, hash_key: \"\"}", "{name: m, positional: false}"] {
            let text = format!("attribute_macros:\n  - {row}\n");
            let err = load_one(&[("ext/macros.yml", &text)]).unwrap_err();
            assert!(err.message.contains("exactly one of positional and hash_key"), "{row}: {err}");
        }
        let err = load_one(&[("ext/macros.yml", "attribute_macros:\n  - {name: \"\", positional: true}\n")]).unwrap_err();
        assert!(err.message.contains("need a name"), "{err}");
    }

    #[test]
    fn a_runtime_gem_row_names_exactly_one_marker() {
        for row in ["{gem: g}", "{gem: g, constant: C, any_text: [x]}", "{gem: g, constant: \"\"}", "{gem: g, any_text: []}"] {
            let text = format!("runtime_gems:\n  - {row}\n");
            let err = load_one(&[("ext/gems.yml", &text)]).unwrap_err();
            assert!(err.message.contains("exactly one of constant and any_text"), "{row}: {err}");
        }
        let err = load_one(&[("ext/gems.yml", "runtime_gems:\n  - {gem: g, any_text: [x, \"\"]}\n")]).unwrap_err();
        assert!(err.message.contains("empty any_text entry"), "{err}");
        let err = load_one(&[("ext/gems.yml", "runtime_gems:\n  - {gem: \"\", constant: C}\n")]).unwrap_err();
        assert!(err.message.contains("need a gem"), "{err}");
    }

    #[test]
    fn empty_gem_fields_are_refused() {
        let err = load_one(&[("ext/gems.yml", "lock_gated_requires:\n  - {require: \"\", gem: g}\n")]).unwrap_err();
        assert!(err.message.contains("non-empty require and gem"), "{err}");
        let err = load_one(&[("ext/gems.yml", "rails_dependent:\n  - {gem: g, why: \"\"}\n")]).unwrap_err();
        assert!(err.message.contains("evidence (why)"), "{err}");
    }

    #[test]
    fn a_rails_dependent_gem_the_tree_loads_is_refused() {
        let text = "rails_dependent:\n  - {gem: g, why: w}\nlock_gated_requires:\n  - {require: g, gem: g}\n";
        let err = load_one(&[("ext/gems.yml", text)]).unwrap_err();
        assert_eq!(err.file, "ext/gems.yml");
        assert!(err.message.contains("\"g\" is rails_dependent but the tree also loads it (lock_gated_requires in ext/gems.yml)"), "{err}");

        let text = "rails_dependent:\n  - {gem: g, why: w}\nruntime_gems:\n  - {constant: G, gem: g}\n";
        let err = load_one(&[("ext/gems.yml", text)]).unwrap_err();
        assert!(err.message.contains("(runtime_gems in ext/gems.yml)"), "{err}");

        // Across extensions: the refusal names both files.
        let files = vfs(&[
            ("one/gems.yml", "rails_dependent:\n  - {gem: g, why: w}\n"),
            ("two/gems.yml", "lock_gated_requires:\n  - {require: g, gem: g}\n"),
        ]);
        let err = load(&files, &[PathBuf::from("one"), PathBuf::from("two")]).unwrap_err();
        assert_eq!(err.file, "one/gems.yml");
        assert!(err.message.contains("lock_gated_requires in two/gems.yml"), "{err}");

        // A gem the compiler's own runtime table declares.
        let err = load_one(&[("ext/gems.yml", "rails_dependent:\n  - {gem: rotp, why: w}\n")]).unwrap_err();
        assert!(err.message.contains("the compiler's own runtime gems"), "{err}");
    }

    #[test]
    fn a_rails_dependent_gem_the_compiler_stands_in_for_is_refused() {
        let err = load_one(&[("ext/gems.yml", "rails_dependent:\n  - {gem: kaminari, why: w}\n")]).unwrap_err();
        assert!(err.message.contains("\"kaminari\" is rails_dependent but the compiler stands in for it (a modeled gem)"), "{err}");
        let err = load_one(&[("ext/gems.yml", "rails_dependent:\n  - {gem: actionpack, why: w}\n")]).unwrap_err();
        assert!(err.message.contains("(a framework gem)"), "{err}");
    }

    #[test]
    fn source_gems_load_and_contradictions_are_refused() {
        let loaded = load_one(&[("ext/gems.yml", "source_gems:\n  - {gem: pundit}\n")]).unwrap();
        assert_eq!(loaded.source_gems, vec!["pundit".to_string()]);

        let err = load_one(&[("ext/gems.yml", "source_gems:\n  - {gem: pundit}\n  - {gem: pundit}\n")]).unwrap_err();
        assert!(err.message.contains("lists \"pundit\" twice"), "{err}");

        let text = "source_gems:\n  - {gem: g}\nlock_gated_requires:\n  - {require: g, gem: g}\n";
        let err = load_one(&[("ext/gems.yml", text)]).unwrap_err();
        assert!(err.message.contains("\"g\" is source_gems but the tree also loads it"), "{err}");

        let text = "source_gems:\n  - {gem: g}\nrails_dependent:\n  - {gem: g, why: w}\n";
        let err = load_one(&[("ext/gems.yml", text)]).unwrap_err();
        assert!(err.message.contains("but also"), "{err}");

        let err = load_one(&[("ext/gems.yml", "source_gems:\n  - {gem: kaminari}\n")]).unwrap_err();
        assert!(err.message.contains("the compiler stands in for it (a modeled gem)"), "{err}");
    }
}
