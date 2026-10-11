//! `--target futamura`, stage 0: the identity.
//!
//! The target's mandate is partial evaluation — specialize what the
//! analysis resolves, leave the rest as residual Ruby running on the
//! app's real gems. Stage 0 specializes nothing, so the residue is the
//! whole app and the output must be the app as source control holds it.
//! These gates exist before the first specialization so that every
//! later one is measured against the same contract: the app's own
//! suite, run by Rails, passes against the emitted tree.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("roundhouse-futamura-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(root: &Path, rel: &str, content: &[u8]) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";

/// A tree with source the identity must carry (dotfiles, `.keep`
/// placeholders, a binary asset, an executable) and run-time state it
/// must not (logs, databases, caches, the master key).
fn app_tree(root: &Path) {
    write(root, "Gemfile", b"source \"https://rubygems.org\"\ngem \"rails\"\n");
    write(root, ".ruby-version", b"ruby-4.0.2\n");
    write(root, ".gitignore", b"/log/*\n/tmp/*\n");
    write(root, "config/routes.rb", b"Rails.application.routes.draw do\n  get \"up\" => \"rails/health#show\", as: :rails_health_check\nend\n");
    write(root, "app/models/application_record.rb", b"class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n");
    write(root, "app/models/widget.rb", b"class Widget < ApplicationRecord\n  def method_missing(name, *args) = name\nend\n");
    write(root, "app/assets/images/logo.png", PNG);
    write(root, "bin/rails", b"#!/usr/bin/env ruby\n");
    write(root, "log/.keep", b"");
    write(root, "log/test.log", b"noise\n");
    write(root, "tmp/.keep", b"");
    write(root, "tmp/cache/x", b"cached\n");
    write(root, "storage/.keep", b"");
    write(root, "storage/test.sqlite3", b"SQLite format 3\0");
    write(root, "config/master.key", b"secret\n");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(root.join("bin/rails"), fs::Permissions::from_mode(0o755)).unwrap();
    }
}

#[test]
fn identity_carries_source_and_drops_run_time_state() {
    let src = scratch("src");
    let out = scratch("out");
    app_tree(&src);
    let status = Command::new(env!("CARGO_BIN_EXE_roundhouse"))
        .args(["--target", "futamura", "-o"])
        .arg(&out)
        .arg(&src)
        .output()
        .unwrap();
    assert!(status.status.success(), "{}", String::from_utf8_lossy(&status.stderr));
    // `method_missing` is outside what the strict targets type, and the
    // other targets drop Rails' own health controller; here both are
    // residue Rails runs, so neither is a diagnostic at all.
    let stderr = String::from_utf8_lossy(&status.stderr);
    assert!(!stderr.contains("error") && !stderr.contains("warning"), "{stderr}");

    for rel in [
        "Gemfile",
        ".ruby-version",
        ".gitignore",
        "config/routes.rb",
        "app/models/widget.rb",
        "app/assets/images/logo.png",
        "bin/rails",
        "log/.keep",
        "tmp/.keep",
        "storage/.keep",
    ] {
        assert_eq!(fs::read(out.join(rel)).ok(), fs::read(src.join(rel)).ok(), "{rel}");
    }
    for rel in ["log/test.log", "tmp/cache/x", "storage/test.sqlite3", "config/master.key"] {
        assert!(!out.join(rel).exists(), "{rel} is run-time state, not source");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(out.join("bin/rails")).unwrap().permissions().mode();
        assert_eq!(mode & 0o111, 0o111, "bin/rails must stay executable");
    }
    // The source app ships no README, so the target supplies one.
    assert!(fs::read_to_string(out.join("README.md")).unwrap().contains("bin/rails test"));

    fs::remove_dir_all(&src).unwrap();
    fs::remove_dir_all(&out).unwrap();
}

/// The behavioral gate: `scripts/futamura-suite` emits `fixtures/real-blog`
/// and runs its own suite with Rails against a copy of the source and
/// against the emitted tree; the two must give the same result. The
/// script is the gate's one definition, also used for apps outside the
/// repo (campfire). Needs the generated fixture and its bundle.
#[test]
#[ignore = "requires fixtures/real-blog and its Rails bundle"]
fn real_blog_suite_gives_the_same_result_on_source_and_emitted_tree() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let fixture = root.join("fixtures/real-blog");
    assert!(fixture.join("Gemfile.lock").exists(), "generate fixtures/real-blog first");
    let run = Command::new(root.join("scripts/futamura-suite"))
        .arg(&fixture)
        .env("ROUNDHOUSE_BIN", env!("CARGO_BIN_EXE_roundhouse"))
        .env_remove("BUNDLE_GEMFILE")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(
        run.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
    // The fixture's suite is green, so SAME must mean zero failures.
    assert!(stdout.contains(" 0 failures, 0 errors"), "{stdout}");
}

