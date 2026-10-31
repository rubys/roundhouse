//! Paired regressions for finder ownership, STI hydration and form parameter keys.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::analyze::Analyzer;
use roundhouse::project::BuildTarget;

fn native(components: &[&str], program: &str) {
    let oracle = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/support/rails_oracle.rb");
    let gems = components.iter().map(|name| format!("{name:?}")).collect::<Vec<_>>().join(", ");
    let probe = emit_and_run::ruby().env("RH_RAILS_ORACLE", "8.1.4")
        .args(["-e", &format!("require {oracle:?}; puts RailsOracle.available?({gems}, sqlite3: \">= 2.9\")")])
        .output().expect("oracle availability");
    let available = String::from_utf8_lossy(&probe.stdout).trim().to_owned();
    assert!(probe.status.success() && matches!(available.as_str(), "true" | "false"),
        "oracle availability: {available}\n{}", String::from_utf8_lossy(&probe.stderr));
    if available == "false" {
        eprintln!("skipping native Rails 8.1.4 oracle: missing {components:?}");
        return;
    }
    let program = format!("require {oracle:?}\nRailsOracle.activate({gems}, sqlite3: \">= 2.9\")\nrequire 'tmpdir'\nDir.mktmpdir('roundhouse-finder-oracle') do |root|\nDir.mkdir(File.join(root, 'lib'))\nDir.chdir(root) do\n{program}\nend\nend\n");
    let result = emit_and_run::ruby().env("RH_RAILS_ORACLE", "8.1.4")
        .args(["-e", &program]).output().expect("native Rails oracle");
    assert!(result.status.success(), "native Rails 8.1.4: {}", String::from_utf8_lossy(&result.stderr));
}

const AR: &str = "require 'active_record'\nActiveRecord::Base.establish_connection(adapter: 'sqlite3', database: ':memory:')\nActiveRecord::Schema.verbose = false\n";
const CATEGORIES: &str = "ActiveRecord::Schema.define do\ncreate_table(:categories) { |t| t.string :name }\nend\n";

#[test]
fn extension_include_hooks_keep_finder_returns_unknown() {
    let hook = "module FinderHook\ndef self.included(klass)\nklass.define_method(:find_by) { |attributes| \"extension hook\" }\nklass.define_method(:find_by!) { |attributes| \"extension hook\" }\nend\nend\n";
    let extension = "module FinderExtension\ninclude FinderHook\nend\n";
    let category = "class Category < ActiveRecord::Base\nextend FinderExtension\nend\n";
    let actions = "class CategoriesController < ActionController::Base\ndef dynamic; Category.find_by_name(\"found\"); end\ndef ordinary; Category.find_by(name: \"found\"); end\ndef bang; Category.find_by_name!(\"found\"); end\ndef reflected; Category.public_send(:find_by_name, \"found\"); end\nend\n";
    native(&["activerecord"], &format!("{AR}{CATEGORIES}{hook}{extension}{category}\n[:find_by_name, :find_by_name!].each {{ |name| raise name.to_s unless Category.public_send(name, 'found') == 'extension hook' }}"));
    let tree: HashMap<PathBuf, Vec<u8>> = [
        ("db/schema.rb", CATEGORIES), ("app/models/finder_hook.rb", hook),
        ("app/models/finder_extension.rb", extension), ("app/models/category.rb", category),
        ("app/controllers/categories_controller.rb", actions),
    ].into_iter().map(|(p, s)| (PathBuf::from(p), s.as_bytes().to_vec())).collect();
    let mut app = roundhouse::ingest::ingest_app_from_tree(tree).unwrap();
    let mut analyzer = Analyzer::new(&app);
    analyzer.analyze(&mut app);
    let ownership: Vec<_> = analyzer.class_registry().iter().filter(|(id, _)| ["Category", "FinderExtension", "FinderHook"].contains(&id.0.as_str())).map(|(id, info)|
        format!("{}: extensions={:?} includes={:?} unresolved={} hook={}", id.0, info.class_method_extensions, info.includes, info.unresolved_include, info.unknown_class_side_hook)).collect();
    for action in app.controllers[0].actions() {
        assert!(action.body.ty.as_ref().unwrap().is_unknown(), "{}: {:?}; {ownership:?}; {:?}", action.name, action.body.ty, action.body.node);
    }
    let (_, errors) = emit_and_run::empty_app().write("db/schema.rb", CATEGORIES)
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("app/models/finder_hook.rb", hook).write("app/models/finder_extension.rb", extension)
        .write("app/models/category.rb", category).write("app/controllers/categories_controller.rb", actions)
        .emit(BuildTarget::Ruby);
    assert!(errors.iter().any(|error| error.contains("dynamic finder without established implementation")), "{errors:?}");
}

