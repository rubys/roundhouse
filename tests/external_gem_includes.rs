use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::gems::statically_resolved_external_includes;
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::project::{self, BuildTarget};

fn app(source: &str) -> roundhouse::App {
    let files = [
        (
            "Gemfile.lock",
            "GEM\n  specs:\n    redcarpet (3.6.0)\n    rouge (4.5.1)\n\nDEPENDENCIES\n  redcarpet\n  rouge\n",
        ),
        ("lib/markdown_renderer.rb", source),
    ];
    let tree: HashMap<PathBuf, Vec<u8>> = files
        .into_iter()
        .map(|(path, source)| (PathBuf::from(path), source.as_bytes().to_vec()))
        .collect();
    ingest_app_from_tree(tree).expect("ingest external gem include")
}

#[test]
fn locked_external_include_keeps_its_literal_feature_require() {
    let app = app("require \"rouge/plugins/redcarpet\"\n\
         class MarkdownRenderer < Redcarpet::Render::HTML\n\
           include Rouge::Plugins::Redcarpet\n\
         end\n");

    let resolved = statically_resolved_external_includes(&app);
    assert_eq!(
        resolved.get("MarkdownRenderer includes unresolved Rouge::Plugins::Redcarpet"),
        Some(&vec!["rouge/plugins/redcarpet".to_string()]),
        "only the locked gem plus exact literal require resolves the include"
    );

    let files = project::target_files(&app, &PathBuf::from("."), BuildTarget::Ruby)
        .expect("emit Ruby project");
    let renderer = files
        .iter()
        .find(|(_, content)| content.contains("class MarkdownRenderer"))
        .expect("emitted MarkdownRenderer");
    assert!(
        renderer
            .1
            .starts_with("require \"rouge/plugins/redcarpet\"\n"),
        "external plugin load must precede the class definition:\n{}",
        renderer.1
    );
    assert!(renderer.1.contains("include Rouge::Plugins::Redcarpet"));

    let gemfile = files
        .iter()
        .find(|(path, _)| path == "Gemfile")
        .expect("generated Gemfile");
    assert!(gemfile.1.contains("gem \"redcarpet\", \"= 3.6.0\""));
    assert!(gemfile.1.contains("gem \"rouge\", \"= 4.5.1\""));
    let boot = files
        .iter()
        .find(|(path, _)| path == "boot.rb")
        .expect("generated boot.rb");
    assert!(boot.1.contains("require \"redcarpet\""));
    assert!(boot.1.contains("require \"rouge\""));
    assert!(!files.iter().any(|(path, _)| path == "Gemfile.lock"));
}

#[test]
fn namespace_match_requires_identifier_boundary() {
    assert!(roundhouse::gems::detail_names_external_namespace(
        "Rouge::Plugins::Redcarpet",
        "Rouge"
    ));
    assert!(
        !roundhouse::gems::detail_names_external_namespace("NotRouge::X", "Rouge"),
        "longer identifiers that merely end with the root must not match"
    );
}

#[test]
fn multiline_conditional_require_does_not_resolve_include() {
    let conditional = app(
        "if ENV[\"ENABLE_MARKDOWN\"]\n\
         require \"rouge/plugins/redcarpet\"\n\
         end\n\
         class MarkdownRenderer < Redcarpet::Render::HTML\n\
           include Rouge::Plugins::Redcarpet\n\
         end\n",
    );
    assert!(
        statically_resolved_external_includes(&conditional).is_empty(),
        "column-zero require inside multiline if must not resolve"
    );
}

#[test]
fn git_sourced_gem_stays_unresolved() {
    let files = [
        (
            "Gemfile.lock",
            "GIT\n  remote: https://github.com/acme/rouge.git\n  revision: abc\n  specs:\n    rouge (4.5.1)\n\nGEM\n  specs:\n    redcarpet (3.6.0)\n\nDEPENDENCIES\n  redcarpet\n  rouge!\n",
        ),
        (
            "lib/markdown_renderer.rb",
            "require \"rouge/plugins/redcarpet\"\n\
             class MarkdownRenderer < Redcarpet::Render::HTML\n\
               include Rouge::Plugins::Redcarpet\n\
             end\n",
        ),
    ];
    let tree: HashMap<PathBuf, Vec<u8>> = files
        .into_iter()
        .map(|(path, source)| (PathBuf::from(path), source.as_bytes().to_vec()))
        .collect();
    let app = ingest_app_from_tree(tree).expect("ingest git-sourced rouge");
    assert!(
        statically_resolved_external_includes(&app).is_empty(),
        "Git-sourced gems must stay unresolved"
    );
    assert!(
        !roundhouse::gems::locked_external_gem_namespaces(&app).contains("Rouge"),
        "Git-sourced namespaces must not pass through emission"
    );
}

