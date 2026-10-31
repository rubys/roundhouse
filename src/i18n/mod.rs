//! The app's translations for its default locale, loaded the way Rails'
//! I18n simple backend loads them: Rails' own `en.yml` files first, then
//! `config/locales/**/*.yml` in sorted path order, each deep-merged over
//! the ones before, YAML merge keys (`<<: *errors`) applied. Lookups
//! answer string leaves by dotted key, so text Rails derives from I18n
//! (`human_attribute_name`, `errors.format`, validation messages, form
//! labels) is answered as Rails would answer it, not re-derived.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_yaml_ng::Value;

pub mod gems;
pub mod plural;
pub mod rails_i18n;

/// Rails 8.1.4's bundled English, in the order its frameworks load it.
const RAILS_EN: &[(&str, &str)] = &[
    ("activesupport", include_str!("rails/activesupport.en.yml")),
    ("activemodel", include_str!("rails/activemodel.en.yml")),
    ("activerecord", include_str!("rails/activerecord.en.yml")),
    ("actionview", include_str!("rails/actionview.en.yml")),
];

pub const DEFAULT_ERRORS_FORMAT: &str = "%{attribute} %{message}";

/// Why a lookup that lands on a node is refused, read back by the lowering
/// that answers a plain one as a Hash.
pub const SUBTREE: &str = "names a subtree, which I18n answers as a Hash";

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Catalog {
    /// `config.i18n.default_locale`, `en` when the app sets none.
    pub locale: String,
    /// Dotted key (`activerecord.attributes.user.email`) → string leaf.
    pub entries: BTreeMap<String, String>,
    /// Keys whose value is not a String (an Array, a number, a boolean):
    /// I18n answers them as they are, which `entries` cannot hold.
    #[serde(default, skip_serializing_if = "std::collections::BTreeSet::is_empty")]
    pub opaque: std::collections::BTreeSet<String>,
    /// The opaque values a lookup can answer as they are, by key.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub values: BTreeMap<String, Opaque>,
    /// Each node's children in the order the files give them, as the
    /// deep merge of Ruby Hashes keeps it ("" for the locale's root).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub order: BTreeMap<String, Vec<String>>,
    /// What a translation the locale lacks does, per environment.
    #[serde(default)]
    pub missing: MissingPolicy,
    /// The file that sets `config.action_view.field_error_proc`, when the
    /// app replaces Rails' `field_with_errors` wrapper.
    #[serde(default)]
    pub field_error_proc: Option<String>,
    /// `config.i18n.available_locales`, this locale among them; empty
    /// when the app has only its default locale.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub available: Vec<String>,
    /// Why `config.i18n.available_locales` cannot be read at compile time
    /// (a value computed at boot); `available` then holds the locales the
    /// app's files name, and a translation that can run in more than one
    /// stays an error.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub available_unknown: Option<String>,
    /// The catalogs of the other available locales, and of any locale a
    /// fallback chain names, loaded as this one is.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub others: Vec<Catalog>,
    /// `config.i18n.fallbacks`, per environment.
    #[serde(default)]
    pub fallbacks: FallbackPolicy,
    /// Each locale's own plural rule (rails-i18n's pluralization, an
    /// app's `config/locales/plurals.rb`) in [`plural`]'s text, empty for
    /// one roundhouse cannot read: a count then picks `few`, `many` or
    /// `two` by the locale. A locale with none uses I18n's default.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub plurals: BTreeMap<String, String>,
}

/// A non-String value as I18n answers it: an Array of Strings and
/// Symbols (`order: [:year, :month, :day]`), a number or a boolean.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Opaque {
    /// Each item's text, `true` for a Symbol.
    Array(Vec<(bool, String)>),
    Int(i64),
    Float(f64),
    Bool(bool),
}

impl Opaque {
    fn from_yaml(value: &Value, colons: &Colons) -> Option<Opaque> {
        match value {
            Value::Bool(b) => Some(Opaque::Bool(*b)),
            Value::Number(n) if n.is_f64() => n.as_f64().map(Opaque::Float),
            // Not `as_f64` for one past i64: a Float would not be the Integer I18n answers.
            Value::Number(n) => n.as_i64().map(Opaque::Int),
            Value::Sequence(items) => items.iter().map(|i| match i {
                // The run-time table separates its fields and records with these.
                Value::String(s) if s.chars().any(|c| ('\u{1c}'..='\u{1f}').contains(&c)) => None,
                Value::String(s) if s.chars().count() > 1 && s.starts_with(':') => match (colons.quoted.contains(s), colons.plain.contains(s)) {
                    (true, true) => None,
                    (true, false) => Some((false, s.clone())),
                    // A plain `:year` loads as a Symbol in Psych; serde's YAML keeps it a String with the colon.
                    _ => Some((true, s[1..].to_string())),
                },
                Value::String(s) => Some((false, s.clone())),
                _ => None,
            }).collect::<Option<_>>().map(Opaque::Array),
            _ => None,
        }
    }

    /// The run-time table's kind letter, then the value's fields (an
    /// item's `y` or `s` before its text).
    pub fn fields(&self) -> Vec<String> {
        match self {
            Opaque::Array(items) => std::iter::once("A".to_string())
                .chain(items.iter().map(|(sym, text)| format!("{}{text}", if *sym { 'y' } else { 's' })))
                .collect(),
            Opaque::Int(n) => vec!["I".into(), n.to_string()],
            Opaque::Float(f) => vec!["F".into(), f.to_string()],
            Opaque::Bool(b) => vec!["B".into(), b.to_string()],
        }
    }
}

/// The `:`-led scalars a file writes quoted, which Psych loads as
/// Strings, and those it writes plain, which it loads as Symbols.
#[derive(Default)]
struct Colons {
    quoted: crate::det::HashSet<String>,
    plain: crate::det::HashSet<String>,
}

impl Colons {
    fn of(source: &str) -> Colons {
        use yaml_rust2::parser::{Event, Parser};
        use yaml_rust2::scanner::TScalarStyle;
        let mut out = Colons::default();
        let mut parser = Parser::new_from_str(source);
        while let Ok((event, _)) = parser.next_token() {
            match event {
                Event::StreamEnd => break,
                Event::Scalar(text, style, ..) if text.starts_with(':') => {
                    match style {
                        TScalarStyle::Plain => out.plain.insert(text),
                        _ => out.quoted.insert(text),
                    };
                }
                _ => {}
            }
        }
        out
    }
}

/// `config.i18n.fallbacks`: Rails includes I18n's Fallbacks backend when
/// it is set, and a lookup then tries the requested locale, its parents
/// (`pt-BR`, `pt`), then these default locales. `true` is
/// `[default_locale]`; `false`, `nil` or unset leaves the backend out.
/// An environment's file runs after `config/application.rb`, so its own
/// setting wins there.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FallbackPolicy {
    /// `config/application.rb`'s setting.
    pub app: Option<Vec<String>>,
    /// Each environment whose own file sets it, with its setting.
    pub envs: Vec<(String, Option<Vec<String>>)>,
    /// Why a setting cannot be read at compile time (a Hash of chains, a
    /// value computed at boot); a translation then stays an error.
    pub unknown: Option<String>,
}

/// One way a translation can be looked up at run time: in `locale`,
/// with `fallbacks`, in the environments `envs` names (every environment
/// but `except` when `envs` is `None`).
#[derive(Clone, Debug, PartialEq, Hash)]
pub struct Variant {
    pub locale: String,
    pub fallbacks: Option<Vec<String>>,
    pub envs: Option<Vec<String>>,
    pub except: Vec<String>,
}

impl Catalog {
    /// The locales `I18n.locale` can be, the default first.
    pub fn locales(&self) -> Vec<String> {
        let mut out = vec![self.locale.clone()];
        out.extend(self.available.iter().filter(|l| **l != self.locale).cloned());
        out
    }

