//! `ActiveSupport` helpers the emitted code calls as `ActiveSupport::x(..)`.
//!
//! Covers the blank/present family (receivers of every static type the
//! emitter produces, `serde_json::Value` params included), `presence`,
//! `presence_in`, `squish`, `many?`, `sole` and `to_sentence`. Anything
//! else (`errors_for`, `Duration`, …) is deliberately absent so rustc
//! keeps naming it as unsupported rather than this module faking it.

use serde_json::Value;
use std::collections::HashMap;
use std::fmt::Display;

pub struct ActiveSupport;

/// Rails' `Object#blank?`: nil, false, empty collections and
/// whitespace-only strings. Numbers and everything else are present.
pub trait Blank {
    fn is_blank(&self) -> bool;
}

impl Blank for str {
    fn is_blank(&self) -> bool {
        self.trim().is_empty()
    }
}
impl Blank for String {
    fn is_blank(&self) -> bool {
        self.as_str().is_blank()
    }
}
impl Blank for bool {
    fn is_blank(&self) -> bool {
        !*self
    }
}
macro_rules! never_blank {
    ($($t:ty),*) => { $(impl Blank for $t { fn is_blank(&self) -> bool { false } })* };
}
never_blank!(i8, i16, i32, i64, isize, u8, u16, u32, u64, usize, f32, f64);

impl Blank for () {
    fn is_blank(&self) -> bool {
        true
    }
}
impl<T: Blank + ?Sized> Blank for &T {
    fn is_blank(&self) -> bool {
        (**self).is_blank()
    }
}
impl<T: Blank> Blank for Option<T> {
    fn is_blank(&self) -> bool {
        self.as_ref().is_none_or(|v| v.is_blank())
    }
}
impl<T> Blank for Vec<T> {
    fn is_blank(&self) -> bool {
        self.is_empty()
    }
}
impl<K, V> Blank for HashMap<K, V> {
    fn is_blank(&self) -> bool {
        self.is_empty()
    }
}
impl Blank for Value {
    fn is_blank(&self) -> bool {
        match self {
            Value::Null => true,
            Value::Bool(b) => !*b,
            Value::String(s) => s.is_blank(),
            Value::Array(a) => a.is_empty(),
            Value::Object(o) => o.is_empty(),
            Value::Number(_) => false,
        }
    }
}

impl ActiveSupport {
    pub fn blank_pred<T: Blank>(value: T) -> bool {
        value.is_blank()
    }

    pub fn present_pred<T: Blank>(value: T) -> bool {
        !value.is_blank()
    }

    /// `value.presence`: the value itself unless blank.
    pub fn presence<T: Blank>(value: T) -> Option<T> {
        if value.is_blank() { None } else { Some(value) }
    }

    /// `value.presence_in(list)`: the value when the list includes it.
    pub fn presence_in<T, U>(value: T, list: Vec<U>) -> Option<T>
    where
        T: PartialEq<U>,
    {
        if list.iter().any(|item| value == *item) { Some(value) } else { None }
    }

    /// `String#squish`: strip, and collapse whitespace runs to one space.
    pub fn squish(text: impl AsRef<str>) -> String {
        text.as_ref().split_whitespace().collect::<Vec<_>>().join(" ")
    }

    /// `Enumerable#many?` without a block: more than one element.
    pub fn many_pred<I: IntoIterator>(items: I) -> bool {
        items.into_iter().nth(1).is_some()
    }

    /// `Enumerable#sole`: the only element; panics on zero or several
    /// (Rails raises `Enumerable::SoleItemExpectedError`).
    pub fn sole<I: IntoIterator>(items: I) -> I::Item {
        let mut it = items.into_iter();
        match (it.next(), it.next()) {
            (Some(item), None) => item,
            _ => panic!("Enumerable::SoleItemExpectedError: expected exactly one item"),
        }
    }

    /// `Array#to_sentence` with the connectors the call site resolved.
    pub fn to_sentence<I>(items: I, words_connector: &str, two_words_connector: &str, last_word_connector: &str) -> String
    where
        I: IntoIterator,
        I::Item: Display,
    {
        let parts: Vec<String> = items.into_iter().map(|i| i.to_string()).collect();
        match parts.len() {
            0 => String::new(),
            1 => parts[0].clone(),
            2 => format!("{}{}{}", parts[0], two_words_connector, parts[1]),
            n => format!("{}{}{}", parts[..n - 1].join(words_connector), last_word_connector, parts[n - 1]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn blank_follows_rails() {
        assert!(ActiveSupport::blank_pred(json!(null)));
        assert!(ActiveSupport::blank_pred(json!("  ")));
        assert!(ActiveSupport::blank_pred(json!([])));
        assert!(ActiveSupport::blank_pred(None::<String>));
        assert!(ActiveSupport::blank_pred(false));
        assert!(ActiveSupport::present_pred(json!(0)));
        assert!(ActiveSupport::present_pred(Some("x".to_string())));
        assert!(ActiveSupport::present_pred(json!({"a": 1})));
    }

    #[test]
    fn helpers() {
        assert_eq!(ActiveSupport::presence("".to_string()), None);
        assert_eq!(ActiveSupport::presence_in(json!("member"), vec!["member", "administrator"]), Some(json!("member")));
        assert_eq!(ActiveSupport::presence_in(json!("x"), vec!["member"]), None);
        assert_eq!(ActiveSupport::squish("  a \n b\t"), "a b");
        assert!(ActiveSupport::many_pred(vec![1, 2]));
        assert!(!ActiveSupport::many_pred(vec![1]));
        assert_eq!(ActiveSupport::sole(vec![7]), 7);
        assert_eq!(ActiveSupport::to_sentence(vec!["a", "b", "c"], ", ", " and ", ", and "), "a, b, and c");
        assert_eq!(ActiveSupport::to_sentence(vec!["a", "b"], ", ", " and ", ", and "), "a and b");
    }
}
