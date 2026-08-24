//! A tableless model's `attr_accessor` surface, and the two places it
//! goes missing.
//!
//! `Opengraph::Metadata` names its four attributes through the constant
//! above them:
//!
//! ```text
//! ATTRIBUTES = %i[ title url image description ]
//! attr_accessor *ATTRIBUTES
//! ```
//!
//! `lower::model_to_library::markers::push_attr_accessor_methods`
//! expands that splat and synthesizes the reader/writer pair — at the
//! emit seam, after the analyzer. So the same rule the `has_one_attached`
//! reader learned applies: a name the pipeline writes must be registered
//! where the analyzer can see it, or the method is in the emitted tree
//! and nowhere else. `self.title = sanitize(strip_tags(title))`, in the
//! model's own `sanitize_fields`, reported `no known method title= on
//! Class(Opengraph::Metadata)`.
//!
//! The second half is `lower::as_json_poro`, which builds
//! `{ "title" => @title, … }` from the same list. Those reads are
//! synthetic — no file, no line — and unstamped they reported `@title
//! has no known type` four times, in a diagnostic a reader cannot even
//! navigate to.
//!
//! Both gates go through `declared_attr_names`, which is the LOWERING's
//! list rather than a second copy: it is the only place that knows the
//! splat form names four fields.
//!
//! NOT gated, because it does not measure: stamping the reader body
//! `push_attr_accessor_methods` writes. An `AccessorKind::AttributeReader`
//! body is not walked by `diagnose`, so the stamp changed neither the
//! ledger nor one byte of the emit. It was written, ablated, and dropped.

use roundhouse::analyze::{diagnose, Analyzer};
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::lower::as_json_poro::apply_as_json_synthesis;

const SCHEMA: &str = "ActiveRecord::Schema.define(version: 1) do\n  \
    create_table :users do |t|\n    t.string :name\n  end\nend\n";

const ROUTES: &str = "Rails.application.routes.draw do\n  resources :users\nend\n";

/// The shape campfire writes: the names come from a constant, and the
/// class assigns through its own writer.
const CARD: &str = "class Card\n  \
    include ActiveModel::Model\n\n  \
    ATTRIBUTES = %i[ title url ]\n  \
    attr_accessor *ATTRIBUTES\n\n  \
    def normalize\n    \
      self.title = title\n  \
    end\nend\n";

/// `as_json` is only synthesized for a class some action actually
/// renders as JSON, and the pass finds it BY TYPE, so the action has to
/// hold a real `Card`.
const CARDS_CONTROLLER: &str = "class CardsController < ApplicationController\n  \
    def show\n    \
      @card = Card.new\n    \
      render json: @card\n  \
    end\nend\n";

fn app_with_card() -> roundhouse::App {
    let tree = vec![
        ("db/schema.rb", SCHEMA),
        ("config/routes.rb", ROUTES),
        ("app/models/user.rb", "class User < ApplicationRecord\nend\n"),
        ("app/models/card.rb", CARD),
        ("app/controllers/cards_controller.rb", CARDS_CONTROLLER),
    ]
    .into_iter()
    .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest tree");
    Analyzer::new(&app).analyze(&mut app);
    app
}

#[test]
fn splatted_attr_accessor_registers_reader_and_writer() {
    let app = app_with_card();
    let offenders: Vec<String> = diagnose(&app)
        .into_iter()
        .map(|d| d.to_string())
        .filter(|d| d.starts_with("error") && d.contains("title"))
        .collect();
    assert!(
        offenders.is_empty(),
        "`attr_accessor *ATTRIBUTES` declares `title` and `title=`: {offenders:?}"
    );
}

#[test]
fn as_json_poro_stamps_the_ivars_it_reads() {
    let mut app = app_with_card();
    apply_as_json_synthesis(&mut app);
    let offenders: Vec<String> = diagnose(&app)
        .into_iter()
        .map(|d| d.to_string())
        .filter(|d| d.starts_with("error") && d.contains("has no known type"))
        .collect();
    assert!(
        app.models.iter().any(|m| m.body.iter().any(|i| matches!(
            i,
            roundhouse::dialect::ModelBodyItem::Method { method, .. }
                if method.name.as_str() == "as_json"
        ))),
        "the fixture must actually reach the synthesizer, or this gate measures nothing"
    );
    assert!(
        offenders.is_empty(),
        "a synthetic ivar read reports with no file and no line; it must carry a type: {offenders:?}"
    );
}
