//! `db:migrate` for the emitted app: each migration recorded as Rails runs
//! it (version, schema steps), rendered to the SQLite statements Rails'
//! SQLite adapter issues, and stamped into db/seed.sql as
//! `rails db:schema:load` records it. The emitted-run half (a database
//! built at one migration and brought forward by the binary's
//! `db:migrate`) is tests/spinel_toolchain/db_migrate.rs.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::emit::shared::schema_sql::render_migration_step;
use roundhouse::ingest::schema::{record_migration, MigrationFold};
use roundhouse::schema::{Migration, MigrationStep};

/// real-blog's migration versions, oldest first, read from the fixture:
/// `bin/rh fixture` generates it, so the timestamps are the day it ran.
fn real_blog_versions() -> Vec<String> {
    let dir = roundhouse::fixtures::real_blog().join("db/migrate");
    let mut versions: Vec<String> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .filter_map(|entry| {
            let name = entry.ok()?.file_name().to_string_lossy().into_owned();
            let version: String = name.chars().take_while(|c| c.is_ascii_digit()).collect();
            (!version.is_empty() && name.ends_with(".rb")).then_some(version)
        })
        .collect();
    versions.sort();
    assert_eq!(versions.len(), 2, "real-blog has two migrations: {versions:?}");
    versions
}

fn record(files: &[(&str, &str)]) -> Vec<Migration> {
    let mut scratch = MigrationFold::default();
    files
        .iter()
        .map(|(name, src)| record_migration(src.as_bytes(), name, &mut scratch))
        .collect()
}

fn sql(m: &Migration) -> Vec<String> {
    m.steps
        .iter()
        .flat_map(|s| render_migration_step(s).expect("renders"))
        .collect()
}

const CREATE_ARTICLES: &str = r#"
class CreateArticles < ActiveRecord::Migration[8.1]
  def change
    create_table :articles do |t|
      t.string :title
      t.timestamps
    end
  end
end
"#;

const CREATE_COMMENTS: &str = r#"
class CreateComments < ActiveRecord::Migration[8.1]
  def change
    create_table :comments do |t|
      t.references :article, null: false, foreign_key: true
      t.text :body
    end
  end
end
"#;

#[test]
fn each_migration_keeps_its_version_and_steps_in_order() {
    let ms = record(&[
        (
            "db/migrate/20260101000001_create_articles.rb",
            CREATE_ARTICLES,
        ),
        (
            "db/migrate/20260101000002_create_comments.rb",
            CREATE_COMMENTS,
        ),
    ]);
    assert_eq!(
        ms.iter().map(|m| m.version.as_str()).collect::<Vec<_>>(),
        ["20260101000001", "20260101000002"]
    );
    assert!(ms.iter().all(|m| m.not_renderable.is_none()));
    assert!(matches!(
        &ms[0].steps[..],
        [MigrationStep::CreateTable { .. }]
    ));
    let create = sql(&ms[0]);
    assert!(
        create[0].starts_with("CREATE TABLE articles ("),
        "{create:?}"
    );
    // a migration's create_table fails on an existing table, as under Rails
    assert!(!create[0].contains("IF NOT EXISTS"));
}

#[test]
fn a_reference_is_an_id_column_with_its_index_and_foreign_key() {
    let ms = record(&[
        (
            "db/migrate/20260101000001_create_articles.rb",
            CREATE_ARTICLES,
        ),
        (
            "db/migrate/20260101000002_create_comments.rb",
            CREATE_COMMENTS,
        ),
    ]);
    let out = sql(&ms[1]).join("\n");
    assert!(out.contains("article_id INTEGER NOT NULL"), "{out}");
    assert!(
        out.contains("FOREIGN KEY (article_id) REFERENCES articles (id)"),
        "{out}"
    );
    assert!(
        out.contains("CREATE INDEX index_comments_on_article_id ON comments (article_id)"),
        "{out}"
    );
}

