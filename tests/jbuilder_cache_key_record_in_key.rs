//! Regression for `ActiveRecord::Base#cache_key_with_version` — the
//! single-record half of the fragment-cache key a jbuilder `cache!`
//! names, companion to `tests/jbuilder_cache_key_with_version.rs`'s
//! Relation case.
//!
//! `json.cache!([current_account, @widget]) do … end`, with
//! `current_account` a `helper_method` answering a RECORD (not the
//! collection this template renders): Jbuilder's `cache!(key=nil,
//! options={})` evaluates `key` as a plain positional argument before
//! it ever checks `perform_caching` (confirmed against jbuilder
//! 2.15.1's `jbuilder_template.rb`), so `current_account.
//! cache_key_with_version` is called whether or not caching is on.
//! Real Rails answers it via `ActiveRecord::Integration`
//! (`active_record/integration.rb`); roundhouse's own
//! `ActiveRecord::Base` runtime never defined it.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

#[test]
fn a_record_in_the_cache_key_answers_cache_key_with_version() {
    let run = emit_and_run::empty_app()
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"accounts\", force: :cascade do |t|\n    t.string \"name\"\n    t.timestamps\n  end\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n",
        )
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"widgets/:id\", to: \"widgets#show\", defaults: { format: :json }\nend\n",
        )
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n",
        )
        .write("app/models/account.rb", "class Account < ApplicationRecord\nend\n")
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n")
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\n  helper_method :current_account\n\n  def current_account\n    Account.first\n  end\nend\n",
        )
        .write(
            "app/controllers/widgets_controller.rb",
            "class WidgetsController < ApplicationController\n  def show\n    @widget = Widget.find(params[:id])\n    render :show\n  end\nend\n",
        )
        .write(
            "app/views/widgets/show.json.jbuilder",
            "json.cache!([current_account, @widget]) do\n  json.id @widget.id\n  json.account_key current_account.cache_key_with_version\nend\n",
        )
        .run_ruby(
            "account = Account.create!(name: \"acme\")\nwidget = Widget.create!(name: \"gizmo\")\nputs Views::Widgets.show_json(widget)\n",
        );
    run.assert_passes();
    assert!(
        run.stdout.contains("accounts/"),
        "stdout: {}\nstderr: {}",
        run.stdout,
        run.stderr
    );
}

/// `cache_version` reads `updated_at` through `self[:updated_at]`
/// rather than the bare per-model reader (see `base.rb` for why that
/// bare call left an unresolved type variable) — this confirms the
/// escape is not just type-checker-satisfying theater: `[]`'s
/// CONCRETE per-model override answers the real column value, so a
/// record's `cache_key_with_version` changes when its `updated_at`
/// does, the same as Rails' own cache_version answering a different
/// string after a touch.
#[test]
fn a_touched_record_gets_a_different_cache_version() {
    let run = emit_and_run::empty_app()
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"accounts\", force: :cascade do |t|\n    t.string \"name\"\n    t.timestamps\n  end\nend\n",
        )
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\nend\n",
        )
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n",
        )
        .write("app/models/account.rb", "class Account < ApplicationRecord\nend\n")
        .run_ruby(
            "account = Account.create!(name: \"acme\")\nbefore = account.cache_key_with_version\naccount.update!(updated_at: Time.now + 3600)\nafter = account.cache_key_with_version\nputs before\nputs after\nputs(before == after ? \"SAME\" : \"DIFFERENT\")\n",
        );
    run.assert_passes();
    assert!(
        run.stdout.contains("DIFFERENT"),
        "a touched record's cache_key_with_version should change: stdout: {}\nstderr: {}",
        run.stdout,
        run.stderr
    );
}
