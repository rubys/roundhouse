use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FileId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Span {
    pub file: FileId,
    pub start: u32,
    pub end: u32,
}

impl Span {
    pub fn synthetic() -> Self {
        Span { file: FileId(0), start: 0, end: 0 }
    }

    pub fn is_synthetic(&self) -> bool {
        self.file.0 == 0 && self.start == 0 && self.end == 0
    }

    pub fn len(&self) -> u32 {
        self.end - self.start
    }
}

impl Default for Span {
    fn default() -> Self {
        Self::synthetic()
    }
}

/// One source file captured at ingest. `FileId(n)` (1-based) indexes
/// entry `n - 1` of `App::sources`; `FileId(0)` is the synthetic
/// sentinel and maps to no file. `text` is the text prism actually
/// parsed — for `.html.erb` views that's the compiled Ruby out of
/// `compile_erb`, so spans always index correctly into `text` even
/// when it differs from the on-disk bytes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SourceFile {
    pub path: String,
    pub text: String,
}

impl SourceFile {
    /// 1-based (line, column) of a byte offset, by newline count.
    /// Offsets past the end clamp to the last position — a span from
    /// a stale registration shouldn't panic a diagnostics printer.
    pub fn line_col(&self, offset: u32) -> (u32, u32) {
        let offset = (offset as usize).min(self.text.len());
        let before = &self.text.as_bytes()[..offset];
        let line = before.iter().filter(|&&b| b == b'\n').count() as u32 + 1;
        let line_start = before
            .iter()
            .rposition(|&b| b == b'\n')
            .map(|p| p + 1)
            .unwrap_or(0);
        // Column counts characters, not bytes, so editors that treat
        // col as a character offset land on the right spot in UTF-8.
        let col = self.text[line_start..offset].chars().count() as u32 + 1;
        (line, col)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_col_is_one_based_and_newline_aware() {
        let f = SourceFile { path: "a.rb".into(), text: "ab\ncd\nef".into() };
        assert_eq!(f.line_col(0), (1, 1));
        assert_eq!(f.line_col(1), (1, 2));
        assert_eq!(f.line_col(3), (2, 1));
        assert_eq!(f.line_col(7), (3, 2));
    }

    #[test]
    fn line_col_clamps_past_end() {
        let f = SourceFile { path: "a.rb".into(), text: "ab\n".into() };
        assert_eq!(f.line_col(99), (2, 1));
    }

    #[test]
    fn line_col_counts_chars_not_bytes() {
        let f = SourceFile { path: "a.rb".into(), text: "é = 1".into() };
        // "é" is two bytes; the `=` at byte offset 3 is character column 3.
        assert_eq!(f.line_col(3), (1, 3));
    }
}