    /// Every (locale, fallback setting, environments) a lookup in one of
    /// `locales` can run under.
    pub fn variants(&self, locales: &[String]) -> Vec<Variant> {
        let except: Vec<String> = self.fallbacks.envs.iter().map(|(e, _)| e.clone()).collect();
        let mut out = Vec::new();
        for locale in locales {
            out.push(Variant { locale: locale.clone(), fallbacks: self.fallbacks.app.clone(), envs: None, except: except.clone() });
            for (env, setting) in &self.fallbacks.envs {
                out.push(Variant { locale: locale.clone(), fallbacks: setting.clone(), envs: Some(vec![env.clone()]), except: Vec::new() });
            }
        }
        out
    }
}

/// The keys a YAML file names at its top level (`en:`, `"pt-BR":`), as
/// `Catalog::merge_source` looks for them.
fn top_level_keys(source: &str) -> crate::det::HashSet<String> {
    source
        .lines()
        .filter(|l| !l.starts_with([' ', '\t', '#', '-']))
        .filter_map(|l| l.split_once(':').map(|(k, _)| k.trim().trim_matches(['"', '\'']).to_string()))
        .collect()
}

/// `I18n::Locale::Fallbacks#[]`: `locale` and its parents, then each
/// default locale and its parents, each once.
pub fn fallback_chain(locale: &str, defaults: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for tag in std::iter::once(locale).chain(defaults.iter().map(String::as_str)) {
        let mut tag = tag.to_string();
        loop {
            if !out.contains(&tag) {
                out.push(tag.clone());
            }
            match tag.rsplit_once('-') {
                Some((parent, _)) => tag = parent.to_string(),
                None => break,
            }
        }
    }
    out
}

/// `config.i18n.raise_on_missing_translations`: Rails answers a missing
/// translation with text unless the running environment sets it, and
/// then raises `I18n::MissingTranslationData`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MissingPolicy {
    /// `config/application.rb`'s value, Rails' `false` when unset.
    pub raises: bool,
    /// The environments whose own file sets the other value.
    pub flipped: Vec<String>,
    /// Why the setting cannot be read at compile time (`:strict`, or a
    /// value computed at boot); a missing translation then stays an error.
    pub unknown: Option<String>,
}

/// One piece of an interpolated message: text known at compile time, or
/// the validated attribute's value (`%{value}`), read when it runs.
#[derive(Clone, Debug, PartialEq)]
pub enum Part {
    Text(String),
    Value,
}

impl Catalog {
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Rails' English alone, for an app or model built outside ingest.
    pub fn rails_default() -> &'static Catalog {
        static DEFAULT: std::sync::OnceLock<Catalog> = std::sync::OnceLock::new();
        DEFAULT.get_or_init(|| Catalog::load("en", &mut Vec::new()).0)
    }

    pub fn or_rails_default(&self) -> &Catalog {
        if self.locale.is_empty() { Catalog::rails_default() } else { self }
    }

    /// Rails' English, then `files` (path, source), for `locale`. A file
    /// that does not parse is skipped and reported.
    pub fn load(locale: &str, files: &mut Vec<(String, String)>) -> (Catalog, Vec<(String, String)>) {
        Catalog::load_with_gems(locale, &[], files)
    }

    /// As [`Catalog::load`], with the gems' files (already in load order)
    /// between Rails' English and the app's.
    pub fn load_with_gems(locale: &str, gems: &[(String, String)], files: &mut Vec<(String, String)>) -> (Catalog, Vec<(String, String)>) {
        files.sort_by(|a, b| a.0.cmp(&b.0));
        let mut catalog = Catalog { locale: locale.to_string(), ..Default::default() };
        let mut problems = Vec::new();
        let rails = RAILS_EN.iter().map(|(name, src)| (name.to_string(), src.to_string()));
        for (path, source) in rails.collect::<Vec<_>>().iter().chain(gems.iter()).chain(files.iter()) {
            if let Err(e) = catalog.merge_source(source) {
                problems.push((path.clone(), e));
            }
        }
        (catalog, problems)
    }

    /// The other locales: each of `available` but this one, and each a
    /// fallback chain from one of them reaches, loaded from the same
    /// files as this one.
    pub fn load_locales(&mut self, available: Vec<String>, fallbacks: FallbackPolicy, gems: &[(String, String)], files: &mut Vec<(String, String)>) {
        let settings: Vec<&Vec<String>> = fallbacks.app.iter().chain(fallbacks.envs.iter().filter_map(|(_, s)| s.as_ref())).collect();
        let mut wanted: Vec<String> = Vec::new();
        for locale in &available {
            let chains = settings.iter().flat_map(|d| fallback_chain(locale, d));
            for l in std::iter::once(locale.clone()).chain(chains) {
                if l != self.locale && !wanted.contains(&l) {
                    wanted.push(l);
                }
            }
        }
        files.sort_by(|a, b| a.0.cmp(&b.0));
        // Not every file for every locale: a hundred locales each scanned every line of every file.
        let tops: Vec<crate::det::HashSet<String>> = gems.iter().chain(files.iter()).map(|(_, src)| top_level_keys(src)).collect();
        self.others = wanted
            .iter()
            .map(|l| {
                let mut at = tops.iter();
                let gems: Vec<(String, String)> = gems.iter().filter(|_| at.next().is_some_and(|t| t.contains(l))).cloned().collect();
                let mut files: Vec<(String, String)> = files.iter().filter(|_| at.next().is_some_and(|t| t.contains(l))).cloned().collect();
                Catalog::load_with_gems(l, &gems, &mut files).0
            })
            .collect();
        self.available = if available.len() > 1 { available } else { Vec::new() };
        self.fallbacks = fallbacks;
    }

    fn merge_source(&mut self, source: &str) -> Result<(), String> {
        let top = format!("{}:", self.locale);
        let quoted = [format!("\"{}\":", self.locale), format!("'{}':", self.locale)];
        // Not parsed: a file with no top-level key for this locale adds nothing to it.
        if !source.lines().any(|l| l.trim_end().starts_with(&top) || quoted.iter().any(|q| l.starts_with(q.as_str()))) {
            return Ok(());
        }
        let mut value: Value = serde_yaml_ng::from_str(source).map_err(|e| e.to_string())?;
        value.apply_merge().map_err(|e| e.to_string())?;
        if let Some(tree) = value.get(self.locale.as_str()) {
            let colons = Colons::of(source);
            flatten(tree, String::new(), &mut Flat { entries: &mut self.entries, opaque: &mut self.opaque, values: &mut self.values, order: &mut self.order, colons: &colons });
        }
        Ok(())
    }

    pub fn lookup(&self, key: &str) -> Option<&str> {
        self.entries.get(key).map(String::as_str)
    }

    /// `key`, or with a `count` its plural form for English (`zero` when
    /// defined and the count is 0, `one` for 1, else `other`).
    pub fn translate(&self, key: &str, count: Option<i64>) -> Option<&str> {
        if let Some(found) = self.lookup(key) {
            return Some(found);
        }
        let count = count?;
        let form = match count {
            0 if self.entries.contains_key(&format!("{key}.zero")) => "zero",
            1 => "one",
            _ => "other",
        };
        self.lookup(&format!("{key}.{form}"))
    }

    /// The entries a model's own lookups can reach: everything under its
    /// i18n keys in `scope`, and the global error and attribute tables.
    pub fn subset_for(&self, scope: &str, keys: &[String]) -> Catalog {
        let mut prefixes: Vec<String> = keys
            .iter()
            .flat_map(|k| {
                [
                    format!("{scope}.attributes.{k}."),
                    format!("{scope}.errors.models.{k}."),
                    format!("{scope}.models.{k}"),
                ]
            })
            .collect();
        prefixes.extend(
            ["attributes.", "errors.format", "errors.messages.", "errors.attributes."]
                .map(str::to_string),
        );
        prefixes.push(format!("{scope}.errors.messages."));
        // Each prefix names one run of the sorted keys: seek to it rather than
        // test every key of the catalog (every locale file of every gem) per model.
        let mut entries = BTreeMap::new();
        for prefix in &prefixes {
            let from = (std::ops::Bound::Included(prefix.as_str()), std::ops::Bound::Unbounded);
            for (k, v) in self.entries.range::<str, _>(from).take_while(|(k, _)| k.starts_with(prefix.as_str())) {
                entries.insert(k.clone(), v.clone());
            }
        }
        Catalog { locale: self.locale.clone(), entries, ..Default::default() }
    }

    /// `Model.human_attribute_name(attr)`: the first of
    /// `<scope>.attributes.<i18n_key>.<attr>` over `keys` (the model,
    /// then its model ancestors), then `attributes.<attr>`, then
    /// `attr.humanize`.
    pub fn human_attribute_name(&self, scope: &str, keys: &[String], attr: &str) -> String {
        keys.iter()
            .map(|k| format!("{scope}.attributes.{k}.{attr}"))
            .chain(std::iter::once(format!("attributes.{attr}")))
            .find_map(|key| self.lookup(&key).map(str::to_string))
            .unwrap_or_else(|| humanize(&attr.replace('.', "_")))
    }

    /// `Model.model_name.human`: `<scope>.models.<i18n_key>` over `keys`
    /// (singular), else the class name humanized.
    pub fn model_human_name(&self, scope: &str, keys: &[String]) -> String {
        keys.iter()
            .find_map(|k| self.translate(&format!("{scope}.models.{k}"), Some(1)))
            .map(str::to_string)
            .unwrap_or_else(|| {
                let last = keys.first().map(|k| k.rsplit('/').next().unwrap_or(k)).unwrap_or("");
                humanize(last)
            })
    }

    /// `errors.format`, Rails' `"%{attribute} %{message}"` when unset.
    pub fn errors_format(&self) -> &str {
        self.lookup("errors.format").unwrap_or(DEFAULT_ERRORS_FORMAT)
    }

    /// A full message as `ActiveModel::Error.full_message` builds it: the
    /// bare message for `:base`, else `errors.format` over the attribute's
    /// human name.
    pub fn full_message(&self, scope: &str, keys: &[String], attr: &str, message: &str) -> String {
        if attr == "base" {
            return message.to_string();
        }
        format_full(self.errors_format(), &self.human_attribute_name(scope, keys, attr), message)
    }

    /// The message `errors.add(attr, kind, **opts)` stores, as
    /// `ActiveModel::Error.generate_message` looks it up: per model
    /// ancestor `<scope>.errors.models.<key>.attributes.<attr>.<kind>`
    /// then `<scope>.errors.models.<key>.<kind>`, then
    /// `<scope>.errors.messages.<kind>`, `errors.attributes.<attr>.<kind>`,
    /// `errors.messages.<kind>`. A `message:` String replaces every key
    /// after the first. Interpolated with `%{attribute}`, `%{model}` and
    /// `%{count}`; `%{value}` is left for run time.
    pub fn error_message(
        &self,
        scope: &str,
        keys: &[String],
        attr: &str,
        kind: &str,
        opts: &ErrorOpts,
    ) -> Option<Vec<Part>> {
        let mut defaults: Vec<String> = keys
            .iter()
            .flat_map(|k| {
                [
                    format!("{scope}.errors.models.{k}.attributes.{attr}.{kind}"),
                    format!("{scope}.errors.models.{k}.{kind}"),
                ]
            })
            .collect();
        defaults.push(format!("{scope}.errors.messages.{kind}"));
        defaults.push(format!("errors.attributes.{attr}.{kind}"));
        defaults.push(format!("errors.messages.{kind}"));
        let count = opts.count.as_deref().and_then(|c| c.parse::<i64>().ok()).or(opts.count.as_ref().map(|_| 2));
        let template = match &opts.message {
            Some(message) => self.translate(&defaults[0], count).unwrap_or(message),
            None => defaults.iter().find_map(|k| self.translate(k, count))?,
        };
        let attribute = opts.attribute.clone().unwrap_or_else(|| self.human_attribute_name(scope, keys, attr));
        let model = self.model_human_name(scope, keys);
        let value = |name: &str| match name {
            "attribute" => Some(attribute.clone()),
            "model" => Some(model.clone()),
            "count" => Some(opts.count.clone().unwrap_or_default()),
            _ => None,
        };
        let parts = pieces(template)
            .unwrap_or_else(|_| vec![Piece::Text(template.to_string())])
            .into_iter()
            .map(|p| match p {
                Piece::Name(n) if n == "value" => Part::Value,
                Piece::Name(n) => Part::Text(value(&n).unwrap_or_else(|| format!("%{{{n}}}"))),
                Piece::Text(t) => Part::Text(t),
            })
            .collect();
        Some(merge_text(parts))
    }
}

