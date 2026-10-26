//! Not rubygems' 0.2.1: forem's fork (06393d3) stores an STI base class name; answers are ActiveRecord 8.1.4's.

use std::path::Path;

use super::emit_and_run;

const GEM: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/acts_as_follower_gem");

fn with_gem(mut overlay: emit_and_run::Overlay, dir: &Path, rel: &str) -> emit_and_run::Overlay {
    let mut entries: Vec<_> = std::fs::read_dir(dir).expect("read acts_as_follower_gem").map(|e| e.unwrap().path()).collect();
    entries.sort();
    for path in entries {
        let name = path.file_name().unwrap().to_str().unwrap();
        let rel = format!("{rel}/{name}");
        overlay = if path.is_dir() {
            with_gem(overlay, &path, &rel)
        } else {
            overlay.write(&rel, &std::fs::read_to_string(&path).unwrap())
        };
    }
    overlay
}

fn app() -> emit_and_run::Overlay {
    let overlay = emit_and_run::real_blog()
        .edit(
            "Gemfile.lock",
            "    debug (1.11.1)\n",
            "    acts_as_follower (0.2.1)\n      activerecord (>= 4.0)\n    debug (1.11.1)\n",
        )
        .edit("Gemfile.lock", "DEPENDENCIES\n", "DEPENDENCIES\n  acts_as_follower\n")
        .write(".bundle/config", "---\nBUNDLE_PATH: \"vendor/bundle\"\n")
        .write("roundhouse.yml", "extensions:\n  - config/roundhouse\n")
        .write("config/roundhouse/gems.yml", "source_gems:\n  - {gem: \"acts_as_follower\"}\n")
        .edit(
            "db/schema.rb",
            "  create_table \"articles\", force: :cascade do |t|\n",
            "  create_table \"follows\", force: :cascade do |t|\n    \
             t.string \"followable_type\", null: false\n    \
             t.integer \"followable_id\", null: false\n    \
             t.string \"follower_type\", null: false\n    \
             t.integer \"follower_id\", null: false\n    \
             t.boolean \"blocked\", default: false, null: false\n    \
             t.datetime \"created_at\", null: false\n    \
             t.datetime \"updated_at\", null: false\n    \
             t.index [\"followable_id\", \"followable_type\", \"follower_id\", \"follower_type\"], name: \"index_follows_on_followable_and_follower\", unique: true\n  \
             end\n\n  create_table \"articles\", force: :cascade do |t|\n",
        )
        .write(
            "app/models/follow.rb",
            "class Follow < ApplicationRecord\n  \
             extend ActsAsFollower::FollowerLib\n  \
             extend ActsAsFollower::FollowScopes\n\n  \
             belongs_to :followable, polymorphic: true\n  \
             belongs_to :follower, polymorphic: true\n\n  \
             def block!\n    self.update_attribute(:blocked, true)\n  end\nend\n",
        )
        .edit("app/models/article.rb", "class Article < ApplicationRecord\n", "class Article < ApplicationRecord\n  acts_as_followable\n")
        .edit("app/models/comment.rb", "class Comment < ApplicationRecord\n", "class Comment < ApplicationRecord\n  acts_as_follower\n");
    with_gem(overlay, Path::new(GEM), "vendor/bundle/ruby/3.4.0/gems/acts_as_follower-0.2.1")
}

#[test]
fn follows_through_the_gem_source_as_rails_does() {
    let run = app().run_ruby(
        r##"
def check(label, got, want)
  raise "#{label}: got #{got.inspect}, want #{want.inspect}" unless got == want
end
a = Article.create!(title: "Hello world", body: "body number one is long enough")
b = Article.create!(title: "Second post", body: "body number two is long enough")
c = Comment.create!(article: a, commenter: "x", body: "hi")
first = c.follow(a)
check("followable_type", first.followable_type, "Article")
check("follow again finds the row", [c.follow(a).id, Follow.count], [first.id, 1])
check("following?", [c.following?(a), c.following?(b)], [true, false])
check("follow_count", c.follow_count, 1)
check("follows_by_type", c.follows_by_type("Article").map(&:followable_id), [a.id])
check("all_following", c.all_following.map(&:id), [a.id])
check("followers", a.followers.map(&:id), [c.id])
check("followers_count", a.followers_count, 1)
check("followed_by?", a.followed_by?(c), true)
a.block(c)
check("blocked", [a.followed_by?(c), a.blocked_followers_count, Follow.first.blocked], [false, 1, true])
a.unblock(c)
check("unblocked", [a.followed_by?(c), Follow.count], [false, 0])
c.follow(b)
c.stop_following(b)
check("stop_following", [c.following?(b), Follow.count], [false, 0])
puts "ok"
"##,
    );
    let mut reported: Vec<&str> = run
        .errors
        .iter()
        .map(|e| if e.contains("ActiveSupport::Deprecation") { "deprecation" } else if e.contains("association join") { "join" } else { e.as_str() })
        .collect();
    reported.sort();
    assert_eq!(reported, ["deprecation", "deprecation", "join", "join"], "{:#?}", run.errors);
    assert!(run.success, "{}\n{}", run.stdout, run.stderr);
}
