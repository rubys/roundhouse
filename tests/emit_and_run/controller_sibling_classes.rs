//! `class LoginFailedError < StandardError; end` ahead of the controller
//! in its file is a class the app declares: its actions raise and rescue
//! it as Rails' do, and a target that does not emit it says so.

use super::emit_and_run;

fn sibling_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog().edit(
        "app/controllers/articles_controller.rb",
        "class ArticlesController < ApplicationController\n",
        "class ArticleLookupError < StandardError; end\n\nclass ArticlesController < ApplicationController\n  def self.guarded(fail)\n    raise ArticleLookupError, \"no article\" if fail\n    \"found\"\n  rescue ArticleLookupError => e\n    \"rescued: #{e.message}\"\n  end\n\n",
    )
}

const ASSERTIONS: &str = r##"require_relative "app/controllers/articles_controller"
got = [ArticlesController.guarded(false), ArticlesController.guarded(true), ArticleLookupError.superclass.name]
raise got.inspect unless got == ["found", "rescued: no article", "StandardError"]
puts "sibling classes passed"
"##;

#[test]
fn a_class_beside_the_controller_is_raised_and_rescued() {
    sibling_app().run_ruby(ASSERTIONS).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_class_beside_the_controller_is_raised_and_rescued_on_spinel() {
    sibling_app().run_spinel(ASSERTIONS).assert_passes();
}

#[test]
fn a_class_beside_the_controller_is_unsupported_where_it_is_not_emitted() {
    let (_emitted, errors) = emit_and_run::real_blog()
        .edit(
            "app/controllers/articles_controller.rb",
            "class ArticlesController < ApplicationController\n",
            "class ArticleLookupError < StandardError; end\n\nclass ArticlesController < ApplicationController\n",
        )
        .emit(roundhouse::project::BuildTarget::Rust);
    assert!(errors.iter().any(|e| e.contains("`ArticleLookupError`, declared beside `ArticlesController`")), "{errors:#?}");
}
