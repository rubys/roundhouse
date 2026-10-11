//! A bare constant reaches a class the way Ruby reaches it: through the
//! lexical scope, then the ancestors, then the top level -- not by
//! sharing a last segment with some class buried in the app.
//!
//! The analyzer used to fall back to "the one registered class whose
//! name ends in `::Name`". For the framework catalog that is a fair
//! stand-in for top-level aliases (`HashWithIndifferentAccess`), but for
//! the app's own classes it captured a gem's top-level constant: with
//! `Apps::Listeners::Monorail` declared somewhere in the app, every bare
//! `Monorail.track(...)` (the gem's `Monorail`) was typed as that
//! unrelated class, and the calls failed against its methods.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::analyze::{diagnose, Analyzer};
use roundhouse::ingest::ingest_app_from_tree;

fn diagnostics(extra: &[(&str, &str)], body: &str) -> Vec<String> {
    let mut files: Vec<(String, String)> = vec![
        (
            "db/schema.rb".into(),
            "ActiveRecord::Schema.define do\n  create_table \"things\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n".into(),
        ),
        (
            "app/models/application_record.rb".into(),
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n".into(),
        ),
        (
            "app/controllers/application_controller.rb".into(),
            "class ApplicationController < ActionController::Base\nend\n".into(),
        ),
        (
            "config/routes.rb".into(),
            "Rails.application.routes.draw do\n  get \"/things\", to: \"things#show\"\nend\n".into(),
        ),
        (
            "app/controllers/things_controller.rb".into(),
            format!("class ThingsController < ApplicationController\n  def show\n{body}\n  end\nend\n"),
        ),
    ];
    files.extend(extra.iter().map(|(p, c)| (p.to_string(), c.to_string())));
    let tree: HashMap<PathBuf, Vec<u8>> =
        files.into_iter().map(|(p, c)| (PathBuf::from(p), c.into_bytes())).collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    diagnose(&app)
        .iter()
        .filter(|d| matches!(d.code(), "send_dispatch_failed" | "unsupported"))
        .map(|d| d.message.clone())
        .collect()
}

const LISTENER: (&str, &str) = (
    "app/services/apps/listeners/monorail.rb",
    "module Apps\n  module Listeners\n    class Monorail\n      def self.publish; 1; end\n    end\n  end\nend\n",
);

#[test]
fn a_bare_name_is_not_captured_by_a_nested_app_class_of_that_name() {
    // The read is not inside `Apps::Listeners`, so `Monorail` is the top
    // level's, not that class -- and `track` is not its method either.
    let found = diagnostics(&[LISTENER], "    @a = Monorail.track");
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("constant not supported (all targets): Monorail"), "{found:?}");
    assert!(!found[0].contains("Apps::Listeners"), "{found:?}");
}

