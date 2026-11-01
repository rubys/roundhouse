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
    native(&["activerecord"], &format!("require 'active_record'\n{INFLECTIONS}module Admin; class OAuthApplication < ActiveRecord::Base; end; end\nraise 'registered acronym' unless Admin::OAuthApplication.model_name.param_key == 'admin_oauth_application'\nraise 'registered i18n acronym' unless Admin::OAuthApplication.model_name.i18n_key.to_s == 'admin/oauth_application'"));
    let previous = roundhouse::naming::app_inflections();
    roundhouse::naming::install_app_inflections(roundhouse::naming::AppInflections {
        acronym: vec!["OAuth".to_owned()], ..Default::default()
    });
    let key = roundhouse::naming::model_param_key("Admin::OAuthApplication");
    let i18n_key = roundhouse::i18n::i18n_key("Admin::OAuthApplication");
    roundhouse::naming::install_app_inflections(previous);
    assert_eq!(key, "admin_oauth_application");
    assert_eq!(i18n_key, "admin/oauth_application");
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

#[test]
fn unresolved_model_includes_keep_finder_ownership_unknown() {
    use roundhouse::ident::{ClassId, Symbol};
    let extension = "module ExternalFinders\ndef self.included(base)\nbase.define_singleton_method(:find_by_name) { |value| 'external owner' }\nbase.define_singleton_method(:find_by_name!) { |value| 'external owner' }\nbase.define_singleton_method(:find_by) { |attributes| 'external owner' }\nbase.define_singleton_method(:find_by!) { |attributes| 'external owner' }\nend\nend\n";
    let model = "class Category < ActiveRecord::Base\ninclude ExternalFinders\nend\n";
    native(&["activerecord"], &format!("{AR}{CATEGORIES}{extension}{model}\n[:find_by_name, :find_by_name!].each {{ |name| raise 'external dynamic owner' unless Category.public_send(name, 'found') == 'external owner' }}\n[:find_by, :find_by!].each {{ |name| raise 'external ordinary owner' unless Category.public_send(name, name: 'found') == 'external owner' }}\n"));
    let mut app = roundhouse::ingest::ingest_app_from_tree([
        ("db/schema.rb", CATEGORIES), ("app/models/category.rb", model),
        ("app/models/other_category.rb", "class OtherCategory < ActiveRecord::Base\nself.table_name = 'categories'\nend\n"),
        ("app/controllers/categories_controller.rb", "class CategoriesController < ActionController::Base\ndef dynamic; Category.find_by_name('found'); end\ndef bang; Category.find_by_name!('found'); end\ndef ordinary; Category.find_by(name: 'found'); end\ndef ordinary_bang; Category.find_by!(name: 'found'); end\nend\n"),
    ].into_iter().map(|(p,s)| (PathBuf::from(p),s.as_bytes().to_vec())).collect()).unwrap();
    let mut analyzer = Analyzer::new(&app);
    analyzer.analyze(&mut app);
    for action in app.controllers[0].actions() {
        assert!(action.body.ty.as_ref().unwrap().is_unknown(), "{}: {:?}", action.name, action.body.ty);
    }
    let registry = analyzer.class_registry();
    assert!(registry[&ClassId(Symbol::from("Category"))].unresolved_include, "unresolved model include must mark its own ownership");
    assert!(!registry[&ClassId(Symbol::from("OtherCategory"))].unresolved_include, "unrelated model must remain resolved");
    let (_, errors) = emit_and_run::empty_app().write("db/schema.rb", CATEGORIES)
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("app/models/category.rb", model)
        .write("app/models/finder_probe.rb", "class FinderProbe\ndef self.value; Category.find_by_name('found'); end\nend\n")
        .emit(BuildTarget::Ruby);
    assert!(errors.iter().any(|e| e.contains("include") && e.contains("ExternalFinders")), "{errors:?}");
}

#[test]
fn resolved_model_includes_keep_framework_finders() {
    let extension = "module FinderMarker\ndef marker; 'marker'; end\nend\n";
    let model = "class Category < ActiveRecord::Base\ninclude FinderMarker\nend\n";
    let probe = "class FinderProbe\ndef self.value; Category.find_by_name('found'); end\nend\n";
    let check = "record = Category.create!(name: 'found')\nraise 'resolved marker' unless FinderProbe.value.id == record.id\n";
    native(&["activerecord"], &format!("{AR}{CATEGORIES}{extension}{model}{probe}{check}"));
    emit_and_run::empty_app().write("db/schema.rb", CATEGORIES)
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("app/models/finder_marker.rb", extension).write("app/models/category.rb", model)
        .write("app/models/finder_probe.rb", probe).run_ruby(check).assert_passes();
}

