//! Naming helpers shared across TypeScript emit modules. Method/field
//! names preserve Ruby's snake_case (Juntos's ActiveRecord accessors
//! match Rails column names exactly, and ruby2js's rails model filter
//! does the same).

/// Instance-field name: preserves snake_case. Juntos's ActiveRecord
/// accessors match the Rails column names exactly (`article_id`, not
/// `articleId`), and ruby2js's rails model filter does the same.
/// Single-word idiomatic JS (`title`) is the same either way; the
/// difference is only visible on multi-word names.
pub(super) fn ts_field_name(ruby_name: &str) -> String {
    ruby_name.to_string()
}

/// Method name: same snake_case preservation as fields. Method calls
/// that should resolve to JS-native APIs (e.g. `findBy` vs Ruby's
/// `find_by`) will need a per-method translation table later; until
/// then, the Rails-side name survives and Juntos maps at runtime.
pub(super) fn ts_method_name(ruby_name: &str) -> String {
    ruby_name.to_string()
}
