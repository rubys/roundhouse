//! A nested class is emitted inside its parent's file, as the source
//! wrote it.
//!
//! Given a file of its own, each part-file re-opened the outer class
//! and CLOSED it again. `TracePoint(:end)` — how a gem implements "end
//! of class body" — fires on every one of those closes, so a gem that
//! validates there saw a body missing whatever the parent's own file
//! sets, and the tree stopped loading. Naming the same superclass in
//! both places (`outer_header`) makes the ORDER irrelevant; it cannot
//! make the COUNT of body-ends one.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

fn emitted(files: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(
        PathBuf::from("db/schema.rb"),
        b"ActiveRecord::Schema.define do\n  create_table \"posts\", force: :cascade do |t|\n    t.string \"body\", null: false\n  end\nend\n".to_vec(),
    );
    for (p, c) in files {
        tree.insert(PathBuf::from(*p), c.as_bytes().to_vec());
    }
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    ruby::emit_library(&app)
        .into_iter()
        .map(|f| (f.path.display().to_string(), f.content))
        .collect()
}

fn file(files: &[(String, String)], stem: &str) -> String {
    files
        .iter()
        .find(|(p, _)| p.ends_with(stem))
        .map(|(_, c)| c.clone())
        .unwrap_or_else(|| panic!("no {stem} in {:?}", files.iter().map(|(p, _)| p).collect::<Vec<_>>()))
}

#[test]
fn a_nested_class_has_no_file_of_its_own() {
    let files = emitted(&[(
        "app/services/gauge.rb",
        "class Gauge\n  class Reading\n    def value\n      1\n    end\n  end\n\n  def read\n    Reading.new\n  end\nend\n",
    )]);
    let gauge = file(&files, "gauge.rb");
    assert!(gauge.contains("class Reading"), "got:\n{gauge}");
    assert!(
        files.iter().all(|(p, _)| !p.ends_with("gauge/reading.rb")),
        "the nested class keeps no file of its own: {:?}",
        files.iter().map(|(p, _)| p).collect::<Vec<_>>()
    );
}

#[test]
fn the_body_of_the_outer_class_ends_once() {
    // The point of the change, asserted directly: one `class Gauge`
    // and one matching `end` at that indentation, rather than one pair
    // per nested class.
    let files = emitted(&[(
        "app/services/gauge.rb",
        "class Gauge\n  class Reading\n  end\n\n  class Sample\n  end\nend\n",
    )]);
    // Counted across ALL emitted files, which is where the property
    // lives: with a file per nested class each one opened and closed
    // `class Gauge` again, and a single file's count says nothing
    // about that.
    let opens: usize = files
        .iter()
        .filter(|(p, _)| p.ends_with(".rb"))
        .map(|(_, c)| {
            c.matches("\nclass Gauge").count() + usize::from(c.starts_with("class Gauge"))
        })
        .sum();
    assert_eq!(opens, 1, "the outer class body is opened once in the whole tree: {files:?}");
}

#[test]
fn a_sibling_superclass_comes_first() {
    // They share a file now, so `class B < A` runs where `A` must
    // already be defined — the requirement a require between two files
    // used to satisfy.
    let files = emitted(&[(
        "app/services/gauge.rb",
        "class Gauge\n  class Sample < Gauge::Reading\n  end\n\n  class Reading\n  end\nend\n",
    )]);
    let gauge = file(&files, "gauge.rb");
    let base = gauge.find("class Reading").expect("the base is emitted");
    let sub = gauge.find("class Sample <").expect("the subclass is emitted");
    assert!(base < sub, "the base must come first:\n{gauge}");
}

#[test]
fn every_require_resolves_to_an_emitted_file() {
    // The invariant that matters, and the one that found three bugs
    // while this was being written: moving a class changes what the
    // paths around it mean. A require left pointing at the file the
    // class used to have is a LoadError on the first boot — not a
    // warning, not a type error.
    //
    // Asserted over the whole tree rather than over one file, because
    // the require that goes stale is rarely in the file that moved.
    let files = emitted(&[
        (
            "app/services/gauge.rb",
            "class Gauge\n  class Reading\n  end\n\n  class Sample\n    BASE = Gauge::Reading\n  end\nend\n",
        ),
        ("app/services/other.rb", "class Other\n  BASE = Gauge::Reading\nend\n"),
    ]);
    let emitted_stems: Vec<String> = files
        .iter()
        .map(|(p, _)| p.trim_end_matches(".rb").to_string())
        .collect();
    for (path, content) in &files {
        if !path.ends_with(".rb") {
            continue;
        }
        let dir: Vec<&str> = path.split('/').collect();
        let dir = &dir[..dir.len() - 1];
        for line in content.lines() {
            let Some(rest) = line.strip_prefix("require_relative \"") else { continue };
            let Some(target) = rest.split('"').next() else { continue };
            let mut resolved: Vec<String> = dir.iter().map(|s| s.to_string()).collect();
            for seg in target.split('/') {
                match seg {
                    "." | "" => {}
                    ".." => {
                        resolved.pop();
                    }
                    other => resolved.push(other.to_string()),
                }
            }
            let joined = resolved.join("/");
            assert!(
                emitted_stems.iter().any(|s| *s == joined),
                "`{path}` requires `{target}`, which resolves to `{joined}` and is not emitted: {:?}",
                files.iter().map(|(p, _)| p).collect::<Vec<_>>()
            );
        }
    }
}

#[test]
fn a_class_nested_in_a_model_keeps_its_file() {
    // The guard, and a limit worth stating: a model's file comes from
    // a different emit, so there is nothing here to splice into. Left
    // alone rather than dropped — filtering it out with no owner made
    // it vanish from the tree entirely.
    let files = emitted(&[
        (
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\nend\n",
        ),
        (
            "app/models/post.rb",
            "class Post < ApplicationRecord\n  class Presenter\n    def show\n      1\n    end\n  end\nend\n",
        ),
    ]);
    assert!(
        files.iter().any(|(p, _)| p.ends_with("post/presenter.rb")),
        "a class nested in a model keeps its own file: {:?}",
        files.iter().map(|(p, _)| p).collect::<Vec<_>>()
    );
}

#[test]
fn a_class_nested_in_a_controller_reopens_the_controller_as_a_class() {
    // A controller is emitted by its own pipeline too, so a class nested
    // in one keeps its own file, like a model's. That file reopened the
    // controller as `module ApplicationController`, which cannot load
    // after (or before) `class ApplicationController`: "is not a module".
    let files = emitted(&[
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\n  class Failure < StandardError\n    def initialize(status)\n      @status = status\n      super(\"failed\")\n    end\n  end\nend\n",
        ),
        (
            "app/controllers/posts_controller.rb",
            "class PostsController < ApplicationController\n  class Missing < StandardError\n  end\n\n  def index\n    raise Missing if params[:q]\n    raise ApplicationController::Failure.new(404)\n  end\nend\n",
        ),
    ]);
    for (stem, header) in [
        ("application_controller/failure.rb", "class ApplicationController < ActionController::Base\n"),
        ("posts_controller/missing.rb", "class PostsController < ApplicationController\n"),
    ] {
        let nested = file(&files, stem);
        assert!(nested.contains(header), "{stem}:\n{nested}");
        assert!(!nested.contains("module "), "{stem}:\n{nested}");
    }
}