#[test]
fn acronym_i18n_keys_match_rails_model_ancestry() {
    let schema = "ActiveRecord::Schema.define do\ncreate_table(:entries) { |t| t.string :title }\ncreate_table(:clients) { |t| t.string :title; t.string :type }\nend\n";
    let entry = "module Blog\nclass APIEntry < ActiveRecord::Base\nself.table_name = 'entries'\nend\nend\n";
    let record = "module Blog\nclass Record < ActiveRecord::Base\nself.table_name = 'clients'\nend\nend\n";
    let client = "module Blog\nclass APIClient < Blog::Record\nend\nend\n";
    let featured = "module Blog\nclass Featured < Blog::APIClient\nend\nend\n";
    let entries = format!("{entry}{record}{client}{featured}");
    let locale = "en:\n  activerecord:\n    models:\n      blog/api_entry: Submission\n      blog/api_client: Integration client\n    attributes:\n      blog/api_entry:\n        title: Entry caption\n      blog/api_client:\n        title: Client caption\n";
    let check = "raise 'model key' unless Blog::APIEntry.model_name.i18n_key.to_s == 'blog/api_entry'\nraise 'subclass key' unless Blog::APIClient.model_name.i18n_key.to_s == 'blog/api_client'\nraise 'inherited attribute' unless Blog::Featured.human_attribute_name('title') == 'Client caption'\n";
    native(&["activerecord"], &format!("{AR}{schema}{entries}\nrequire 'yaml'\nI18n.backend.eager_load!\nI18n.backend.store_translations(:en, YAML.safe_load({locale:?})['en'])\n{check}"));
    assert_eq!(roundhouse::i18n::i18n_key("Blog::APIEntry"), "blog/api_entry");
    assert_eq!(roundhouse::i18n::i18n_key("Blog::APIClient"), "blog/api_client");
    let sources = [("db/schema.rb",schema),("app/models/blog/api_entry.rb",entry),("app/models/blog/record.rb",record),("app/models/blog/api_client.rb",client),("app/models/blog/featured.rb",featured),("config/locales/en.yml",locale)];
    let mut app=roundhouse::ingest::ingest_app_from_tree(sources.iter().map(|(p,s)|(PathBuf::from(p),s.as_bytes().to_vec())).collect()).unwrap();
    Analyzer::new(&app).analyze(&mut app);
    let i18n=roundhouse::i18n::FormI18n::of(&app);
    for (class,key,title,model) in [("Blog::APIEntry","blog_api_entry","Entry caption","Submission"),("Blog::APIClient","blog_api_client","Client caption","Integration client"),("Blog::Featured","blog_featured","Client caption","Integration client")] {
        assert_eq!(i18n.label(key,"title"),title,"{class}");
        assert_eq!(i18n.submit(key,"create"),format!("Create {model}"),"{class}");
        let controller=format!("class PagesController < ActionController::Base\ndef form; @entry = {class}.new; end\nend\n");
        let template="<%= form_with model: @entry, url: '/', method: :post do |form| %><%= form.label :title %><%= form.submit %><% end %>";
        let ruby=format!("html=Views::Pages.form({class}.new)\nraise 'acronym label' unless html.include?('>{title}</label>')\nraise 'acronym submit' unless html.include?('value=\"Create {model}\"')\n");
        let mut emitted=emit_and_run::empty_app().write("config/routes.rb","Rails.application.routes.draw do\nend\n").write("app/controllers/pages_controller.rb",&controller).write("app/views/pages/form.html.erb",template);
        for (path,source) in sources {emitted=emitted.write(path,source);}
        emitted.run_ruby(&ruby).assert_passes();
    }
}

