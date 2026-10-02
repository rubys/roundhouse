//! Route DSL gaps in `config/routes.rb` draw blocks (#F2):
//!
//! - `draw('sub/dir_file')` — a `draw` argument that names a file in a
//!   subdirectory of `config/routes/`. The split-file map used to be
//!   keyed by bare file stem, so a subdirectory-nested draw never
//!   resolved.
//! - `Dir.glob(PATTERN, base: 'config/routes').each { |v| draw(...) }`
//!   (and `Dir[...]`) — a mass-draw idiom over every file a glob
//!   matches, previously invisible to the receiver-skip in
//!   `ingest_route_stmts`.
//! - Two top-level `X.routes.draw do ... end` blocks in the same file —
//!   only the first was ever ingested.
//!
//! `draw_resolves_subdirectory_relative_path` exercises the app-level
//! `draw_files` map (`src/ingest/app.rs`) end-to-end via
//! `ingest_app_from_tree`; the rest exercise `ingest_routes_with_draws`
//! directly against a manually-built `draws` map.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::App;
use roundhouse::ingest::routes::ingest_routes_with_draws;
use roundhouse::ingest::{ingest_app_from_tree, survey};
use roundhouse::lower::flatten_routes;

const SCHEMA: &str = "ActiveRecord::Schema[7.1].define(version: 1) do\nend\n";

fn flat_paths(table: roundhouse::dialect::RouteTable) -> Vec<(String, String)> {
    let mut app = App::default();
    app.routes = table;
    flatten_routes(&app)
        .into_iter()
        .map(|r| (format!("{:?}", r.method), r.path))
        .collect()
}

/// `draw('financials/financials_erp_routes')` — Procore's actual shape
/// (34 such calls): the file lives under a subdirectory of
/// `config/routes/`, and the `draw` argument spells that whole
/// relative path, not the bare file stem. Before the fix, `draw_files`
/// was keyed by `file_stem()` alone, so this never resolved.
#[test]
fn draw_resolves_subdirectory_relative_path() {
    let tree: HashMap<PathBuf, Vec<u8>> = [
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  draw('financials/financials_erp_routes')\nend\n",
        ),
        (
            "config/routes/financials/financials_erp_routes.rb",
            "get '/erp/status', to: 'erp#status'\n",
        ),
        ("db/schema.rb", SCHEMA),
    ]
    .into_iter()
    .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();

    let app = ingest_app_from_tree(tree).expect("ingest tree");
    let flat = flatten_routes(&app);
    assert!(
        flat.iter().any(|r| r.path == "/erp/status" && r.action.as_str() == "status"),
        "draw() of a subdirectory-relative name did not resolve; routes: {:?}",
        flat.iter().map(|r| &r.path).collect::<Vec<_>>()
    );
}

/// A bare stem still resolves when unambiguous — the flat
/// `config/routes/<name>.rb` layout `draw(:name)` originally targeted.
#[test]
fn draw_resolves_bare_stem_when_unambiguous() {
    let source = b"Rails.application.routes.draw do\n  draw('admin')\nend\n";
    let draws: HashMap<String, (Vec<u8>, String)> = [(
        "admin".to_string(),
        (b"get '/admin', to: 'admin#index'\n".to_vec(), "config/routes/admin.rb".to_string()),
    )]
    .into_iter()
    .collect();

    let (result, _) = roundhouse::ingest::prism::scope(|| {
        ingest_routes_with_draws(source, "config/routes.rb", &draws)
    });
    let table = result.expect("bare stem resolves");
    let flat = flat_paths(table);
    assert!(flat.iter().any(|(_, p)| p == "/admin"), "{flat:?}");
}

/// `Dir.glob('rest_routes/**/*.rb', base: 'config/routes').each do |r|
/// draw(r.sub(/\.rb$/, '')) end` over two files nested at different
/// depths — every matched file must draw, sorted, and every route from
/// every file must land.
#[test]
fn dir_glob_each_draws_every_matched_file() {
    let source = b"Rails.application.routes.draw do\n  Dir.glob('rest_routes/**/*.rb', base: 'config/routes').each do |route|\n    draw(route.sub(/\\.rb$/, ''))\n  end\nend\n";
    let draws: HashMap<String, (Vec<u8>, String)> = [
        (
            "rest_routes/alpha".to_string(),
            (
                b"get '/alpha', to: 'alpha#index'\n".to_vec(),
                "config/routes/rest_routes/alpha.rb".to_string(),
            ),
        ),
        (
            "rest_routes/nested/beta".to_string(),
            (
                b"get '/beta', to: 'beta#index'\n".to_vec(),
                "config/routes/rest_routes/nested/beta.rb".to_string(),
            ),
        ),
        // A sibling file OUTSIDE the glob's directory must not draw.
        (
            "other/gamma".to_string(),
            (
                b"get '/gamma', to: 'gamma#index'\n".to_vec(),
                "config/routes/other/gamma.rb".to_string(),
            ),
        ),
    ]
    .into_iter()
    .collect();

    let (result, _) = roundhouse::ingest::prism::scope(|| {
        ingest_routes_with_draws(source, "config/routes.rb", &draws)
    });
    let table = result.expect("glob-each draws every matched file");
    let flat = flat_paths(table);
    let paths: Vec<&String> = flat.iter().map(|(_, p)| p).collect();
    assert!(paths.contains(&&"/alpha".to_string()), "{flat:?}");
    assert!(paths.contains(&&"/beta".to_string()), "{flat:?}");
    assert!(!paths.contains(&&"/gamma".to_string()), "glob must not draw outside its pattern: {flat:?}");
}

