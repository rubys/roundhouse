//! PostgreSQL: sql shapes, on the Ruby target against native Rails.
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

CREATE TABLE readings (
    id bigserial NOT NULL,
    name character varying,
    payload jsonb,
    CONSTRAINT readings_pkey PRIMARY KEY (id)
);

CREATE UNIQUE INDEX index_readings_on_name ON readings USING btree (name);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_sql_shapes
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const POST: &str = "class Post < ApplicationRecord\n  has_many :comments\nend\n";
const USER: &str = "class User < ApplicationRecord\n  has_many :comments\nend\n";
const COMMENT: &str = "class Comment < ApplicationRecord\n  belongs_to :post\n  belongs_to :user\nend\n";
const READING: &str = "class Reading < ApplicationRecord\n  attribute :payload, default: {}\nend\n";

const PROBE: &str = r##"class SqlShapesProbe
  AGG = "LEFT JOIN (SELECT user_id, COUNT(*) AS n FROM comments GROUP BY user_id) AS agg ON agg.user_id = users.id"

  def self.lines
    out = []
    ann = User.create!(name: "ann")
    bob = User.create!(name: "bob")
    one = Post.create!(title: "one")
    two = Post.create!(title: "two")
    Comment.create!(post_id: one.id, user_id: ann.id, body: "a")
    Comment.create!(post_id: one.id, user_id: bob.id, body: "b")
    Comment.create!(post_id: two.id, user_id: ann.id, body: "c")
    # Rails joins `comments` and `users` as one tree, ahead of the string join
    # that names `users`; the lowering renders each call where it is written.
    rows = Post.joins(:comments).joins(AGG).joins(comments: :user).order("posts.title", "users.name").pluck("posts.title", "users.name", "agg.n")
    out << "late table: #{rows.map { |r| r.join("/") }.join(",")}"
    out << "first alias: #{Post.joins(comments: :user).joins(AGG).order("posts.title", "users.name").pluck("agg.n").join(",")}"

    out << "same hop once: #{Post.joins(:comments).joins(comments: :user).count} #{Post.joins(:comments).joins(comments: :user).where(users: { name: "bob" }).pluck(:title).join(",")}"
    out << "pluck hash: #{Comment.joins(:post).order("comments.id").pluck(posts: :title).join(",")} #{Comment.joins(:post).order("comments.id").pluck(posts: [:title], comments: :body).map { |r| r.join("/") }.join(",")}"

    seen = []
    Comment.order(:id).in_batches(of: 2).each_record { |c| seen << c.body }
    out << "each_record: #{seen.join(",")}"
    Comment.where(post_id: one.id).order(:id).in_batches(of: 1).each_record { |c| seen << c.body }
    out << "each_record scoped: #{seen.join(",")}"

    empty = Reading.create!(name: "empty")
    full = Reading.create!(name: "full", payload: { sessions: 100, platform: "google" })
    Reading.upsert_all([{ name: "empty", payload: { "x" => 1 } }, { name: "new", payload: {} }], unique_by: :name, on_duplicate: :skip)
    out << "upsert skip: #{Reading.order(:name).pluck(:name).join(",")} #{empty.reload.payload.inspect}"
    out << "json default: #{Reading.new.payload.inspect}"
    out << "json empty: #{empty.reload.payload.inspect}"
    out << "json full: #{full.reload.payload["sessions"]} #{full.payload["platform"]}"
    full.update!(payload: { "sessions" => 7, "tags" => ["a", "b"] })
    out << "json update: #{Reading.find(full.id).payload.inspect}"
    out
  end
end
"##;

const EXPECTED: [&str; 11] = [
    "late table: one/ann/2,one/bob/1,two/ann/2",
    "first alias: 2,1,2",
    "same hop once: 3 one",
    "pluck hash: one,one,two one/a,one/b,two/c",
    "each_record: a,b,c",
    "each_record scoped: a,b,c,a,b",
    "upsert skip: empty,full,new {}",
    "json default: {}",
    "json empty: {}",
    "json full: 100 google",
    "json update: {\"tags\" => [\"a\", \"b\"], \"sessions\" => 7}",
];

const LOADS: [&str; 6] = [
    "app/models/application_record",
    "app/models/post",
    "app/models/user",
    "app/models/comment",
    "app/models/reading",
    "app/models/sql_shapes_probe",
];

const FILES: [(&str, &str); 10] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/post.rb", POST),
    ("app/models/user.rb", USER),
    ("app/models/comment.rb", COMMENT),
    ("app/models/reading.rb", READING),
    ("app/models/sql_shapes_probe.rb", PROBE),
];

fn overlay() -> emit_and_run::Overlay {
    FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

#[test]
#[ignore = "fork spec: postgresql"]
fn relation_sql_shapes_match_native_rails_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "sql-shapes",
        &FILES,
        &emitted,
        "rh_spec_sql_shapes",
        "SqlShapesProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}
