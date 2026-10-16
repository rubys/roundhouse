//! `permit(*helper, …)` where `helper` answers a literal Symbol list
//! (#858).
//!
//! ```ruby
//! def common_fields = %i[name color]
//! def gadget_params = params.require(:gadget).permit(*common_fields, :size)
//! ```
//!
//! The permit recognizer failed on the splat and left the chain as
//! written: `@params.require(:gadget)` on the request's plain Hash is
//! `Kernel#require`, which raised "private method 'require' called for
//! an instance of Hash". A splat of a literal list is the list written
//! inline, so it now lowers like `permit(:name, :color, :size)`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::project::{target_files, BuildTarget};

const SCHEMA: &str = "ActiveRecord::Schema.define(version: 1) do\n  \
    create_table :gadgets do |t|\n    t.string :name\n    t.string :color\n    t.string :size\n  end\nend\n";

/// The emitted `gadgets_controller` (Ruby and RBS) with `helpers` as its
/// private methods and an `update` action that hands `gadget_params` to
/// `update!`. `base` is the body of an `ApplicationController` the
/// controller inherits from.
fn controller_for(target: BuildTarget, helpers: &str, base: &str) -> String {
    let controller = format!(
        "class GadgetsController < ApplicationController\n  def update\n    @gadget = Gadget.find(params[:id])\n    @gadget.update!(gadget_params)\n    head :no_content\n  end\n\n  private\n\n{helpers}\nend\n"
    );
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(PathBuf::from("db/schema.rb"), SCHEMA.as_bytes().to_vec());
    tree.insert(PathBuf::from("app/models/gadget.rb"), b"class Gadget < ApplicationRecord\nend\n".to_vec());
    tree.insert(
        PathBuf::from("config/routes.rb"),
        b"Rails.application.routes.draw do\n  resources :gadgets, only: :update\nend\n".to_vec(),
    );
    tree.insert(
        PathBuf::from("app/controllers/application_controller.rb"),
        format!("class ApplicationController < ActionController::Base\n{base}\nend\n").into_bytes(),
    );
    tree.insert(PathBuf::from("app/controllers/gadgets_controller.rb"), controller.into_bytes());
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let files = target_files(&app, Path::new("fixtures/tiny-blog"), target).expect("emit");
    files
        .into_iter()
        .filter(|(p, _)| p.contains("gadgets_controller"))
        .map(|(_, c)| c)
        .collect::<Vec<_>>()
        .join("\n")
}

const TYPED: &str = r#"GadgetParams.from_raw(Params.require_present(@params, "gadget"))"#;

#[test]
fn a_splat_of_a_literal_symbol_list_is_the_list() {
    let helper = "  def gadget_params\n    params.require(:gadget).permit(*common_fields, :size)\n  end\n";
    for list in ["%i[name color]", "[:name, :color]", "%i[name color].freeze"] {
        let helpers = format!("  def common_fields\n    {list}\n  end\n\n{helper}");
        for target in [BuildTarget::Ruby, BuildTarget::Spinel] {
            let src = controller_for(target, &helpers, "");
            assert!(src.contains(TYPED), "{target:?} {list}:\n{src}");
            assert!(!src.contains(".permit("), "{target:?} {list}:\n{src}");
        }
    }
}

/// `self.common_fields`, and a list defined on the superclass.
#[test]
fn a_self_call_and_an_inherited_list_are_read() {
    let helpers = "  def common_fields\n    %i[name color]\n  end\n\n  def gadget_params\n    params.require(:gadget).permit(*self.common_fields)\n  end\n";
    let src = controller_for(BuildTarget::Spinel, helpers, "");
    assert!(src.contains(TYPED), "{src}");
    let helpers = "  def gadget_params\n    params.require(:gadget).permit(*common_fields, :size)\n  end\n";
    let base = "  private\n\n  def common_fields\n    %i[name color]\n  end\n";
    let src = controller_for(BuildTarget::Spinel, helpers, base);
    assert!(src.contains(TYPED), "{src}");
}

/// A list the source does not spell out stays as it was.
#[test]
fn a_list_built_at_run_time_is_left_alone() {
    let helper = "  def gadget_params\n    params.require(:gadget).permit(*common_fields(true))\n  end\n";
    let helpers = format!("  def common_fields(all)\n    all ? %i[name color] : %i[name]\n  end\n\n{helper}");
    let src = controller_for(BuildTarget::Spinel, &helpers, "");
    assert!(!src.contains(TYPED), "{src}");
    let helper = "  def gadget_params\n    params.require(:gadget).permit(*common_fields)\n  end\n";
    let helpers = format!("  def common_fields\n    %i[name color] + extra\n  end\n\n  def extra\n    %i[size]\n  end\n\n{helper}");
    let src = controller_for(BuildTarget::Spinel, &helpers, "");
    assert!(!src.contains(TYPED), "{src}");
}