/// `Dir[...]` (index form, `config/routes/` folded into the pattern
/// itself rather than a `base:` kwarg) is the same idiom.
#[test]
fn dir_index_glob_each_draws_matched_files() {
    let source = b"Rails.application.routes.draw do\n  Dir['config/routes/rest/*.rb'].each do |route|\n    draw(route.sub('config/routes/', '').sub(/\\.rb$/, ''))\n  end\nend\n";
    let draws: HashMap<String, (Vec<u8>, String)> = [(
        "rest/one".to_string(),
        (b"get '/one', to: 'one#index'\n".to_vec(), "config/routes/rest/one.rb".to_string()),
    )]
    .into_iter()
    .collect();

    let (result, _) = roundhouse::ingest::prism::scope(|| {
        ingest_routes_with_draws(source, "config/routes.rb", &draws)
    });
    let table = result.expect("Dir[...] glob-each draws");
    let flat = flat_paths(table);
    assert!(flat.iter().any(|(_, p)| p == "/one"), "{flat:?}");
}

/// A glob-each whose block body is anything other than a bare
/// `draw(...)` call must not be silently trusted — survey mode records
/// the shape as a gap, strict mode fails loud.
#[test]
fn dir_glob_each_with_non_draw_body_is_ledged_not_dropped() {
    let source = b"Rails.application.routes.draw do\n  Dir.glob('rest_routes/**/*.rb', base: 'config/routes').each do |route|\n    puts route\n  end\n  get '/health', to: 'health#show'\nend\n";
    let draws: HashMap<String, (Vec<u8>, String)> =
        [("rest_routes/alpha".to_string(), (b"get '/alpha', to: 'alpha#index'\n".to_vec(), "config/routes/rest_routes/alpha.rb".to_string()))]
            .into_iter()
            .collect();

    let (strict, _) = roundhouse::ingest::prism::scope(|| {
        ingest_routes_with_draws(source, "config/routes.rb", &draws)
    });
    assert!(strict.is_err(), "a non-draw glob-each body must fail loud in strict mode");

    survey::activate();
    let (result, _) = roundhouse::ingest::prism::scope(|| {
        ingest_routes_with_draws(source, "config/routes.rb", &draws)
    });
    let gaps = survey::drain();
    let table = result.expect("survey mode recovers the sibling route");
    let flat = flat_paths(table);
    assert!(flat.iter().any(|(_, p)| p == "/health"), "the good route survives: {flat:?}");
    assert!(
        !flat.iter().any(|(_, p)| p == "/alpha"),
        "an un-recognized glob-each shape must not be trusted to draw anything: {flat:?}"
    );
    assert!(
        gaps.iter().any(|g| format!("{g:?}").contains("Dir.glob")),
        "the unsupported shape is ledgered, not silent: {gaps:?}"
    );
}

/// Two top-level `X.routes.draw do ... end` blocks in the same file —
/// Procore's actual shape (`Procore::Application.routes.draw` for
/// legacy routes, a second `Rails.application.routes.draw` for
/// Rails-native ones). Every entry from every block must land.
#[test]
fn two_top_level_draw_blocks_both_ingest() {
    let source = b"Procore::Application.routes.draw do\n  get '/legacy', to: 'legacy#index'\nend\n\nRails.application.routes.draw do\n  get '/native', to: 'native#index'\nend\n";

    let (result, _) = roundhouse::ingest::prism::scope(|| {
        ingest_routes_with_draws(source, "config/routes.rb", &HashMap::new())
    });
    let table = result.expect("two top-level draw blocks both ingest");
    let flat = flat_paths(table);
    let paths: Vec<&String> = flat.iter().map(|(_, p)| p).collect();
    assert!(paths.contains(&&"/legacy".to_string()), "first block dropped: {flat:?}");
    assert!(paths.contains(&&"/native".to_string()), "second block dropped: {flat:?}");
}
