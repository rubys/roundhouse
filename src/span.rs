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

    pub fn len(&self) -> u32 {
        self.end - self.start
    }
}