fn memoized_association_condition_case(kind: &str) {
    let schema="ActiveRecord::Schema.define do\ncreate_table(:users) { |t| t.string :name }\ncreate_table(:rooms) { |t| t.string :name }\ncreate_table(:messages) { |t| t.integer :room_id; t.integer :creator_id; t.string :title; t.integer :score }\nend\n";
    let user="class User < ActiveRecord::Base\nend\n";
    let room="class Room < ActiveRecord::Base\nhas_many :messages\nend\n";
    let message="class Message < ActiveRecord::Base\nbelongs_to :room\nbelongs_to :creator, class_name: 'User'\nend\n";
    let expression=match kind {
        "creator"=>"Room.find(id).messages.find_by(creator: creator)",
        "range"=>"Room.find(id).messages.find_by(score: 2..4)",
        "range!"=>"Room.find(id).messages.find_by!(score: 2..4)",
        "nested"=>"Room.find(Room.where(name: 'owner').first.id).messages.find_by_title(Message.where(score: 1).first.title)",
        _=>unreachable!(),
    };
    let probe=format!("class FinderProbe\n#: (Integer, User) -> Message?\ndef self.value(id, creator); {expression}; end\nend\n");
    let check="creator=User.create!(name: 'writer')\nroom=Room.create!(name: 'owner')\nother=Room.create!(name: 'other')\nMessage.create!(room_id: other.id, creator_id: creator.id, title: 'found', score: 3)\nMessage.create!(room_id: other.id, creator_id: creator.id, title: 'found', score: 1)\nrecord=Message.create!(room_id: room.id, creator_id: creator.id, title: 'found', score: 3)\nresult=FinderProbe.value(room.id, creator)\nraise 'scoped condition' unless result && result.id == record.id\n";
    native(&["activerecord"],&format!("{AR}{schema}{user}{room}{message}{probe}{check}"));
    let run = emit_and_run::empty_app().write("db/schema.rb",schema).write("config/routes.rb","Rails.application.routes.draw do\nend\n")
        .write("app/models/user.rb",user).write("app/models/room.rb",room).write("app/models/message.rb",message)
        .write("app/models/finder_probe.rb",&probe).run_ruby(check);
    run.assert_passes();
    if kind == "nested" {
        let source = std::fs::read_to_string(run.emitted.join("app/models/finder_probe.rb")).unwrap();
        assert!(!source.contains("Message.where("), "argument query must receive relation lowering");
        assert!(!source.contains("Room.where("), "owner query must receive relation lowering");
    }
}

#[test]
fn association_finder_memoization_lowers_nested_queries() { memoized_association_condition_case("nested"); }
#[test]
fn association_finder_memoization_keeps_association_conditions() { memoized_association_condition_case("creator"); }
#[test]
fn association_finder_memoization_keeps_range_conditions() { memoized_association_condition_case("range"); }
#[test]
fn association_finder_memoization_keeps_bang_range_conditions() { memoized_association_condition_case("range!"); }

