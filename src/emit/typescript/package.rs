use std::path::PathBuf;

use crate::emit::EmittedFile;

const PACKAGE_JSON: &str = r#"{
  "name": "app",
  "version": "0.1.0",
  "private": true,
  "type": "module",
  "scripts": {
    "start": "tsx main.ts"
  },
  "dependencies": {
    "better-sqlite3": "^11.5.0",
    "ws": "^8.18.0"
  },
  "devDependencies": {
    "@types/node": "^20",
    "@types/better-sqlite3": "^7.6.0",
    "@types/ws": "^8.5.0",
    "typescript": "5.7.3",
    "tsx": "4.19.2"
  }
}
"#;

pub fn emit_package_json() -> EmittedFile {
    EmittedFile {
        path: PathBuf::from("package.json"),
        content: PACKAGE_JSON.to_string(),
    }
}
