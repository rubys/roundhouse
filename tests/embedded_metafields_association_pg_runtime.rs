//! PostgreSQL: embedded metafields association, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

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

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_literal_association
"#;

const APPLICATION_RECORD: &str = "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n";

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
      metafields_to_embed.each_with_object([]) do |(namespace, keys), conditions|
        next if except_namespaces.include?(namespace.to_s)

        conditions[0] =
          "(" + [conditions[0], "(metafields.namespace = ? AND metafields.key IN (?))"].compact.join(" OR ") + ")"
        conditions << namespace
        conditions << keys
      end
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

const POST: &str = r##"class Post < ApplicationRecord
  include Embeds

  embed_accessor :code, namespace: "global", key: "code"
end
"##;

const METAFIELD: &str = r##"class Metafield < ApplicationRecord
  belongs_to :owner, polymorphic: true, optional: true
end
"##;

const PROBE: &str = r##"class LiteralAssociationProbe
  def self.lines
    out = []
    post = Post.create!(title: "a")
    other = Post.create!(title: "b")
    Metafield.create!(owner_id: post.id, owner_type: "Post", namespace: "global", key: "code", value: "1")
    Metafield.create!(owner_id: post.id, owner_type: "Post", namespace: "global", key: "skipped", value: "2")
    Metafield.create!(owner_id: other.id, owner_type: "Post", namespace: "global", key: "code", value: "3")
    reflection = Post.reflect_on_association(:embedded_metafields)
    out << "reflection: #{reflection.class_name} #{reflection.foreign_key} #{reflection.options[:as]}"
    out << "read: #{Post.find(post.id).embedded_metafields.map(&:value).inspect}"
    loaded = Post.where(id: [post.id, other.id]).order(:id).preload(:embedded_metafields).to_a
    out << "preload: #{loaded.map { |r| r.embedded_metafields.map(&:value) }.inspect}"
    out << "conditions: #{Post.embeds_conditions.inspect}"
    out
  end
end
"##;

const EXPECTED: [&str; 4] = [
    "reflection: Metafield owner_id owner",
    "read: [\"1\"]",
    "preload: [[\"1\"], [\"3\"]]",
    "conditions: [\"((metafields.namespace = ? AND metafields.key IN (?)))\", :global, [\"code\"]]",
];

const LOADS: [&str; 5] = [
    "app/models/concerns/embeds",
    "app/models/application_record",
    "app/models/metafield",
    "app/models/post",
    "app/models/literal_association_probe",
];

const FILES: [(&str, &str); 9] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/concerns/embeds.rb", CONCERN),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/metafield.rb", METAFIELD),
    ("app/models/post.rb", POST),
    ("app/models/literal_association_probe.rb", PROBE),
];

fn overlay() -> emit_and_run::Overlay {
    FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

fn real_errors(errors: &[String]) -> Vec<&String> {
    errors
        .iter()
        .filter(|e| !e.contains("database_adapter not supported") && !e.contains("`has_many autosave: true` not supported"))
        .collect()
}

#[test]
#[ignore = "fork spec: postgresql"]
fn a_helper_declared_association_matches_native_rails_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "literal-association",
        &FILES,
        &emitted,
        "rh_spec_literal_association",
        "LiteralAssociationProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}