mod query_specialization {
    use std::collections::HashMap;
    use std::path::PathBuf;

    use roundhouse::analyze::Analyzer;
    use roundhouse::emit::futamura::{INITIALIZER, specialize};
    use roundhouse::ingest::ingest_app_from_tree;

    const SCHEMA: &str = r#"ActiveRecord::Schema.define do
  create_table "rooms", force: :cascade do |t|
    t.string "name", null: false
  end
  create_table "messages", force: :cascade do |t|
    t.integer "room_id", null: false
    t.integer "creator_id", null: false
    t.boolean "pinned", default: false
    t.datetime "created_at", null: false
  end
  create_table "users", force: :cascade do |t|
    t.string "name", null: false
  end
end
"#;

    const MESSAGE: &str = r#"class Message < ApplicationRecord
  PAGE_SIZE = 40

  belongs_to :room
  belongs_to :creator, class_name: "User"

  scope :ordered, -> { order(:created_at) }
  scope :with_creator, -> { preload(:creator) }
  scope :last_page, -> { ordered.last(PAGE_SIZE) }
  scope :before, ->(message) { where("created_at < ?", message.created_at) }
end
"#;

    const CONTROLLER: &str = r#"class MessagesController < ApplicationController
  def index
    @room = Room.find(params[:room_id])
    @messages = @room.messages.with_creator.last_page
    @pinned = Message.where(room_id: @room.id, pinned: true).order(created_at: :desc).first(3)
    @older = @room.messages.before(@messages.first).to_a
  end
end
"#;

    fn app_and_files() -> (roundhouse::App, Vec<(String, String)>) {
        let files: Vec<(&str, &str)> = vec![
            ("db/schema.rb", SCHEMA),
            ("app/models/room.rb", "class Room < ApplicationRecord\n  has_many :messages\nend\n"),
            ("app/models/user.rb", "class User < ApplicationRecord\nend\n"),
            ("app/models/message.rb", MESSAGE),
            ("app/controllers/messages_controller.rb", CONTROLLER),
        ];
        let tree: HashMap<PathBuf, Vec<u8>> =
            files.iter().map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec())).collect();
        let mut app = ingest_app_from_tree(tree).expect("ingest");
        Analyzer::new(&app).analyze(&mut app);
        (app, files.iter().map(|(p, c)| (p.to_string(), c.to_string())).collect())
    }

    fn file<'f>(files: &'f [(String, String)], path: &str) -> &'f str {
        &files.iter().find(|(p, _)| p == path).unwrap_or_else(|| panic!("{path} not emitted")).1
    }

    #[test]
    fn association_chain_through_scopes_becomes_a_statement_with_its_owner_bound() {
        let (app, mut files) = app_and_files();
        let report = specialize(&app, &mut files).unwrap();
        let controller = file(&files, "app/controllers/messages_controller.rb");
        let init = file(&files, INITIALIZER);
        // The call site binds the owner's id and keeps the chain as fallback.
        assert!(controller.contains(
            "@messages = Futamura.records(:controllers_messages_controller_4_17, [@room.id], owner: @room, \
             association: :messages) { @room.messages.with_creator.last_page }"
        ), "{controller}");
        // Static scopes stay calls for Rails to build; the terminal scope is
        // inlined, its constant qualified so the initializer can name it,
        // and `last(n)` follows Rails' FinderMethods: ordered_relation,
        // limit, reverse_order, then the rows reversed.
        assert!(init.contains(
            "Futamura.define(:controllers_messages_controller_4_17, Message, reverse: true) \
             { |p| Message.all.where(room_id: p.bind).with_creator.ordered.send(:ordered_relation).limit(Message::PAGE_SIZE).reverse_order }"
        ), "{init}");
        assert!(report.specialized.iter().any(|s| s.ends_with("messages_controller.rb:4:17")), "{:?}", report.specialized);
    }

    #[test]
    fn constant_rooted_chain_binds_request_values_and_bakes_literals() {
        let (app, mut files) = app_and_files();
        specialize(&app, &mut files).unwrap();
        let controller = file(&files, "app/controllers/messages_controller.rb");
        let init = file(&files, INITIALIZER);
        assert!(controller.contains("@pinned = Futamura.records(:controllers_messages_controller_5_15, [@room.id]) {"), "{controller}");
        assert!(init.contains(
            "Message.all.where(room_id: p.bind, pinned: true).order({ created_at: :desc }).send(:ordered_relation).limit(3) }"
        ), "{init}");
    }

    #[test]
    fn a_string_condition_over_a_record_is_left_to_rails_with_a_reason() {
        let (app, mut files) = app_and_files();
        let report = specialize(&app, &mut files).unwrap();
        let controller = file(&files, "app/controllers/messages_controller.rb");
        assert!(controller.contains("@older = @room.messages.before(@messages.first).to_a\n"), "{controller}");
        assert!(report.residue.iter().any(|r| r.contains("messages_controller.rb:6:")), "{:?}", report.residue);
    }
}

