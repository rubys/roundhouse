//! Hand-written Kotlin runtime primitives.
//!
//! These are the target-specific bottom layer (per `project_two_layer_
//! runtime.md`): types the transpiled framework runtime calls into but
//! that have no Ruby source — they bridge to the JVM/JDBC/Javalin stack.
//! The transpiled `runtime/ruby/*.rb` files reach them by name (same
//! `roundhouse` package), so the surface each exposes is dictated by how
//! the emitter renders the corresponding Ruby calls.
//!
//! Grown one primitive at a time, mirroring the runtime-transpile order:
//! Time first (the only thing standing between `ActiveRecordBase.kt` and a
//! clean compile), then Db / ParamValue (both self-contained and added
//! here), then Server / the adapter once controllers + a `Main` entry
//! exist (Server is coupled to `ArticlesController`, so it's held back to
//! keep every emitted primitive independently compileable).
//!
//! Each primitive is ported from the hand-written Phase R reference under
//! `kotlin-reference/src/main/kotlin/runtime/`, adapted where the emitter's
//! rendering differs from the reference's hand-written call sites (notably:
//! the emitter renders `Ty::Int` literals with an `L` suffix, so `Db`'s
//! column-index params are `Long` here, not the reference's `Int`).

use std::path::PathBuf;

use crate::emit::EmittedFile;

/// `Time.now.utc.iso8601` is the sole Time API the framework runtime uses
/// (`ActiveRecord::Base#fill_timestamps`). The emitter renders that chain
/// as `Time.now().utc.iso8601` — a method call then two property reads —
/// so `now()` returns a `TimeInstant` whose `utc`/`iso8601` are `val`s.
const TIME_KT: &str = include_str!("../../../runtime/kotlin/time.kt");

/// The sqlite primitive layer the lowered model IR dispatches against
/// (`Db.prepare` / `Db.step` / `Db.columnInt` / `Db.columnText` /
/// `Db.finalize` / `Db.exec` / `Db.escape*`). Ported from
/// `kotlin-reference/runtime/Db.kt`, with one adaptation: the emitter
/// renders `Ty::Int` literals with an `L` suffix (`Db.columnInt(stmt, 0L)`),
/// so the column-index params are `Long` and shifted to JDBC's 1-based,
/// `Int`-typed column index internally.
const DB_KT: &str = include_str!("../../../runtime/kotlin/db.kt");

// The previous `ParamValue` sealed-union primitive was removed: the params
// layer now holds untyped values as Kotlin's top type `Any?` (nested Hash →
// `MutableMap<String, Any?>`, scalar → `String`), which is what
// `<Resource>Params.from_raw`'s lowered `is_a?(Hash)` / `is_a?(String)` checks
// (emitted as `is Map<*,*>` / `is String`) actually match against. A typed
// wrapper failed every check, silently dropping create params. See the
// `Roundhouse::ParamValue` → `Any?` mapping in `ty.rs::render_class` and
// `setParam` in `Server.kt`.

/// The Javalin HTTP listener — the per-target server primitive (cf.
/// `runtime/crystal/server.cr`, `runtime/go/v2/server.go`). Parses the
/// request, dispatches through the transpiled `Router.match` against the
/// app's routes table, instantiates the matched controller, populates its
/// request state (params/flash/session/format), runs `process_action`, and
/// formats the response (redirect, html-with-layout, or json). The routes
/// table, controller factory map, and layout function are passed in by the
/// emitted `Main.kt` (they're app-specific).
const SERVER_KT: &str = include_str!("../../../runtime/kotlin/server.kt");

/// The adapter contract `ActiveRecord::Base`'s class-level CRUD defaults
/// (`_adapter_all` / `_adapter_find_by_id` / `where` / `find_by` / …)
/// dispatch against — the Kotlin analog of the per-target adapter primitive
/// every other backend ships (crystal `db.cr`, go `adapter_interface.go`,
/// rust `adapter_interface.rs`, ts `juntos.ts`). Surface mirrors
/// `runtime/ruby/active_record/base.rbs`'s `AdapterInterface`.
///
/// The legacy *functional* adapter path is DROPPED for Kotlin: there is no
/// Db-backed implementation and `ActiveRecord.adapter` is never assigned.
/// All real CRUD goes Db-direct through the Level-3 per-model overrides
/// (each model's companion re-emits `_adapter_*` calling `Db` itself —
/// Kotlin companions aren't inherited, so Base's defaults are never
/// reached). This interface exists purely as the compile-time contract for
/// those (dead, for real-blog) Base defaults; the only callers without a
/// per-model override are `where`/`find_by`, which real-blog never invokes
/// and which therefore throw `UninitializedPropertyAccessException` if hit
/// — the correct "this path is unsupported" behavior.
const ADAPTER_INTERFACE_KT: &str = include_str!("../../../runtime/kotlin/adapter_interface.kt");

/// Turbo Streams broadcast sink — the object the model `after_*_commit`
/// callbacks dispatch to (`Broadcasts.append`/`prepend`/`replace`/`remove`,
/// each taking a kwargs bag lowered to a `MutableMap<String, Any?>` carrying
/// `stream`/`target`/`html`). Composes the `<turbo-stream>` fragment and
/// hands it to the cable fan-out (`Cable.dispatch`). Mirrors the
/// go2/rust2/crystal Broadcasts shim.
const BROADCASTS_KT: &str = include_str!("../../../runtime/kotlin/broadcasts.kt");

/// Action Cable WebSocket + Turbo Streams broadcaster — the per-target
/// transport primitive (cf. `runtime/go/v2/cable.go`,
/// `runtime/crystal/cable.cr`, `runtime/rust/cable.rs`). Same wire format
/// (actioncable-v1-json), same per-channel subscriber map.
///
/// Mounted as a RAW Jetty 11 WebSocket servlet (not Javalin's `app.ws`):
/// Javalin's `onConnect` fires after the upgrade response is already sent, so
/// it can't echo the `Sec-WebSocket-Protocol: actioncable-v1-json` header
/// ActionCable requires (javalin#957) — and the client closes the socket
/// without it. The servlet's creator sets the accepted subprotocol DURING the
/// upgrade. Server.kt mounts it via `config.jetty.modifyServletContextHandler`.
const CABLE_KT: &str = include_str!("../../../runtime/kotlin/cable.kt");

/// The hand-written runtime primitives, emitted under `src/main/kotlin/`.
pub fn primitives() -> Vec<EmittedFile> {
    vec![
        EmittedFile {
            path: PathBuf::from("src/main/kotlin/Time.kt"),
            content: TIME_KT.to_string(),
        },
        EmittedFile {
            path: PathBuf::from("src/main/kotlin/Db.kt"),
            content: DB_KT.to_string(),
        },
        EmittedFile {
            path: PathBuf::from("src/main/kotlin/AdapterInterface.kt"),
            content: ADAPTER_INTERFACE_KT.to_string(),
        },
        EmittedFile {
            path: PathBuf::from("src/main/kotlin/Broadcasts.kt"),
            content: BROADCASTS_KT.to_string(),
        },
        EmittedFile {
            path: PathBuf::from("src/main/kotlin/Cable.kt"),
            content: CABLE_KT.to_string(),
        },
        EmittedFile {
            path: PathBuf::from("src/main/kotlin/Server.kt"),
            content: SERVER_KT.to_string(),
        },
    ]
}
