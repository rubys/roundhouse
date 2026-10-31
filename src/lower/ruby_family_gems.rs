//! Where the gems only the Ruby family ships are allowed to run.
//!
//! The analyzer types each of these gems from the gem catalog
//! (`catalog::gems`), and the Ruby and JRuby trees declare and require
//! the real gem (`gems::RUNTIME_GEMS`), so the app's own calls run there.
//! No other transpile target has anything behind the constant: each
//! reference is refused there, a `raise` with one unsupported diagnostic.

use crate::App;
use crate::build_target::BuildTarget;

/// (root constant, gem, what a reference would be written against).
const GEMS: &[(&str, &str, &str)] = &[
    ("Faraday", "faraday", "a missing HTTP client"),
    ("Retryable", "retryable", "a missing retry helper"),
    ("JWT", "jwt", "a missing token codec"),
    ("RestClient", "rest-client", "a missing HTTP client"),
    ("ElasticAPM", "elastic-apm", "a missing APM agent"),
];

/// Targets that run the real gem: Ruby and JRuby load it via
/// `gems::RUNTIME_GEMS`. Blog and Futamura keep the app's own Ruby and
/// Gemfile, so the gem stays if the app declared it. Roda emits a fresh
/// Gemfile without it and does not run that wiring — refuse there.
fn ships_gem(target: BuildTarget) -> bool {
    matches!(
        target,
        BuildTarget::Ruby | BuildTarget::Jruby | BuildTarget::Blog | BuildTarget::Futamura
    )
}

/// `None` when `app` is already what `target` should emit. `Some` is a
/// copy with each call or constant of a gem in [`GEMS`] rewritten into a
/// refusal.
pub fn without_unshipped_gems(app: &App, target: BuildTarget) -> Option<App> {
    if ships_gem(target) {
        return None;
    }
    let named: Vec<_> = GEMS
        .iter()
        .filter(|(root, _, _)| crate::lower::gem_refusal::mentions(app, root))
        .collect();
    if named.is_empty() {
        return None;
    }
    let mut app = app.clone();
    for (root, gem, missing) in named {
        crate::lower::gem_refusal::refuse_references(
            &mut app,
            target.as_str(),
            &crate::lower::gem_refusal::Refusal {
                root,
                construct: root,
                why: &format!(
                    "is the {gem} gem, which only the ruby and jruby trees ship. \
                     The reference is refused instead of being written against {missing}"
                ),
                raised: &format!("{gem} is not shipped on this target"),
            },
        );
    }
    Some(app)
}
