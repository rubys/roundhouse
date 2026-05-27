//! Survey mode: gather all ingest errors instead of failing on the
//! first one.
//!
//! By default the ingester is fail-fast — `IngestError` aborts the
//! whole pipeline at the first unsupported construct. That's correct
//! for CI and for targeted "what's the next thing to fix" feedback,
//! but it's a poor fit for *scope-estimation* work: each rebuild +
//! rerun reveals one gap at a time, and N gaps cost N cycles.
//!
//! Survey mode flips the polarity. When active, [`ingest_expr`]
//! intercepts its own `Err` returns: records the error here and
//! substitutes a [`Literal::Nil`] placeholder so the parent ingester
//! keeps going. A single run produces a deduplicated punch list of
//! every distinct gap across the app — feeding triage decisions
//! about batch-fix opportunities, stub candidates, and total scope.
//!
//! Toggle: `roundhouse-check --continue` or
//! `ROUNDHOUSE_INGEST_SURVEY=1`. Default is off; the strict path is
//! unchanged when the flag isn't set.
//!
//! State lives in a thread-local so concurrent tests don't bleed —
//! the flag only affects the calling thread.

use std::cell::RefCell;

use super::IngestError;

thread_local! {
    static SURVEY_STATE: RefCell<Option<Vec<IngestError>>> = const { RefCell::new(None) };
}

/// Activate survey mode for the current thread. Subsequent
/// `ingest_expr` failures record into the per-thread collector and
/// return a placeholder Expr instead of aborting. Calling again
/// resets the collector.
pub fn activate() {
    SURVEY_STATE.with(|s| *s.borrow_mut() = Some(Vec::new()));
}

/// True when survey mode is active on the calling thread.
pub fn is_active() -> bool {
    SURVEY_STATE.with(|s| s.borrow().is_some())
}

/// Push an ingest error into the per-thread collector. No-op if
/// survey mode isn't active (so callers can record unconditionally).
pub fn record(err: &IngestError) {
    SURVEY_STATE.with(|s| {
        if let Some(buf) = s.borrow_mut().as_mut() {
            // IngestError doesn't implement Clone (std::io::Error has
            // no clone), but every variant we care about does — flatten
            // to a string-shaped Unsupported so the collector entries
            // are uniform and Clone-able by downstream printers.
            buf.push(IngestError::Unsupported {
                file: err_file(err).to_string(),
                message: err_message(err).to_string(),
            });
        }
    });
}

/// Drain the collector and deactivate survey mode. Returns every
/// error captured during the active window in record order.
pub fn drain() -> Vec<IngestError> {
    SURVEY_STATE.with(|s| s.borrow_mut().take().unwrap_or_default())
}

fn err_file(err: &IngestError) -> &str {
    match err {
        IngestError::Io(_) => "<io>",
        IngestError::Parse { file, .. } | IngestError::Unsupported { file, .. } => file,
    }
}

fn err_message(err: &IngestError) -> String {
    match err {
        IngestError::Io(e) => format!("io error: {e}"),
        IngestError::Parse { message, .. } => format!("parse error: {message}"),
        IngestError::Unsupported { message, .. } => message.clone(),
    }
}

/// Bucket key for aggregation: the message prefix up to the first `(`
/// (which truncates the Prism-node-Debug repr's pointer-bearing
/// payload). Used by the punch-list printer to dedupe "ConstantWriteNode
/// at FOO" + "ConstantWriteNode at BAR" into a single grouped entry.
pub fn bucket_key(err: &IngestError) -> String {
    let msg = err_message(err);
    let trimmed = msg.split('(').next().unwrap_or(&msg);
    // Strip trailing whitespace introduced by the split + truncate to
    // a reasonable display width.
    let key = trimmed.trim_end();
    if key.len() > 120 {
        key[..120].to_string()
    } else {
        key.to_string()
    }
}