fn isolated_form_metadata_case(partial: bool) {
    let schema="ActiveRecord::Schema.define do\ncreate_table(:posts) { |t| t.string :title }\ncreate_table(:blog_posts) { |t| t.string :title }\nend\n";
    let engine="module Blog\nclass Engine < ::Rails::Engine\nisolate_namespace Blog\nend\nend\n";
    let post="class Post < ActiveRecord::Base\nattribute :nickname, :string\nattribute :enabled, :boolean\nvalidates :title, presence: true\nend\n";
    let blog="module Blog\nclass Post < ActiveRecord::Base\nself.table_name = 'blog_posts'\nattribute :tagline, :string\nattribute :featured, :boolean\nvalidates :title, presence: true\nend\nend\n";
    let locale="en:\n  helpers:\n    submit:\n      create: Publish %{model}\n  activerecord:\n    models:\n      post: Public entry\n      blog/post: Engine entry\n    attributes:\n      post:\n        title: Public caption\n      blog/post:\n        title: Engine caption\n";
    let root_template="<%= form_with model: @entry, url: '/', method: :post do |form| %><%= form.label :title %><%= form.text_field :nickname %><%= form.check_box :enabled %><%= form.submit %><% end %>";
    let blog_template="<%= form_with model: @entry, url: '/', method: :post do |form| %><%= form.label :title %><%= form.text_field :tagline %><%= form.check_box :featured %><%= form.submit %><% end %>";
    let root_fields = "<%= form.label :title %><%= form.text_field :nickname %><%= form.check_box :enabled %><%= form.submit %>";
    let blog_fields = "<%= form.label :title %><%= form.text_field :tagline %><%= form.check_box :featured %><%= form.submit %>";
    let root_template = if partial { root_template.replace(root_fields, "<%= render 'pages/root_fields', form: form, post: @entry %>") } else { root_template.to_owned() };
    let blog_template = if partial { blog_template.replace(blog_fields, "<%= render 'pages/blog_fields', form: form, post: @entry %>") } else { blog_template.to_owned() };
    let partial_setup = if partial { format!("Dir.mkdir('views')\nDir.mkdir('views/pages')\nFile.write('views/pages/_root_fields.html.erb', {root_fields:?})\nFile.write('views/pages/_blog_fields.html.erb', {blog_fields:?})\n") } else { String::new() };
    let checks=r##"
def check_identity_form(html, field, value, boolean, caption, model, invalid)
  raise 'virtual value' unless html.include?(%Q(value="#{value}"))
  raise 'parameter key' unless html.include?(%Q(name="post[#{field}]"))
  checkbox=html.scan(/<input[^>]*>/).find { |tag| tag.include?('type="checkbox"') }
  raise 'boolean reader' unless checkbox && checkbox.include?('checked="checked"')
  raise 'class label' unless html.include?(">#{caption}</label>")
  raise 'class submit' unless html.include?(%Q(value="Publish #{model}"))
  raise 'class errors' unless html.include?('field_with_errors') == invalid
end
"##;
    let native_program=format!(r#"{AR}
require 'rails/engine'
require 'action_view'
require 'action_controller'
{engine}{schema}{post}{blog}
require 'yaml'
ActionView::Helpers::FormHelper.form_with_generates_ids=true
{partial_setup}
view=ActionView::Base.with_empty_template_cache.new(ActionView::LookupContext.new(['views']), {{}}, nil)
view.define_singleton_method(:protect_against_forgery?) {{ false }}
I18n.backend.eager_load!
I18n.backend.store_translations(:en,YAML.safe_load({locale:?})['en'])
{checks}
root=Post.new(nickname: 'Root value', enabled: true)
blog=Blog::Post.new(tagline: 'Blog value', featured: true)
[false,true].each do |invalid|
  root.valid? if invalid
  blog.valid? if invalid
  view.assign(entry: root)
  check_identity_form(view.render(inline: {root_template:?}), 'nickname', 'Root value', 'enabled', 'Public caption', 'Public entry', invalid)
  view.assign(entry: blog)
  check_identity_form(view.render(inline: {blog_template:?}), 'tagline', 'Blog value', 'featured', 'Engine caption', 'Engine entry', invalid)
end
"#);
    native(&["activerecord","actionview","actionpack","railties"],&native_program);
    let controller="class PagesController < ActionController::Base\ndef root; @entry=Post.new(nickname: 'Root value', enabled: true); end\ndef blog; @entry=Blog::Post.new(tagline: 'Blog value', featured: true); end\nend\n";
    let run=format!(r#"{checks}
root=Post.new(nickname: 'Root value', enabled: true)
blog=Blog::Post.new(tagline: 'Blog value', featured: true)
[false,true].each do |invalid|
  root.valid? if invalid
  blog.valid? if invalid
  check_identity_form(Views::Pages.root(root), 'nickname', 'Root value', 'enabled', 'Public caption', 'Public entry', invalid)
  check_identity_form(Views::Pages.blog(blog), 'tagline', 'Blog value', 'featured', 'Engine caption', 'Engine entry', invalid)
end
"#);
    let mut emitted = emit_and_run::empty_app().write("db/schema.rb",schema).write("config/routes.rb","Rails.application.routes.draw do\nend\n")
        .write("lib/blog/engine.rb",engine).write("app/models/post.rb",post).write("app/models/blog/post.rb",blog)
        .write("config/locales/en.yml",locale).write("app/controllers/pages_controller.rb",controller)
        .write("app/views/pages/root.html.erb",&root_template).write("app/views/pages/blog.html.erb",&blog_template);
    if partial { emitted = emitted.write("app/views/pages/_root_fields.html.erb",root_fields).write("app/views/pages/_blog_fields.html.erb",blog_fields); }
    emitted.run_ruby(&run).assert_passes();
}

#[test]
fn isolated_form_metadata_uses_class_identity() { isolated_form_metadata_case(false); }

#[test]
fn isolated_partial_form_metadata_uses_class_identity() { isolated_form_metadata_case(true); }


#[test]
fn aliased_partial_forms_keep_record_metadata() {
    let schema = "ActiveRecord::Schema.define do\ncreate_table(:users) { |t| t.string :name; t.string :avatar; t.string :webhook_url }\nend\n";
    let model = "class User < ActiveRecord::Base\nvalidates :name, presence: true\nvalidates :avatar, presence: true\nvalidates :webhook_url, presence: true\nend\n";
    let locale = "en:\n  activerecord:\n    models:\n      user: Service account\n    attributes:\n      user:\n        name: Account name\n";
    let template = "<%= form_with model: @bot, scope: :bot, url: '/', method: :post do |form| %><%= render 'pages/fields', form: form, bot: @bot %><% end %>";
    let fields = "<%= form.label :name %><%= form.text_field :name %><%= form.file_field :avatar %><%= form.url_field :webhook_url %><%= form.submit %>";
    let checks = r#"
def check_aliased_form(html, invalid)
  raise 'aliased parameter key' unless html.include?('name="bot[name]"') && html.include?('id="bot_name"')
  raise 'record label' unless html.include?('>Account name</label>')
  raise 'record submit' unless html.include?('value="Create Service account"')
  raise 'record errors' unless html.include?('field_with_errors') == invalid
  raise 'file errors' unless html.match?(/<div class="field_with_errors"><input[^>]*type="file"[^>]*>/) == invalid
  raise 'url errors' unless html.match?(/<div class="field_with_errors"><input[^>]*type="url"[^>]*>/) == invalid
end
"#;
    native(&["activerecord", "actionview", "actionpack"], &format!(r#"{AR}
require 'action_view'
require 'action_controller'
require 'yaml'
{schema}{model}
Dir.mkdir('views')
Dir.mkdir('views/pages')
File.write('views/pages/_fields.html.erb', {fields:?})
ActionView::Helpers::FormHelper.form_with_generates_ids=true
view=ActionView::Base.with_empty_template_cache.new(ActionView::LookupContext.new(['views']), {{}}, nil)
view.define_singleton_method(:protect_against_forgery?) {{ false }}
I18n.backend.eager_load!
I18n.backend.store_translations(:en, YAML.safe_load({locale:?})['en'])
{checks}
bot=User.new
[false,true].each do |invalid|
  bot.valid? if invalid
  view.assign(bot: bot)
  check_aliased_form(view.render(inline: {template:?}), invalid)
end
"#));
    let run = format!(r#"{checks}
bot=User.new
[false,true].each do |invalid|
  bot.valid? if invalid
  check_aliased_form(Views::Pages.bot(bot), invalid)
end
"#);
    emit_and_run::empty_app().write("db/schema.rb",schema)
        .write("config/routes.rb","Rails.application.routes.draw do\nend\n")
        .write("app/models/user.rb",model).write("config/locales/en.yml",locale)
        .write("app/controllers/pages_controller.rb","class PagesController < ActionController::Base\ndef bot; @bot=User.new; end\nend\n")
        .write("app/views/pages/bot.html.erb",template).write("app/views/pages/_fields.html.erb",fields)
        .run_ruby(&run).assert_passes();
}


fn nested_fields_for_association_case(kind: &str) {
    let (nested, association, target, unresolved) = match kind {
        "namespaced" => ("profile", "has_one :profile, class_name: 'Admin::Profile', foreign_key: :account_id, autosave: true", "Admin::Profile", false),
        "class_name" => ("details", "has_one :details, class_name: 'Admin::Profile', foreign_key: :account_id, autosave: true", "Admin::Profile", false),
        "ordinary" => ("profile", "has_one :profile, foreign_key: :account_id, autosave: true", "Profile", false),
        "unresolved" => ("profile", "belongs_to :profile, polymorphic: true, optional: true", "Admin::Profile", true),
        _ => unreachable!(),
    };
    let schema = if unresolved {
        "ActiveRecord::Schema.define do\ncreate_table(:accounts) { |t| t.integer :profile_id; t.string :profile_type }\ncreate_table(:profiles) { |t| t.integer :account_id }\ncreate_table(:admin_profiles) { |t| t.integer :account_id; t.string :nickname; t.boolean :enabled }\ncreate_table(:details) { |t| t.integer :account_id }\nend\n"
    } else {
        "ActiveRecord::Schema.define do\ncreate_table(:accounts) { |t| t.integer :profile_id; t.string :profile_type }\ncreate_table(:profiles) { |t| t.integer :account_id }\ncreate_table(:admin_profiles) { |t| t.integer :account_id }\ncreate_table(:details) { |t| t.integer :account_id }\nend\n"
    };
    let profile = if kind == "ordinary" || unresolved {
        "class Profile < ActiveRecord::Base\nattribute :nickname, :string\nattribute :enabled, :boolean\nend\n"
    } else { "class Profile < ActiveRecord::Base\nattribute :unrelated, :string\nend\n" };
    let admin = if unresolved { "module Admin\nclass Profile < ActiveRecord::Base\nself.table_name = 'admin_profiles'\nend\nend\n" }
        else { "module Admin\nclass Profile < ActiveRecord::Base\nself.table_name = 'admin_profiles'\nattribute :nickname, :string\nattribute :enabled, :boolean\nend\nend\n" };
    let details = "class Details < ActiveRecord::Base\nattribute :unrelated, :string\nend\n";
    let parent_attributes = if unresolved { "attribute :nickname, :string\n" } else { "" };
    let writer = if unresolved { format!("def {nested}_attributes=(attributes); end\n") }
        else { format!("accepts_nested_attributes_for :{nested}\n") };
    let account = format!("class Account < ActiveRecord::Base\n{parent_attributes}{association}\n{writer}end\n");
    let setup = if unresolved {
        format!("account=Account.new\nrecord={target}.new(nickname: 'Nested value', enabled: true)\naccount.define_singleton_method(:{nested}) {{ record }}\n")
    } else {
        format!("account=Account.new\naccount.{nested}={target}.new(nickname: 'Nested value', enabled: true)\n")
    };
    let template = format!("<%= form_with model: @account, url: '/', method: :post do |form| %><%= form.fields_for :{nested} do |child| %><%= child.text_field :nickname %><%= child.check_box :enabled %><% end %><% end %>");
    let check = r#"
def check_nested_values(html)
  raise 'nested value' unless html.include?('value="Nested value"')
  checkbox=html.scan(/<input[^>]*>/).find { |tag| tag.include?('type="checkbox"') }
  raise 'nested boolean' unless checkbox && checkbox.include?('checked="checked"')
end
"#;
    let reflection = if unresolved { String::new() }
        else { format!("raise 'association target' unless Account.reflect_on_association(:{nested}).klass == {target}\n") };
    native(&["activerecord", "actionview", "actionpack"], &format!(r#"{AR}
require 'action_view'
require 'action_controller'
{schema}{profile}{admin}{details}{account}
{reflection}
ActionView::Helpers::FormHelper.form_with_generates_ids=true
view=ActionView::Base.with_empty_template_cache.new(ActionView::LookupContext.new([]), {{}}, nil)
view.define_singleton_method(:protect_against_forgery?) {{ false }}
{setup}
view.assign(account: account)
{check}
check_nested_values(view.render(inline: {template:?}))
"#));
    let run = emit_and_run::empty_app().write("db/schema.rb",schema)
        .write("config/routes.rb","Rails.application.routes.draw do\nend\n")
        .write("app/models/account.rb",&account).write("app/models/profile.rb",profile)
        .write("app/models/admin/profile.rb",admin).write("app/models/details.rb",details)
        .write("app/controllers/pages_controller.rb","class PagesController < ActionController::Base\ndef form; @account=Account.new; end\nend\n")
        .write("app/views/pages/form.html.erb",&template)
        .run_ruby(&format!("{check}\n{setup}\ncheck_nested_values(Views::Pages.form(account))\n"));
    run.assert_passes();
    let source = std::fs::read_to_string(run.emitted.join("app/views/pages/form.rb")).unwrap();
    if unresolved {
        assert!(!source.contains(".nickname"), "an unresolved association must not borrow a model's virtual reader metadata");
    } else {
        assert!(source.contains(".nickname"), "a resolved association keeps its non-column reader");
    }
}

#[test]
fn nested_fields_for_uses_namespaced_association_target() { nested_fields_for_association_case("namespaced"); }

#[test]
fn nested_fields_for_uses_explicit_class_name_target() { nested_fields_for_association_case("class_name"); }

#[test]
fn nested_fields_for_keeps_ordinary_association_precision() { nested_fields_for_association_case("ordinary"); }

#[test]
fn nested_fields_for_does_not_guess_unresolved_association_target() { nested_fields_for_association_case("unresolved"); }

fn reopened_finder_ownership_case(library: bool) {
    let owner = if library { "LookupService" } else { "Category" };
    let base = if library { "LookupParent" } else { "ActiveRecord::Base" };
    let prelude = if library { "class LookupParent < ActiveRecord::Base\nself.table_name='categories'\nend\n" } else { "" };
    let extra_declaration = if library { format!("class {owner} < {base}\nend\n") } else { String::new() };
    let finder_header = if library { format!("class {owner}") } else { format!("class {owner} < {base}") };
    let methods = format!("{extra_declaration}{finder_header}\ndef self.find_by_name(value); 'source finder'; end\ndef self.find_by_name!(value); 'source finder'; end\ndef self.find_by(attributes); 'source finder'; end\ndef self.find_by!(attributes); 'source finder'; end\nend\n");
    let include_header = if library { format!("class {owner}") } else { format!("class {owner} < {base}") };
    let ancestry = format!("{include_header}\ninclude MissingFinderExtension\nend\n");
    let controller = format!("class CategoriesController < ActionController::Base\ndef dynamic; {owner}.find_by_name('found'); end\ndef dynamic_bang; {owner}.find_by_name!('found'); end\ndef ordinary; {owner}.find_by(name: 'found'); end\ndef ordinary_bang; {owner}.find_by!(name: 'found'); end\nend\n");
    let mut failures = Vec::new();
    for reversed in [false, true] {
        let (first, second) = if reversed { (&ancestry, &methods) } else { (&methods, &ancestry) };
        // Bootstrap the class so both orders are valid Ruby reopenings;
        // the compiler still receives its written superclass declaration.
        native(&["activerecord"], &format!("{AR}{CATEGORIES}{prelude}\nmodule MissingFinderExtension; end\nclass {owner} < {base}; end\neval({first:?}, TOPLEVEL_BINDING, 'first.rb')\neval({second:?}, TOPLEVEL_BINDING, 'second.rb')\nraise 'dynamic' unless {owner}.find_by_name('found') == 'source finder'\nraise 'dynamic bang' unless {owner}.find_by_name!('found') == 'source finder'\nraise 'ordinary' unless {owner}.find_by(name: 'found') == 'source finder'\nraise 'ordinary bang' unless {owner}.find_by!(name: 'found') == 'source finder'\n"));
        let root = if library { "app/services" } else { "app/models" };
        let a = format!("{root}/first.rb");
        let b = format!("{root}/second.rb");
        let tree: HashMap<PathBuf, Vec<u8>> = [
            ("db/schema.rb".to_owned(), CATEGORIES.to_owned()),
            ("app/models/lookup_parent.rb".to_owned(), prelude.to_owned()),
            (a.clone(), first.clone()), (b.clone(), second.clone()),
            ("app/controllers/categories_controller.rb".to_owned(), controller.clone()),
        ].into_iter().map(|(p,s)| (p.into(),s.into_bytes())).collect();
        let mut app = roundhouse::ingest::ingest_app_from_tree(tree).unwrap();
        let declarations = app.models.iter().filter(|m| m.name.0.as_str()==owner).count()
            + app.library_classes.iter().filter(|c| c.name.0.as_str()==owner).count();
        assert_eq!(declarations, if library { 3 } else { 2 }, "the fixture must retain its declarations");
        let mut analyzer = Analyzer::new(&app);
        analyzer.analyze(&mut app);
        for action in app.controllers[0].actions() {
            if !action.body.ty.as_ref().unwrap().is_unknown() {
                failures.push(format!("order={reversed}, {} inferred {:?}", action.name, action.body.ty));
            }
        }
        let (_, errors) = emit_and_run::empty_app().write("db/schema.rb", CATEGORIES)
            .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
            .write("app/models/lookup_parent.rb", prelude)
            .write(&a, first).write(&b, second)
            .write("app/controllers/categories_controller.rb", &controller).emit(BuildTarget::Ruby);
        if !errors.iter().any(|e| e.contains("dynamic finder without established implementation")) {
            failures.push(format!("order={reversed}, missing ambiguous finder refusal"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn reopened_model_finder_ownership_is_order_independent() { reopened_finder_ownership_case(false); }
#[test]
fn reopened_library_finder_ownership_is_order_independent() { reopened_finder_ownership_case(true); }