/// The options a validator passes `errors.add` that reach its message.
#[derive(Clone, Debug, Default)]
pub struct ErrorOpts {
    /// `count:` as Rails interpolates it (`5`, `0.5`); an integer also
    /// picks the plural form.
    pub count: Option<String>,
    /// A `message:` String.
    pub message: Option<String>,
    /// An `attribute:` override (`confirmation` names the confirmed one).
    pub attribute: Option<String>,
}

impl ErrorOpts {
    pub fn count(count: impl ToString) -> Self {
        ErrorOpts { count: Some(count.to_string()), ..Default::default() }
    }
}

pub fn format_full(format: &str, attribute: &str, message: &str) -> String {
    interpolate_text(format, |name| match name {
        "attribute" => Some(attribute.to_string()),
        "message" => Some(message.to_string()),
        _ => None,
    })
}

/// A `t` / `I18n.t` call reduced to what its lookup needs.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Translate {
    /// The key as written; a leading `.` is a view's lazy lookup.
    pub key: String,
    /// The template the call sits in (`articles/_form`), for a lazy key.
    pub view: Option<String>,
    /// `scope:`, dotted.
    pub scope: Option<String>,
    /// `default:` entries in order: a key (`Symbol`) or literal text.
    pub defaults: Vec<Fallback>,
    /// `default:` was an Array (`default: [:a]`, not `default: :a`), which
    /// is what makes I18n's missing message list every key it considered.
    pub default_list: bool,
    /// The call is ActionView's `t`, which tries its key and each default
    /// key in turn rather than leaving the defaults to I18n.
    pub template: bool,
    /// `count:` given.
    pub counted: bool,
    /// The other keywords, each an interpolation value.
    pub values: Vec<String>,
}

impl Translate {
    /// `key` as I18n normalizes it for this call: a view's lazy `.key`
    /// expanded from the template's path, then `scope:` in front.
    pub fn full_key(call: &Translate, key: &str) -> Result<String, String> {
        let key = match key.strip_prefix('.') {
            Some(rest) => {
                let view = call.view.as_deref().ok_or("a lazy key outside a template")?;
                format!("{}.{rest}", view.replace("/_", ".").replace('/', "."))
            }
            None => key.to_string(),
        };
        Ok(match &call.scope {
            Some(scope) => format!("{scope}.{key}"),
            None => key,
        })
    }