#[test]
fn add_column_is_in_place_and_change_column_default_rebuilds_keeping_rows() {
    let ms = record(&[
        (
            "db/migrate/20260101000001_create_articles.rb",
            CREATE_ARTICLES,
        ),
        (
            "db/migrate/20260101000002_create_comments.rb",
            CREATE_COMMENTS,
        ),
        (
            "db/migrate/20260102000001_more.rb",
            r#"
class More < ActiveRecord::Migration[8.1]
  def change
    add_column :articles, :subtitle, :string, default: "", null: false
    change_column_default :comments, :body, from: nil, to: "none"
  end
end
"#,
        ),
    ]);
    let out = sql(&ms[2]);
    assert_eq!(
        out[0],
        "ALTER TABLE articles ADD COLUMN subtitle TEXT NOT NULL DEFAULT ''"
    );
    let rest = out[1..].join("\n");
    // SQLite's procedure: new definition, rows copied, old dropped, renamed
    // into place, indexes and the foreign key kept
    assert!(rest.contains("PRAGMA defer_foreign_keys = ON"), "{rest}");
    assert!(rest.contains("CREATE TABLE comments__rh_alter ("), "{rest}");
    assert!(rest.contains("body TEXT DEFAULT 'none'"), "{rest}");
    assert!(
        rest.contains("FOREIGN KEY (article_id) REFERENCES articles (id)"),
        "{rest}"
    );
    assert!(
        rest.contains("INSERT INTO comments__rh_alter (id, article_id, body) SELECT id, article_id, body FROM comments"),
        "{rest}"
    );
    assert!(
        rest.contains("DROP TABLE comments\nALTER TABLE comments__rh_alter RENAME TO comments"),
        "{rest}"
    );
    assert!(
        rest.contains("CREATE INDEX index_comments_on_article_id ON comments (article_id)"),
        "{rest}"
    );
}

#[test]
fn rename_column_renames_the_index_named_for_it() {
    let ms = record(&[
        (
            "db/migrate/20260101000001_create_posts.rb",
            r#"
class CreatePosts < ActiveRecord::Migration[8.1]
  def change
    create_table :posts do |t|
      t.string :titel
      t.index :titel
    end
  end
end
"#,
        ),
        (
            "db/migrate/20260101000002_fix.rb",
            r#"
class Fix < ActiveRecord::Migration[8.1]
  def change
    rename_column :posts, :titel, :title
  end
end
"#,
        ),
    ]);
    assert_eq!(
        sql(&ms[1]),
        [
            "ALTER TABLE posts RENAME COLUMN titel TO title",
            "DROP INDEX index_posts_on_titel",
            "CREATE INDEX index_posts_on_title ON posts (title)",
        ]
    );
}

#[test]
fn a_literal_data_change_runs_and_anything_else_is_not_renderable() {
    let ms = record(&[
        (
            "db/migrate/20260101000001_create_articles.rb",
            CREATE_ARTICLES,
        ),
        (
            "db/migrate/20260101000002_backfill.rb",
            r#"
class Backfill < ActiveRecord::Migration[8.1]
  def up
    execute "UPDATE articles SET title = 'untitled' WHERE title IS NULL"
  end
end
"#,
        ),
        (
            "db/migrate/20260101000003_ddl.rb",
            r#"
class Ddl < ActiveRecord::Migration[8.1]
  def up
    execute "ALTER TABLE articles ADD COLUMN x TEXT"
  end
end
"#,
        ),
        (
            "db/migrate/20260101000004_ruby.rb",
            r#"
class Ruby < ActiveRecord::Migration[8.1]
  def up
    add_column :articles, :rank, :integer
    Article.update_all(rank: 1)
  end
end
"#,
        ),
    ]);
    assert_eq!(
        sql(&ms[1]),
        ["UPDATE articles SET title = 'untitled' WHERE title IS NULL"]
    );
    assert!(
        ms[2].not_renderable.is_some(),
        "DDL through execute is not followed"
    );
    let why = ms[3]
        .not_renderable
        .as_deref()
        .expect("a Ruby backfill is not renderable");
    assert!(why.contains("Article.update_all"), "{why}");
}

/// A table changed by a migration the fold does not follow (here
/// `change_table`) is not known afterwards: a later migration that would
/// rebuild it from the folded state would drop the column the database
/// has, so it is not renderable, and `db:migrate` stops there.
#[test]
fn a_rebuild_after_an_unfollowed_change_to_its_table_is_not_renderable() {
    let ms = record(&[
        ("db/migrate/20260101000001_create_articles.rb", CREATE_ARTICLES),
        (
            "db/migrate/20260101000002_change.rb",
            r#"
class Change < ActiveRecord::Migration[8.1]
  def change
    change_table :articles do |t|
      t.string :subtitle
    end
  end
end
"#,
        ),
        (
            "db/migrate/20260101000003_default.rb",
            r#"
class Default < ActiveRecord::Migration[8.1]
  def change
    change_column_default :articles, :title, from: nil, to: "untitled"
    add_column :articles, :rank, :integer
  end
end
"#,
        ),
    ]);
    let why = ms[2].not_renderable.as_deref().expect("the rebuild is refused");
    assert!(why.contains("the state of `articles` before this migration is not known"), "{why}");
    assert!(why.contains("20260101000002_change.rb"), "{why}");
}

