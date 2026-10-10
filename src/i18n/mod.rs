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

/// Rails 8.1.4's bundled English, in the order its frameworks load it.
const RAILS_EN: &[(&str, &str)] = &[
    ("activesupport", include_str!("rails/activesupport.en.yml")),
    ("activemodel", include_str!("rails/activemodel.en.yml")),
    ("activerecord", include_str!("rails/activerecord.en.yml")),
    ("actionview", include_str!("rails/actionview.en.yml")),
];

pub const DEFAULT_ERRORS_FORMAT: &str = "%{attribute} %{message}";

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Catalog {
    /// `config.i18n.default_locale`, `en` when the app sets none.
    pub locale: String,
    /// Dotted key (`activerecord.attributes.user.email`) → string leaf.
    pub entries: BTreeMap<String, String>,
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
    pub fn load(locale: &str, files: &mut Vec<(String, String)>) -> (Catalog, Vec<String>) {
        files.sort_by(|a, b| a.0.cmp(&b.0));
        let mut catalog = Catalog { locale: locale.to_string(), entries: BTreeMap::new() };
        let mut problems = Vec::new();
        let rails = RAILS_EN.iter().map(|(name, src)| (name.to_string(), src.to_string()));
        for (path, source) in rails.collect::<Vec<_>>().iter().chain(files.iter()) {
            if let Err(e) = catalog.merge_source(source) {
                problems.push(format!("{path}: {e}"));
            }
        }
        (catalog, problems)
    }

    fn merge_source(&mut self, source: &str) -> Result<(), String> {
        let top = format!("{}:", self.locale);
        let quoted = format!("\"{}\":", self.locale);
        // Not parsed: a file with no top-level key for this locale adds nothing to it.
        if !source.lines().any(|l| l.trim_end().starts_with(&top) || l.starts_with(&quoted)) {
            return Ok(());
        }
        let mut value: Value = serde_yaml_ng::from_str(source).map_err(|e| e.to_string())?;
        value.apply_merge().map_err(|e| e.to_string())?;
        if let Some(tree) = value.get(self.locale.as_str()) {
            flatten(tree, String::new(), &mut self.entries);
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
        let entries = self
            .entries
            .iter()
            .filter(|(k, _)| prefixes.iter().any(|p| k.starts_with(p.as_str())))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        Catalog { locale: self.locale.clone(), entries }
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
        let text = template
            .replace("%{attribute}", &attribute)
            .replace("%{model}", &self.model_human_name(scope, keys))
            .replace("%{count}", opts.count.as_deref().unwrap_or(""));
        let mut parts = Vec::new();
        for (i, piece) in text.split("%{value}").enumerate() {
            if i > 0 {
                parts.push(Part::Value);
            }
            if !piece.is_empty() {
                parts.push(Part::Text(piece.to_string()));
            }
        }
        Some(parts)
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
    format.replace("%{attribute}", attribute).replace("%{message}", message)
}

/// A `t` / `I18n.t` call reduced to what its lookup needs.
#[derive(Clone, Debug, Default)]
pub struct Translate {
    /// The key as written; a leading `.` is a view's lazy lookup.
    pub key: String,
    /// The template the call sits in (`articles/_form`), for a lazy key.
    pub view: Option<String>,
    /// `scope:`, dotted.
    pub scope: Option<String>,
    /// `default:` entries in order: a key (`Symbol`) or literal text.
    pub defaults: Vec<Fallback>,
    /// `count:` given.
    pub counted: bool,
    /// The other keywords, each an interpolation value.
    pub values: Vec<String>,
}

#[derive(Clone, Debug)]
pub enum Fallback {
    Key(String),
    Text(String),
}

/// What a resolved translation renders: one template, or the plural
/// forms a run-time `count` picks between.
#[derive(Clone, Debug, PartialEq)]
pub enum Translation {
    One(String),
    Plural { zero: Option<String>, one: String, other: String },
}

impl Catalog {
    /// Resolve `call` as I18n's simple backend does, or say why it
    /// cannot be answered at compile time.
    pub fn resolve(&self, call: &Translate) -> Result<Translation, String> {
        let full = |key: &str| -> Result<String, String> {
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
        };
        let mut candidates: Vec<Result<String, String>> = vec![Ok(full(&call.key)?)];
        for default in &call.defaults {
            candidates.push(match default {
                Fallback::Key(k) => Ok(full(k)?),
                Fallback::Text(t) => Err(t.clone()),
            });
        }
        let found = candidates.iter().find_map(|c| match c {
            Ok(key) => self.entry(key, call.counted),
            Err(text) => Some(Translation::One(text.clone())),
        });
        let translation = found.ok_or_else(|| format!("translation missing: {}.{}", self.locale, full(&call.key).unwrap_or_default()))?;
        let templates: Vec<&String> = match &translation {
            Translation::One(t) => vec![t],
            Translation::Plural { zero, one, other } => zero.iter().chain([one, other]).collect(),
        };
        for template in templates {
            for name in placeholders(template) {
                let given = (name == "count" && call.counted) || call.values.iter().any(|v| v == &name);
                if !given {
                    return Err(format!("missing interpolation argument %{{{name}}}"));
                }
            }
        }
        Ok(translation)
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

/// The `%{name}`s in `template`.
pub fn placeholders(template: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = template;
    while let Some(i) = rest.find("%{") {
        let after = &rest[i + 2..];
        let Some(j) = after.find('}') else { break };
        out.push(after[..j].to_string());
        rest = &after[j + 1..];
    }
    out
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

    /// The full message for an error of `kind` on `attr`, as parts.
    pub fn full_error(&self, attr: &str, kind: &str, opts: &ErrorOpts) -> Vec<Part> {
        let message = self
            .catalog()
            .error_message(self.scope(), &self.keys, attr, kind, opts)
            .unwrap_or_else(|| vec![Part::Text(format!("translation missing: {}.errors.messages.{kind}", self.catalog().locale))]);
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
    models: std::collections::HashMap<String, ModelI18n>,
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
) -> std::collections::HashMap<crate::ident::ClassId, (String, Vec<String>)> {
    let by_name: std::collections::HashMap<_, _> =
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

/// Deep-merge `value` under `prefix`: a leaf replaces whatever subtree
/// stood at its key, as a later file's string replaces an earlier hash.
fn flatten(value: &Value, prefix: String, out: &mut BTreeMap<String, String>) {
    match value {
        Value::Mapping(map) => {
            out.remove(&prefix);
            for (k, v) in map {
                let key = match k {
                    Value::String(s) => s.clone(),
                    Value::Bool(b) => b.to_string(),
                    Value::Number(n) => n.to_string(),
                    _ => continue,
                };
                let path = if prefix.is_empty() { key } else { format!("{prefix}.{key}") };
                flatten(v, path, out);
            }
        }
        Value::String(s) => {
            let nested = format!("{prefix}.");
            let stale: Vec<String> = out.range(nested.clone()..).take_while(|(k, _)| k.starts_with(&nested)).map(|(k, _)| k.clone()).collect();
            for k in stale {
                out.remove(&k);
            }
            out.insert(prefix, s.clone());
        }
        Value::Tagged(t) => flatten(&t.value, prefix, out),
        _ => {}
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
        assert!(c.resolve(&t("greet")).is_err());
        assert!(c.resolve(&Translate { values: vec!["name".into()], ..t("greet") }).is_ok());
        assert!(c.resolve(&t("nope")).is_err());
        assert!(c.resolve(&t(".title")).is_err());
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
}
