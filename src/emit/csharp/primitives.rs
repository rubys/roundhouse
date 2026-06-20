//! Hand-written C# runtime primitives.
//!
//! These are the target-specific bottom layer (per `project_two_layer_
//! runtime.md`): types the transpiled framework runtime / emitted models call
//! into but that have no Ruby source — they bridge to the .NET stack. The
//! emitted models reach them by name (same `Roundhouse` namespace).
//!
//! **Phase 2:** the base class + the Db/Time/Broadcasts/Errors stubs the model
//! layer needs to compile (`Db` is a no-op shim until Phase 3 wires
//! `Microsoft.Data.Sqlite`). Phase 3/4 add `Server`, the real adapter, and
//! the Cable transport, grown one file at a time like `runtime/kotlin/`.

use std::path::PathBuf;

use crate::emit::EmittedFile;

const ACTIVE_RECORD_BASE_CS: &str = include_str!("../../../runtime/csharp/ActiveRecordBase.cs");
const DB_CS: &str = include_str!("../../../runtime/csharp/Db.cs");
const TIME_CS: &str = include_str!("../../../runtime/csharp/Time.cs");
const BROADCASTS_CS: &str = include_str!("../../../runtime/csharp/Broadcasts.cs");
const ERRORS_CS: &str = include_str!("../../../runtime/csharp/Errors.cs");
const RH_RUNTIME_CS: &str = include_str!("../../../runtime/csharp/RhRuntime.cs");

/// The hand-written runtime primitives, emitted under `app/runtime/`.
pub fn primitives() -> Vec<EmittedFile> {
    let files = [
        ("app/runtime/ActiveRecordBase.cs", ACTIVE_RECORD_BASE_CS),
        ("app/runtime/Db.cs", DB_CS),
        ("app/runtime/Time.cs", TIME_CS),
        ("app/runtime/Broadcasts.cs", BROADCASTS_CS),
        ("app/runtime/Errors.cs", ERRORS_CS),
        ("app/runtime/RhRuntime.cs", RH_RUNTIME_CS),
    ];
    files
        .iter()
        .map(|(path, content)| EmittedFile {
            path: PathBuf::from(path),
            content: content.to_string(),
        })
        .collect()
}