/// `rename_table` on SQLite rewrites the foreign keys that name the table
/// (Rails 8.1.4's schema then names the new table), so a later rebuild of
/// the referencing table keeps a foreign key that resolves.
#[test]
fn rename_table_moves_the_foreign_keys_that_name_it() {
    let ms = record(&[
        ("db/migrate/20260101000001_create_articles.rb", CREATE_ARTICLES),
        ("db/migrate/20260101000002_create_comments.rb", CREATE_COMMENTS),
        (
            "db/migrate/20260101000003_rename.rb",
            r#"
class Rename < ActiveRecord::Migration[8.1]
  def change
    rename_table :articles, :posts
    change_column_default :comments, :body, from: nil, to: "none"
  end
end
"#,
        ),
    ]);
    let out = sql(&ms[2]).join("\n");
    assert!(out.contains("FOREIGN KEY (article_id) REFERENCES posts (id)"), "{out}");
    assert!(!out.contains("REFERENCES articles"), "{out}");
}

/// `add_reference ..., foreign_key: true`: SQLite cannot add the
/// constraint in place, so Rails' adapter rebuilds the table
/// (alter_table) and the database has the foreign key, as here.
#[test]
fn add_reference_with_a_foreign_key_rebuilds_the_table() {
    let ms = record(&[
        ("db/migrate/20260101000001_create_articles.rb", CREATE_ARTICLES),
        (
            "db/migrate/20260101000002_create_notes.rb",
            r#"
class CreateNotes < ActiveRecord::Migration[8.1]
  def change
    create_table :notes do |t|
      t.text :body
    end
  end
end
"#,
        ),
        (
            "db/migrate/20260101000003_reference.rb",
            r#"
class Reference < ActiveRecord::Migration[8.1]
  def change
    add_reference :notes, :article, foreign_key: true
  end
end
"#,
        ),
    ]);
    assert!(matches!(&ms[2].steps[..], [MigrationStep::RebuildTable { .. }]), "{:?}", ms[2].steps);
    let out = sql(&ms[2]).join("\n");
    assert!(out.contains("FOREIGN KEY (article_id) REFERENCES articles (id)"), "{out}");
    assert!(out.contains("INSERT INTO notes__rh_alter (id, body) SELECT id, body FROM notes"), "{out}");
    assert!(out.contains("CREATE INDEX index_notes_on_article_id ON notes (article_id)"), "{out}");
}

#[test]
fn seed_sql_records_the_versions_the_schema_includes_as_schema_load_does() {
    let mut app = roundhouse::App::default();
    app.migrations = record(&[
        (
            "db/migrate/20260101000001_create_articles.rb",
            CREATE_ARTICLES,
        ),
        (
            "db/migrate/20260101000002_create_comments.rb",
            CREATE_COMMENTS,
        ),
        ("db/migrate/20260101000003_newer.rb", CREATE_COMMENTS),
    ]);
    // schema.rb at version 20260101000002: the third is newer
    app.migrations[0].in_schema = true;
    app.migrations[1].in_schema = true;
    let stamps = roundhouse::emit::shared::seed_sql::render_migration_stamps(&app);
    assert!(
        stamps.contains("CREATE TABLE IF NOT EXISTS \"schema_migrations\""),
        "{stamps}"
    );
    assert!(
        stamps.contains("VALUES ('20260101000001'), ('20260101000002');"),
        "{stamps}"
    );
    assert!(
        stamps.contains("('environment', 'development',"),
        "{stamps}"
    );
    assert_eq!(
        roundhouse::emit::shared::seed_sql::render_migration_stamps(&roundhouse::App::default()),
        ""
    );
}

