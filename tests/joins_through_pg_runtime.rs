//! PostgreSQL: joins through, on the Ruby target against native Rails.
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

CREATE TABLE users (
    id bigserial NOT NULL,
    name character varying,
    CONSTRAINT users_pkey PRIMARY KEY (id)
);

CREATE TABLE comments (
    id bigserial NOT NULL,
    post_id bigint,
    user_id bigint,
    body character varying,
    CONSTRAINT comments_pkey PRIMARY KEY (id)
);

CREATE TABLE taggings (
    id bigserial NOT NULL,
    post_id bigint,
    tag_id bigint,
    CONSTRAINT taggings_pkey PRIMARY KEY (id)
);

CREATE TABLE tags (
    id bigserial NOT NULL,
    label character varying,
    CONSTRAINT tags_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_joins_through
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const POST: &str = r#"class Post < ApplicationRecord
  has_many :comments
  has_many :commenters, through: :comments, source: :user
  has_many :taggings
  has_many :tags, through: :taggings
end
"#;

const USER: &str = "class User < ApplicationRecord\n  has_many :comments\nend\n";
const COMMENT: &str = "class Comment < ApplicationRecord\n  belongs_to :post\n  belongs_to :user\nend\n";
const TAGGING: &str = "class Tagging < ApplicationRecord\n  belongs_to :post\n  belongs_to :tag\nend\n";
const TAG: &str = "class Tag < ApplicationRecord\n  has_many :taggings\n  has_many :posts, through: :taggings\nend\n";

const PROBE: &str = r##"class JoinsThroughProbe
  def self.lines
    out = []
    ann = User.create!(name: "ann")
    bob = User.create!(name: "bob")
    one = Post.create!(title: "one")
    two = Post.create!(title: "two")
    Post.create!(title: "bare")
    Comment.create!(post_id: one.id, user_id: ann.id, body: "a")
    Comment.create!(post_id: one.id, user_id: bob.id, body: "b")
    Comment.create!(post_id: two.id, user_id: ann.id, body: "c")
    red = Tag.create!(label: "red")
    Tagging.create!(post_id: one.id, tag_id: red.id)
    out << "has_many: #{Post.joins(:comments).count} #{Post.joins(:comments).order("posts.id", "comments.id").pluck(:title).join(",")}"
    out << "through: #{Post.joins(:commenters).count} #{Post.joins(:commenters).where(users: { name: "bob" }).pluck(:title).join(",")}"
    out << "through tags: #{Post.joins(:tags).where(tags: { label: "red" }).pluck(:title).join(",")} #{Tag.joins(:posts).pluck(:label).join(",")}"
    out << "hash: #{Post.joins(comments: :user).where(users: { name: "ann" }).order(:id).pluck(:title).join(",")}"
    out << "left: #{Post.left_joins(:comments).where(comments: { id: nil }).pluck(:title).join(",")} #{Post.left_joins(:commenters).where(users: { id: nil }).pluck(:title).join(",")}"
    out << "reflection: #{Post.reflect_on_association(:commenters).through_reflection?} #{Post.reflect_on_association(:comments).through_reflection?} #{Post.reflect_on_association(:commenters).through_reflection.name}"
    out
  end
end
"##;

const EXPECTED: [&str; 6] = [
    "has_many: 3 one,one,two",
    "through: 3 one",
    "through tags: one red",
    "hash: one,two",
    "left: bare bare",
    "reflection: true false comments",
];

const LOADS: [&str; 7] = [
    "app/models/application_record",
    "app/models/post",
    "app/models/user",
    "app/models/comment",
    "app/models/tagging",
    "app/models/tag",
    "app/models/joins_through_probe",
];

const FILES: [(&str, &str); 11] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/post.rb", POST),
    ("app/models/user.rb", USER),
    ("app/models/comment.rb", COMMENT),
    ("app/models/tagging.rb", TAGGING),
    ("app/models/tag.rb", TAG),
    ("app/models/joins_through_probe.rb", PROBE),
];

fn overlay() -> emit_and_run::Overlay {
    FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

#[test]
#[ignore = "fork spec: postgresql"]
fn joins_over_through_reflections_match_native_rails_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "joins-through",
        &FILES,
        &emitted,
        "rh_spec_joins_through",
        "JoinsThroughProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}
