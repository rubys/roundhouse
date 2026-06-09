//! Turbo Streams broadcasts shim.
//!
//! The model lowerer's `broadcasts_to` expansion (see
//! `src/lower/broadcasts.rs`) produces calls like
//! `Broadcasts.prepend(stream: "x", target: "y", html: "...")`
//! from inside model callback methods (`after_create`, etc.).
//! rust2 emits the kwargs as a `HashMap<String, Value>`, so each
//! shim method here accepts the unified hash shape and pulls
//! the named fields out.
//!
//! State lives in a thread-local log so framework tests can assert
//! on what got emitted; production installs a broadcaster via
//! `install_broadcaster` that fans fragments out to the cable
//! websocket. Mirrors `runtime/crystal/broadcasts.cr` member-for-
//! member at the shim level.
//!
//! For Phase 5 the production path is a no-op stub — Cable wiring
//! arrives in a later phase.

use std::cell::RefCell;
use std::collections::HashMap;

use serde_json::Value;

thread_local! {
    /// In-memory broadcast log: `(action, stream, target, html)`
    /// tuples in emission order. Tests inspect this after running
    /// model callbacks; production reads it through `log()` if a
    /// fan-out plugin needs the trail.
    static LOG: RefCell<Vec<(String, String, String, String)>> =
        const { RefCell::new(Vec::new()) };
}

/// `Broadcasts` namespace — `pub struct Broadcasts;` + impl gives
/// the same `Broadcasts::method(...)` call shape the lowered model
/// callbacks emit.
pub struct Broadcasts;

impl Broadcasts {
    /// Reset the in-memory log. Framework tests call this between
    /// assertions; production typically doesn't.
    pub fn reset_log_bang() {
        LOG.with(|c| c.borrow_mut().clear());
    }

    /// Snapshot the log as a fresh Vec.
    pub fn log() -> Vec<(String, String, String, String)> {
        LOG.with(|c| c.borrow().clone())
    }

    pub fn append(attrs: HashMap<String, Value>) {
        Self::record("append", &attrs);
    }

    pub fn prepend(attrs: HashMap<String, Value>) {
        Self::record("prepend", &attrs);
    }

    pub fn replace(attrs: HashMap<String, Value>) {
        Self::record("replace", &attrs);
    }

    pub fn remove(attrs: HashMap<String, Value>) {
        Self::record("remove", &attrs);
    }

    fn record(action: &str, attrs: &HashMap<String, Value>) {
        let stream = attrs.get("stream").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let target = attrs.get("target").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let html = attrs.get("html").and_then(|v| v.as_str()).unwrap_or("").to_string();
        LOG.with(|c| {
            c.borrow_mut().push((
                action.to_string(),
                stream.clone(),
                target.clone(),
                html.clone(),
            ))
        });
        // Forward to the live Action Cable fan-out. The model callbacks
        // (`broadcasts_to` lowering) hand us the already-rendered partial
        // as `html`; compose the `<turbo-stream>` wrapper and fan it out to
        // every subscriber of `stream`. Without this the production path is
        // a no-op (only the in-memory LOG is populated) and a subscribed
        // `<turbo-cable-stream-source>` never sees the create/destroy
        // broadcast — the e2e action_cable spec. The cable server fans out
        // via per-subscriber mpsc channels, so this is safe to call from
        // any axum worker thread.
        let fragment = crate::cable::turbo_stream_html(action, &target, &html);
        crate::cable::CABLE.broadcast(&stream, &fragment);
    }
}
