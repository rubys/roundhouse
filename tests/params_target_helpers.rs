use std::collections::HashMap;
use std::path::{Path, PathBuf};

use roundhouse::build_target::BuildTarget;
use roundhouse::ingest::ingest_app_from_tree;

fn emitted(target: BuildTarget, nested: bool) -> String {
    let helper = if nested {
        "params.require(:item).permit(:name, settings: {})"
    } else {
        "params.require(:item).permit(:name)"
    };
    let source = format!(r#"class ItemsController < ActionController::API
  def create
    p = item_params
    p.key?(:name)
    p.include?("name")
    p[:settings]["name"]
    head :ok
  end

  private

  def item_params
    {helper}
  end
end
"#);
    let tree: HashMap<PathBuf, Vec<u8>> = [
        ("app/models/item.rb", "class Item < ActiveRecord::Base\nend\n".into()),
        ("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table :items do |t|\n    t.string :name\n  end\nend\n".into()),
        ("app/controllers/items_controller.rb", source),
        ("config/routes.rb", "Rails.application.routes.draw do\n  post '/items', to: 'items#create'\nend\n".into()),
    ].into_iter().map(|(path, source)| (path.into(), source.into_bytes())).collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower_for(&mut app, target);
    roundhouse::project::target_files(&app, Path::new("fixtures/tiny-api"), target)
        .expect("emit target").into_iter().map(|(_, text)| text).collect::<Vec<_>>().join("\n")
}

#[test]
fn non_ruby_targets_keep_synthesized_scalar_helpers() {
    for target in [
        BuildTarget::Crystal, BuildTarget::Elixir, BuildTarget::Go,
        BuildTarget::Kotlin, BuildTarget::Python, BuildTarget::Rust,
        BuildTarget::Swift, BuildTarget::CSharp, BuildTarget::Typescript,
        BuildTarget::TypescriptWorker,
    ] {
        for nested in [false, true] {
            let source = emitted(target, nested);
            let normalized: String = source.chars().filter(|c| c.is_ascii_alphanumeric())
                .map(|c| c.to_ascii_lowercase()).collect();
            assert!(!normalized.contains("filteredvalues"), "{} nested={nested}: Ruby-only filtered_values", target.as_str());
            assert!(!normalized.contains("nestedlookup"), "{} nested={nested}: Ruby-only nested_lookup", target.as_str());
            assert!(normalized.contains("rhscalarvalues"), "{} nested={nested}: missing synthesized scalar filter", target.as_str());
            assert!(normalized.contains("rhparamvalue"), "{} nested={nested}: missing synthesized value reader", target.as_str());
        }
    }
}