mod single_record_terminals {
    use std::collections::HashMap;
    use std::path::PathBuf;

    use roundhouse::analyze::Analyzer;
    use roundhouse::emit::futamura::{INITIALIZER, specialize};
    use roundhouse::ingest::ingest_app_from_tree;

    #[test]
    fn first_without_order_takes_rails_implicit_order_and_find_by_is_where_take() {
        let schema = "ActiveRecord::Schema.define do\n  create_table \"stories\", force: :cascade do |t|\n    t.string \"short_id\", null: false\n  end\nend\n";
        let controller = "class StoriesController < ApplicationController\n  def show\n    @story = Story.where(short_id: params[:id].to_s).first\n    @same = Story.find_by(short_id: params[:id] || params[:story_id])\n  end\nend\n";
        let files: Vec<(&str, &str)> = vec![
            ("db/schema.rb", schema),
            ("app/models/story.rb", "class Story < ApplicationRecord\nend\n"),
            ("app/controllers/stories_controller.rb", controller),
        ];
        let tree: HashMap<PathBuf, Vec<u8>> =
            files.iter().map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec())).collect();
        let mut app = ingest_app_from_tree(tree).expect("ingest");
        Analyzer::new(&app).analyze(&mut app);
        let mut files: Vec<(String, String)> = files.iter().map(|(p, c)| (p.to_string(), c.to_string())).collect();
        specialize(&app, &mut files).unwrap();
        let get = |path: &str| files.iter().find(|(p, _)| p == path).unwrap().1.clone();
        let init = get(INITIALIZER);
        let out = get("app/controllers/stories_controller.rb");
        assert!(init.contains("Story, one: true) { |p| Story.all.where(short_id: p.bind).send(:ordered_relation).limit(1) }"), "{init}");
        assert!(init.contains("Story, one: true) { |p| Story.all.where(short_id: p.bind).limit(1) }"), "{init}");
        assert!(out.contains("[params[:id].to_s]) { Story.where(short_id: params[:id].to_s).first }"), "{out}");
        assert!(out.contains("[params[:id] || params[:story_id]]) {"), "{out}");
    }
}

mod preload_scopes {
    use std::collections::HashMap;
    use std::path::PathBuf;

    use roundhouse::analyze::Analyzer;
    use roundhouse::emit::futamura::{INITIALIZER, specialize};
    use roundhouse::ingest::ingest_app_from_tree;

    /// The runtime preloader builds an association's scope once, so only a
    /// scope Roundhouse can see is built from literals is certified to it.
    #[test]
    fn literal_association_scopes_are_certified_and_request_dependent_ones_are_not() {
        let schema = "ActiveRecord::Schema.define do\n  create_table \"users\", force: :cascade do |t|\n    t.string \"name\"\n  end\n  create_table \"posts\", force: :cascade do |t|\n    t.integer \"user_id\"\n    t.boolean \"published\"\n    t.integer \"viewer_id\"\n  end\nend\n";
        let user = "class User < ApplicationRecord\n  has_many :posts\n  has_many :published_posts, -> { where(published: true).order(:id) }, class_name: \"Post\"\n  has_many :seen_posts, -> { where(viewer_id: Current.user.id) }, class_name: \"Post\"\nend\n";
        let controller = "class UsersController < ApplicationController\n  def index\n    @users = User.preload(:published_posts).order(:id).first(10)\n  end\nend\n";
        let files: Vec<(&str, &str)> = vec![
            ("db/schema.rb", schema),
            ("app/models/user.rb", user),
            ("app/models/post.rb", "class Post < ApplicationRecord\n  belongs_to :user\nend\n"),
            ("app/controllers/users_controller.rb", controller),
        ];
        let tree: HashMap<PathBuf, Vec<u8>> =
            files.iter().map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec())).collect();
        let mut app = ingest_app_from_tree(tree).expect("ingest");
        Analyzer::new(&app).analyze(&mut app);
        let mut files: Vec<(String, String)> = files.iter().map(|(p, c)| (p.to_string(), c.to_string())).collect();
        specialize(&app, &mut files).unwrap();
        let init = &files.iter().find(|(p, _)| p == INITIALIZER).expect("initializer").1;
        assert!(init.contains("Futamura::Preload.static_scopes(\"User#published_posts\")\n"), "{init}");
        assert!(!init.contains("User#seen_posts"), "{init}");
    }
}
