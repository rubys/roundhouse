//! Rails naming conventions (snake_case, camelize, singular/plural).
//!
//! Deliberately naive. Real Rails uses `ActiveSupport::Inflector`'s rule
//! tables; we'll grow this as fixtures demand. If a test fails because of a
//! missed irregular plural, fix the rule here rather than working around it
//! in the caller.

pub fn snake_case(class_name: &str) -> String {
    let mut s = String::with_capacity(class_name.len() + 4);
    for (i, c) in class_name.char_indices() {
        if c.is_uppercase() && i > 0 {
            let prev = class_name.as_bytes()[i - 1] as char;
            if prev.is_lowercase() || prev.is_ascii_digit() {
                s.push('_');
            }
        }
        s.push(c.to_ascii_lowercase());
    }
    s
}

pub fn camelize(snake: &str) -> String {
    let mut out = String::with_capacity(snake.len());
    let mut upper_next = true;
    for c in snake.chars() {
        if c == '_' {
            upper_next = true;
        } else if upper_next {
            out.push(c.to_ascii_uppercase());
            upper_next = false;
        } else {
            out.push(c);
        }
    }
    out
}

pub fn pluralize_snake(class_name: &str) -> String {
    let snake = snake_case(class_name);
    if snake.ends_with('s') {
        format!("{snake}es")
    } else if let Some(stem) = snake.strip_suffix('y') {
        format!("{stem}ies")
    } else {
        format!("{snake}s")
    }
}

pub fn singularize(plural: &str) -> String {
    if let Some(s) = plural.strip_suffix("ies") {
        format!("{s}y")
    } else if let Some(s) = plural.strip_suffix("es") {
        s.to_string()
    } else if let Some(s) = plural.strip_suffix('s') {
        s.to_string()
    } else {
        plural.to_string()
    }
}

pub fn singularize_camelize(plural_symbol: &str) -> String {
    camelize(&singularize(plural_symbol))
}

pub fn habtm_join_table(owner_class: &str, target_plural_sym: &str) -> String {
    let a = pluralize_snake(owner_class);
    let b = target_plural_sym.to_string();
    if a < b { format!("{a}_{b}") } else { format!("{b}_{a}") }
}