#[test]
fn the_emitted_ruby_app_sets_up_migrates_and_rolls_back_as_rails_does() {
    // real-blog without schema.rb: its own migrations are the schema. The
    // harness boots on a fresh in-memory database, which `boot` sets up.
    emit_and_run::real_blog()
        .remove("db/schema.rb")
        .run_ruby(
            r#"m = ActiveRecord::Migrator
all = Migrations.all
raise "set up at boot" unless m.set_up?
raise "every version recorded: #{m.applied_versions.inspect}" unless m.applied_versions == all.map { |r| r[0] }
raise "nothing pending" unless m.pending(all).empty?
more = all + [["20991231000001", "", "ALTER TABLE articles ADD COLUMN subtitle TEXT NOT NULL DEFAULT ''"]]
raise "one applied" unless m.migrate(more, "development") == 1
raise "recorded" unless m.applied_versions.include?("20991231000001")
raise "column" unless ActiveRecord.adapter.select_rows("SELECT subtitle FROM articles").is_a?(Array)
# its first statement succeeds, its second fails: the first is rolled back
broken = more + [["20991231000002", "", "ALTER TABLE articles ADD COLUMN rank INTEGER", "ALTER TABLE no_such_table ADD COLUMN x TEXT"]]
begin
  m.migrate(broken, "development")
  raise "a failing migration must raise"
rescue ActiveRecord::MigrationError => e
  raise "names it: #{e.message}" unless e.message.include?("20991231000002") && e.message.include?("rolled back")
end
raise "not recorded" if m.applied_versions.include?("20991231000002")
columns = ActiveRecord.adapter.select_rows("SELECT name FROM pragma_table_info('articles')").map { |r| r["name"] }
raise "rolled back: #{columns.inspect}" if columns.include?("rank")
gap = more + [["20991231000003", "`execute` is not a change roundhouse renders as SQL"]]
begin
  m.migrate(gap, "development")
  raise "an unrenderable migration must stop"
rescue ActiveRecord::MigrationError => e
  raise "says why: #{e.message}" unless e.message.include?("cannot run here")
end
"#,
        )
        .assert_passes();
}

#[test]
fn a_schema_rb_older_than_db_migrate_leaves_the_newer_migrations_pending() {
    // real-blog's schema.rb is at its last migration; one more is newer.
    // Rails loads schema.rb, records the versions up to its `version:`, and
    // `db:prepare` then runs the newer one.
    let script = r#"m = ActiveRecord::Migrator
raise "schema versions: #{Migrations.schema_versions.inspect}" unless Migrations.schema_versions == VERSIONS
# boot loaded schema.rb and recorded its versions only
raise "applied: #{m.applied_versions.inspect}" unless m.applied_versions.sort == Migrations.schema_versions
raise "pending" unless m.pending(Migrations.all).map { |r| r[0] } == ["20991231000001"]
environment = ActiveRecord.adapter.select_rows("SELECT value FROM ar_internal_metadata WHERE key = 'environment'").map { |r| r["value"] }
raise "environment: #{environment.inspect}" unless environment == [Rails.env.to_s] && Rails.env.to_s == "development"
raise "one applied" unless m.prepare(Schema.statements, Migrations.schema_versions, Migrations.all, "development") == 1
columns = ActiveRecord.adapter.select_rows("SELECT name FROM pragma_table_info('articles')").map { |r| r["name"] }
raise "subtitle: #{columns.inspect}" unless columns.include?("subtitle")
"#
    .replace("VERSIONS", &format!("{:?}", real_blog_versions()));
    emit_and_run::real_blog()
        .write(
            "db/migrate/20991231000001_add_subtitle_to_articles.rb",
            "class AddSubtitleToArticles < ActiveRecord::Migration[8.1]\n  def change\n    add_column :articles, :subtitle, :string\n  end\nend\n",
        )
        .run_ruby(&script)
        .assert_passes();
}

#[test]
fn structure_sql_includes_the_versions_it_inserts() {
    // the dump ends with the versions it was taken at; here only the
    // first of real-blog's two
    let [first, second] = <[String; 2]>::try_from(real_blog_versions()).expect("two versions");
    let dump = include_str!("support/real_blog_structure.sql")
        .replace("('20261002052542'),\n('20261002052541');", &format!("('{first}');"));
    let script = r#"raise "schema versions: #{Migrations.schema_versions.inspect}" unless Migrations.schema_versions == ["FIRST"]
raise "pending" unless ActiveRecord::Migrator.pending(Migrations.all).map { |r| r[0] } == ["SECOND"]
"#
    .replace("FIRST", &first)
    .replace("SECOND", &second);
    emit_and_run::real_blog()
        .remove("db/schema.rb")
        .write("db/structure.sql", &dump)
        .run_ruby(&script)
        .assert_passes();
}
