//! ancestry 4.3.3, compiled from its source as a source gem. Its
//! `has_ancestry` is a macro the class macro evaluator runs: options
//! checks, `cattr_accessor`s with their defaults, a format chosen by a
//! module class variable no one sets, `include`/`extend` of the gem's
//! modules, callbacks and depth scopes. The modules the model extends
//! become its class methods, so a relation reaches `ancestors_of` and
//! `scope_depth` as Rails' relation does.

use std::path::Path;

use super::emit_and_run;

const GEM: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/ancestry_gem");

fn with_gem(mut overlay: emit_and_run::Overlay, dir: &Path, rel: &str) -> emit_and_run::Overlay {
    let mut entries: Vec<_> = std::fs::read_dir(dir).expect("read ancestry_gem").map(|e| e.unwrap().path()).collect();
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
            "    ancestry (4.3.3)\n      activerecord (>= 5.2.6)\n      logger\n    debug (1.11.1)\n",
        )
        .edit("Gemfile.lock", "DEPENDENCIES\n", "DEPENDENCIES\n  ancestry\n")
        .write(".bundle/config", "---\nBUNDLE_PATH: \"vendor/bundle\"\n")
        .write("roundhouse.yml", "extensions:\n  - config/roundhouse\n")
        .write("config/roundhouse/gems.yml", "source_gems:\n  - {gem: \"ancestry\"}\n")
        .edit(
            "db/schema.rb",
            "  create_table \"articles\", force: :cascade do |t|\n",
            "  create_table \"articles\", force: :cascade do |t|\n    t.string \"ancestry\"\n",
        )
        .edit("app/models/article.rb", "class Article < ApplicationRecord\n", "class Article < ApplicationRecord\n  has_ancestry\n");
    with_gem(overlay, Path::new(GEM), "vendor/bundle/ruby/3.4.0/gems/ancestry-4.3.3")
}

#[test]
fn has_ancestry_builds_and_walks_a_tree_from_the_gem_source() {
    app()
        .run_ruby(
            r##"
def check(label, got, want)
  raise "#{label}: got #{got.inspect}, want #{want.inspect}" unless got == want
end
def titles(records) = records.to_a.map(&:title)
root = Article.create!(title: "Root", body: "body number one is long enough")
child = Article.new(title: "Child", body: "body number two is long enough")
child.parent = root
child.save!
grand = Article.new(title: "Grand", body: "body number three is long enough")
grand.parent = child
grand.save!
check("ancestry column", [root.ancestry, child.ancestry, grand.ancestry], [nil, root.id.to_s, "#{root.id}/#{child.id}"])
check("parent", [root.parent, grand.parent&.title], [nil, "Child"])
check("root", grand.root.title, "Root")
check("ancestors", titles(grand.ancestors), ["Root", "Child"])
check("path", titles(grand.path), ["Root", "Child", "Grand"])
check("children", titles(root.children), ["Child"])
check("descendants", titles(root.descendants), ["Child", "Grand"])
check("subtree", titles(child.subtree), ["Child", "Grand"])
check("siblings", titles(child.siblings), ["Child"])
check("depth", [root.depth, child.depth, grand.depth], [0, 1, 2])
check("predicates", [root.is_root?, root.has_children?, grand.is_childless?], [true, true, true])
check("roots", titles(Article.roots), ["Root"])
puts "ok"
"##,
        )
        .assert_passes();
}