    /// I18n interpolates only when the call passes a value, so with none
    /// `%{x}` and `%%` stay as written.
    pub fn interpolates(&self) -> bool {
        self.counted || !self.values.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Fallback {
    Key(String),
    Text(String),
}

/// What a resolved translation renders: one template, or the plural
/// forms a run-time `count` picks between.
#[derive(Clone, Debug, PartialEq, Hash)]
pub enum Translation {
    One(String),
    Plural { zero: Option<String>, one: String, other: String },
    /// Nothing found: Rails' missing-translation answer for `key`, with
    /// the locale in front (`en.shop.title`). `considered` is every
    /// `default:` key after it, normalized the same way, when the call
    /// gave its defaults as an Array: I18n's message then lists them all
    /// (`Translation missing. Options considered were:`).
    Missing { key: String, considered: Vec<String> },
}

impl Catalog {
    /// Resolve `call` as I18n's simple backend does, or say why it
    /// cannot be answered at compile time.
    pub fn resolve(&self, call: &Translate) -> Result<Translation, String> {
        self.resolve_in(&self.locale, None, call)
    }

    /// The catalog of `locale`: this one, or one of [`Catalog::others`].
    pub fn catalog_for(&self, locale: &str) -> Option<&Catalog> {
        if self.locale == locale {
            return Some(self);
        }
        self.others.iter().find(|c| c.locale == locale)
    }

    /// Resolve `call` for `locale` as I18n does with `fallbacks` (`None`
    /// when the Fallbacks backend is not included, else its default
    /// locales). With fallbacks I18n tries the key in the locale, then each
    /// `default:` key before the first text across the whole chain, then
    /// the key in each later locale, then the text; ActionView's `t`
    /// instead walks its key and each default key in turn, each across
    /// the chain. A missing translation names the requested locale.
    pub fn resolve_in(&self, locale: &str, fallbacks: Option<&[String]>, call: &Translate) -> Result<Translation, String> {
        let chain: Vec<&Catalog> = match fallbacks {
            None => self.catalog_for(locale).into_iter().collect(),
            Some(defaults) => fallback_chain(locale, defaults).iter().filter_map(|l| self.catalog_for(l)).collect(),
        };
        if chain.first().is_none_or(|c| c.locale != locale) {
            return Err(format!("`{locale}` is not an available locale"));
        }
        let full = |key: &str| Translate::full_key(call, key);
        let key = full(&call.key)?;
        let mut defaults: Vec<Result<String, String>> = Vec::new();
        for default in &call.defaults {
            defaults.push(match default {
                Fallback::Key(k) => Ok(full(k)?),
                Fallback::Text(t) => Err(t.clone()),
            });
        }
        // What is tried, in order: `Ok((catalog, key))` or `Err(text)`.
        let across = |k: &str| -> Vec<Result<(&Catalog, String), String>> { chain.iter().map(|c| Ok((*c, k.to_string()))).collect() };
        let candidates: Vec<Result<(&Catalog, String), String>> = match (fallbacks, call.template) {
            (None, _) => std::iter::once(Ok((chain[0], key.clone())))
                .chain(defaults.iter().map(|d| d.clone().map(|k| (chain[0], k))))
                .collect(),
            (Some(_), true) => std::iter::once(Ok(key.clone()))
                .chain(defaults.iter().cloned())
                .flat_map(|d| match d {
                    Ok(k) => across(&k),
                    Err(t) => vec![Err(t)],
                })
                .collect(),
            (Some(_), false) => {
                let symbols: Vec<&String> = defaults.iter().map_while(|d| d.as_ref().ok()).collect();
                let text = defaults.iter().find_map(|d| d.clone().err());
                std::iter::once(Ok((chain[0], key.clone())))
                    .chain(symbols.iter().flat_map(|k| across(k)))
                    .chain(chain[1..].iter().map(|c| Ok((*c, key.clone()))))
                    .chain(text.map(Err))
                    .collect()
            }
        };
        let mut found = None;
        for c in &candidates {
            match c {
                Ok((catalog, key)) => {
                    if let Some(t) = catalog.entry(key, call.counted) {
                        found = Some(t);
                        break;
                    }
                    if catalog.opaque.contains(key) {
                        return Err(format!("`{key}` is not a String (an Array, a number or a boolean), which I18n answers as it is"));
                    }
                    if catalog.is_subtree(key) {
                        return Err(format!("`{key}` {SUBTREE}"));
                    }
                }
                Err(text) => {
                    found = Some(Translation::One(text.clone()));
                    break;
                }
            }
        }
        let missing = format!("{locale}.{key}");
        let translation = match found {
            Some(t) => t,
            // Every default was a key, and none is here either.
            None if self.missing.unknown.is_none() => {
                let considered = match call.default_list {
                    true => defaults.iter().flatten().map(|k| format!("{locale}.{k}")).collect(),
                    false => Vec::new(),
                };
                return Ok(Translation::Missing { key: missing, considered });
            }
            None => {
                let why = self.missing.unknown.as_deref().map(|u| format!(" ({u})")).unwrap_or_default();
                return Err(format!("translation missing: {missing}{why}"));
            }
        };
        let templates: Vec<&String> = match &translation {
            Translation::One(t) => vec![t],
            Translation::Plural { zero, one, other } => zero.iter().chain([one, other]).collect(),
            Translation::Missing { .. } => Vec::new(),
        };
        if !call.interpolates() {
            return Ok(translation);
        }
        for template in templates {
            for piece in pieces(template)? {
                let Piece::Name(name) = piece else { continue };
                let given = (name == "count" && call.counted) || call.values.iter().any(|v| v == &name);
                if !given {
                    return Err(format!("missing interpolation argument %{{{name}}}"));
                }
            }
        }
        Ok(translation)
    }

        /// I18n's own `available_locales` for the app's files: each locale a
    /// file names at its top level, in path order.
    pub fn file_locales(files: &[(String, String)]) -> Vec<String> {
        let mut sorted: Vec<&(String, String)> = files.iter().collect();
        sorted.sort_by(|a, b| a.0.cmp(&b.0));
        let mut out: Vec<String> = Vec::new();
        for (_, source) in sorted {
            let Ok(Value::Mapping(top)) = serde_yaml_ng::from_str::<Value>(source) else { continue };
            for key in top.keys() {
                if let Value::String(l) = key {
                    if !out.contains(l) {
                        out.push(l.clone());
                    }
                }
            }
        }
        out
    }

/// Whether `key` has entries beneath it.
    pub fn is_subtree(&self, key: &str) -> bool {
        let below = format!("{key}.");
        self.order.contains_key(key) || self.entries.range(below.clone()..).next().is_some_and(|(k, _)| k.starts_with(&below))
    }

    /// Whether `key` is a subtree holding only Strings (and empty
    /// mappings), which a Hash of them answers whole.
    pub fn plain_subtree(&self, key: &str) -> bool {
        let below = format!("{key}.");
        self.is_subtree(key) && !self.opaque.iter().any(|k| k.starts_with(&below))
    }

    /// Every String leaf and every empty mapping (`None`), in the order
    /// the files give them, as a Ruby Hash built from them iterates.
    pub fn ordered_nodes(&self) -> Vec<(String, Option<&String>)> {
        let mut out = Vec::new();
        let mut seen = crate::det::HashSet::default();
        fn walk<'a>(c: &'a Catalog, node: &str, out: &mut Vec<(String, Option<&'a String>)>, seen: &mut crate::det::HashSet<String>) {
            for child in c.order.get(node).into_iter().flatten() {
                let path = if node.is_empty() { child.clone() } else { format!("{node}.{child}") };
                if let Some(v) = c.entries.get(&path) {
                    if seen.insert(path.clone()) {
                        out.push((path, Some(v)));
                    }
                } else if c.order.get(&path).is_some_and(|children| children.is_empty()) {
                    if seen.insert(path.clone()) {
                        out.push((path, None));
                    }
                } else {
                    walk(c, &path, out, seen);
                }
            }
        }
        walk(self, "", &mut out, &mut seen);
        for (k, v) in &self.entries {
            if seen.insert(k.clone()) {
                out.push((k.clone(), Some(v)));
            }
        }
        out
    }

    /// The keys a call can reach when its key is `base` (already scoped
    /// and expanded) followed by text known only at run time and ending
    /// in `tail`: the translations to fold, and the subtrees Rails would
    /// answer as a Hash. Supersets are harmless, since the run-time key
    /// is compared to each exactly.
    pub fn reach(&self, base: &str, tail: &str, counted: bool) -> (Vec<String>, Vec<String>) {
        let tail = tail.trim_matches('.');
        let stem = base.trim_end_matches('.');
        let under = |k: &str| k.starts_with(base) || k == stem;
        let mut nodes: Vec<String> = Vec::new();
        // Not every key filtered: the ones under `base` sit together in the sorted map, and `stem` (base without its dot) sorts before them.
        let within = self.entries.range(base.to_string()..).map(|(k, _)| k).take_while(|k| k.starts_with(base));
        let at_stem = self.entries.get_key_value(stem).map(|(k, _)| k).filter(|k| !k.starts_with(base));
        for key in at_stem.into_iter().chain(within) {
            let mut node = key.as_str();
            nodes.push(node.to_string());
            while let Some((parent, _)) = node.rsplit_once('.') {
                if !under(parent) {
                    break;
                }
                nodes.push(parent.to_string());
                node = parent;
            }
        }
        nodes.sort();
        nodes.dedup();
        let (mut keys, mut subtrees) = (Vec::new(), Vec::new());
        for node in nodes {
            match self.entry(&node, counted) {
                Some(_) if node.ends_with(tail) => keys.push(node),
                Some(_) => {}
                None => subtrees.push(node),
            }
        }
        (keys, subtrees)
    }

    fn entry(&self, key: &str, counted: bool) -> Option<Translation> {
        if let Some(text) = self.lookup(key) {
            return Some(Translation::One(text.to_string()));
        }
        if !counted {
            return None;
        }
        let form = |f: &str| self.lookup(&format!("{key}.{f}")).map(str::to_string);
        Some(Translation::Plural { zero: form("zero"), one: form("one")?, other: form("other")? })
    }
}

/// `ActiveSupport::HtmlSafeTranslation.html_safe_translation_key?`: a
/// key ending in `html` after `_`, `.` or a word boundary.
pub fn is_html_safe_key(key: &str) -> bool {
    key == "html" || key.ends_with("_html") || key.ends_with(".html")
}

/// A template as `I18n.interpolate` reads it.
#[derive(Clone, Debug, PartialEq)]
pub enum Piece {
    Text(String),
    Name(String),
}

/// `%%` is a literal `%` and `%{name}` a value; a `%<name>` takes a
/// format, which no lowering applies, so it is refused.
pub fn pieces(template: &str) -> Result<Vec<Piece>, String> {
    let mut out = Vec::new();
    let mut text = String::new();
    let mut rest = template;
    while let Some(i) = rest.find('%') {
        text.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        if let Some(r) = after.strip_prefix('%') {
            text.push('%');
            rest = r;
            continue;
        }
        let named = |open: char, close: char| {
            let inner = after.strip_prefix(open)?;
            let j = inner.find(close)?;
            let name = &inner[..j];
            (!name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')).then(|| (name, &inner[j + 1..]))
        };
        if let Some((name, r)) = named('{', '}') {
            if !text.is_empty() {
                out.push(Piece::Text(std::mem::take(&mut text)));
            }
            out.push(Piece::Name(name.to_string()));
            rest = r;
        } else if named('<', '>').is_some() {
            return Err(format!("a formatted placeholder in {template:?}"));
        } else {
            text.push('%');
            rest = after;
        }
    }
    text.push_str(rest);
    if !text.is_empty() {
        out.push(Piece::Text(text));
    }
    Ok(out)
}

/// `template` interpolated as I18n does, a name `value` does not answer kept as written.
fn interpolate_text(template: &str, value: impl Fn(&str) -> Option<String>) -> String {
    match pieces(template) {
        Ok(pieces) => pieces
            .into_iter()
            .map(|p| match p {
                Piece::Text(t) => t,
                Piece::Name(n) => value(&n).unwrap_or_else(|| format!("%{{{n}}}")),
            })
            .collect(),
        Err(_) => template.to_string(),
    }
}

/// What one model's messages need from the catalog: its i18n scope
/// (`activerecord`, or `activemodel` for a plain ActiveModel class), its
/// `lookup_ancestors` as i18n keys, and the entries those can reach.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ModelI18n {
    pub scope: String,
    pub keys: Vec<String>,
    pub catalog: Catalog,
}

impl ModelI18n {
    pub fn is_default(&self) -> bool {
        *self == ModelI18n::default()
    }

    fn scope(&self) -> &str {
        if self.scope.is_empty() { "activerecord" } else { &self.scope }
    }

    fn catalog(&self) -> &Catalog {
        self.catalog.or_rails_default()
    }

    pub fn model_human_name(&self) -> String {
        self.catalog().model_human_name(self.scope(), &self.keys)
    }

    pub fn human_attribute_name(&self, attr: &str) -> String {
        self.catalog().human_attribute_name(self.scope(), &self.keys, attr)
    }

    pub fn full_message(&self, attr: &str, message: &str) -> String {
        self.catalog().full_message(self.scope(), &self.keys, attr, message)
    }

    pub fn errors_format(&self) -> &str {
        self.catalog().errors_format()
    }

    /// Each attribute the locale names for this model, with that name.
    /// The model's own error messages, in Rails' lookup order: under each
    /// of its keys (its own, then its ancestors'), `<scope>.errors.models.
    /// <key>.attributes.<attr>.<kind>` as "<attr>.<kind>" and `<scope>.
    /// errors.models.<key>.<kind>` as "<kind>", with any plural form's
    /// suffix (".one", ".other", ".zero"). An earlier key's entry wins.
    pub fn error_overrides(&self) -> Vec<(String, String)> {
        let scope = self.scope();
        let mut out: Vec<(String, String)> = Vec::new();
        for key in &self.keys {
            let prefix = format!("{scope}.errors.models.{key}.");
            for (entry, text) in &self.catalog().entries {
                let Some(rest) = entry.strip_prefix(prefix.as_str()) else { continue };
                let name = match rest.strip_prefix("attributes.") {
                    Some(attr_kind) => attr_kind.to_string(),
                    None => rest.to_string(),
                };
                if !out.iter().any(|(k, _)| *k == name) {
                    out.push((name, text.clone()));
                }
            }
        }
        out
    }

    /// `activerecord`, or `activemodel` for a plain ActiveModel class.
    pub fn error_scope(&self) -> &str {
        self.scope()
    }

    pub fn translated_attribute_names(&self) -> Vec<(String, String)> {
        let scope = self.scope();
        let prefixes: Vec<String> =
            self.keys.iter().map(|k| format!("{scope}.attributes.{k}.")).chain(std::iter::once("attributes.".to_string())).collect();
        let mut attrs: Vec<String> = self
            .catalog()
            .entries
            .keys()
            .filter_map(|key| prefixes.iter().find_map(|p| key.strip_prefix(p.as_str())))
            .filter(|attr| !attr.contains('.'))
            .map(str::to_string)
            .collect();
        attrs.sort();
        attrs.dedup();
        attrs
            .into_iter()
            .map(|attr| {
                let name = self.human_attribute_name(&attr);
                (attr, name)
            })
            .collect()
    }

    /// The message for an error of `kind` on `attr`, without the attribute, as parts.
    pub fn error(&self, attr: &str, kind: &str, opts: &ErrorOpts) -> Vec<Part> {
        self.catalog()
            .error_message(self.scope(), &self.keys, attr, kind, opts)
            .unwrap_or_else(|| vec![Part::Text(format!("translation missing: {}.errors.messages.{kind}", self.catalog().locale))])
    }

    /// The full message for an error of `kind` on `attr`, as parts.
    pub fn full_error(&self, attr: &str, kind: &str, opts: &ErrorOpts) -> Vec<Part> {
        let message = self.error(attr, kind, opts);
        if attr == "base" {
            return message;
        }
        let name = self.human_attribute_name(attr);
        let format = self.errors_format().replace("%{attribute}", &name);
        let (before, after) = format.split_once("%{message}").unwrap_or((format.as_str(), ""));
        let mut parts = vec![Part::Text(before.to_string())];
        parts.extend(message);
        parts.push(Part::Text(after.to_string()));
        merge_text(parts)
    }
}

fn merge_text(parts: Vec<Part>) -> Vec<Part> {
    let mut out: Vec<Part> = Vec::new();
    for part in parts {
        match (out.last_mut(), part) {
            (_, Part::Text(t)) if t.is_empty() => {}
            (Some(Part::Text(prev)), Part::Text(t)) => prev.push_str(&t),
            (_, part) => out.push(part),
        }
    }
    out
}

/// What form helpers translate: the app's catalog, and each model's i18n
/// by the param key a form names it with (`article`, `admin_user`).
#[derive(Clone, Debug, Default)]
pub struct FormI18n {
    catalog: Catalog,
    models: crate::det::HashMap<String, ModelI18n>,
}

impl FormI18n {
    pub fn of(app: &crate::app::App) -> Self {
        FormI18n {
            catalog: app.i18n.clone(),
            models: app
                .models
                .iter()
                .map(|m| (crate::naming::underscore(m.name.0.as_str()).replace('/', "_"), m.i18n.clone()))
                .collect(),
        }
    }

    /// `form.label :method`'s text, as `Tags::Label#translation` finds it:
    /// `helpers.label.<object_name>.<method>`, then the model's
    /// `helpers.label.<i18n_key>.<method>`, then its `human_attribute_name`,
    /// then `method.humanize`.
    pub fn label(&self, object_name: &str, method: &str) -> String {
        let catalog = self.catalog.or_rails_default();
        let model = self.models.get(object_name);
        let keys = std::iter::once(format!("helpers.label.{object_name}.{method}"))
            .chain(model.and_then(|m| m.keys.first()).map(|k| format!("helpers.label.{k}.{method}")));
        keys.filter_map(|k| catalog.lookup(&k))
            .find(|t| !t.is_empty())
            .map(str::to_string)
            .or_else(|| model.map(|m| m.human_attribute_name(method)))
            .unwrap_or_else(|| humanize(method))
    }

    /// Whether the form's record is one of the app's models, or a record
    /// nested under one, so it answers `errors` as Rails' wrapper requires.
    pub fn has_errors(&self, object_name: &str) -> bool {
        self.models.contains_key(object_name.split('[').next().unwrap_or(object_name))
    }

    /// The app's own `field_error_proc`, which this wrapper does not model.
    pub fn field_error_proc(&self) -> Option<&str> {
        self.catalog.field_error_proc.as_deref()
    }

    /// `form.submit`'s default text for `key` (`create`/`update`), as
    /// `submit_default_value` finds it: `helpers.submit.<object_name>.<key>`,
    /// `helpers.submit.<key>`, then "<Key> <model>", over the model's human name.
    pub fn submit(&self, object_name: &str, key: &str) -> String {
        let catalog = self.catalog.or_rails_default();
        let model = self
            .models
            .get(object_name)
            .map(|m| m.model_human_name())
            .unwrap_or_else(|| humanize(object_name));
        [format!("helpers.submit.{object_name}.{key}"), format!("helpers.submit.{key}")]
            .iter()
            .find_map(|k| catalog.lookup(k))
            .map(|t| t.replace("%{model}", &model))
            .unwrap_or_else(|| format!("{} {model}", humanize(key)))
    }
}

/// Every app model's `lookup_ancestors` as i18n keys and its scope:
/// itself, then each parent that is an app model, `ApplicationRecord`
/// aside (abstract). A chain that never reaches Active Record is a plain
/// ActiveModel class, looked up under `activemodel`.
pub fn model_i18n_keys(
    app: &crate::app::App,
) -> crate::det::HashMap<crate::ident::ClassId, (String, Vec<String>)> {
    let by_name: crate::det::HashMap<_, _> =
        app.models.iter().map(|m| (m.name.clone(), m)).collect();
    app.models
        .iter()
        .map(|m| {
            let mut keys = vec![i18n_key(m.name.0.as_str())];
            let mut record = false;
            let mut parent = m.parent.as_ref();
            while let Some(p) = parent {
                let name = p.0.as_str();
                if matches!(name, "ApplicationRecord" | "ActiveRecord::Base") {
                    record = true;
                    break;
                }
                let Some(model) = by_name.get(p) else { break };
                if keys.len() > 32 {
                    break;
                }
                keys.push(i18n_key(name));
                parent = model.parent.as_ref();
            }
            let scope = if record { "activerecord" } else { "activemodel" };
            (m.name.clone(), (scope.to_string(), keys))
        })
        .collect()
}

/// Stamp each model with the catalog entries its messages can reach.
pub fn stamp_models(app: &mut crate::app::App) {
    let keys = model_i18n_keys(app);
    for model in &mut app.models {
        let Some((scope, keys)) = keys.get(&model.name) else { continue };
        model.i18n = ModelI18n {
            scope: scope.clone(),
            keys: keys.clone(),
            catalog: app.i18n.subset_for(scope, keys),
        };
    }
}

/// `Admin::User` → `admin/user`, ActiveModel::Name#i18n_key.
pub fn i18n_key(class_name: &str) -> String {
    crate::naming::underscore(class_name)
}

/// ActiveSupport's `String#humanize` on an attribute name: drop leading
/// underscores and a trailing `_id`, `_` to spaces, words downcased,
/// first letter up.
pub fn humanize(attr: &str) -> String {
    let trimmed = attr.trim_start_matches('_');
    let trimmed = trimmed.strip_suffix("_id").unwrap_or(trimmed);
    let spaced = trimmed.replace('_', " ").to_lowercase();
    let mut chars = spaced.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

struct Flat<'a> {
    entries: &'a mut BTreeMap<String, String>,
    opaque: &'a mut std::collections::BTreeSet<String>,
    values: &'a mut BTreeMap<String, Opaque>,
    colons: &'a Colons,
    order: &'a mut BTreeMap<String, Vec<String>>,
}

impl Flat<'_> {
    /// A leaf at `prefix` replaces whatever subtree stood there.
    fn clear_below(&mut self, prefix: &str) {
        let nested = format!("{prefix}.");
        let stale: Vec<String> = self.entries.range(nested.clone()..).take_while(|(k, _)| k.starts_with(&nested)).map(|(k, _)| k.clone()).collect();
        for k in stale {
            self.entries.remove(&k);
        }
        let stale: Vec<String> = self.opaque.range(nested.clone()..).take_while(|k| k.starts_with(&nested)).cloned().collect();
        for k in stale {
            self.opaque.remove(&k);
            self.values.remove(&k);
        }
        let stale: Vec<String> = self.order.range(nested.clone()..).take_while(|(k, _)| k.starts_with(&nested)).map(|(k, _)| k.clone()).collect();
        for k in stale {
            self.order.remove(&k);
        }
        self.order.remove(prefix);
    }
}

/// Deep-merge `value` under `prefix`: a leaf replaces whatever subtree
/// stood at its key, as a later file's string replaces an earlier hash.
fn flatten(value: &Value, prefix: String, out: &mut Flat<'_>) {
    match value {
        Value::Mapping(map) => {
            out.entries.remove(&prefix);
            out.opaque.remove(&prefix);
            out.values.remove(&prefix);
            // An empty mapping is still a node: I18n answers `{}` for it rather than falling back.
            if !prefix.is_empty() {
                out.order.entry(prefix.clone()).or_default();
            }
            for (k, v) in map {
                let key = match k {
                    Value::String(s) => s.clone(),
                    Value::Bool(b) => b.to_string(),
                    Value::Number(n) => n.to_string(),
                    _ => continue,
                };
                let children = out.order.entry(prefix.clone()).or_default();
                if !children.contains(&key) {
                    children.push(key.clone());
                }
                let path = if prefix.is_empty() { key } else { format!("{prefix}.{key}") };
                flatten(v, path, out);
            }
        }
        Value::String(s) => {
            out.clear_below(&prefix);
            out.opaque.remove(&prefix);
            out.values.remove(&prefix);
            out.entries.insert(prefix, s.clone());
        }
        Value::Tagged(t) => flatten(&t.value, prefix, out),
        // A nil value: I18n finds nothing there, as for a missing key.
        Value::Null => {
            out.clear_below(&prefix);
            out.entries.remove(&prefix);
            out.opaque.remove(&prefix);
            out.values.remove(&prefix);
        }
        _ => {
            out.clear_below(&prefix);
            out.entries.remove(&prefix);
            match Opaque::from_yaml(value, out.colons) {
                Some(v) => out.values.insert(prefix.clone(), v),
                None => out.values.remove(&prefix),
            };
            out.opaque.insert(prefix);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog(files: &[(&str, &str)]) -> Catalog {
        let mut files = files.iter().map(|(p, s)| (p.to_string(), s.to_string())).collect();
        let (catalog, problems) = Catalog::load("en", &mut files);
        assert!(problems.is_empty(), "{problems:?}");
        catalog
    }

    fn keys(k: &[&str]) -> Vec<String> {
        k.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn an_opaque_value_keeps_what_psych_loads() {
        let c = catalog(&[(
            "config/locales/en.yml",
            "en:\n  plain:\n    - :year\n    - day\n  quoted:\n    - \":month\"\n    - ':day'\n  both:\n    - \":x\"\n    - :x\n  big: 18446744073709551615\n  ratio: 1.5\n  count: 3\n  split: [\"a\\x1Fb\"]\n",
        )]);
        assert_eq!(c.values.get("plain"), Some(&Opaque::Array(vec![(true, "year".into()), (false, "day".into())])));
        assert_eq!(c.values.get("quoted"), Some(&Opaque::Array(vec![(false, ":month".into()), (false, ":day".into())])));
        assert_eq!(c.values.get("ratio"), Some(&Opaque::Float(1.5)));
        assert_eq!(c.values.get("count"), Some(&Opaque::Int(3)));
        for key in ["both", "big", "split"] {
            assert!(c.opaque.contains(key) && !c.values.contains_key(key), "{key}");
        }
    }

    #[test]
    fn later_files_override_and_merge_keys_apply() {
        let c = catalog(&[
            ("config/locales/b.yml", "en:\n  activerecord:\n    attributes:\n      user_profile:\n        bio_raw: About Me\n"),
            ("config/locales/a.yml", "en:\n  errors: &errors\n    format: \"%{attribute}: %{message}\"\n  activerecord:\n    errors:\n      <<: *errors\n    attributes:\n      user_profile:\n        bio_raw: Bio\n"),
        ]);
        let k = keys(&["user_profile"]);
        assert_eq!(c.human_attribute_name("activerecord", &k, "bio_raw"), "About Me");
        assert_eq!(c.lookup("activerecord.errors.format"), Some("%{attribute}: %{message}"));
        assert_eq!(c.full_message("activerecord", &k, "bio_raw", "is long"), "About Me: is long");
        assert_eq!(c.full_message("activerecord", &k, "base", "Whole"), "Whole");
    }

    #[test]
    fn human_attribute_name_walks_ancestors_then_global_then_humanize() {
        let c = catalog(&[(
            "config/locales/en.yml",
            "en:\n  attributes:\n    created_at: Created\n  activerecord:\n    attributes:\n      post:\n        raw: Body\n",
        )]);
        assert_eq!(c.human_attribute_name("activerecord", &keys(&["reply", "post"]), "raw"), "Body");
        assert_eq!(c.human_attribute_name("activerecord", &keys(&["post"]), "created_at"), "Created");
        assert_eq!(c.human_attribute_name("activerecord", &keys(&["post"]), "author_id"), "Author");
        assert_eq!(c.human_attribute_name("activerecord", &keys(&["post"]), "first_name"), "First name");
        assert_eq!(c.human_attribute_name("activerecord", &keys(&["post"]), "URL"), "Url");
    }

    #[test]
    fn error_messages_follow_rails_lookup_and_pluralize() {
        let c = catalog(&[(
            "config/locales/en.yml",
            "en:\n  activerecord:\n    errors:\n      models:\n        post:\n          attributes:\n            title:\n              blank: \"needs a %{attribute} for %{model}\"\n          too_short: \"is short: %{value}\"\n    models:\n      post:\n        one: Article\n        other: Articles\n",
        )]);
        let k = keys(&["post"]);
        let text = |parts: Option<Vec<Part>>| parts.unwrap();
        assert_eq!(text(c.error_message("activerecord", &k, "title", "blank", &ErrorOpts::default())), vec![Part::Text("needs a Title for Article".into())]);
        assert_eq!(text(c.error_message("activerecord", &k, "body", "blank", &ErrorOpts::default())), vec![Part::Text("can't be blank".into())]);
        assert_eq!(text(c.error_message("activerecord", &k, "body", "too_short", &ErrorOpts::count(3))), vec![Part::Text("is short: ".into()), Part::Value]);
        assert_eq!(text(c.error_message("activerecord", &k, "body", "too_long", &ErrorOpts::count(1))), vec![Part::Text("is too long (maximum is 1 character)".into())]);
        assert_eq!(text(c.error_message("activerecord", &k, "body", "too_long", &ErrorOpts::count(5))), vec![Part::Text("is too long (maximum is 5 characters)".into())]);
        assert_eq!(text(c.error_message("activerecord", &k, "author", "required", &ErrorOpts::default())), vec![Part::Text("must exist".into())]);
        let custom = ErrorOpts { count: Some("4".into()), message: Some("needs %{count}".into()), attribute: None };
        assert_eq!(text(c.error_message("activerecord", &k, "body", "too_short", &custom)), vec![Part::Text("needs 4".into())]);
        assert_eq!(text(c.error_message("activerecord", &k, "body", "greater_than", &ErrorOpts::count("0.5"))), vec![Part::Text("must be greater than 0.5".into())]);
    }

    #[test]
    fn translate_resolves_scope_lazy_keys_defaults_and_plurals() {
        let c = catalog(&[(
            "config/locales/en.yml",
            "en:\n  hello: \"Hello world\"\n  greet: \"Hi %{name}\"\n  inbox:\n    one: \"1 message\"\n    other: \"%{count} messages\"\n  articles:\n    form:\n      title: \"Write\"\n",
        )]);
        let t = |key: &str| Translate { key: key.into(), ..Default::default() };
        assert_eq!(c.resolve(&t("hello")), Ok(Translation::One("Hello world".into())));
        assert_eq!(c.resolve(&Translate { scope: Some("articles.form".into()), ..t("title") }), Ok(Translation::One("Write".into())));
        assert_eq!(c.resolve(&Translate { view: Some("articles/_form".into()), ..t(".title") }), Ok(Translation::One("Write".into())));
        assert_eq!(c.resolve(&Translate { defaults: vec![Fallback::Key("hello".into())], ..t("nope") }), Ok(Translation::One("Hello world".into())));
        assert_eq!(c.resolve(&Translate { defaults: vec![Fallback::Text("Fallback".into())], ..t("nope") }), Ok(Translation::One("Fallback".into())));
        assert_eq!(
            c.resolve(&Translate { counted: true, ..t("inbox") }),
            Ok(Translation::Plural { zero: None, one: "1 message".into(), other: "%{count} messages".into() })
        );
        assert!(c.resolve(&Translate { values: vec!["other".into()], ..t("greet") }).is_err());
        assert!(c.resolve(&Translate { values: vec!["name".into()], ..t("greet") }).is_ok());
        let missing = |considered: &[&str]| Translation::Missing { key: "en.nope".into(), considered: keys(considered) };
        assert_eq!(c.resolve(&t("nope")), Ok(missing(&[])));
        let also = vec![Fallback::Key("also_nope".into())];
        assert_eq!(c.resolve(&Translate { defaults: also.clone(), ..t("nope") }), Ok(missing(&[])));
        assert_eq!(c.resolve(&Translate { defaults: also, default_list: true, scope: None, ..t("nope") }), Ok(missing(&["en.also_nope"])));
        let unknown = Catalog { missing: MissingPolicy { unknown: Some("`:strict`".into()), ..Default::default() }, ..c.clone() };
        assert!(unknown.resolve(&t("nope")).is_err());
        assert!(c.resolve(&t(".title")).is_err());
    }

    #[test]
    fn a_lookup_stops_at_the_first_candidate_that_resolves() {
        let c = catalog(&[("config/locales/en.yml", "en:\n  title: \"Hello\"\n  list:\n    - a\n    - b\n")]);
        let t = |key: &str| Translate { key: key.into(), ..Default::default() };
        assert_eq!(c.resolve(&Translate { defaults: vec![Fallback::Key("list".into())], ..t("title") }), Ok(Translation::One("Hello".into())));
        assert!(c.resolve(&Translate { defaults: vec![Fallback::Key("title".into())], ..t("list") }).is_err());
    }

    #[test]
    fn a_leaf_replacing_a_subtree_drops_its_nested_nodes() {
        let c = catalog(&[
            ("config/locales/a.yml", "en:\n  foo:\n    bar:\n      title: \"T\"\n"),
            ("config/locales/b.yml", "en:\n  foo: \"plain\"\n"),
        ]);
        assert!(!c.is_subtree("foo.bar"));
        assert!(!c.is_subtree("foo"));
    }

    #[test]
    fn an_empty_mapping_is_a_node_in_the_files_order() {
        let c = catalog(&[("config/locales/en.yml", "en:\n  feed: {}\n  menu:\n    more: {}\n    home: \"Home\"\n")]);
        assert!(c.is_subtree("feed"));
        assert!(c.plain_subtree("feed"));
        assert!(c.plain_subtree("menu"));
        let home = "Home".to_string();
        let ours: Vec<_> = c.ordered_nodes().into_iter().filter(|(k, _)| k == "feed" || k.starts_with("menu.")).collect();
        assert_eq!(ours, vec![("feed".to_string(), None), ("menu.more".to_string(), None), ("menu.home".to_string(), Some(&home))]);
    }

    #[test]
    fn interpolation_reads_escaped_percents_and_refuses_formats() {
        assert_eq!(
            pieces("100%% %%{x} %{name}%"),
            Ok(vec![Piece::Text("100% %{x} ".into()), Piece::Name("name".into()), Piece::Text("%".into())])
        );
        assert!(pieces("%<n>05d").is_err());
        let c = catalog(&[("config/locales/en.yml", "en:\n  greet: \"Hi %{name}\"\n  fmt: \"%<n>05d\"\n")]);
        let t = |key: &str| Translate { key: key.into(), ..Default::default() };
        assert_eq!(c.resolve(&t("greet")), Ok(Translation::One("Hi %{name}".into())));
        assert!(c.resolve(&Translate { values: vec!["n".into()], ..t("fmt") }).is_err());
        assert_eq!(format_full("%%{attribute} %{attribute}: %{message}", "Title", "is short"), "%{attribute} Title: is short");
    }

    #[test]
    fn a_single_quoted_locale_key_is_read_and_a_broken_file_reported() {
        let c = catalog(&[("config/locales/en.yml", "'en':\n  hello: Hi\n")]);
        assert_eq!(c.lookup("hello"), Some("Hi"));
        let mut files = vec![("config/locales/bad.yml".to_string(), "en:\n  a: [\n".to_string())];
        let (_, problems) = Catalog::load("en", &mut files);
        assert_eq!(problems.len(), 1);
        assert_eq!(problems[0].0, "config/locales/bad.yml");
    }

    #[test]
    fn a_run_time_key_reaches_the_translations_under_its_prefix() {
        let c = catalog(&[(
            "config/locales/en.yml",
            "en:\n  vis:\n    public: Everyone\n    group:\n      staff: Staff\n    inbox:\n      one: \"1\"\n      other: \"%{count}\"\n  visitor: Guest\n",
        )]);
        let (keys, subtrees) = c.reach("vis.", "", false);
        assert_eq!(keys, vec!["vis.group.staff", "vis.inbox.one", "vis.inbox.other", "vis.public"]);
        assert_eq!(subtrees, vec!["vis", "vis.group", "vis.inbox"]);
        let (keys, subtrees) = c.reach("vis.", "", true);
        assert!(keys.contains(&"vis.inbox".to_string()) && !subtrees.contains(&"vis.inbox".to_string()));
        assert_eq!(c.reach("vis.", ".staff", false).0, vec!["vis.group.staff"]);
        assert!(c.resolve(&Translate { key: "vis.group".into(), ..Default::default() }).is_err());
    }

    /// i18n 1.15.2 with `I18n.fallbacks = I18n::Locale::Fallbacks.new(:en)`:
    /// the key in the locale, then the Symbol defaults across the chain,
    /// then the key in the later locales, then the text default.
    #[test]
    fn fallbacks_follow_the_gem_s_order() {
        assert_eq!(fallback_chain("pt-BR", &keys(&["en"])), keys(&["pt-BR", "pt", "en"]));
        assert_eq!(fallback_chain("en", &keys(&["en"])), keys(&["en"]));
        let mut files = vec![
            ("config/locales/en.yml".to_string(), "en:\n  a: A-en\n  b: B-en\n".to_string()),
            ("config/locales/pl.yml".to_string(), "pl:\n  b: B-pl\n  d: D-pl\n".to_string()),
        ];
        let (mut c, _) = Catalog::load("en", &mut files);
        let policy = FallbackPolicy { app: Some(keys(&["en"])), ..Default::default() };
        c.load_locales(keys(&["en", "pl"]), policy, &[], &mut files);
        let en = Some(&["en".to_string()][..]);
        let t = |key: &str, defaults: Vec<Fallback>| Translate { key: key.into(), defaults, ..Default::default() };
        let one = |s: &str| Ok(Translation::One(s.into()));
        assert_eq!(c.resolve_in("pl", en, &t("a", vec![])), one("A-en"));
        assert_eq!(c.resolve_in("pl", None, &t("a", vec![])), Ok(Translation::Missing { key: "pl.a".into(), considered: vec![] }));
        assert_eq!(c.resolve_in("pl", en, &t("a", vec![Fallback::Key("d".into())])), one("D-pl"));
        assert_eq!(c.resolve_in("pl", en, &t("a", vec![Fallback::Key("b".into())])), one("B-pl"));
        assert_eq!(c.resolve_in("pl", en, &t("x", vec![Fallback::Key("y".into()), Fallback::Text("txt".into())])), one("txt"));
        assert!(c.resolve_in("de", en, &t("a", vec![])).is_err());
    }

    #[test]
    fn a_later_string_replaces_an_earlier_subtree() {
        let c = catalog(&[(
            "config/locales/en.yml",
            "en:\n  errors:\n    messages:\n      too_long: \"is long\"\n",
        )]);
        assert_eq!(c.translate("errors.messages.too_long", Some(5)), Some("is long"));
        assert_eq!(c.lookup("errors.messages.too_long.other"), None);
    }

    /// `subset_for` seeks each prefix's run of the sorted keys; it keeps what
    /// a test of every key against every prefix kept, including keys that
    /// share a leading run with a prefix without matching it.
    #[test]
    fn subset_for_seeks_the_same_entries_a_scan_of_every_key_keeps() {
        let c = catalog(&[(
            "config/locales/en.yml",
            "en:\n  attributes:\n    name: Name\n  errors:\n    format: \"%{attribute} %{message}\"\n    messages:\n      blank: blank\n    attributes:\n      base: Base\n    models:\n      user:\n        x: ux\n      user_group:\n        x: gx\n  activerecord:\n    attributes:\n      user:\n        name: User name\n      user_group:\n        name: Group name\n      account:\n        name: Account name\n    errors:\n      messages:\n        taken: taken\n      models:\n        user:\n          attributes:\n            name:\n              blank: no name\n    models:\n      user: User\n      user_group: Group\n      users: Users\n  zz: last\n",
        )]);
        let wanted = |scope: &str, keys: &[String]| {
            let mut prefixes: Vec<String> = keys
                .iter()
                .flat_map(|k| {
                    [format!("{scope}.attributes.{k}."), format!("{scope}.errors.models.{k}."), format!("{scope}.models.{k}")]
                })
                .collect();
            prefixes.extend(["attributes.", "errors.format", "errors.messages.", "errors.attributes."].map(str::to_string));
            prefixes.push(format!("{scope}.errors.messages."));
            c.entries
                .iter()
                .filter(|(k, _)| prefixes.iter().any(|p| k.starts_with(p.as_str())))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect::<BTreeMap<_, _>>()
        };
        for keys in [keys(&[]), keys(&["user"]), keys(&["user", "user_group"]), keys(&["account", "absent"])] {
            assert_eq!(c.subset_for("activerecord", &keys).entries, wanted("activerecord", &keys), "{keys:?}");
        }
        assert!(c.subset_for("activerecord", &keys(&["user"])).entries.contains_key("activerecord.models.users"));
        assert!(!c.subset_for("activerecord", &keys(&["user"])).entries.contains_key("zz"));
    }
}