#[test]
fn unresolved_extension_includes_do_not_invent_record_returns() {
    let extension = "module FinderExtension\ninclude ExternalFinders\nend\n";
    let category = "class Category < ActiveRecord::Base\nextend FinderExtension\nend\n";
    native(&["activerecord"], &format!("{AR}{CATEGORIES}\nmodule ExternalFinders\ndef find_by(attributes); 'external owner'; end\nend\n{extension}{category}\nraise 'external owner' unless Category.find_by_name('found') == 'external owner'"));
    // ExternalFinders is supplied by the native environment, but its source
    // is absent from the compiler input. Its method surface stays unknown.
    let tree: HashMap<PathBuf, Vec<u8>> = [
        ("db/schema.rb", CATEGORIES), ("app/models/finder_extension.rb", extension),
        ("app/models/category.rb", category),
        ("app/controllers/categories_controller.rb", "class CategoriesController < ActionController::Base\ndef dynamic; Category.find_by_name('found'); end\nend\n"),
    ].into_iter().map(|(p, s)| (PathBuf::from(p), s.as_bytes().to_vec())).collect();
    let mut app = roundhouse::ingest::ingest_app_from_tree(tree).unwrap();
    let mut analyzer = Analyzer::new(&app);
    analyzer.analyze(&mut app);
    let ownership: Vec<_> = analyzer.class_registry().iter().filter(|(id, _)| ["Category", "FinderExtension", "FinderHook"].contains(&id.0.as_str())).map(|(id, info)|
        format!("{}: extensions={:?} includes={:?} unresolved={} hook={}", id.0, info.class_method_extensions, info.includes, info.unresolved_include, info.unknown_class_side_hook)).collect();
    assert!(app.controllers[0].actions().next().unwrap().body.ty.as_ref().unwrap().is_unknown(), "{ownership:?}; {:?}", app.controllers[0].actions().next().unwrap().body);
}

#[test]
fn disabled_sti_hydrates_the_queried_plain_subclass() {
    const SCHEMA: &str = "ActiveRecord::Schema.define do\ncreate_table(:articles) { |t| t.string :title; t.string :type }\nend\n";
    const ARTICLE: &str = "class Article < ActiveRecord::Base\nend\n";
    const STICKY: &str = "class Sticky < Article\nend\n";
    const FEATURE: &str = "class Feature < Article\nself.inheritance_column = nil\ndef marker; \"feature\"; end\nend\n";
    const CHECK: &str = r#"
Article.create!(title: "plain")
sticky = Sticky.create!(title: "sticky")
raise "base STI" unless Article.find(sticky.id).class == Sticky
raise "plain collection hydration" unless Feature.all.to_a.map(&:class) == [Feature, Feature]
raise "plain finder hydration" unless Feature.find(sticky.id).class == Feature
raise "plain finder method" unless Feature.find_by(title: "sticky").marker == "feature"
"#;
    native(&["activerecord"], &format!("{AR}{SCHEMA}{ARTICLE}{STICKY}{FEATURE}{CHECK}"));
    emit_and_run::empty_app().write("db/schema.rb", SCHEMA)
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("app/models/article.rb", ARTICLE).write("app/models/sticky.rb", STICKY)
        .write("app/models/feature.rb", FEATURE).run_ruby(CHECK).assert_passes();
}

