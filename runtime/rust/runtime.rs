//! Roundhouse Rust runtime.
//!
//! Hand-written Rust shipped alongside each generated app. Roundhouse's
//! Rust emitter copies this file into the generated project as
//! `src/runtime.rs`, so generated code can reference it as
//! `crate::runtime::ValidationError` et al. The file is intentionally
//! minimal — the Phase 4 architectural bet is that most logic lives
//! as IR-level lowerings rendered inline by each target's emitter, so
//! the per-target runtime shrinks to "adapters around target-specific
//! primitives" rather than a mini-framework.
//!
//! Current contents: `ValidationError` only. Future additions (the
//! Model trait, DB adapter wrapping rusqlite, HTTP adapter wrapping
//! axum, ActionCable adapter over tokio-broadcast) land in separate
//! runtime files as each lowering/emit step demands them.

// ── Validation ──

/// A single validation failure produced by a model's generated
/// `validate()` method. Carries the attribute name and a
/// human-readable message; `full_message()` composes them into a
/// Rails-compatible display string (`"Title can't be blank"`).
///
/// Generated code constructs these inline — the lowered IR plus the
/// target-specific render in `src/emit/rust.rs` is what produces each
/// `push(ValidationError::new("field", "message"))` call.
#[derive(Debug, Clone, PartialEq)]
pub struct ValidationError {
    pub field: String,
    pub message: String,
}

impl ValidationError {
    /// Constructor matching the one generated Rust calls at each
    /// check site. Accepts `&str` for both arguments so the emitted
    /// code can pass string literals directly without `.to_string()`
    /// noise.
    pub fn new(field: &str, message: &str) -> Self {
        Self {
            field: field.to_string(),
            message: message.to_string(),
        }
    }

    /// Rails-compatible display form: capitalize the field name,
    /// replace underscores with spaces, prepend to the message.
    /// `ValidationError::new("post_id", "can't be blank")` becomes
    /// `"Post id can't be blank"`.
    pub fn full_message(&self) -> String {
        let mut field = self.field.replace('_', " ");
        if let Some(first) = field.get_mut(0..1) {
            first.make_ascii_uppercase();
        }
        format!("{} {}", field, self.message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation_error_stores_field_and_message() {
        let err = ValidationError::new("title", "can't be blank");
        assert_eq!(err.field, "title");
        assert_eq!(err.message, "can't be blank");
    }

    #[test]
    fn full_message_capitalizes_and_expands_underscores() {
        let err = ValidationError::new("post_id", "can't be blank");
        assert_eq!(err.full_message(), "Post id can't be blank");
    }

    #[test]
    fn full_message_handles_single_word_fields() {
        let err = ValidationError::new("title", "is invalid");
        assert_eq!(err.full_message(), "Title is invalid");
    }

    #[test]
    fn full_message_handles_empty_field() {
        let err = ValidationError::new("", "bad");
        assert_eq!(err.full_message(), " bad");
    }
}