#[test]
fn unrequired_or_nonliteral_external_includes_remain_unresolved() {
    let mut missing_lock = app("require \"rouge/plugins/redcarpet\"\n\
         class MarkdownRenderer < Redcarpet::Render::HTML\n\
           include Rouge::Plugins::Redcarpet\n\
         end\n");
    missing_lock.gem_lock = None;
    assert!(statically_resolved_external_includes(&missing_lock).is_empty());

    let unrequired = app("class MarkdownRenderer < Redcarpet::Render::HTML\n\
           include Rouge::Plugins::Redcarpet\n\
         end\n");
    assert!(statically_resolved_external_includes(&unrequired).is_empty());

    let dynamic = app("feature = \"rouge/plugins/redcarpet\"\n\
         require feature\n\
         class MarkdownRenderer < Redcarpet::Render::HTML\n\
           include Rouge::Plugins::Redcarpet\n\
         end\n");
    assert!(statically_resolved_external_includes(&dynamic).is_empty());

    let conditional = app(
        "require \"rouge/plugins/redcarpet\" if ENV[\"ENABLE_MARKDOWN\"]\n\
         class MarkdownRenderer < Redcarpet::Render::HTML\n\
           include Rouge::Plugins::Redcarpet\n\
         end\n",
    );
    assert!(statically_resolved_external_includes(&conditional).is_empty());
}

#[test]
fn include_only_class_without_superclass_still_resolves() {
    // No superclass → synthetic parent_span; no methods → no method spans.
    // Source must still be recovered so the literal require can resolve.
    let app = app(
        "require \"rouge/plugins/redcarpet\"\n\
         class MarkdownRenderer\n\
           include Rouge::Plugins::Redcarpet\n\
         end\n",
    );
    let resolved = statically_resolved_external_includes(&app);
    assert_eq!(
        resolved.get("MarkdownRenderer includes unresolved Rouge::Plugins::Redcarpet"),
        Some(&vec!["rouge/plugins/redcarpet".to_string()]),
        "include-only / no-superclass class must not silently stay unresolved"
    );
}

#[test]
fn locked_wiring_is_cruby_only() {
    let app = app(
        "require \"rouge/plugins/redcarpet\"\n\
         class MarkdownRenderer < Redcarpet::Render::HTML\n\
           include Rouge::Plugins::Redcarpet\n\
         end\n",
    );

    let jruby = project::target_files(&app, &PathBuf::from("."), BuildTarget::Jruby)
        .expect("emit JRuby project");
    let jruby_gemfile = jruby
        .iter()
        .find(|(path, _)| path == "Gemfile")
        .map(|(_, c)| c.as_str())
        .unwrap_or("");
    assert!(
        !jruby_gemfile.contains("gem \"rouge\""),
        "JRuby must not receive locked external-gem Gemfile pins:\n{jruby_gemfile}"
    );
    let jruby_boot = jruby
        .iter()
        .find(|(path, _)| path == "boot.rb")
        .map(|(_, c)| c.as_str())
        .unwrap_or("");
    assert!(
        !jruby_boot.contains("require \"rouge\""),
        "JRuby must not receive locked external-gem boot requires:\n{jruby_boot}"
    );
}

#[test]
fn locked_wiring_rewrites_unpinned_runtime_gem_overlap() {
    // `parslet` is both RUNTIME_GEMS (unpinned `gem "parslet"`) and Unknown
    // in the lock — locked wiring must rewrite the bare line to `= version`
    // before deleting Gemfile.lock.
    let files = [
        (
            "Gemfile.lock",
            "GEM\n  specs:\n    parslet (2.0.0)\n\nDEPENDENCIES\n  parslet\n",
        ),
        (
            "app/models/search_parser.rb",
            "class SearchParser < Parslet::Parser\nend\n",
        ),
    ];
    let tree: HashMap<PathBuf, Vec<u8>> = files
        .into_iter()
        .map(|(path, source)| (PathBuf::from(path), source.as_bytes().to_vec()))
        .collect();
    let app = ingest_app_from_tree(tree).expect("ingest parslet app");
    let emitted = project::target_files(&app, &PathBuf::from("."), BuildTarget::Ruby)
        .expect("emit Ruby project");
    let gemfile = emitted
        .iter()
        .find(|(path, _)| path == "Gemfile")
        .expect("generated Gemfile");
    assert!(
        gemfile.1.contains("gem \"parslet\", \"= 2.0.0\""),
        "unpinned RUNTIME_GEMS overlap must be rewritten to the lock pin:\n{}",
        gemfile.1
    );
    assert!(
        !gemfile
            .1
            .lines()
            .any(|line| line.trim() == "gem \"parslet\""),
        "bare unpinned parslet line must not survive:\n{}",
        gemfile.1
    );
    assert!(
        !emitted.iter().any(|(path, _)| path == "Gemfile.lock"),
        "scaffold lock is removed after pins are authored"
    );
}