fn association_finder_case(hook: bool) {
    const SCHEMA: &str = "ActiveRecord::Schema.define do\ncreate_table(:users) { |t| t.string :name }\ncreate_table(:posts) { |t| t.string :title; t.integer :user_id }\nend\n";
    const USER: &str = "class User < ActiveRecord::Base\nhas_many :posts\nend\n";
    for (index, (module, declaration, refusal)) in [
        ("module Finders\ndef find_by_title(value); \"extension owner\"; end\nend\n", "extend Finders", "relation dynamic finder with source delegate"),
        ("module Finders\ndef self.included(klass)\nklass.define_singleton_method(:find_by_title) { |value| \"extension owner\" }\nend\nend\n", "include Finders", "unknown class-side hook"),
    ].into_iter().enumerate() {
        if (index == 1) != hook { continue; }
        let post = format!("class Post < ActiveRecord::Base\nbelongs_to :user\n{declaration}\nend\n");
        let probe = "class FinderProbe\n#: (User) -> String\ndef self.lookup(user); user.posts.find_by_title(\"found\"); end\nend\n";
        let check = "user = User.create!(name: 'owner')\nPost.create!(title: 'found', user_id: user.id)\nraise 'association dispatch' unless FinderProbe.lookup(user) == 'extension owner'\n";
        native(&["activerecord"], &format!("{AR}{SCHEMA}{module}{USER}{post}{probe}{check}"));
        let (emitted, errors) = emit_and_run::empty_app().write("db/schema.rb", SCHEMA)
            .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
            .write("app/models/finders.rb", module).write("app/models/user.rb", USER)
            .write("app/models/post.rb", &post).write("app/models/finder_probe.rb", probe).emit(BuildTarget::Ruby);
        if errors.is_empty() {
            let script = format!("require File.expand_path(\"main\", Dir.pwd)\nMain.configure_default_adapter!\n{check}");
            let run = emit_and_run::ruby().args(["-e", &script]).current_dir(&emitted).env("BLOG_DB", ":memory:").output().unwrap();
            assert!(run.status.success(), "{declaration}: {}", String::from_utf8_lossy(&run.stderr));
        } else {
            assert!(errors.iter().any(|error| error.contains(refusal)), "{declaration}: {errors:?}");
        }
    }
}

#[test]
fn association_finders_keep_extension_ownership() { association_finder_case(false); }

#[test]
fn association_finders_keep_hook_ownership() { association_finder_case(true); }

