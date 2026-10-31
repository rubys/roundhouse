//! Where in its file an ingest error happened.
//!
//! [`IngestError`] carries a file and a message, not a span, and its
//! constructors are spread over every ingester. A caller that reports
//! the error against the source (`check`'s `ingest_unsupported`, for a
//! file the isolate-tolerant support-dir walk leaves out of the IR)
//! needs the line, so the frames that hold a node note it here as the
//! error passes up through them.
//!
//! The innermost frame wins: an outer frame's node encloses the inner
//! one, so a site already noted for the same error inside the outer
//! node is kept. A site for the same message outside it is stale (an
//! error some probe raised and recovered from) and is replaced.
//!
//! Thread-local like [`super::survey`], so concurrent tests don't bleed.

use std::cell::RefCell;

use super::IngestError;
use crate::ingest::prism_api::PrismLocation;

#[derive(Clone)]
struct Site {
    file: String,
    message: String,
    start: usize,
    end: usize,
}

thread_local! {
    static SITE: RefCell<Option<Site>> = const { RefCell::new(None) };
}

fn key(err: &IngestError) -> Option<(&str, &str)> {
    match err {
        IngestError::Parse { file, message } | IngestError::Unsupported { file, message } => {
            Some((file, message))
        }
        IngestError::Io(_) => None,
    }
}

/// Note `start..end` as the site of `err`, unless a frame inside it
/// already noted one.
pub(super) fn note(err: &IngestError, start: usize, end: usize) {
    let Some((file, message)) = key(err) else { return };
    SITE.with(|s| {
        let mut site = s.borrow_mut();
        if site.as_ref().is_some_and(|site| {
            site.file == file && site.message == message && start <= site.start && site.end <= end
        }) {
            return;
        }
        *site = Some(Site { file: file.to_string(), message: message.to_string(), start, end });
    });
}

/// [`note`] the node's range for `err` and hand the error on.
pub(super) fn at(err: IngestError, node: &ruby_prism::Node<'_>) -> IngestError {
    let loc = node.location();
    note(&err, loc.start_offset(), loc.end_offset());
    err
}

/// The byte range noted for `err`, if any frame noted one.
pub(super) fn of(err: &IngestError) -> Option<(usize, usize)> {
    let (file, message) = key(err)?;
    SITE.with(|s| {
        s.borrow()
            .as_ref()
            .filter(|site| site.file == file && site.message == message)
            .map(|site| (site.start, site.end))
    })
}
