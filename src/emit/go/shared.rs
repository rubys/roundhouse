//! Cross-cutting helpers used by multiple Go emit modules.
//!
//! Naming conversions live here because they're called from every
//! per-output-kind emitter (model, controller, view, route, test, …).

pub(super) fn pascalize_word(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(c) => c.to_ascii_uppercase().to_string() + chars.as_str(),
        None => String::new(),
    }
}

pub(super) fn go_field_name(ruby_name: &str) -> String {
    // Convert snake_case to PascalCase, special-casing common initialisms.
    ruby_name
        .split('_')
        .map(|part| match part {
            "id" => "ID".to_string(),
            "url" => "URL".to_string(),
            "http" => "HTTP".to_string(),
            "json" => "JSON".to_string(),
            "html" => "HTML".to_string(),
            "api" => "API".to_string(),
            other => pascalize_word(other),
        })
        .collect::<String>()
}

/// Convert a snake_case Ruby parameter name to a Go camelCase
/// identifier with the standard initialism rules. `article_id` →
/// `articleID`. Used for route-helper parameters and any other
/// place where Go's local-var naming convention applies.
pub(super) fn go_param_name(ruby: &str) -> String {
    let mut out = String::new();
    for (i, part) in ruby.split('_').enumerate() {
        let upper = match part {
            "id" => "ID".to_string(),
            "url" => "URL".to_string(),
            "http" => "HTTP".to_string(),
            "json" => "JSON".to_string(),
            "html" => "HTML".to_string(),
            "api" => "API".to_string(),
            other => pascalize_word(other),
        };
        if i == 0 {
            // Lowercase the first chunk to keep it unexported.
            out.push_str(&upper.to_ascii_lowercase());
        } else {
            out.push_str(&upper);
        }
    }
    out
}

pub(super) fn go_method_name(ruby_name: &str) -> String {
    ruby_name
        .split('_')
        .map(|part| match part {
            "id" => "ID".to_string(),
            other => pascalize_word(other),
        })
        .collect::<String>()
}

pub(super) fn go_action_handler_name(controller: &str, action: &str) -> String {
    let base = controller.strip_suffix("Controller").unwrap_or(controller);
    format!("{}{}", base, pascalize_word(action))
}
