//! Hand-written Swift primitives — the bottom layer the transpiled
//! framework runtime and lowered app code call into. The analog of
//! `src/emit/kotlin/primitives.rs` (and of `swift-reference/`'s
//! `runtime/` directory, which is the verified template). Grown one
//! primitive at a time as the transpiled runtime needs them.

use std::path::PathBuf;

use crate::emit::EmittedFile;

// String helpers with no clean inline-emit idiom. `gsubMap` is the
// regex-replace-with-lookup-table JsonBuilder's escaping uses;
// `gsub` is the plain regex template replace.
const RHSTRING_SWIFT: &str = include_str!("../../../runtime/swift/rhstring.swift");

// The sqlite layer the lowered `_adapter_*` model emit calls — ported
// from `swift-reference/Sources/App/runtime/Db.swift` (the verified
// Phase R shape): system SQLite3 C API via CSQLite, THREAD-CONFINED via
// ThreadSpecificVariable (each pool thread opens its own connection +
// statement table; a request's whole prepare→step→finalize runs on one
// thread — the Kotlin 7k→54k lesson, applied proactively).
const DB_SWIFT: &str = include_str!("../../../runtime/swift/db.swift");

// `Time.now.utc.iso8601` (the timestamp path in Base#save) — emitted as
// `Time.now().utc.iso8601`, so: a method + two property reads. Ported
// from the Kotlin Time.kt design: truncate to seconds, `Z` offset
// rendering to match Ruby. Hand-rolled formatter (the plan's Linux
// Foundation caveat — no ISO8601DateFormatter divergence risk).
const TIME_SWIFT: &str = include_str!("../../../runtime/swift/time.swift");

// Native-`Date` seam for temporal columns: `Roundhouse.RhDateTime.parse`
// (stored ISO-8601 text → Date, the `parse_db_time` intrinsic target)
// plus the `JsonBuilder.encodeDatetime(Date?)` overload (native Date →
// Rails' canonical `...Z` millisecond JSON form). The `String?` overload
// stays in the transpiled JsonBuilder for the pre-formatted-text path.
const DATETIME_SWIFT: &str = include_str!("../../../runtime/swift/datetime.swift");

// Turbo Streams broadcast sink. The model after_*_commit callbacks pass
// a {stream, target, html} bag; compose the <turbo-stream> wrapper and
// fan it out to /cable subscribers via Cable. Mirrors Kotlin's
// Broadcasts.kt (and go/rust/crystal's Broadcasts).
const BROADCASTS_SWIFT: &str = include_str!("../../../runtime/swift/broadcasts.swift");

// Action Cable WebSocket + Turbo Streams broadcaster — the per-target
// transport primitive (cf. Kotlin's Cable.kt, runtime/go/v2/cable.go,
// runtime/crystal/cable.cr). Same wire format (actioncable-v1-json),
// same per-channel subscriber map. The concurrency bridge is an
// AsyncStream per connection: `dispatch` (called synchronously from the
// Db pool threads' after-commit hooks) yields into the stream — the
// continuation is thread-safe — and a per-connection writer task drains
// it to the WebSocket. Heartbeat every 3s (ActionCable clients treat a
// ~6s ping gap as a dead connection).
const CABLE_SWIFT: &str = include_str!("../../../runtime/swift/cable.swift");

// The compile-time contract for `ActiveRecord.adapter` (base.rbs
// AdapterInterface). NO implementation ships — the adapter slot is never
// assigned (the Kotlin "drop the functional adapter" decision): all real
// CRUD is Db-direct via the per-model `_adapter_*` overrides; Base's
// where/find_by are the only callers and real-blog never invokes them
// (an unwrapped-nil crash there is the correct "unsupported" signal).
const ADAPTER_INTERFACE_SWIFT: &str = include_str!("../../../runtime/swift/adapter_interface.swift");

// NOTE: no ParamValue primitive. The enum-union shape (locked in
// swift-reference) doesn't survive the runtime's untyped `is_a?(Hash)`
// narrowing — the Kotlin arc hit this exact failure and resolved it by
// mapping ParamValue → the top type (see ty.rs); params are nested
// `[String: Any?]` maps end-to-end.

// The HTTP listener — Hummingbird 2, the locked choice (plan decision
// 1). THE BRIDGE: Hummingbird handlers are async and hop executors,
// which would break the thread-confined Db/slot state — so the handler
// collects the body asynchronously, then runs the ENTIRE synchronous
// dispatch (router match → controller → Db → render) in ONE
// `NIOThreadPool.runIfActive` closure on a stable pool thread.
// `processAction` is the throws boundary: RecordNotFound → 404,
// RecordInvalid → 422 (the Phase 5 throws-propagation contract).
// NOTE: `Hummingbird.Router` is qualified — the transpiled
// ActionDispatch router is this module's `Router`.
const SERVER_SWIFT: &str = include_str!("../../../runtime/swift/server.swift");

// Thread-confined mutable slot — the Swift analog of the Kotlin
// OBJECT_TL_FIELDS ThreadLocal conversion (the fix that ended Kotlin's
// cross-request state bleed). Module-level mutable `@ivar` state
// (ViewHelpers' content_for slots) emits as a computed static property
// backed by one of these: each NIOThreadPool thread sees its own value,
// and since a request's whole dispatch runs on one pool thread
// (Server.swift's runIfActive bridge), per-thread IS per-request.
// ThreadSpecificVariable requires a class value, hence the Box.
const RHTHREADLOCAL_SWIFT: &str = include_str!("../../../runtime/swift/rhthreadlocal.swift");

/// The hand-written primitive files, emitted under `Sources/App/runtime/`.
pub fn primitives() -> Vec<EmittedFile> {
    vec![
        EmittedFile {
            path: PathBuf::from("Sources/App/runtime/RhString.swift"),
            content: RHSTRING_SWIFT.to_string(),
        },
        EmittedFile {
            path: PathBuf::from("Sources/App/runtime/Db.swift"),
            content: DB_SWIFT.to_string(),
        },
        EmittedFile {
            path: PathBuf::from("Sources/App/runtime/Time.swift"),
            content: TIME_SWIFT.to_string(),
        },
        EmittedFile {
            path: PathBuf::from("Sources/App/runtime/DateTime.swift"),
            content: DATETIME_SWIFT.to_string(),
        },
        EmittedFile {
            path: PathBuf::from("Sources/App/runtime/Broadcasts.swift"),
            content: BROADCASTS_SWIFT.to_string(),
        },
        EmittedFile {
            path: PathBuf::from("Sources/App/runtime/AdapterInterface.swift"),
            content: ADAPTER_INTERFACE_SWIFT.to_string(),
        },
        EmittedFile {
            path: PathBuf::from("Sources/App/runtime/Server.swift"),
            content: SERVER_SWIFT.to_string(),
        },
        EmittedFile {
            path: PathBuf::from("Sources/App/runtime/RhThreadLocal.swift"),
            content: RHTHREADLOCAL_SWIFT.to_string(),
        },
        EmittedFile {
            path: PathBuf::from("Sources/App/runtime/Cable.swift"),
            content: CABLE_SWIFT.to_string(),
        },
    ]
}
