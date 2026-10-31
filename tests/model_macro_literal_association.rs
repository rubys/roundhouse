//! PostgreSQL: model macro literal association, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const STRUCTURE: &str = r#"CREATE TABLE posts (
    id bigserial NOT NULL,
    title character varying,
    CONSTRAINT posts_pkey PRIMARY KEY (id)
);

CREATE TABLE metafields (
    id bigserial NOT NULL,
    owner_id bigint,
    owner_type character varying,
    namespace character varying,
    key character varying,
    value character varying,
    CONSTRAINT metafields_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = "default: &default\n  adapter: postgresql\n  encoding: unicode\n\ntest:\n  <<: *default\n  database: rh_spec_literal_association\n";

const CONCERN: &str = r##"module Embeds
  extend ActiveSupport::Concern

  included do
    class_attribute :metafields_to_embed, default: {}
  end

  class_methods do
    #: (Symbol, namespace: String, key: String, ?association_options: Hash[untyped, untyped]) -> void
    def embed_accessor(accessor, namespace:, key:, association_options: {})
      ensure_embeds_association_defined!(association_options)

      register_embed(namespace, key)

    end

    #: (?except_namespaces: Array[String]) -> Array[untyped]
    def embeds_conditions(except_namespaces: [])
      ["metafields.namespace = ?", "global"]
    end

    private

    #: (?Hash[untyped, untyped]) -> void
    def ensure_embeds_association_defined!(association_options = {})
      if (reflection = reflect_on_association(:embedded_metafields))
        return unless association_options.present?
      end

      model_class = self

      has_many(
        :embedded_metafields,
        -> {
          #: self as ActiveRecord::Relation
          where(model_class.embeds_conditions)
        },
        class_name: "Metafield",
        as: "owner",
        inverse_of: :owner,
        autosave: true,
        **association_options, primary_key: :id, foreign_key: :owner_id)
    end

    #: (String, String) -> void
    def register_embed(namespace, key)
      metafields_to_embed[namespace.to_sym] ||= []
      metafields_to_embed[namespace.to_sym] << key
    end
  end
end
"##;

const MODEL: &str = r##"class Post < ApplicationRecord
  include Embeds

  embed_accessor :code, namespace: "global", key: "code"
end
"##;

const METAFIELD: &str = r##"class Metafield < ApplicationRecord
  belongs_to :owner, polymorphic: true, optional: true
end
"##;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("Gemfile", "source \"https://rubygems.org\"\n")
        .write("roundhouse.yml", "")
        .write("config/application.rb", "Bundler.require(*Rails.groups)\n")
        .write("config/database.yml", DATABASE_YML)
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("db/structure.sql", STRUCTURE)
        .write("app/models/concerns/embeds.rb", CONCERN)
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n")
        .write("app/models/metafield.rb", METAFIELD)
        .write("app/models/post.rb", MODEL)
}

fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported") && !e.contains("`has_many autosave: true` not supported")).collect()
}

#[test]
#[ignore = "fork spec: postgresql"]
fn a_macro_declaring_a_literal_association_in_a_helper_is_replayed_on_postgresql() {
    let (dir, errors) = app().emit(BuildTarget::Ruby);
    let post = std::fs::read_to_string(dir.join("app/models/post.rb")).expect("post.rb");
    assert!(post.contains("embed_accessor :code, namespace: \"global\", key: \"code\""), "the macro call was dropped:\n{post}\n{errors:#?}");
    assert!(post.contains("def embedded_metafields"), "the association was not compiled:\n{post}");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
}

/// A call that overrides the helper's `association_options:` keeps its
/// refusal: the compiled association would not carry the override.
#[test]
#[ignore = "fork spec: postgresql"]
fn a_call_overriding_the_helpers_association_options_is_still_refused() {
    let (_dir, errors) = app()
        .write(
            "app/models/post.rb",
            "class Post < ApplicationRecord\n  include Embeds\n\n  embed_accessor :code, namespace: \"global\", key: \"code\", association_options: { primary_key: :id }\nend\n",
        )
        .emit(BuildTarget::Ruby);
    let expected = "model macro call on `Post` not lowered; `embed_accessor`";
    assert!(errors.iter().any(|e| e.contains(expected)), "{errors:#?}");
}
