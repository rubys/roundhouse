//! sorbet-runtime's annotations do not survive the transpile.
//!
//! A `sig` carries types and nothing else, and roundhouse has read
//! them by the time the class body is walked (#95, #99). Emitting it
//! anyway would be the one thing this project exists not to do: it
//! makes sorbet-runtime wrap the method and re-check its types on
//! every call — per-request work whose answer cannot differ between
//! requests. And since the emitted tree carries no sorbet-runtime, a
//! surviving `sig` is a `NameError` at load time besides.
//!
//! Dropped from the EMIT, not from the analysis: the same test checks
//! that the declared types are still in `rbs_signatures`.
//!
//! `T::Struct` and `T::Enum` are not annotations — `const :name,
//! String` is a constructor and a reader — so they stay until they are
//! lowered, and this test pins that boundary too.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ident::{ClassId, Symbol};
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::ty::Ty;

const LIBRARY: &str = r#"module Reporting
  class Reporter < T::ImmutableStruct
    extend T::Sig
    extend T::Helpers
    abstract!

    const :name, String

    sig { params(prefix: String).returns(String) }
    def slug(prefix)
      prefix.downcase
    end

    sig do
      params(prefix: String).returns(String)
    end
    def label(prefix)
      prefix.upcase
    end
  end
end
"#;

fn app_with(source: &str) -> roundhouse::App {
    let tree: HashMap<PathBuf, Vec<u8>> = [
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"reports\", force: :cascade do |t|\n    t.string \"name\", null: false\n  end\nend\n",
        ),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\nend\n"),
        ("app/services/reporter.rb", source),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"/reports\", to: \"reports#index\"\nend\n",
        ),
    ]
    .into_iter()
    .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    app
}

fn emitted(app: &roundhouse::App) -> String {
    ruby::emit_library(app)
        .into_iter()
        .find(|f| f.path.display().to_string().ends_with("reporter.rb"))
        .map(|f| f.content)
        .expect("the library class is emitted")
}

#[test]
fn the_annotations_are_gone_from_the_emit() {
    let emitted = emitted(&app_with(LIBRARY));
    for annotation in ["sig ", "extend T::Sig", "extend T::Helpers", "abstract!"] {
        assert!(
            !emitted.contains(annotation),
            "`{annotation}` has no runtime in the emitted tree; got:\n{emitted}"
        );
    }
    // What the annotations described is still there.
    assert!(emitted.contains("def slug"), "{emitted}");
    assert!(emitted.contains("def label"), "{emitted}");
}

#[test]
fn what_the_annotations_declared_is_kept_where_it_belongs() {
    // The point of dropping them: the types were read first, so they
    // are in the signature table rather than in the output.
    let app = app_with(LIBRARY);
    let signature = app
        .rbs_signatures
        .get(&ClassId(Symbol::from("Reporting::Reporter")))
        .and_then(|methods| methods.get(&Symbol::from("slug")))
        .cloned()
        .expect("the sig was read before it was dropped");
    match signature {
        Ty::Fn { ret, .. } => assert_eq!(*ret, Ty::Str),
        other => panic!("expected a function type, got {other:?}"),
    }
}

#[test]
fn a_struct_is_not_an_annotation_and_stays_until_it_is_lowered() {
    // `const :name, String` IS the constructor and the reader. Dropping
    // it would leave a class that cannot be built, so it is a lowering
    // job, not a filter job — and this pins that the filter does not
    // reach for it.
    let source = r#"class Row < T::Struct
  const :name, String
end
"#;
    let app = app_with(source);
    // The emitted file is named for the class, not for the source file.
    let emitted = ruby::emit_library(&app)
        .into_iter()
        .find(|f| f.path.display().to_string().ends_with("row.rb"))
        .map(|f| f.content)
        .expect("the struct class is emitted");
    assert!(emitted.contains("const :name"), "got:\n{emitted}");
}