#[test]
fn a_sibling_in_the_lexical_scope_still_resolves() {
    let found = diagnostics(
        &[
            LISTENER,
            (
                "app/services/apps/listeners/relay.rb",
                "module Apps\n  module Listeners\n    class Relay\n      def go; Monorail.publish; end\n    end\n  end\nend\n",
            ),
        ],
        "    @a = Apps::Listeners::Relay.new.go",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn a_subclass_reads_a_class_nested_in_its_superclass() {
    let found = diagnostics(
        &[
            (
                "app/services/base_job.rb",
                "class BaseJob\n  class Helper\n    def self.help; 1; end\n  end\nend\n",
            ),
            (
                "app/services/sub_job.rb",
                "class SubJob < BaseJob\n  def go; Helper.help; end\nend\n",
            ),
        ],
        "    @a = SubJob.new.go",
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn a_class_nested_in_a_mixin_of_a_mixin_resolves() {
    // `Parser` is read inside a concern method, which is typed in the
    // controller that includes `WithCarts`; `WithCarts` includes
    // `HeadersHelper`, which declares `Parser`. Ruby reaches it through
    // the controller's ancestors, the transitive ones included.
    let files: Vec<(&str, &str)> = vec![
        (
            "app/controllers/concerns/headers_helper.rb",
            "module HeadersHelper\n  extend ActiveSupport::Concern\n\n  def header_value\n    Parser.parse(\"?1\")\n  end\n\n  class Parser\n    class << self\n      def parse(value)\n        value == \"?1\"\n      end\n    end\n  end\nend\n",
        ),
        (
            "app/controllers/concerns/with_carts.rb",
            "module WithCarts\n  include(HeadersHelper)\n\n  def cart_header\n    header_value\n  end\nend\n",
        ),
        (
            "app/controllers/carts_controller.rb",
            "class CartsController < ApplicationController\n  include WithCarts\n\n  def show\n    @a = cart_header\n  end\nend\n",
        ),
    ];
    let found = diagnostics(&files, "    @a = 1");
    assert!(found.iter().all(|d| !d.contains("parse")), "{found:?}");
}

#[test]
fn a_mixin_method_reads_constants_from_the_mixins_own_namespace() {
    // `OneTimeCode#verify` is typed in `Auth::SessionsController`, but
    // Ruby resolves `Validator` where the method was written, inside
    // `Auth::Concerns`, so it is `Auth::Concerns::Validator`.
    let files: Vec<(&str, &str)> = vec![
        (
            "app/models/auth/concerns/validator.rb",
            "module Auth\n  module Concerns\n    module Validator\n      class << self\n        def ok?(email)\n          email.present?\n        end\n      end\n    end\n  end\nend\n",
        ),
        (
            "app/controllers/auth/concerns/one_time_code.rb",
            "module Auth\n  module Concerns\n    module OneTimeCode\n      def verify(email)\n        Validator.ok?(email)\n      end\n    end\n  end\nend\n",
        ),
        (
            "app/controllers/auth/sessions_controller.rb",
            "module Auth\n  class SessionsController < ApplicationController\n    include Concerns::OneTimeCode\n\n    def show\n      @a = verify(\"a@b.c\")\n    end\n  end\nend\n",
        ),
    ];
    let found = diagnostics(&files, "    @a = 1");
    assert!(found.iter().all(|d| !d.contains("ok?")), "{found:?}");
}

#[test]
fn a_top_level_class_beats_a_class_in_a_mixins_namespace() {
    // `Thing` inside `Thing#sibling` is the top-level model, even though
    // the model includes `Catalog::Types::Lookup` and `Catalog::Types`
    // declares a `Thing` of its own (core's `OnlineStoreEditor::Session`
    // reading `Theme` while including `OnlineStore::Types::Lens`).
    let files: Vec<(&str, &str)> = vec![
        (
            "app/models/thing.rb",
            "class Thing < ApplicationRecord\n  include Catalog::Types::Lookup\n\n  def self.fetch_by_name(name); find_by(name: name); end\n\n  def sibling\n    Thing.fetch_by_name(\"x\")\n  end\nend\n",
        ),
        (
            "app/models/catalog/types/thing.rb",
            "module Catalog\n  module Types\n    module Thing\n      def label; \"t\"; end\n    end\n  end\nend\n",
        ),
        (
            "app/models/catalog/types/lookup.rb",
            "module Catalog\n  module Types\n    module Lookup\n      def look; 1; end\n    end\n  end\nend\n",
        ),
    ];
    let found = diagnostics(&files, "    @a = Thing.new.sibling");
    assert!(found.iter().all(|d| !d.contains("fetch_by_name")), "{found:?}");
}

#[test]
fn the_top_level_hash_with_indifferent_access_is_active_supports_and_only_new_is_supported() {
    let found = diagnostics(&[], "    @a = HashWithIndifferentAccess.new(a: 1)\n    @b = ActiveSupport::HashWithIndifferentAccess.new");
    assert!(found.is_empty(), "{found:?}");
    let found = diagnostics(&[], "    @a = [HashWithIndifferentAccess]");
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("constant not supported (all targets): HashWithIndifferentAccess"), "{found:?}");
    let found = diagnostics(&[], "    @a = Foo::HashWithIndifferentAccess.new");
    assert!(found.iter().any(|d| d.contains("Foo::HashWithIndifferentAccess")), "{found:?}");
}