fn paired_form(model: &str, schema: &str, sources: &[(&str, &str)], native_prefix: &str, key: &str, values: bool) {
    let constructor = format!("{model}.new(nickname: \"kept\", enabled: true, published: true)");
    let controller = format!("class PagesController < ActionController::Base\ndef ivar; @entry = {constructor}; end\ndef constructed; end\nend\n");
    let fields = "<%= form.text_field :nickname %><%= form.check_box :enabled %><%= form.check_box :published %>";
    let ivar = format!("<%= form_with model: @entry, url: \"/\" do |form| %>{fields}<% end %>");
    let constructed = format!("<%= form_with model: {constructor}, url: \"/\" do |form| %>{fields}<% end %>");
    let check = format!(r#"
def check_form(html)
  raise "parameter key #{{html}}" unless html.include?('name="{key}[nickname]"')
  raise "input id #{{html}}" unless html.include?('id="{key}_nickname"')
  if {values}
    raise "stored value #{{html}}" unless html.match?(/value="kept"/)
    ["enabled", "published"].each do |name|
      input = html.scan(/<input[^>]*>/).find {{ |tag| tag.include?(%Q(id="{key}_#{{name}}")) }}
      raise "checked #{{name}} #{{html}}" unless input&.include?('checked="checked"')
    end
  end
end
"#);
    let declarations = sources.iter().filter(|(p, _)| p.starts_with("app/models/")).map(|(_, s)| *s).collect::<Vec<_>>().join("\n");
    let native_program = format!(r#"{AR}
require 'action_view'
require 'action_controller'
{native_prefix}
{schema}
{declarations}
ActionView::Helpers::FormHelper.form_with_generates_ids = true
view = ActionView::Base.with_empty_template_cache.new(ActionView::LookupContext.new([]), {{}}, nil)
view.define_singleton_method(:protect_against_forgery?) {{ false }}
view.assign(entry: {constructor})
{check}
check_form(view.render(inline: {ivar:?}))
check_form(view.render(inline: {constructed:?}))
"#);
    let mut components = vec!["activerecord", "actionview", "actionpack"];
    if sources.iter().any(|(path, _)| path.ends_with("/engine.rb")) { components.push("railties"); }
    native(&components, &native_program);
    let mut app = emit_and_run::empty_app().write("db/schema.rb", schema)
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("app/controllers/pages_controller.rb", &controller)
        .write("app/views/pages/ivar.html.erb", &ivar)
        .write("app/views/pages/constructed.html.erb", &constructed);
    for (p, s) in sources { app = app.write(p, s); }
    app.run_ruby(&format!("{check}\ncheck_form(Views::Pages.ivar({constructor}))\ncheck_form(Views::Pages.constructed)\n")).assert_passes();
}

#[test]
fn isolated_engine_form_keys_are_relative() {
    let engine = "module Blog\nclass Engine < ::Rails::Engine\nisolate_namespace Blog\nend\nend\n";
    paired_form("Blog::Post", "ActiveRecord::Schema.define do\ncreate_table(:blog_posts) { |t| t.boolean :published }\nend\n", &[
        ("lib/blog/engine.rb", engine),
        ("app/models/blog/post.rb", "module Blog\nclass Post < ActiveRecord::Base\nattribute :nickname, :string\nattribute :enabled, :boolean\nend\nend\n"),
    ], &format!("require 'rails'\n{engine}"), "post", false);
}

#[test]
fn namespaced_form_metadata_preserves_attribute_values() {
    paired_form("Admin::User", "ActiveRecord::Schema.define do\ncreate_table(:users) { |t| t.boolean :published }\nend\n", &[
        ("app/models/admin/user.rb", "module Admin\nclass User < ActiveRecord::Base\nself.table_name = \"users\"\nattribute :nickname, :string\nattribute :enabled, :boolean\nend\nend\n"),
    ], "", "admin_user", true);
}

#[test]
fn acronym_form_keys_keep_word_boundaries() {
    native(&["activerecord"], "require 'active_record'\nmodule API\nclass HTTPResponse < ActiveRecord::Base; end\nend\nraise 'param_key' unless API::HTTPResponse.model_name.param_key == 'api_http_response'\n");
    assert_eq!(roundhouse::naming::model_param_key("API::HTTPResponse"), "api_http_response");
    paired_form("API::HTTPResponse", "ActiveRecord::Schema.define do\ncreate_table(:responses) { |t| t.boolean :published }\nend\n", &[
        ("app/models/api/http_response.rb", "module API\nclass HTTPResponse < ActiveRecord::Base\nself.table_name = \"responses\"\nattribute :nickname, :string\nattribute :enabled, :boolean\nend\nend\n"),
    ], "", "api_http_response", false);
}

#[test]
fn form_keys_respect_registered_acronyms() {
    const INFLECTIONS: &str = "ActiveSupport::Inflector.inflections(:en) { |inflect| inflect.acronym \"OAuth\" }\n";
    native(&["activerecord"], &format!("require 'active_record'\n{INFLECTIONS}module Admin; class OAuthApplication < ActiveRecord::Base; end; end\nraise 'registered acronym' unless Admin::OAuthApplication.model_name.param_key == 'admin_oauth_application'"));
    let previous = roundhouse::naming::app_inflections();
    roundhouse::naming::install_app_inflections(roundhouse::naming::AppInflections {
        acronym: vec!["OAuth".to_owned()], ..Default::default()
    });
    let key = roundhouse::naming::model_param_key("Admin::OAuthApplication");
    roundhouse::naming::install_app_inflections(previous);
    assert_eq!(key, "admin_oauth_application");
    paired_form("Admin::OAuthApplication", "ActiveRecord::Schema.define do\ncreate_table(:applications) { |t| t.boolean :published }\nend\n", &[
        ("config/initializers/inflections.rb", INFLECTIONS),
        ("app/models/admin/oauth_application.rb", "module Admin\nclass OAuthApplication < ActiveRecord::Base\nself.table_name = \"applications\"\nattribute :nickname, :string\nattribute :enabled, :boolean\nend\nend\n"),
    ], INFLECTIONS, "admin_oauth_application", false);
}

#[test]
fn standard_forwardable_keeps_known_finder_returns() {
    use roundhouse::ident::{ClassId, Symbol};
    use roundhouse::ty::Ty;
    for shadow in [false, true] {
        let declaration = if shadow { "module Forwardable\ndef self.extended(klass)\nklass.define_singleton_method(:find_by_name) { |value| \"source forwarder\" }\nend\nend\n" } else { "" };
        let category = "require \"forwardable\"\nclass Category < ActiveRecord::Base\nextend Forwardable\nend\n";
        native(&["activerecord"], &format!("{AR}{CATEGORIES}\nrequire 'forwardable'\n{declaration}{category}\nrecord = Category.create!(name: 'found')\nvalue = Category.find_by_name('found')\nraise 'forwardable owner' unless {}\n", if shadow { "value == 'source forwarder'" } else { "value.id == record.id" }));
        let mut tree: HashMap<PathBuf, Vec<u8>> = [
            ("db/schema.rb", CATEGORIES), ("app/models/category.rb", category),
            ("app/controllers/categories_controller.rb", "class CategoriesController < ActionController::Base\ndef dynamic; Category.find_by_name('found'); end\ndef ordinary; Category.find_by(name: 'found'); end\ndef bang; Category.find_by_name!('found'); end\ndef reflected; Category.send(:find_by_name, 'found'); end\nend\n"),
        ].into_iter().map(|(p, s)| (PathBuf::from(p), s.as_bytes().to_vec())).collect();
        if shadow { tree.insert(PathBuf::from("app/models/forwardable.rb"), declaration.as_bytes().to_vec()); }
        let mut app = roundhouse::ingest::ingest_app_from_tree(tree).unwrap();
        Analyzer::new(&app).analyze(&mut app);
        let record = Ty::Class { id: ClassId(Symbol::from("Category")), args: vec![].into() };
        for action in app.controllers[0].actions() {
            let got = action.body.ty.as_ref().unwrap();
            if shadow { assert!(got.is_unknown(), "source Forwardable {}: {got:?}", action.name); }
            else {
                let expected = if action.name.as_str() == "bang" { record.clone() } else { Ty::Union { variants: vec![record.clone(), Ty::Nil].into() } };
                assert_eq!(got, &expected, "standard Forwardable {}", action.name);
            }
        }
    }
}

fn native_optional_extension(gem: &str, version: &str, program: &str) {
    let probe = emit_and_run::ruby().args(["-rrubygems", "-e", &format!(
        "puts !Gem::Specification.find_all_by_name({gem:?}, {version:?}).empty?"
    )]).output().expect("optional extension availability");
    assert!(probe.status.success(), "optional extension probe");
    if String::from_utf8_lossy(&probe.stdout).trim() == "false" {
        eprintln!("skipping native extension oracle: missing {gem} {version}");
        return;
    }
    native(&["activerecord"], &format!("gem {gem:?}, {version:?}\nrequire {gem:?}\n{program}"));
}

#[test]
fn signature_extension_preserves_framework_finders() {
    use roundhouse::ident::{ClassId, Symbol};
    use roundhouse::ty::Ty;
    let model = "class Category < ActiveRecord::Base\nextend T::Sig\ndef self.lookup; find_by_name('found'); end\ndef self.records; all.to_a; end\nend\n";
    let probe = "class FinderProbe\ndef self.dynamic; Category.find_by_name('found'); end\ndef self.bang; Category.find_by_name!('found'); end\nend\n";
    let check = "record = Category.create!(name: 'found')\nraise 'dynamic' unless FinderProbe.dynamic.id == record.id\nraise 'bang' unless FinderProbe.bang.id == record.id\nraise 'bare' unless Category.lookup.id == record.id\nraise 'qualified all' unless Category.records.map(&:id) == [record.id]\n";
    native_optional_extension("sorbet-runtime", "0.5.11934", &format!("{AR}{CATEGORIES}{model}{probe}{check}"));
    let sources = [
        ("db/schema.rb", CATEGORIES), ("app/models/category.rb", model), ("app/models/finder_probe.rb", probe),
        ("app/controllers/categories_controller.rb", "class CategoriesController < ActionController::Base\ndef dynamic; Category.find_by_name('found'); end\ndef bang; Category.find_by_name!('found'); end\ndef bare; Category.lookup; end\ndef records; Category.records; end\nend\n"),
    ];
    let tree = sources.iter().map(|(p, s)| (PathBuf::from(p), s.as_bytes().to_vec())).collect();
    let mut app = roundhouse::ingest::ingest_app_from_tree(tree).unwrap();
    Analyzer::new(&app).analyze(&mut app);
    let record = Ty::Class { id: ClassId(Symbol::from("Category")), args: vec![].into() };
    for action in app.controllers[0].actions() {
        let actual = action.body.ty.as_ref().unwrap();
        if action.name.as_str() == "records" { assert!(!actual.is_unknown(), "receiverless all must retain its relation result"); }
        else {
            let expected = if action.name.as_str() == "bang" { record.clone() } else { Ty::Union { variants: vec![record.clone(), Ty::Nil].into() } };
            assert_eq!(actual, &expected, "{}", action.name);
        }
    }
    emit_and_run::empty_app().write("db/schema.rb", CATEGORIES)
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("app/models/category.rb", model).write("app/models/finder_probe.rb", probe).run_ruby(check).assert_passes();
}

#[test]
fn missing_gem_extensions_keep_unknown_finder_ownership() {
    let schema = "ActiveRecord::Schema.define do\ncreate_table(:categories) { |t| t.string :name; t.string :slug }\nend\n";
    native_optional_extension("friendly_id", "5.5.1", &format!("{AR}{schema}\nclass Category < ActiveRecord::Base\nextend FriendlyId\nfriendly_id :name, use: [:slugged, :finders]\nend\nrecord = Category.create!(name: 'Found')\nraise 'extension find ownership' unless Category.find('found').id == record.id\nraise 'find was not overridden' if Category.method(:find).owner == ActiveRecord::Base.method(:find).owner\n"));
    for extension in ["FriendlyId", "Enumerize", "UnmodeledFinders"] {
        let model = format!("class Category < ActiveRecord::Base\nextend {extension}\ndef self.lookup; find_by_name('found'); end\nend\n");
        let tree = [("db/schema.rb", schema), ("app/models/category.rb", &model),
            ("app/controllers/categories_controller.rb", "class CategoriesController < ActionController::Base\ndef dynamic; Category.find_by_name('found'); end\ndef ordinary; Category.find_by(name: 'found'); end\ndef bare; Category.lookup; end\nend\n")]
            .into_iter().map(|(p, s)| (PathBuf::from(p), s.as_bytes().to_vec())).collect();
        let mut app = roundhouse::ingest::ingest_app_from_tree(tree).unwrap();
        Analyzer::new(&app).analyze(&mut app);
        for action in app.controllers[0].actions() { assert!(action.body.ty.as_ref().unwrap().is_unknown(), "{extension} {}", action.name); }
        let (_, errors) = emit_and_run::empty_app().write("db/schema.rb", schema)
            .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
            .write("app/models/category.rb", &model)
            .write("app/models/finder_probe.rb", "class FinderProbe\ndef self.value; Category.find_by_name('found'); end\nend\n")
            .emit(BuildTarget::Ruby);
        assert!(errors.iter().any(|error| error.contains("dynamic finder without established implementation")), "{extension}: {errors:?}");
    }
}

#[test]
fn source_signature_extension_keeps_its_finder_owner() {
    let extension = "module T\nmodule Sig\ndef find_by_name(value); 'source signature owner'; end\nend\nend\n";
    let model = "class Category < ActiveRecord::Base\nextend T::Sig\nend\n";
    native(&["activerecord"], &format!("{AR}{CATEGORIES}{extension}{model}\nraise 'source owner' unless Category.find_by_name('found') == 'source signature owner'\n"));
    let sources = [("db/schema.rb", CATEGORIES), ("app/models/t/sig.rb", extension), ("app/models/category.rb", model),
        ("app/controllers/categories_controller.rb", "class CategoriesController < ActionController::Base\ndef dynamic; Category.find_by_name('found'); end\nend\n")];
    let mut app = roundhouse::ingest::ingest_app_from_tree(sources.iter().map(|(p, s)| (PathBuf::from(p), s.as_bytes().to_vec())).collect()).unwrap();
    Analyzer::new(&app).analyze(&mut app);
    assert_eq!(app.controllers[0].actions().next().unwrap().body.ty.as_ref().unwrap(), &roundhouse::ty::Ty::Str);
}

fn isolated_form_i18n_case(subclass: bool) {
    let engine = "module Blog\nclass Engine < ::Rails::Engine\nisolate_namespace Blog\nend\nend\n";
    let schema = "ActiveRecord::Schema.define do\ncreate_table(:posts) { |t| t.string :title; t.string :type }\nend\n";
    let post = "module Blog\nclass Post < ActiveRecord::Base\nself.table_name = 'posts'\nvalidates :title, presence: true\nend\nend\n";
    let archived = "module Blog\nclass ArchivedPost < Post\nend\nend\n";
    let locale = "en:\n  helpers:\n    label:\n      blog/post:\n        title: Ledger headline\n      blog/archived_post:\n        title: Archived headline\n    submit:\n      create: Publish %{model}\n      update: Amend %{model}\n  activerecord:\n    models:\n      blog/post: Journal entry\n      blog/archived_post: Archived entry\n";
    let (class, key, label, human) = if subclass { ("Blog::ArchivedPost", "archived_post", "Archived headline", "Archived entry") }
        else { ("Blog::Post", "post", "Ledger headline", "Journal entry") };
    let template = "<%= form_with model: @entry, url: '/', method: :post do |form| %><%= form.label :title %><%= form.text_field :title %><%= form.submit %><% end %>";
    let edit_template = template.replace("method: :post", "method: :patch");
    let controller = format!("class PagesController < ActionController::Base\ndef form; @entry = {class}.new; end\ndef edit; @entry = {class}.new(title: 'Saved'); @entry.save!; end\nend\n");
    let checks = format!(r##"
def check_localized_form(html, persisted, invalid)
  raise "label #{{html}}" unless html.include?('>{label}</label>')
  text = persisted ? 'Amend {human}' : 'Publish {human}'
  raise "submit #{{html}}" unless html.include?(%Q(value="#{{text}}"))
  raise "relative key #{{html}}" unless html.include?('name="{key}[title]"')
  raise "error metadata #{{html}}" unless html.include?('field_with_errors') == invalid
end
"##);
    native(&["activerecord", "actionview", "actionpack", "railties"], &format!(r#"{AR}
require 'rails/engine'
require 'action_view'
require 'action_controller'
{engine}{schema}{post}{archived}
require 'yaml'
ActionView::Helpers::FormHelper.form_with_generates_ids = true
view = ActionView::Base.with_empty_template_cache.new(ActionView::LookupContext.new([]), {{}}, nil)
view.define_singleton_method(:protect_against_forgery?) {{ false }}
I18n.backend.eager_load!
I18n.backend.store_translations(:en, YAML.safe_load({locale:?})['en'])
{checks}
record = {class}.new
view.assign(entry: record)
check_localized_form(view.render(inline: {template:?}), false, false)
record.valid?
check_localized_form(view.render(inline: {template:?}), false, true)
record = {class}.create!(title: 'Saved')
view.assign(entry: record)
check_localized_form(view.render(inline: {edit_template:?}), true, false)
"#));
    let sources = [("db/schema.rb", schema), ("lib/blog/engine.rb", engine), ("app/models/blog/post.rb", post),
        ("app/models/blog/archived_post.rb", archived), ("config/locales/en.yml", locale)];
    let mut app = roundhouse::ingest::ingest_app_from_tree(sources.iter().map(|(p, s)| (PathBuf::from(p), s.as_bytes().to_vec())).collect()).unwrap();
    Analyzer::new(&app).analyze(&mut app);
    if subclass {
        assert!(app.library_classes.iter().any(|c| c.name.0.as_str() == class), "subclass must cover library-class construction");
        assert!(!app.models.iter().any(|m| m.name.0.as_str() == class), "subclass must exercise the separate library metadata path");
    }
    else { assert!(app.models.iter().any(|m| m.name.0.as_str() == class), "base must cover model construction"); }
    let form = roundhouse::i18n::FormI18n::of(&app);
    assert!(form.has_errors(key), "isolated {class} must retain errors metadata at {key}");
    assert_eq!(form.label(key, "title"), label);
    assert_eq!(form.submit(key, "create"), format!("Publish {human}"));
    assert_eq!(form.submit(key, "update"), format!("Amend {human}"));
    let mut emitted = emit_and_run::empty_app().write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("app/controllers/pages_controller.rb", &controller).write("app/views/pages/form.html.erb", template)
        .write("app/views/pages/edit.html.erb", &edit_template);
    for (path, source) in sources { emitted = emitted.write(path, source); }
    emitted.run_ruby(&format!(r#"{checks}
record = {class}.new
check_localized_form(Views::Pages.form(record), false, false)
record.valid?
check_localized_form(Views::Pages.form(record), false, true)
record = {class}.create!(title: 'Saved')
check_localized_form(Views::Pages.edit(record), true, false)
"#)).assert_passes();
}

#[test]
fn isolated_model_form_i18n_keeps_labels_submit_and_errors() { isolated_form_i18n_case(false); }

#[test]
fn isolated_subclass_form_i18n_keeps_labels_submit_and_errors() { isolated_form_i18n_case(true); }
