//! Ruby-family Makefile tests must come from app emission, not scaffold paths.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::project::{BuildTarget, target_files};

fn emit(tests: &[(&str, &str)], target: BuildTarget) -> Vec<(String, String)> {
    let tree: HashMap<PathBuf, Vec<u8>> = [
        (
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\nend\n",
        ),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\nend\n",
        ),
    ]
    .into_iter()
    .chain(tests.iter().copied())
    .map(|(path, source)| (PathBuf::from(path), source.as_bytes().to_vec()))
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let (files, diagnostics) = roundhouse::emit::diagnostics::scope(|| {
        target_files(&app, Path::new("fixtures/tiny-blog"), target)
    });
    assert!(diagnostics.is_empty(), "{target:?}: {diagnostics:#?}");
    files.expect("emit")
}

fn test_stems(files: &[(String, String)]) -> Vec<&str> {
    let makefile = &files
        .iter()
        .find(|(p, _)| p == "Makefile")
        .expect("Makefile")
        .1;
    let mut lines = makefile
        .lines()
        .skip_while(|line| !line.starts_with("SPINEL_TESTS :="));
    let first = lines.next().expect("SPINEL_TESTS");
    if first == "SPINEL_TESTS :=" {
        return Vec::new();
    }
    assert_eq!(first, "SPINEL_TESTS := \\");
    lines
        .take_while(|line| line.starts_with('\t'))
        .map(|line| line.trim().trim_end_matches(" \\"))
        .collect()
}

#[test]
fn app_test_paths_are_sorted_and_namespace_flattened_before_scaffold_merge() {
    for target in [BuildTarget::Ruby, BuildTarget::Jruby] {
        let files = emit(
            &[
                (
                    "test/controllers/rooms/closeds_controller_test.rb",
                    "class Rooms::ClosedsControllerTest < ActionDispatch::IntegrationTest\n  test \"namespaced\" do\n    assert_equal 7, 7\n  end\nend\n",
                ),
                // Same path as a framework test shipped by the scaffold.
                (
                    "test/models/article_broadcasts_test.rb",
                    "class ArticleBroadcastsTest < ActiveSupport::TestCase\n  test \"app_owned_marker\" do\n    assert_equal 3, 3\n  end\nend\n",
                ),
                (
                    "test/models/zebra_test.rb",
                    "class ZebraTest < ActiveSupport::TestCase\n  test \"last\" do\n    assert_equal 9, 9\n  end\nend\n",
                ),
            ],
            target,
        );
        assert_eq!(
            test_stems(&files),
            [
                "test/controllers/rooms_closeds_controller_test",
                "test/models/article_broadcasts_test",
                "test/models/zebra_test",
            ],
            "{target:?}"
        );
        let article = &files
            .iter()
            .find(|(p, _)| p == "test/models/article_broadcasts_test.rb")
            .unwrap()
            .1;
        assert!(article.contains("app_owned_marker"), "{article}");
    }
}

#[test]
fn an_app_without_tests_does_not_inherit_scaffold_test_stems() {
    for target in [BuildTarget::Ruby, BuildTarget::Jruby] {
        let files = emit(&[], target);
        assert!(test_stems(&files).is_empty(), "{target:?}");
        assert!(
            files
                .iter()
                .any(|(p, _)| p == "test/models/article_broadcasts_test.rb")
        );
    }
}
