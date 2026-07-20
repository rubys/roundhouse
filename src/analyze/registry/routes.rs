//! Route URL-helper name derivation, shared by the view-context,
//! ApplicationController, and library-class registrations. Extracted
//! verbatim from `Analyzer::with_adapter`.

use crate::App;

/// Route URL helper names from the ingested route table — one
/// `<as_name>_path` / `<as_name>_url` per named route (same flattening
/// the route emitters use). Derived from real routes, not a `_path$`
/// name heuristic, so only declared routes resolve. Registered on
/// the view context, ApplicationController, and library classes.
pub(in crate::analyze) fn route_helper_names(app: &App) -> Vec<String> {
    let mut names = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    // Rails auto-names a `:as`-less route from its path's static
    // segments (`get "/settings"` → `settings_path`, `get
    // "/replies/unread"` → `replies_unread_path`). `flatten_routes`
    // (which also feeds *emit*) keeps its action-name fallback, so we
    // add the path-derived candidate here, on the analyze dispatch
    // surface ONLY — purely additive (extra `*_path` readers can only
    // resolve a call, never alter emitted output). A genuinely-named
    // route still registers its real `as_name` first.
    let path_candidate = |path: &str| -> String {
        path.split('/')
            .filter(|seg| {
                !seg.is_empty()
                    && !seg.starts_with(':')
                    && !seg.starts_with('*')
                    && seg.chars().all(|c| c.is_alphanumeric() || c == '_')
            })
            .collect::<Vec<_>>()
            .join("_")
    };
    for route in crate::lower::flatten_routes(app) {
        for candidate in [route.as_name.clone(), path_candidate(&route.path)] {
            if candidate.is_empty() {
                continue;
            }
            if seen.insert(candidate.clone()) {
                names.push(format!("{candidate}_path"));
                names.push(format!("{candidate}_url"));
            }
        }
    }
    names
}
