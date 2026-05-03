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

/// Method name: snake_case preservation. Ruby's `?` (predicate)
/// and `!` (bang) suffixes get renamed instead of silently
/// stripped — silent strip merges three namespaces and creates
/// collisions with same-named fields. `?` → prepend `is_`; `!` →
/// suffix `_bang`. Same rule as `library::sanitize_identifier`
/// for definition emit, so call sites and definitions line up.
pub(super) fn ts_method_name(ruby_name: &str) -> String {
    if let Some(stem) = ruby_name.strip_suffix('?') {
        return format!("is_{stem}");
    }
    if let Some(stem) = ruby_name.strip_suffix('!') {
        return format!("{stem}_bang");
    }
    ruby_name.to_string()
}
