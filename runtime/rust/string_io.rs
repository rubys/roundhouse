//! Minimal owned string buffer for Rust output that uses Ruby's
//! `StringIO` as an in-memory body accumulator.

/// An append-only string buffer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StringIO {
    contents: String,
}

impl StringIO {
    /// Create an empty buffer.
    pub fn new() -> Self {
        Self::default()
    }

    /// Append text to the buffer, preserving append order.
    pub fn append(&mut self, chunk: &str) -> &mut Self {
        self.contents.push_str(chunk);
        self
    }

    /// Return the accumulated string as an owned value.
    pub fn string(&self) -> String {
        self.contents.clone()
    }
}
