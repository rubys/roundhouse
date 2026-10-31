//! Constructs that `check` accepts must run once emitted.
//!
//! See `tests/support/emit_and_run.rs` for the harness and why it
//! exists. The ignored tests below are known places where the two
//! disagree: `check` is clean and the emitted program fails. Each is a
//! complete statement of the fix: make it pass and drop the `#[ignore]`.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/time_change_usec.rs"]
mod time_change_usec_contract;
#[path = "emit_and_run/time_change_usec.rs"]
mod time_change_usec;
#[path = "support/i18n_locales_app.rs"]
#[allow(dead_code)]
mod i18n_locales_app;
#[path = "support/pundit_app.rs"]
mod pundit_app;
#[path = "emit_and_run/render_notifications.rs"]
mod render_notifications;
#[path = "emit_and_run/session_cookie_key.rs"]
mod session_cookie_key;
#[path = "emit_and_run/inertia_runtime_props.rs"]
mod inertia_runtime_props;
#[path = "emit_and_run/form_builder_fields.rs"]
mod form_builder_fields;
#[path = "emit_and_run/multiparameter_params.rs"]
mod multiparameter_params;
#[path = "emit_and_run/session_options_key.rs"]
mod session_options_key;
#[path = "emit_and_run/httpdate_from_now.rs"]
mod httpdate_from_now;
#[path = "emit_and_run/cookie_jar_each.rs"]
mod cookie_jar_each;
#[path = "emit_and_run/create_block.rs"]
mod create_block;
#[path = "emit_and_run/active_model_attribute_alias.rs"]
mod active_model_attribute_alias;
#[path = "emit_and_run/active_model_attributes.rs"]
mod active_model_attributes;
#[path = "emit_and_run/active_model_attributes_form.rs"]
mod active_model_attributes_form;
#[path = "emit_and_run/association_query_arguments.rs"]
mod association_query_arguments;
#[path = "emit_and_run/tableless_params.rs"]
mod tableless_params;
#[path = "emit_and_run/splatted_accessor_params.rs"]
mod splatted_accessor_params;
#[path = "emit_and_run/active_model_superclass.rs"]
mod active_model_superclass;
#[path = "emit_and_run/arel_nodes.rs"]
mod arel_nodes;
#[path = "emit_and_run/permit_private_writer.rs"]
mod permit_private_writer;
#[path = "emit_and_run/database_tasks.rs"]
mod database_tasks;
#[path = "emit_and_run/framework_exceptions.rs"]
mod framework_exceptions;
#[path = "emit_and_run/active_storage_exceptions.rs"]
mod active_storage_exceptions;
#[path = "emit_and_run/rescue_surface.rs"]
mod rescue_surface;
#[path = "emit_and_run/preloader_runtime.rs"]
mod preloader_runtime;
#[path = "emit_and_run/framework_surface_rest.rs"]
mod framework_surface_rest;
#[path = "emit_and_run/date_calendar_duration.rs"]
mod date_calendar_duration;
#[path = "emit_and_run/interval_columns.rs"]
mod interval_columns;
#[path = "emit_and_run/branch_local_join.rs"]
mod branch_local_join;
#[path = "emit_and_run/utc_zone_time.rs"]
mod utc_zone_time;
#[path = "emit_and_run/rspec_specs.rs"]
mod rspec_specs;
#[path = "emit_and_run/discard.rs"]
mod discard;
#[path = "emit_and_run/callback_abort.rs"]
mod callback_abort;
#[path = "emit_and_run/dirty_clear_restore.rs"]
mod dirty_clear_restore;
#[path = "emit_and_run/pundit_source.rs"]
mod pundit_source;
#[path = "emit_and_run/constantize_by_name.rs"]
mod constantize_by_name;
#[path = "emit_and_run/ancestry_source.rs"]
mod ancestry_source;
#[path = "emit_and_run/acts_as_follower_source.rs"]
mod acts_as_follower_source;
#[path = "emit_and_run/hook_macro_source.rs"]
mod hook_macro_source;
#[path = "emit_and_run/model_callbacks.rs"]
mod model_callbacks;
#[path = "emit_and_run/gem_class_macros.rs"]
mod gem_class_macros;
#[path = "emit_and_run/source_gem_autoload.rs"]
mod source_gem_autoload;
#[path = "emit_and_run/source_gem_facade.rs"]
mod source_gem_facade;
#[path = "support/for_loops.rs"]
mod for_loops_contract;
#[path = "emit_and_run/for_loops.rs"]
mod for_loops;
#[path = "support/block_macros.rs"]
mod block_macros_contract;
#[path = "emit_and_run/block_macros.rs"]
mod block_macros;
#[path = "support/stdlib_scan_zlib.rs"]
mod stdlib_scan_zlib_contract;
#[path = "emit_and_run/stdlib_scan_zlib.rs"]
mod stdlib_scan_zlib;
#[path = "support/stdlib_rexml_include.rs"]
mod stdlib_rexml_include_contract;
#[path = "emit_and_run/stdlib_rexml_include.rs"]
mod stdlib_rexml_include;
#[path = "support/mutable_literals.rs"]
mod mutable_literals_contract;
#[path = "emit_and_run/mutable_literals.rs"]
mod mutable_literals;
#[path = "support/auto_source_gem.rs"]
mod auto_source_gem_contract;
#[path = "emit_and_run/auto_source_gem.rs"]
mod auto_source_gem;
#[path = "emit_and_run/pay_exceptions.rs"]
mod pay_exceptions;
#[path = "emit_and_run/net_http_patch.rs"]
mod net_http_patch;
#[path = "emit_and_run/stdlib_psych_rexml_erb.rs"]
mod stdlib_psych_rexml_erb;
#[path = "emit_and_run/temporal_index_assign.rs"]
mod temporal_index_assign;
#[path = "emit_and_run/file_mtime_time.rs"]
mod file_mtime_time;
#[path = "support/class_attribute.rs"]
mod class_attribute;
#[path = "support/string_truncate.rs"]
mod string_truncate;
#[path = "emit_and_run/string_truncate.rs"]
mod string_truncate_runs;
#[path = "support/string_truncate_words.rs"]
mod string_truncate_words;
#[path = "emit_and_run/string_truncate_words.rs"]
mod string_truncate_words_runs;
#[path = "support/array_excluding.rs"]
mod array_excluding;
#[path = "emit_and_run/array_excluding.rs"]
mod array_excluding_runs;
#[path = "emit_and_run/integer_query_find_by.rs"]
mod integer_query_find_by;
#[path = "emit_and_run/time_zone_default.rs"]
mod time_zone_default;
#[path = "emit_and_run/view_presence_url_encode.rs"]
mod view_presence_url_encode;
#[path = "emit_and_run/controller_translate.rs"]
mod controller_translate;
#[path = "emit_and_run/mailer_templates.rs"]
mod mailer_templates;
#[path = "emit_and_run/view_paths.rs"]
mod view_paths;
#[path = "emit_and_run/unreachable_views.rs"]
mod unreachable_views;
#[path = "emit_and_run/job_rest_params.rs"]
mod job_rest_params;
#[path = "emit_and_run/regexp_constants.rs"]
mod regexp_constants;
#[path = "emit_and_run/upsert.rs"]
mod upsert;
#[path = "emit_and_run/relation_parts.rs"]
mod relation_parts;
#[path = "support/relation_fork.rs"]
mod relation_fork_contract;
#[path = "emit_and_run/relation_fork.rs"]
mod relation_fork;
#[path = "emit_and_run/controller_sibling_classes.rs"]
mod controller_sibling_classes;
#[path = "emit_and_run/i18n_locales.rs"]
mod i18n_locales;
#[path = "emit_and_run/i18n_runtime.rs"]
mod i18n_runtime;
#[path = "emit_and_run/helper_translate.rs"]
mod helper_translate;
#[path = "emit_and_run/helper_escape.rs"]
mod helper_escape;
#[path = "emit_and_run/helper_escape_args.rs"]
mod helper_escape_args;
#[path = "emit_and_run/controller_view_assigns.rs"]
mod controller_view_assigns;
#[path = "emit_and_run/controller_test_case.rs"]
mod controller_test_case;
#[path = "emit_and_run/assert_recognizes.rs"]
mod assert_recognizes;
#[path = "emit_and_run/assert_routing.rs"]
mod assert_routing;
#[path = "emit_and_run/class_new_constants.rs"]
mod class_new_constants;
#[path = "emit_and_run/i18n_subtree.rs"]
mod i18n_subtree;
#[path = "emit_and_run/i18n_values.rs"]
mod i18n_values;
#[path = "emit_and_run/i18n_locale_tag.rs"]
mod i18n_locale_tag;
#[path = "emit_and_run/i18n_plural_rules.rs"]
mod i18n_plural_rules;
#[path = "emit_and_run/rails_i18n_locales.rs"]
mod rails_i18n_locales;
#[path = "support/lambda_signatures.rs"]
mod lambda_signatures_contract;
#[path = "emit_and_run/lambda_signatures.rs"]
mod lambda_signatures;
#[path = "support/send_data_filename.rs"]
mod send_data_filename_contract;
#[path = "emit_and_run/send_data_filename.rs"]
mod send_data_filename;
#[path = "support/vary_accept.rs"]
mod vary_accept_contract;
#[path = "emit_and_run/vary_accept.rs"]
mod vary_accept;
#[path = "emit_and_run/strong_params.rs"]
mod strong_params;
#[path = "emit_and_run/parameter_defaults.rs"]
mod parameter_defaults;
#[path = "emit_and_run/params_wrapper.rs"]
mod params_wrapper;
#[path = "support/stdlib_rnp.rs"]
mod stdlib_rnp_contract;
#[path = "emit_and_run/stdlib_rnp.rs"]
mod stdlib_rnp;
#[path = "support/erb_template.rs"]
mod erb_template_contract;
#[path = "support/bare_permit.rs"]
mod bare_permit_contract;
#[path = "support/data_paths.rs"]
mod data_paths_contract;
#[path = "support/eager_load_filter.rs"]
mod eager_load_filter_contract;
#[path = "emit_and_run/data_paths.rs"]
mod data_paths;
#[path = "emit_and_run/bare_permit.rs"]
mod bare_permit;
#[path = "emit_and_run/erb_template.rs"]
mod erb_template;
#[path = "support/representable_decorator.rs"]
mod representable_decorator;
#[path = "emit_and_run/representable.rs"]
mod representable;
#[path = "support/active_model_serializer.rs"]
mod active_model_serializer_app;
#[path = "emit_and_run/active_model_serializer.rs"]
mod active_model_serializer;
#[path = "emit_and_run/instance_extend.rs"]
mod instance_extend;

#[path = "support/class_configuration.rs"]
mod class_configuration;
#[path = "support/dry_initializer.rs"]
mod dry_initializer;
#[path = "support/runtime_block_signature.rs"]
mod runtime_block_signature;
#[path = "support/dry_struct.rs"]
mod dry_struct;
#[path = "support/dry_validation.rs"]
mod dry_validation;
#[path = "support/dry_monads.rs"]
mod dry_monads;
#[path = "support/data_factory.rs"]
mod data_factory;
#[path = "support/rails_root_join.rs"]
mod rails_root_join;
#[path = "support/cable_allowed_origins.rs"]
mod cable_allowed_origins_contract;
#[path = "emit_and_run/cable_allowed_origins.rs"]
mod cable_allowed_origins;
#[path = "support/public_file_server_headers.rs"]
mod public_file_server_headers_contract;
#[path = "emit_and_run/public_file_server_headers.rs"]
mod public_file_server_headers;
#[path = "support/cable_actions.rs"]
mod cable_actions_contract;
#[path = "emit_and_run/cable_actions.rs"]
mod cable_actions;
#[path = "support/cable_broadcast_json.rs"]
mod cable_broadcast_json_contract;
#[path = "support/cable_transmit.rs"]
mod cable_transmit_contract;
#[path = "emit_and_run/cable_transmit.rs"]
mod cable_transmit;
#[path = "emit_and_run/cable_broadcast_json.rs"]
mod cable_broadcast_json;
#[path = "support/index_with.rs"]
mod index_with_contract;
#[path = "emit_and_run/index_with.rs"]
mod index_with;
#[path = "support/request_id_runtime.rs"]
mod request_id_runtime_contract;
#[path = "emit_and_run/request_id_runtime.rs"]
mod request_id_runtime;
#[path = "support/anonymous_keywords.rs"]
mod anonymous_keywords;
#[path = "support/delegate_association.rs"]
mod delegate_association;
#[path = "support/io_process_constants.rs"]
mod io_process_constants_contract;
#[path = "emit_and_run/io_process_constants.rs"]
mod io_process_constants;
#[path = "support/backticks.rs"]
mod backticks_contract;
#[path = "emit_and_run/backticks.rs"]
mod backticks;
#[path = "support/json_params.rs"]
mod json_params_contract;
#[path = "emit_and_run/json_params.rs"]
mod json_params;
#[path = "support/class_side_default.rs"]
mod class_side_default_contract;
#[path = "emit_and_run/class_side_default.rs"]
mod class_side_default;
#[path = "emit_and_run/helper_anonymous_rest.rs"]
mod helper_anonymous_rest;
#[path = "emit_and_run/controller_response_body.rs"]
mod controller_response_body;
#[path = "emit_and_run/relation_projections.rs"]
mod relation_projections;
#[path = "emit_and_run/pluck_projection_semantics.rs"]
mod pluck_projection_semantics;
#[path = "emit_and_run/response_redirect.rs"]
mod response_redirect;
#[path = "emit_and_run/api_response_fidelity.rs"]
mod api_response_fidelity;
#[path = "emit_and_run/constructor_unrelated_mutations.rs"]
mod constructor_unrelated_mutations;
#[path = "emit_and_run/render_to_string_partial_ivar.rs"]
mod render_to_string_partial_ivar;
#[path = "emit_and_run/active_support_extensions.rs"]
mod active_support_extensions;
#[path = "emit_and_run/routing_redirect_host.rs"]
mod routing_redirect_host;
#[path = "emit_and_run/time_find_zone.rs"]
mod time_find_zone;
#[path = "emit_and_run/time_zone_missing_zonefile.rs"]
mod time_zone_missing_zonefile;
#[path = "emit_and_run/time_to_date.rs"]
mod time_to_date;
#[path = "emit_and_run/concurrent_map.rs"]
mod concurrent_map;
#[path = "emit_and_run/request_optional_port.rs"]
mod request_optional_port;
#[path = "emit_and_run/request_port.rs"]
mod request_port;
#[path = "emit_and_run/action_dispatch_request_put.rs"]
mod action_dispatch_request_put;
#[path = "emit_and_run/controller_url_helpers.rs"]
mod controller_url_helpers;
#[path = "emit_and_run/url_helpers_only_path.rs"]
mod url_helpers_only_path;
#[path = "emit_and_run/url_helpers_absolute.rs"]
mod url_helpers_absolute;
#[path = "emit_and_run/route_default_segment_args.rs"]
mod route_default_segment_args;
#[path = "emit_and_run/keyed_collection_cache.rs"]
mod keyed_collection_cache;
#[path = "emit_and_run/association_loaded_list.rs"]
mod association_loaded_list;
#[path = "emit_and_run/controller_super_ivars.rs"]
mod controller_super_ivars;
#[path = "emit_and_run/helper_method_session.rs"]
mod helper_method_session;
#[path = "emit_and_run/assoc_primary_key.rs"]
mod assoc_primary_key;
#[path = "emit_and_run/through_source_class.rs"]
mod through_source_class;
#[path = "emit_and_run/assoc_pluck_typed.rs"]
mod assoc_pluck_typed;
#[path = "emit_and_run/action_controller_head.rs"]
mod action_controller_head;
#[path = "support/conditional_get.rs"]
mod conditional_get_contract;
#[path = "emit_and_run/conditional_get.rs"]
mod conditional_get;
#[path = "emit_and_run/nil_body_length.rs"]
mod nil_body_length;
#[path = "emit_and_run/inline_filter_guard.rs"]
mod inline_filter_guard;
#[path = "emit_and_run/before_action_return.rs"]
mod before_action_return;
#[path = "emit_and_run/sti_global_id.rs"]
mod sti_global_id;
#[path = "emit_and_run/global_id_app.rs"]
mod global_id_app;
#[path = "support/campfire_caches.rs"]
mod campfire_caches_contract;
#[path = "emit_and_run/campfire_caches.rs"]
mod campfire_caches;
#[path = "emit_and_run/action_text_markdown.rs"]
mod action_text_markdown;
#[path = "emit_and_run/ordinalize.rs"]
mod ordinalize;
#[path = "emit_and_run/float_round_digits.rs"]
mod float_round_digits;
#[path = "emit_and_run/autoload_once_paths.rs"]
mod autoload_once_paths;
#[path = "emit_and_run/config_require_relative.rs"]
mod config_require_relative;
#[path = "emit_and_run/defined_const_fold.rs"]
mod defined_const_fold;
#[path = "emit_and_run/job_payload.rs"]
mod job_payload;
#[path = "emit_and_run/sidekiq_push.rs"]
mod sidekiq_adapter;
#[path = "emit_and_run/silent_lowerings.rs"]
mod silent_lowerings;
#[path = "emit_and_run/validates_options.rs"]
mod validates_options;
#[path = "emit_and_run/validates_uniqueness.rs"]
mod validates_uniqueness;
#[path = "emit_and_run/validates_inclusion_constants.rs"]
mod validates_inclusion_constants;
#[path = "emit_and_run/uniqueness_default_scoped.rs"]
mod uniqueness_default_scoped;
#[path = "emit_and_run/uniqueness_association_attribute.rs"]
mod uniqueness_association_attribute;
#[path = "emit_and_run/default_scope_static.rs"]
mod default_scope_static;
#[path = "emit_and_run/default_scope_dynamic.rs"]
mod default_scope_dynamic;
#[path = "emit_and_run/acts_as_tenant.rs"]
mod acts_as_tenant;
#[path = "emit_and_run/validates_integer_constants.rs"]
mod validates_integer_constants;
#[path = "emit_and_run/validation_callback_on.rs"]
mod validation_callback_on;
#[path = "emit_and_run/validates_on_context.rs"]
mod validates_on_context;
#[path = "emit_and_run/relation_from_aggregates.rs"]
mod relation_from_aggregates;
#[path = "emit_and_run/validates_exclusion.rs"]
mod validates_exclusion;
#[path = "emit_and_run/validates_lambda_conditions.rs"]
mod validates_lambda_conditions;
#[path = "emit_and_run/validates_format_flags.rs"]
mod validates_format_flags;
#[path = "emit_and_run/concern_included_constants.rs"]
mod concern_included_constants;
#[path = "emit_and_run/validates_rule_on.rs"]
mod validates_rule_on;
#[path = "emit_and_run/validates_presence_allow_nil.rs"]
mod validates_presence_allow_nil;
#[path = "emit_and_run/validates_custom_message.rs"]
mod validates_custom_message;
#[path = "emit_and_run/presence_rails_blank.rs"]
mod presence_rails_blank;
#[path = "emit_and_run/allow_blank_unicode_space.rs"]
mod allow_blank_unicode_space;
#[path = "emit_and_run/regex_string_interpolation.rs"]
mod regex_string_interpolation;
#[path = "emit_and_run/format_allow_nil_blank.rs"]
mod format_allow_nil_blank;
#[path = "emit_and_run/format_value_to_s.rs"]
mod format_value_to_s;
#[path = "emit_and_run/validates_acceptance.rs"]
mod validates_acceptance;
#[path = "emit_and_run/attribute_api_boolean_cast.rs"]
mod attribute_api_boolean_cast;
#[path = "emit_and_run/hash_fetch_one_arg.rs"]
mod hash_fetch_one_arg;
#[path = "emit_and_run/numericality_unknown_option.rs"]
mod numericality_unknown_option;
#[path = "emit_and_run/belongs_to_nil_writer.rs"]
mod belongs_to_nil_writer;
#[path = "emit_and_run/inclusion_duration_literals.rs"]
mod inclusion_duration_literals;
#[path = "emit_and_run/validates_constant_splat.rs"]
mod validates_constant_splat;
#[path = "emit_and_run/cross_class_constants.rs"]
mod cross_class_constants;
#[path = "emit_and_run/regex_literal_hash_guard.rs"]
mod regex_literal_hash_guard;
#[path = "emit_and_run/numericality_allow_nil_blank.rs"]
mod numericality_allow_nil_blank;
#[path = "emit_and_run/stdlib_email_regexp.rs"]
mod stdlib_email_regexp;
#[path = "emit_and_run/constant_arithmetic_bounds.rs"]
mod constant_arithmetic_bounds;
#[path = "emit_and_run/validates_symbol_or_conditions.rs"]
mod validates_symbol_or_conditions;
#[path = "emit_and_run/stdlib_uri_make_regexp.rs"]
mod stdlib_uri_make_regexp;
#[path = "emit_and_run/validates_inclusion_hash_constants.rs"]
mod validates_inclusion_hash_constants;

#[test]
fn markdown_templates_render_with_their_mime_type_and_erb_escaping() {
    const TEMPLATE: &str = "---\ntitle: <%= @value %>\n---\n# Report\n<%= @value %>\n<%= @value.html_safe %>\n<em>static</em>\n";
    emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema.define(version: 1) do\nend\n")
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\n  layout false\nend\n",
        )
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"/markdown\", to: \"reports#show\", format: \"md\"\n  get \"/html\", to: \"reports#show\"\n  get \"/explicit\", to: \"reports#explicit\"\nend\n",
        )
        .write(
            "app/controllers/reports_controller.rb",
            r#"class ReportsController < ApplicationController
  def show
    @value = params[:value].to_s
  end

  def explicit
    @value = params[:value].to_s
    render formats: :md
  end
end
"#,
        )
        .write("app/views/reports/show.html.erb", "HTML sibling\n")
        .write("app/views/reports/show.md.erb", TEMPLATE)
        .write("app/views/reports/explicit.md.erb", TEMPLATE)
        .run_ruby(
            r##"require "uri"
value = %q(<strong title="x">'A' & B</strong>)
escaped = "&lt;strong title=&quot;x&quot;&gt;&#39;A&#39; &amp; B&lt;/strong&gt;"
expected = "---\ntitle: #{escaped}\n---\n# Report\n#{escaped}\n#{value}\n<em>static</em>\n"

["/markdown", "/explicit"].each do |path|
  status, headers, body = Main.run_rack(
    "REQUEST_METHOD" => "GET",
    "PATH_INFO" => path,
    "QUERY_STRING" => URI.encode_www_form(value: value),
    "HTTP_HOST" => "localhost",
    "rack.input" => StringIO.new("")
  )
  raise "#{path} status: #{status}" unless status == 200
  raise "#{path} content type: #{headers.inspect}" unless headers["content-type"].split(";").first == "text/markdown"
  raise "#{path} body: #{body.inspect}" unless body == [expected]
end

status, headers, body = Main.run_rack(
  "REQUEST_METHOD" => "GET",
  "PATH_INFO" => "/html",
  "QUERY_STRING" => "",
  "HTTP_HOST" => "localhost",
  "rack.input" => StringIO.new("")
)
raise "HTML status: #{status}" unless status == 200
raise "HTML content type: #{headers.inspect}" unless headers["content-type"].split(";").first == "text/html"
raise "HTML sibling: #{body.inspect}" unless body == ["HTML sibling\n"]
puts "Markdown template rendering passed"
"##,
        )
        .assert_passes();
}

/// A generated text column on the real-blog Article model exercises the
/// schema-to-runtime path together with Rails-style symbol callbacks. The
/// create callbacks must see the value returned by the database, while an
/// update remains stale until the model is explicitly reloaded.
#[test]
fn generated_article_text_runs_through_callbacks_and_reload() {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "    t.string \"title\"\n    t.text \"body\"\n    t.datetime \"created_at\", null: false\n",
            "    t.string \"title\"\n    t.text \"body\"\n    t.virtual \"display_text\", type: :string, as: \"coalesce(title, '') || ' / ' || coalesce(body, '')\", stored: true\n    t.datetime \"created_at\", null: false\n",
        )
        .edit(
            "app/models/article.rb",
            "  validates :body, presence: true, length: { minimum: 10 }\nend\n",
            concat!(
                "  validates :body, presence: true, length: { minimum: 10 }\n\n",
                "  after_create :capture_generated_create\n",
                "  after_update :capture_generated_update\n",
                "  after_save :capture_generated_save\n\n",
                "  def capture_generated_create\n    @generated_after_create = display_text\n  end\n\n",
                "  def capture_generated_update\n    @generated_after_update = display_text\n  end\n\n",
                "  def capture_generated_save\n    @generated_after_save = display_text\n  end\n\n",
                "  def generated_after_create\n    @generated_after_create\n  end\n\n",
                "  def generated_after_update\n    @generated_after_update\n  end\n\n",
                "  def generated_after_save\n    @generated_after_save\n  end\n",
                "end\n",
            ),
        )
        .write(
            "test/models/generated_column_article_test.rb",
            r#"require "test_helper"

class GeneratedColumnArticleTest < ActiveSupport::TestCase
  test "generated text hydrates before create callbacks and reload refreshes updates" do
    article = Article.create!(title: "Generated title", body: "A sufficiently long article body.")
    original = "Generated title / A sufficiently long article body."

    assert_equal original, article.display_text
    assert_equal original, article.generated_after_create
    assert_equal original, article.generated_after_save

    article.update!(title: "Updated title")
    assert_equal original, article.display_text
    assert_equal original, article.generated_after_update
    assert_equal original, article.generated_after_save
    assert_equal "Updated title / A sufficiently long article body.", article.reload.display_text
  end
end
"#,
        )
        .run_test("test/models/generated_column_article_test.rb")
        .assert_passes();
}

/// `stub_const` is lowered to a value stack beside the constant, so it
/// means the same on a target that resolves constants at build time:
/// reads see the stub inside the block, nested stubs unwind in order,
/// and a raising block still restores. The CRuby harness also has a
/// `const_set` helper; the emitted text shows this ran through the
/// lowering instead.
#[test]
fn stub_const_replaces_a_constant_for_its_block_and_restores_it() {
    let run = emit_and_run::real_blog()
        .write(
            "app/models/page_limits.rb",
            "class PageLimits\n  SIZE = 40\n\n  def self.size\n    SIZE\n  end\nend\n",
        )
        .edit(
            "app/models/article.rb",
            "  validates :body, presence: true, length: { minimum: 10 }\n",
            concat!(
                "  validates :body, presence: true, length: { minimum: 10 }\n\n",
                "  EXCERPT = 12\n\n",
                "  def excerpt\n    body[0, EXCERPT]\n  end\n",
            ),
        )
        .write(
            "test/models/stub_const_test.rb",
            r#"require "test_helper"

class StubConstTest < ActiveSupport::TestCase
  test "a stub holds for its block, nests, and is put back" do
    assert_equal 40, PageLimits.size
    stub_const(PageLimits, :SIZE, 2) do
      assert_equal 2, PageLimits.size
      stub_const(PageLimits, :SIZE, 3) do
        assert_equal 3, PageLimits.size
      end
      assert_equal 2, PageLimits.size
    end
    assert_equal 40, PageLimits.size
  end

  test "a raising block still restores the constant" do
    assert_raises(RuntimeError) do
      stub_const(PageLimits, :SIZE, 5) { raise "boom" }
    end
    assert_equal 40, PageLimits.size
  end

  test "a model's own constant is stubbed where the model reads it" do
    article = Article.new(title: "Stubbed", body: "A sufficiently long article body.")
    assert_equal "A sufficient", article.excerpt
    stub_const(Article, :EXCERPT, 3) do
      assert_equal "A s", article.excerpt
    end
    assert_equal "A sufficient", article.excerpt
  end
end
"#,
        )
        .run_test("test/models/stub_const_test.rb");
    run.assert_passes();
    let test = std::fs::read_to_string(run.emitted.join("test/models/stub_const_test.rb"))
        .expect("emitted stub_const test");
    assert!(!test.contains("stub_const("), "{test}");
    assert!(test.contains("PageLimits::SIZE_STUB_STACK.push(2)"), "{test}");
    let limits = std::fs::read_to_string(run.emitted.join("app/models/page_limits.rb"))
        .expect("emitted PageLimits");
    assert!(limits.contains("SIZE_STUB_STACK = [PageLimits::SIZE]"), "{limits}");
    let article = std::fs::read_to_string(run.emitted.join("app/models/article.rb"))
        .expect("emitted Article");
    assert!(article.contains("Article::EXCERPT_STUB_STACK.last"), "{article}");
}

/// The same anonymous keyword packet survives defaulting, local-name
/// collisions, and a virtual override in emitted CRuby. Effectful input
/// values also stay left-to-right and run once.
#[test]
fn anonymous_keyword_forwarding_runs_without_capturing_or_reordering_values() {
    let run = emit_and_run::real_blog()
        .write(
            "app/services/keyword_forwarder.rb",
            anonymous_keywords::SOURCE,
        )
        .run_ruby(anonymous_keywords::ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("anonymous keyword forwarding contract passed"));
    let emitted = std::fs::read_to_string(run.emitted.join("app/models/keyword_forwarder.rb"))
        .expect("emitted keyword forwarding class");
    assert!(emitted.contains("class KeywordForwarder"), "{emitted}");
    assert!(emitted.contains("request(kind: :get, path: path, **)"), "{emitted}");
}

#[test]
fn model_concern_delegate_through_belongs_to_runs() {
    let run = delegate_association::overlay().run_ruby(delegate_association::ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("model concern delegate passed"));
}

#[test]
fn active_support_many_block_and_squish_bang_run_in_emitted_ruby() {
    let run = emit_and_run::real_blog()
        .write(
            "app/models/active_support_probe.rb",
            r#"class ActiveSupportProbe
  def self.many_matches
    seen = []
    zero = [1, 2, 3].many? { |n| n > 9 }
    one = [1, 2, 3].many? { |n| n == 2 }
    multiple = [1, 2, 3, 4].many? { |n| seen << n; n > 1 }
    [zero, one, multiple, seen.length]
  end

  def self.squish_bang
    text = "\u00a0foo\u2003bar\u2028".dup
    text.squish!
  end
end
"#,
        )
        .run_ruby(
            r#"raise "many? block zero/one/multiple or short circuit failed" unless ActiveSupportProbe.many_matches == [false, false, true, 3]
raise "squish! did not match Unicode whitespace semantics" unless ActiveSupportProbe.squish_bang == "foo bar"
puts "ActiveSupport core extensions emitted Ruby passed"
"#,
        );
    run.assert_passes();
    assert!(run.stdout.contains("ActiveSupport core extensions emitted Ruby passed"));
}

#[test]
fn later_delegate_replaces_an_earlier_handwritten_method() {
    let run = delegate_association::overlay()
        .write(
            "app/models/comment.rb",
            r#"class Comment < ApplicationRecord
  belongs_to :article

  def article_body
    "handwritten"
  end

  delegate :body, to: :article, prefix: true
end
"#,
        )
        .run_ruby(
            r#"article = Article.create!(title: "Association title", body: "article body")
comment = Comment.create!(article: article, commenter: "Reader", body: "Comment body")
raise "later delegate did not replace the earlier method" unless comment.article_body == "article body"
puts "later delegate ordering passed"
"#,
        );
    run.assert_passes();
    assert!(run.stdout.contains("later delegate ordering passed"));
}

/// `to_sql` renders a relation as SQL another query can embed: the
/// subquery runs, and selects exactly the commented article.
#[test]
fn a_relations_to_sql_runs_as_a_subquery() {
    let run = emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "  validates :title, presence: true\n",
            "  validates :title, presence: true

  def self.commented
    where(\"articles.id IN (#{Comment.select(:article_id).to_sql})\")
  end
",
        )
        .run_ruby(
            r#"commented = Article.create!(title: "Commented", body: "A sufficiently long body.")
Article.create!(title: "Quiet", body: "A sufficiently long body.")
Comment.create!(article: commented, commenter: "Reader", body: "Comment body")
sql = Article.where(title: "Quiet").to_sql
raise "to_sql: #{sql}" unless sql.start_with?("SELECT") && sql.include?("articles")
ids = Article.commented.map(&:id)
raise "subquery: #{ids.inspect}" unless ids == [commented.id]
puts "to_sql subquery passed"
"#,
        );
    run.assert_passes();
    assert!(run.stdout.contains("to_sql subquery passed"));
}

/// `ActiveRecord::Base.connection` is one object, whichever class asks
/// and however often, as Rails' leased connection is: campfire's record
/// cache test stubs `transaction_open?` on it and reads the stub back
/// through `connection_pool.with_connection`.
#[test]
fn the_connection_is_one_object_for_every_caller() {
    let run = emit_and_run::real_blog().run_ruby(
        r#"conn = ActiveRecord::Base.connection
raise "fresh per call" unless conn.equal?(ActiveRecord::Base.connection)
raise "per model" unless conn.equal?(Article.connection)
pooled = ActiveRecord::Base.connection_pool.with_connection { |c| c }
raise "pool yields another" unless conn.equal?(pooled)
puts "one connection passed"
"#,
    );
    run.assert_passes();
    assert!(run.stdout.contains("one connection passed"));
}

/// A has_many reader followed by `to_sql` is rooted as the association's
/// relation: the reader alone answers an Array, which has no `to_sql`,
/// and `check` was clean while the emitted method raised NoMethodError.
#[test]
fn a_has_many_readers_to_sql_is_the_scoped_query() {
    let run = emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "  validates :title, presence: true\n",
            "  validates :title, presence: true

  def comment_sql
    comments.to_sql
  end
",
        )
        .run_ruby(
            r#"a = Article.create!(title: "One", body: "A sufficiently long body.")
sql = a.comment_sql
raise "comment_sql: #{sql}" unless sql.include?("comments") && sql.include?(%("article_id" = #{a.id}))
puts "has_many to_sql passed"
"#,
        );
    run.assert_passes();
    assert!(run.stdout.contains("has_many to_sql passed"));
}

/// `maximum` / `minimum` on a model class and on a has_many reader (of a
/// possibly-nil owner too) run as SQL extrema of the column's type, nil
/// over no rows: the reader alone answers an Array, which has neither.
fn extrema_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog().edit(
        "app/models/article.rb",
        "  validates :title, presence: true\n",
        "  validates :title, presence: true

  def self.newest_id
    Article.maximum(:id)
  end

  def last_commenter
    comments.maximum(:commenter)
  end

  def first_comment_id
    self.comments.minimum(:id)
  end

  def self.first_articles_last_comment_id
    Article.first.comments.maximum(:id)
  end
",
    )
}

const EXTREMA_ASSERTIONS: &str = r#"Article.delete_all
raise "empty newest_id: #{Article.newest_id.inspect}" unless Article.newest_id.nil?
a = Article.create!(title: "One", body: "A sufficiently long body.")
b = Article.create!(title: "Two", body: "A sufficiently long body.")
raise "newest_id: #{Article.newest_id.inspect}" unless Article.newest_id == b.id
raise "empty last_commenter: #{a.last_commenter.inspect}" unless a.last_commenter.nil?
c1 = Comment.create!(article_id: a.id, commenter: "Ann", body: "first comment")
Comment.create!(article_id: a.id, commenter: "Zed", body: "second comment")
Comment.create!(article_id: b.id, commenter: "Zoe", body: "other article")
raise "last_commenter: #{a.last_commenter.inspect}" unless a.last_commenter == "Zed"
raise "first_comment_id: #{a.first_comment_id.inspect}" unless a.first_comment_id == c1.id
raise "first_articles_last_comment_id: #{Article.first_articles_last_comment_id.inspect}" unless Article.first_articles_last_comment_id == Comment.where(article_id: a.id).last.id
puts "extrema passed"
"#;

#[test]
fn extrema_run_on_a_model_and_its_has_many_reader() {
    let run = extrema_app().run_ruby(EXTREMA_ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("extrema passed"));
}

/// A guard on a nullable column read narrows the later reads of that column
/// on the same record: `a.score && a.score >= 0.5` compares a Float, and the
/// emitted program keeps exactly the rows whose score is set and high enough.
fn guarded_column_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit("db/schema.rb", "t.text \"body\"\n    t.datetime \"created_at\", null: false\n    t.datetime \"updated_at\", null: false\n  end\n\n  create_table \"comments\"", "t.text \"body\"\n    t.float \"score\"\n    t.datetime \"created_at\", null: false\n    t.datetime \"updated_at\", null: false\n  end\n\n  create_table \"comments\"")
        .edit(
            "app/models/article.rb",
            "  validates :title, presence: true\n",
            "  validates :title, presence: true

  def self.confident
    Article.all.select { |a| a.score && a.score >= 0.5 }.map { |a| a.title }
  end

  def self.unconfident
    Article.all.select { |a| a.score.nil? || a.score < 0.5 }.map { |a| a.title }
  end

  def self.branch_score(a)
    if a.score
      a.score + 1.0
    else
      -1.0
    end
  end

  def self.ternary_score(a)
    a.score ? a.score * 2 : 0.0
  end
",
        )
}

#[test]
fn a_guarded_nullable_column_read_runs() {
    let script = r#"Article.delete_all
Article.create!(title: "high", body: "A sufficiently long body.", score: 0.9)
Article.create!(title: "edge", body: "A sufficiently long body.", score: 0.5)
Article.create!(title: "low", body: "A sufficiently long body.", score: 0.1)
Article.create!(title: "none", body: "A sufficiently long body.")
got = Article.confident.sort
raise "confident: #{got.inspect}" unless got == ["edge", "high"]
got = Article.unconfident.sort
raise "unconfident: #{got.inspect}" unless got == ["low", "none"]
raise "branch nil" unless Article.branch_score(Article.find_by(title: "none")) == -1.0
raise "branch set" unless Article.branch_score(Article.find_by(title: "high")) == 1.9
raise "ternary nil" unless Article.ternary_score(Article.find_by(title: "none")) == 0.0
raise "ternary set" unless Article.ternary_score(Article.find_by(title: "low")) == 0.2
puts "guarded column passed"
"#;
    let run = guarded_column_app().run_ruby(script);
    run.assert_passes();
    assert!(run.stdout.contains("guarded column passed"));
}

/// Three sites that diagnosed as errors while their emitted Ruby ran, each
/// on code every lobsters request reaches (and so, under `--target
/// futamura`'s whole-route replacement, each kept every route on Rails):
/// a `try` on a nullable receiver, `|| default` over `pluck(:nullable)
/// .first` (a nested nullable), and the TTL the `Rails.cache.fetch`
/// lowering builds. Clean analysis is the claim; this is the run behind it.
#[test]
fn nullable_try_or_default_and_cache_ttl_run() {
    let script = r#"Article.delete_all
Article.create!(title: "scored", body: "A sufficiently long body.", score: 0.9)
Article.create!(title: "unscored", body: "A sufficiently long body.")
scored = Article.find_by(title: "scored")
raise "try found: #{Article.title_of(scored.id).inspect}" unless Article.title_of(scored.id) == "scored"
raise "try missing: #{Article.title_of(-1).inspect}" unless Article.title_of(-1).nil?
raise "try str: #{Article.title_str(scored.id).inspect}" unless Article.title_str(scored.id) == "scored"
raise "score set" unless Article.weighted(scored.id) == 1.8
raise "score nil" unless Article.weighted(Article.find_by(title: "unscored").id) == 1.0
raise "no row" unless Article.weighted(-1) == 1.0
raise "cache miss" unless Article.cached_label("first") == "first"
raise "cache hit" unless Article.cached_label("second") == "first"
puts "nullable sites passed"
"#;
    let run = guarded_column_app()
        .edit(
            "app/models/article.rb",
            "  def self.confident\n",
            "  def self.title_of(id)
    a = Article.find_by(id: id)
    a.try(:title)
  end

  def self.title_str(id)
    Article.find_by(id: id).try(\"title\")
  end

  def self.weighted(id)
    s = Article.where(id: id).limit(1).pluck(:score).first || 0.5
    (s * 2).round(1)
  end

  def self.cached_label(value)
    Rails.cache.fetch(\"label\", :expires_in => (60 * 2)) { value.to_s }
  end

  def self.confident
",
        )
        .run_ruby(script);
    run.assert_passes();
    assert!(run.stdout.contains("nullable sites passed"));
}

/// With a date column the Date package loads and an aggregate over it
/// answers a Date, on Spinel as on CRuby.
#[test]
#[ignore = "requires the Spinel toolchain"]
fn date_extrema_answer_a_date_on_spinel() {
    let overlay = emit_and_run::real_blog()
        .edit("db/schema.rb", "t.string \"commenter\"", "t.string \"commenter\"\n    t.date \"archived_on\"")
        .edit(
            "app/models/comment.rb",
            "  belongs_to :article\n",
            "  belongs_to :article\n\n  def self.first_archived_on\n    Comment.minimum(:archived_on)\n  end\n",
        );
    let script = r#"Db.configure(":memory:")
Schema.statements.each { |sql| Db.exec(sql) }
ActiveRecord.adapter = SqliteAdapter
raise "empty: #{Comment.first_archived_on.inspect}" unless Comment.first_archived_on.nil?
a = Article.create!(title: "One", body: "A sufficiently long body.")
Comment.create!(article_id: a.id, commenter: "Ann", body: "later", archived_on: Date.iso8601("2024-03-04"))
Comment.create!(article_id: a.id, commenter: "Bob", body: "earlier", archived_on: Date.iso8601("2024-03-02"))
raise "first_archived_on: #{Comment.first_archived_on.iso8601}" unless Comment.first_archived_on.iso8601 == "2024-03-02"
puts "date extrema passed"
"#;
    overlay.run_spinel(script).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn extrema_run_on_spinel_without_date_columns() {
    let script = format!(
        "Db.configure(\":memory:\")\nSchema.statements.each {{ |sql| Db.exec(sql) }}\nActiveRecord.adapter = SqliteAdapter\n{EXTREMA_ASSERTIONS}"
    );
    extrema_app().run_spinel(&script).assert_passes();
}

/// The has_many readers `lower::scope_chain` does not seed — one off a
/// call with arguments, or off a local holding the reader — stay
/// unresolved: their Array has no `maximum`.
#[test]
fn extrema_on_an_unseeded_reader_stay_errors() {
    let (_emitted, errors) = emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "  validates :title, presence: true\n",
            "  validates :title, presence: true

  def self.by_title_last_comment_id(title)
    Article.find_by!(title: title).comments.maximum(:id)
  end

  def held_last_comment_id
    held = comments
    held.maximum(:id)
  end
",
        )
        .emit(roundhouse::project::BuildTarget::Ruby);
    assert_eq!(
        errors.iter().filter(|e| e.contains("`maximum`")).count(),
        2,
        "both unseeded readers keep their error: {errors:?}"
    );
}

/// `in_batches` with a block hands each batch as a relation; without
/// one, `update_all` and `touch_all` reach every row.
#[test]
fn in_batches_runs_with_and_without_a_block() {
    let run = emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "  validates :title, presence: true\n",
            "  validates :title, presence: true

  scope :batched, -> { in_batches }

  def self.batch_total
    total = 0
    in_batches(of: 1) { |batch| total += batch.count }
    total
  end

  def self.rename_all(title)
    in_batches.update_all(title: title)
  end

  def self.touch_everything
    in_batches(order: :desc).touch_all
  end

  def touch_comments
    comments.in_batches.touch_all
  end
",
        )
        .run_ruby(
            r#"a = Article.create!(title: "One", body: "A sufficiently long body.")
Article.create!(title: "Two", body: "A sufficiently long body.")
Comment.create!(article: a, commenter: "Reader", body: "Comment body")
raise "batch_total: #{Article.batch_total}" unless Article.batch_total == 2
Article.rename_all("Renamed")
raise "rename_all" unless Article.all.map(&:title).uniq == ["Renamed"]
raise "touch_everything" unless Article.touch_everything == 2
raise "touch_comments" unless a.touch_comments == 1
raise "batched scope" unless Article.batched.where(title: "Renamed").count == 2
puts "in_batches passed"
"#,
        );
    run.assert_passes();
    assert!(run.stdout.contains("in_batches passed"));
}

/// `Rails.public_path` runs as `Rails.root` does: the runtime's AppPath
/// answers `join` and `to_s`.
#[test]
fn rails_public_path_joins() {
    let run = emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "  validates :title, presence: true\n",
            "  validates :title, presence: true

  def self.logo_path
    Rails.public_path.join(\"images\", \"logo.png\").to_s
  end
",
        )
        .run_ruby(
            r#"raise "logo_path: #{Article.logo_path}" unless Article.logo_path == "public/images/logo.png"
puts "public_path passed"
"#,
        );
    run.assert_passes();
    assert!(run.stdout.contains("public_path passed"));
}

/// A nested `transaction(requires_new: true)` is a savepoint: a Rollback
/// or a rescued exception from it undoes only its own writes, and the
/// outer transaction commits the rest; without an error it answers the
/// block's value.
#[test]
fn requires_new_transactions_roll_back_to_their_savepoint() {
    let run = emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "  validates :title, presence: true\n",
            "  validates :title, presence: true

  def self.add(title)
    Article.create!(title: title, body: \"A sufficiently long body.\")
  end

  def self.savepoints
    Article.transaction do
      add(\"outer\")
      Article.transaction(requires_new: true) do
        add(\"rolled-back\")
        raise ActiveRecord::Rollback
      end
      begin
        Article.transaction(requires_new: true) do
          add(\"raised\")
          raise ArgumentError, \"inner\"
        end
      rescue ArgumentError
        nil
      end
      Article.transaction(requires_new: true) do
        add(\"kept\")
        \"kept\"
      end
    end
  end
",
        )
        .run_ruby(
            r#"Article.delete_all
raise "savepoints: #{Article.savepoints.inspect}" unless Article.savepoints == "kept"
titles = Article.all.map(&:title).sort
raise "titles: #{titles.inspect}" unless titles == ["kept", "outer"]
puts "savepoints passed"
"#,
        );
    run.assert_passes();
    assert!(run.stdout.contains("savepoints passed"));
}

/// A save runs in a transaction, and its commit callbacks wait for the
/// COMMIT, as Rails' do:
/// - an exception from an `after_save` rolls the row back and propagates;
/// - an `after_create_commit` runs after the outermost COMMIT, outside
///   the transaction, once per record however often it was saved;
/// - a ROLLBACK drops the held callbacks and makes the record new again;
/// - `touch` is an update to the commit callbacks;
/// - `article.comments.new` hands the article itself to the comment's
///   `belongs_to` (the automatic inverse), so a singleton method on the
///   owner is what the comment reaches.
/// (campfire's `Message::CreationTest` and `MessageCreationCommitTest`.)
#[test]
fn saves_are_transactional_and_commit_callbacks_wait_for_the_commit() {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "  validates :title, presence: true\n",
            "  validates :title, presence: true

  LOG = []
  after_save { raise \"after_save failed\" if title == \"explode\" }
  after_create_commit { LOG << \"create #{title} open=#{Article.connection.transaction_open?}\" }
  after_update_commit { LOG << \"update #{title}\" }
",
        )
        .write(
            "test/models/transactional_save_test.rb",
            r##"require "test_helper"

class TransactionalSaveTest < ActiveSupport::TestCase
  # This example checks a real COMMIT, the way campfire's
  # MessageCreationCommitTest does. The harness wraps other examples
  # in a non-joinable fixture transaction.
  self.use_transactional_tests = false

  setup { Article::LOG.clear }

  test "an exception from after_save rolls the insert back" do
    count = Article.count
    article = Article.new(title: "explode", body: "A sufficiently long article body.")
    assert_raises(RuntimeError) { article.save! }
    assert_equal count, Article.count
    assert_equal [], Article::LOG
  end

  test "commit callbacks run once, after the outermost commit" do
    article = nil
    Article.transaction do
      article = Article.create!(title: "Held", body: "A sufficiently long article body.")
      article.update!(body: "Another sufficiently long body.")
      assert_equal [], Article::LOG
    end
    assert_equal [ "create Held open=false" ], Article::LOG
  end

  test "a rollback drops the held callbacks and the record is new again" do
    count = Article.count
    article = Article.new(title: "Dropped", body: "A sufficiently long article body.")
    Article.transaction do
      article.save!
      raise ActiveRecord::Rollback
    end
    assert_equal count, Article.count
    assert_equal [], Article::LOG
    assert article.new_record?
  end

  test "touch runs the update commit callbacks" do
    article = Article.create!(title: "Touched", body: "A sufficiently long article body.")
    Article::LOG.clear
    article.touch
    assert_equal [ "update Touched" ], Article::LOG
  end

  test "an association constructor hands the owner to the inverse" do
    article = Article.create!(title: "Owner", body: "A sufficiently long article body.")
    comment = article.comments.new(commenter: "Ann", body: "Hello there")
    assert_same article, comment.article
    assert_equal article.id, comment.article_id
  end
end
"##,
        )
        .run_test("test/models/transactional_save_test.rb")
        .assert_passes();
}

/// Campfire's room destroy counts one `RELEASE SAVEPOINT` per record
/// (`messages.find_each(&:destroy)` plus the room) because Rails runs
/// the example inside a non-joinable fixture transaction, so each
/// `destroy` opens its own savepoint. With
/// `use_transactional_tests = false` those are real `COMMIT`s, which is
/// what lets another connection write between them in production.
#[test]
fn each_destroy_is_its_own_transaction() {
    emit_and_run::real_blog()
        .write(
            "test/models/destroy_batch_test.rb",
            r##"require "test_helper"

class DestroyBatchTest < ActiveSupport::TestCase
  test "each destroy releases its own savepoint inside the test transaction" do
    article = Article.create!(title: "Batch", body: "A sufficiently long article body.")
    2.times { |i| article.comments.create!(commenter: "Ann", body: "Note #{i}") }
    n = article.comments.count
    releases = 0
    count = ->(*, payload) { releases += 1 if payload[:sql].start_with?("RELEASE SAVEPOINT") }
    ActiveSupport::Notifications.subscribed(count, "sql.active_record") do
      article.comments.find_each(&:destroy)
      article.destroy
    end
    assert_equal n + 1, releases
    assert_equal 0, Comment.where(article_id: article.id).count
    assert_not Article.exists?(article.id)
  end

  test "the fixture transaction does not look open to caches" do
    assert_not Article.connection.transaction_open?
    Article.transaction do
      assert Article.connection.transaction_open?
    end
    assert_not Article.connection.transaction_open?
  end
end
"##,
        )
        .run_test("test/models/destroy_batch_test.rb")
        .assert_passes();

    emit_and_run::real_blog()
        .write(
            "test/models/destroy_batch_commit_test.rb",
            r##"require "test_helper"

class DestroyBatchCommitTest < ActiveSupport::TestCase
  self.use_transactional_tests = false

  test "each destroy commits on its own when the example is not wrapped" do
    article = Article.create!(title: "Batch", body: "A sufficiently long article body.")
    2.times { |i| article.comments.create!(commenter: "Ann", body: "Note #{i}") }
    n = article.comments.count
    commits = 0
    releases = 0
    count = ->(*, payload) do
      sql = payload[:sql]
      commits += 1 if sql.start_with?("COMMIT")
      releases += 1 if sql.start_with?("RELEASE SAVEPOINT")
    end
    ActiveSupport::Notifications.subscribed(count, "sql.active_record") do
      article.comments.find_each(&:destroy)
      article.destroy
    end
    assert_equal n + 1, commits
    assert_equal 0, releases
    assert_equal 0, Comment.where(article_id: article.id).count
    assert_not Article.exists?(article.id)
  end

  test "a real transaction looks open, and the example itself does not" do
    assert_not Article.connection.transaction_open?
    Article.transaction do
      assert Article.connection.transaction_open?
    end
    assert_not Article.connection.transaction_open?
  end
end
"##,
        )
        .run_test("test/models/destroy_batch_commit_test.rb")
        .assert_passes();
}

/// `belongs_to …, touch: true` is Rails' `touch_later`: inside a
/// transaction the parent's row is stamped just before the COMMIT, not
/// in the middle of the save; outside one a save still stamps it, and a
/// rollback stamps nothing. A required `belongs_to` is
/// checked through its reader, so a missing parent still fails the save.
/// (campfire's message save, whose rich text and message both touch the
/// room inside one transaction.)
#[test]
fn a_belongs_to_touch_waits_for_the_commit_and_lands_once() {
    emit_and_run::real_blog()
        .edit("app/models/comment.rb", "  belongs_to :article\n", "  belongs_to :article, touch: true\n")
        .write(
            "test/models/deferred_touch_test.rb",
            r##"require "test_helper"

class DeferredTouchTest < ActiveSupport::TestCase
  def stored(article)
    Article.find(article.id).updated_at.to_i
  end

  def parent(title)
    travel_to Time.utc(2026, 1, 1, 12, 0, 0)
    article = Article.create!(title: title, body: "A sufficiently long article body.")
    travel_to Time.utc(2026, 1, 2, 12, 0, 0)
    article
  end

  teardown { travel_back }

  test "children saved in one transaction touch the parent at the commit" do
    article = parent("Parent")
    before = stored(article)
    Article.transaction do
      article.comments.create!(commenter: "Ann", body: "First comment")
      article.comments.create!(commenter: "Bob", body: "Second comment")
      assert_equal before, stored(article)
    end
    assert_equal Time.utc(2026, 1, 2, 12, 0, 0).to_i, stored(article)
  end

  test "a save outside a transaction still touches the parent" do
    article = parent("Single")
    article.comments.create!(commenter: "Ann", body: "Only comment")
    assert_equal Time.utc(2026, 1, 2, 12, 0, 0).to_i, stored(article)
  end

  test "a rolled back transaction touches nothing" do
    article = parent("Undone")
    before = stored(article)
    Article.transaction do
      article.comments.create!(commenter: "Ann", body: "Dropped comment")
      raise ActiveRecord::Rollback
    end
    assert_equal before, stored(article)
  end

  test "a comment on an article that does not exist is invalid" do
    comment = Comment.new(commenter: "Ann", body: "Nobody home", article_id: 999_999)
    assert_not comment.save
    assert_includes comment.errors.full_messages, "Article must exist"
  end
end
"##,
        )
        .run_test("test/models/deferred_touch_test.rb")
        .assert_passes();
}

/// `Model.uncached { ... }` (campfire's password-digest and push-endpoint
/// reads past the query cache): the block runs with the query cache off,
/// the cache comes back after it, and the call answers the block's value.
#[test]
fn a_model_uncached_block_reads_past_the_query_cache() {
    emit_and_run::real_blog()
        .write(
            "test/models/uncached_test.rb",
            r##"require "test_helper"

class UncachedTest < ActiveSupport::TestCase
  test "the block reads past the query cache and answers its value" do
    article = Article.create!(title: "Cached", body: "A sufficiently long article body.")
    Db.query_cache_begin
    begin
      inside = Article.uncached { Db.query_cache_enabled? }
      assert_equal false, inside
      assert Db.query_cache_enabled?
      assert_equal "Cached", Article.uncached { Article.where(id: article.id).pick(:title) }
    ensure
      Db.query_cache_end
    end
  end
end
"##,
        )
        .run_test("test/models/uncached_test.rb")
        .assert_passes();
}

/// `current_transaction.after_commit { ... }` (Rails 7.2; campfire's
/// password change closes the user's connections this way): the block
/// runs once the transaction it was registered in commits, never for one
/// that rolls back, and at once where no transaction can hold it.
#[test]
fn current_transaction_after_commit_waits_for_the_commit() {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "  validates :title, presence: true\n",
            "  validates :title, presence: true

  LOG = []
  after_update :note_on_commit

  def self.note_now
    current_transaction.after_commit { LOG << \"now\" }
  end

  private
    def note_on_commit
      self.class.current_transaction.after_commit { LOG << \"committed #{title}\" }
    end

  public
",
        )
        .write(
            "test/models/after_commit_test.rb",
            r##"require "test_helper"

class AfterCommitTest < ActiveSupport::TestCase
  setup do
    @article = Article.create!(title: "Start", body: "A sufficiently long article body.")
    Article::LOG.clear
  end

  test "a save's own transaction runs the block once it commits" do
    @article.update!(title: "Saved")
    assert_equal [ "committed Saved" ], Article::LOG
  end

  test "an enclosing transaction holds the block until its commit" do
    Article.transaction do
      @article.update!(title: "Held")
      assert_equal [], Article::LOG
    end
    assert_equal [ "committed Held" ], Article::LOG
  end

  test "a rolled back transaction never runs the block" do
    Article.transaction do
      @article.update!(title: "Undone")
      raise ActiveRecord::Rollback
    end
    assert_equal [], Article::LOG
  end

  test "with no transaction to hold it the block runs at once" do
    Article.note_now
    assert_equal [ "now" ], Article::LOG
  end
end
"##,
        )
        .run_test("test/models/after_commit_test.rb")
        .assert_passes();
}

/// A module's `mattr_accessor` (campfire's
/// `InlineScriptHashes.last_script_sources`, once-campfire#370) keeps its
/// value in the module's own instance variable rather than a class
/// variable: the same reads and writes on the Ruby tree, and nothing
/// Spinel refuses (it has no class variables).
#[test]
fn a_modules_mattr_storage_is_its_own_instance_variable() {
    let module = "module MemoStore\n  mattr_accessor :last\nend\n";
    emit_and_run::real_blog()
        .write("app/models/memo_store.rb", module)
        .write(
            "test/models/memo_store_test.rb",
            r##"require "test_helper"

class MemoStoreTest < ActiveSupport::TestCase
  test "the module attribute starts nil and holds what it is given" do
    assert_nil MemoStore.last
    MemoStore.last = [ "key", "value" ]
    assert_equal [ "key", "value" ], MemoStore.last
    MemoStore.last = nil
  end
end
"##,
        )
        .run_test("test/models/memo_store_test.rb")
        .assert_passes();

    let (_tree, errors) = emit_and_run::real_blog()
        .write("app/models/memo_store.rb", module)
        .write("app/models/memo_reader.rb", "class MemoReader\n  def self.read\n    MemoStore.last\n  end\nend\n")
        .emit(roundhouse::project::BuildTarget::Spinel);
    let refused: Vec<&String> = errors
        .iter()
        .filter(|e| e.contains("class variable") || e.contains("defined? query"))
        .collect();
    assert!(refused.is_empty(), "spinel refused the module's storage: {refused:?}");
}

/// `generates_token_for` declared in a concern's `included` block with
/// an expiry the concern names as a constant, and a value built with
/// `String#last` over a safe navigation (campfire's
/// `User::Transferable`: `expires_in: TRANSFER_LINK_EXPIRY_DURATION do
/// password_salt&.last(10) end`). The token round-trips, stops verifying
/// when the value changes, and a finder reached through a scope finds
/// only what the scope holds (`User.active.find_by_transfer_id`). The
/// scoped call is the app's, as campfire's is (a test body's call site is
/// not part of the survey that threads the relation).
#[test]
fn a_concerns_token_for_round_trips_within_its_scope() {
    emit_and_run::real_blog()
        .write(
            "app/models/article/shareable.rb",
            r##"module Article::Shareable
  extend ActiveSupport::Concern

  SHARE_LINK_EXPIRY = 2.hours

  included do
    generates_token_for :share, expires_in: SHARE_LINK_EXPIRY do
      title&.last(5)
    end
  end

  class_methods do
    def find_by_share_id(id)
      find_by_token_for(:share, id)
    end
  end

  def share_id
    generate_token_for(:share)
  end
end
"##,
        )
        .edit(
            "app/models/article.rb",
            "  validates :title, presence: true\n",
            "  validates :title, presence: true\n  include Article::Shareable\n  scope :greetings, -> { where(title: \"Hello world\") }\n\n  def self.greeting_shared_by(token)\n    Article.greetings.find_by_share_id(token)\n  end\n",
        )
        .write(
            "test/models/article_share_test.rb",
            r##"require "test_helper"

class ArticleShareTest < ActiveSupport::TestCase
  setup do
    @hello = Article.create!(title: "Hello world", body: "A sufficiently long article body.")
    @other = Article.create!(title: "Another one", body: "A sufficiently long article body.")
  end

  test "a share id finds its article" do
    assert_equal @hello, Article.find_by_share_id(@hello.share_id)
  end

  test "a share id stops working when the value it is bound to changes" do
    token = @hello.share_id
    @hello.update!(title: "Hello there")
    assert_nil Article.find_by_share_id(token)
  end

  test "a share id finds only articles in scope" do
    assert_equal @hello, Article.greeting_shared_by(@hello.share_id)
    assert_nil Article.greeting_shared_by(@other.share_id)
    assert_equal @other, Article.find_by_share_id(@other.share_id)
  end
end
"##,
        )
        .run_test("test/models/article_share_test.rb")
        .assert_passes();
}

/// An association parked in an ivar by a `before_action` (campfire's
/// `@push_subscriptions = Current.user.push_subscriptions`) answers
/// `find_or_initialize_by(<params>)` as a relation scoped to its owner,
/// with the params object as the condition hash, and the same ivar read
/// again in a private helper the action calls answers `find_by` the same
/// way.
#[test]
fn a_parked_association_finds_or_initializes_and_a_helper_reads_it() {
    emit_and_run::real_blog()
        .write(
            "app/controllers/comments_controller.rb",
            r##"class CommentsController < ApplicationController
  before_action :set_article
  before_action :set_comments

  def create
    @comment = @comments.find_or_initialize_by(comment_params)
    if @comment.persisted? || @comment.save
      redirect_to @article, notice: posted_notice
    else
      redirect_to @article, alert: "Could not create comment."
    end
  end

  def destroy
    @comment = @article.comments.find(params.expect(:id))
    @comment.destroy
    redirect_to @article, notice: "Comment was successfully deleted."
  end

  private

    def set_article
      @article = Article.find(params.expect(:article_id))
    end

    def set_comments
      @comments = @article.comments
    end

    def posted_notice
      @comments.find_by(commenter: "Twice") ? "Posted by Twice." : "Comment was successfully created."
    end

    def comment_params
      params.expect(comment: [ :commenter, :body ])
    end
end
"##,
        )
        .write(
            "test/controllers/comments_find_or_initialize_controller_test.rb",
            r##"require "test_helper"

class CommentsFindOrInitializeControllerTest < ActionDispatch::IntegrationTest
  setup do
    @article = articles(:one)
    @other = Article.create!(title: "Other article", body: "A sufficiently long article body.")
  end

  test "the same comment posted twice is one comment, on its own article" do
    params = { comment: { commenter: "Twice", body: "Said it once." } }
    assert_difference("Comment.count", 1) do
      post article_comments_url(@article), params: params
      post article_comments_url(@article), params: params
    end
    comment = Comment.find_by(commenter: "Twice")
    assert_equal @article.id, comment.article_id
    follow_redirect!
    assert_includes response.body, "Posted by Twice."
  end

  test "another article's comments are another scope" do
    params = { comment: { commenter: "Twice", body: "Said it once." } }
    post article_comments_url(@article), params: params
    assert_difference("Comment.count", 1) do
      post article_comments_url(@other), params: params
    end
    assert_equal [ @article.id, @other.id ].sort, Comment.where(commenter: "Twice").map(&:article_id).sort
  end
end
"##,
        )
        .run_test("test/controllers/comments_find_or_initialize_controller_test.rb")
        .assert_passes();
}

/// A params helper that ends `permit(…).compact.merge(k: v)` (campfire's
/// `Users::ProfilesController#user_params`, which always adds the
/// password challenge) is still the permit list's typed record with the
/// merged key on it, so `update(<that>)` takes the typed update. The
/// `.compact` between them hid the merge, and the update called the
/// record's `title_provided` on the Hash the merge answered instead.
#[test]
fn a_compacted_permit_list_merges_into_its_typed_update() {
    emit_and_run::real_blog()
        .edit(
            "app/controllers/articles_controller.rb",
            "      params.expect(article: [ :title, :body ])\n",
            "      params.require(:article).permit(:title).compact.merge(body: \"Merged by the controller, not the form.\")\n",
        )
        .write(
            "test/controllers/articles_compact_merge_controller_test.rb",
            r##"require "test_helper"

class ArticlesCompactMergeControllerTest < ActionDispatch::IntegrationTest
  test "the permitted field and the merged one both reach the record" do
    article = articles(:one)
    patch article_url(article), params: { article: { title: "Updated Title", body: "From the form" } }
    assert_redirected_to article_url(article)
    article.reload
    assert_equal "Updated Title", article.title
    assert_equal "Merged by the controller, not the form.", article.body
  end
end
"##,
        )
        .run_test("test/controllers/articles_compact_merge_controller_test.rb")
        .assert_passes();
}

/// `rescue URI::Error` catches `URI.parse`'s `URI::InvalidURIError`, and
/// `SocketError` resolves where the app names it without a require (Rails
/// has loaded socket); both lanes define the classes.
fn uri_and_socket_errors_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog().edit(
        "app/models/article.rb",
        "  validates :title, presence: true\n",
        "  validates :title, presence: true

  def self.parsed?(url)
    URI.parse(url)
    true
  rescue URI::Error
    false
  end

  def self.socket_failure
    raise SocketError, \"unreachable\"
  rescue SocketError => e
    e.message
  end
",
    ).edit(
        "app/models/comment.rb",
        "  validates :body, presence: true\n",
        "  validates :body, presence: true

  def self.contained?
    yield
    false
  rescue ArgumentError,SocketError
    true
  end
",
    )
}

const URI_AND_SOCKET_ASSERTIONS: &str = r#"raise "parsed good" unless Article.parsed?("https://example.com/a")
raise "parsed bad" if Article.parsed?("http://bad uri")
raise "socket: #{Article.socket_failure}" unless Article.socket_failure == "unreachable"
raise "contained" unless Comment.contained? { raise ArgumentError, "x" }
raise "uncontained" if Comment.contained? { 1 }
puts "uri and socket errors passed"
"#;

#[test]
fn uri_and_socket_errors_are_rescued() {
    let run = uri_and_socket_errors_app().run_ruby(URI_AND_SOCKET_ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("uri and socket errors passed"));
}

/// The emitted model requires what it rescues, rather than relying on
/// another runtime file to have loaded socket first.
#[test]
fn rescued_socket_and_uri_errors_bring_their_requires() {
    let (emitted, errors) = uri_and_socket_errors_app().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:?}");
    let model = std::fs::read_to_string(emitted.join("app/models/article.rb")).expect("emitted model");
    for line in ["require \"socket\"", "require \"uri\""] {
        assert!(model.lines().any(|l| l.trim() == line), "missing {line}:\n{model}");
    }
    let comment = std::fs::read_to_string(emitted.join("app/models/comment.rb")).expect("emitted model");
    assert!(comment.lines().any(|l| l.trim() == "require \"socket\""), "rescue A,SocketError:\n{comment}");
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn uri_and_socket_errors_are_rescued_on_spinel() {
    uri_and_socket_errors_app().run_spinel(URI_AND_SOCKET_ASSERTIONS).assert_passes();
}

/// `errors.full_message(:field, msg)` answers Rails' text without adding
/// to the record: the humanized field before the message, the bare message
/// for `:base`.
fn full_message_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog().edit(
        "app/models/article.rb",
        "  validates :title, presence: true\n",
        "  validates :title, presence: true

  def self.taken_title_message
    article = Article.new
    article.errors.full_message(:title, \"is taken\")
  end

  def self.base_message(text)
    article = Article.new
    article.errors.full_message(:base, text)
  end

  def body_message(count)
    errors.full_message(:body, \"needs #{count} words\")
  end
",
    )
}

const FULL_MESSAGE_ASSERTIONS: &str = r#"raise "taken: #{Article.taken_title_message}" unless Article.taken_title_message == "Title is taken"
raise "base: #{Article.base_message("Whole")}" unless Article.base_message("Whole") == "Whole"
article = Article.new(title: "x", body: "y")
raise "body: #{article.body_message(3)}" unless article.body_message(3) == "Body needs 3 words"
raise "added" unless article.errors.empty?
puts "full_message passed"
"#;

#[test]
fn errors_full_message_answers_the_humanized_text() {
    full_message_app().run_ruby(FULL_MESSAGE_ASSERTIONS).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn errors_full_message_answers_the_humanized_text_on_spinel() {
    full_message_app().run_spinel(FULL_MESSAGE_ASSERTIONS).assert_passes();
}

/// The app's locale names attributes and formats full messages, as
/// Rails' I18n does: `activerecord.attributes.article.title` and
/// `errors.format` reach validation messages, `errors.add`,
/// `full_message` and `errors[:field]` alike.
fn localized_messages_app() -> emit_and_run::Overlay {
    full_message_app()
        .write(
            "config/locales/en.yml",
            "en:\n  hello: \"Hello world\"\n  errors:\n    format: \"%{attribute}: %{message}\"\n  activerecord:\n    attributes:\n      article:\n        title: \"Headline\"\n",
        )
        .edit(
            "app/models/article.rb",
            "  def self.taken_title_message\n",
            "  def flag_title\n    errors.add(:title, \"is flagged\")\n  end\n\n  def listed_errors\n    errors.full_messages\n  end\n\n  def title_errors\n    errors[:title]\n  end\n\n  def self.taken_title_message\n",
        )
}

const LOCALIZED_MESSAGES_ASSERTIONS: &str = r#"article = Article.new(title: "", body: "short")
raise "valid" if article.valid?
raise "full: #{article.listed_errors.inspect}" unless article.listed_errors == ["Headline: can't be blank", "Body: is too short (minimum is 10 characters)"]
raise "taken: #{Article.taken_title_message}" unless Article.taken_title_message == "Headline: is taken"
raise "body: #{article.body_message(3)}" unless article.body_message(3) == "Body: needs 3 words"
raise "base: #{Article.base_message("Whole")}" unless Article.base_message("Whole") == "Whole"
flagged = Article.new(title: "t", body: "long enough body")
flagged.flag_title
raise "add: #{flagged.listed_errors.inspect}" unless flagged.listed_errors == ["Headline: is flagged"]
puts "localized messages passed"
"#;

#[test]
fn localized_attribute_names_and_format_reach_error_messages() {
    let index = r#"raise "index: #{Article.new(title: "", body: "long enough body").tap(&:valid?).title_errors.inspect}" unless Article.new(title: "", body: "long enough body").tap(&:valid?).title_errors == ["can't be blank"]
"#;
    localized_messages_app().run_ruby(&format!("{index}{LOCALIZED_MESSAGES_ASSERTIONS}")).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn localized_attribute_names_and_format_reach_error_messages_on_spinel() {
    let index = r#"raise "index: #{Article.new(title: "", body: "long enough body").tap(&:valid?).title_errors.inspect}" unless Article.new(title: "", body: "long enough body").tap(&:valid?).title_errors == ["can't be blank"]
"#;
    localized_messages_app().run_spinel(&format!("{index}{LOCALIZED_MESSAGES_ASSERTIONS}")).assert_passes();
}

/// Validation and `errors.add` messages come from the locale as Rails
/// looks them up: a model's attribute override, the plural form for the
/// count, `%{model}` and `%{value}` interpolated.
fn localized_error_kinds_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write(
            "config/locales/en.yml",
            "en:\n  hello: \"Hello world\"\n  activerecord:\n    models:\n      article:\n        one: \"Story\"\n        other: \"Stories\"\n    errors:\n      models:\n        article:\n          attributes:\n            title:\n              blank: \"is required for %{model}\"\n      messages:\n        too_short:\n          one: \"needs at least one character, got %{value}\"\n          other: \"needs %{count} characters, got %{value}\"\n",
        )
        .edit(
            "app/models/article.rb",
            "  validates :title, presence: true\n",
            "  validates :title, presence: true

  def listed_errors
    errors.full_messages
  end

  def flag
    errors.add(:title, :blank)
    errors.add(:body)
  end
",
        )
}

const LOCALIZED_ERROR_KINDS_ASSERTIONS: &str = r#"article = Article.new(title: "", body: "short")
raise "valid" if article.valid?
raise "validated: #{article.listed_errors.inspect}" unless article.listed_errors == ["Title is required for Story", "Body needs 10 characters, got short"]
flagged = Article.new(title: "t", body: "long enough body")
flagged.flag
raise "added: #{flagged.listed_errors.inspect}" unless flagged.listed_errors == ["Title is required for Story", "Body is invalid"]
puts "localized error kinds passed"
"#;

#[test]
fn localized_error_kinds_reach_validations_and_errors_add() {
    localized_error_kinds_app().run_ruby(LOCALIZED_ERROR_KINDS_ASSERTIONS).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn localized_error_kinds_reach_validations_and_errors_add_on_spinel() {
    localized_error_kinds_app().run_spinel(LOCALIZED_ERROR_KINDS_ASSERTIONS).assert_passes();
}

/// A plain ActiveModel class reads its attribute names under
/// `activemodel`, not `activerecord`, as Rails' i18n_scope says.
fn active_model_scope_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write(
            "config/locales/en.yml",
            "en:\n  hello: \"Hello world\"\n  activemodel:\n    attributes:\n      search:\n        query: \"Keywords\"\n  activerecord:\n    attributes:\n      search:\n        query: \"Wrong scope\"\n",
        )
        .write(
            "app/models/search.rb",
            "class Search\n  include ActiveModel::Model\n\n  attr_accessor :query\n\n  validates :query, presence: true\nend\n",
        )
}

const ACTIVE_MODEL_SCOPE_ASSERTIONS: &str = r#"search = Search.new(query: "")
raise "valid" if search.valid?
raise "messages: #{search.errors.inspect}" unless search.errors == ["Keywords can't be blank"]
puts "active model scope passed"
"#;

#[test]
fn an_active_model_class_reads_the_activemodel_scope() {
    active_model_scope_app().run_ruby(ACTIVE_MODEL_SCOPE_ASSERTIONS).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn an_active_model_class_reads_the_activemodel_scope_on_spinel() {
    active_model_scope_app().run_spinel(ACTIVE_MODEL_SCOPE_ASSERTIONS).assert_passes();
}

/// `human_attribute_name` answers the locale's name, from a model method,
/// a view and `record.class`; a dynamic attribute stays an error.
fn human_attribute_name_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write(
            "config/locales/en.yml",
            "en:\n  hello: \"Hello world\"\n  activerecord:\n    attributes:\n      article:\n        title: \"Headline\"\n",
        )
        .edit(
            "app/models/article.rb",
            "  validates :title, presence: true\n",
            "  validates :title, presence: true

  def self.title_label
    human_attribute_name(:title)
  end

  def self.body_label
    Article.human_attribute_name(\"body\")
  end

  def own_created_at_label
    self.class.human_attribute_name(:created_at)
  end
",
        )
}

const HUMAN_ATTRIBUTE_NAME_ASSERTIONS: &str = r#"raise "title: #{Article.title_label}" unless Article.title_label == "Headline"
raise "body: #{Article.body_label}" unless Article.body_label == "Body"
raise "created: #{Article.new.own_created_at_label}" unless Article.new.own_created_at_label == "Created at"
puts "human_attribute_name passed"
"#;

#[test]
fn human_attribute_name_answers_the_locale() {
    human_attribute_name_app().run_ruby(HUMAN_ATTRIBUTE_NAME_ASSERTIONS).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn human_attribute_name_answers_the_locale_on_spinel() {
    human_attribute_name_app().run_spinel(HUMAN_ATTRIBUTE_NAME_ASSERTIONS).assert_passes();
}

#[test]
fn human_attribute_name_on_a_dynamic_attribute_stays_an_error() {
    let (_emitted, errors) = emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "  validates :title, presence: true\n",
            "  validates :title, presence: true\n\n  def self.label_for(field)\n    Article.human_attribute_name(field)\n  end\n",
        )
        .emit(roundhouse::project::BuildTarget::Ruby);
    assert!(errors.iter().any(|e| e.contains("human_attribute_name")), "{errors:?}");
}

/// Form labels and submit text come from the locale as Action View finds
/// them: `helpers.label`, then the model's attribute name, then the
/// humanized method; `helpers.submit` over the model's human name.
fn localized_form_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write(
            "config/locales/en.yml",
            "en:\n  hello: \"Hello world\"\n  helpers:\n    label:\n      article:\n        title: \"Your headline\"\n    submit:\n      create: \"Publish %{model}\"\n  activerecord:\n    attributes:\n      article:\n        body: \"Content\"\n",
        )
        .write(
            "test/controllers/localized_forms_controller_test.rb",
            "require \"test_helper\"\n\nclass LocalizedFormsControllerTest < ActionDispatch::IntegrationTest\n  test \"labels and submit read the locale\" do\n    get new_article_url\n    assert_includes response.body, \">Your headline</label>\"\n    assert_includes response.body, \">Content</label>\"\n    assert_includes response.body, \"value=\\\"Publish Article\\\"\"\n    get edit_article_url(articles(:one))\n    assert_includes response.body, \"value=\\\"Update Article\\\"\"\n  end\nend\n",
        )
}

#[test]
fn localized_form_labels_and_submit_text() {
    localized_form_app().run_test("test/controllers/localized_forms_controller_test.rb").assert_passes();
}

/// A `class:` array over `errors[:field]` picks its classes per field,
/// as Rails' `build_tag_values` does.
const FORM_ERROR_CLASSES: &str = r#"a = Article.new(title: "", body: "long enough body")
a.valid?
html = Views::Articles.form(a)
raise html unless html.include?("id=\"article_title\" class=\"block shadow-sm rounded-md border px-3 py-2 mt-2 w-full border-red-400 focus:outline-red-600\"")
raise html unless html.include?("rows=\"4\" class=\"block shadow-sm rounded-md border px-3 py-2 mt-2 w-full border-gray-400 focus:outline-blue-600\">")
"#;

#[test]
fn a_class_array_over_field_errors_picks_the_error_classes() {
    emit_and_run::real_blog().run_ruby(FORM_ERROR_CLASSES).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_class_array_over_field_errors_picks_the_error_classes_on_spinel() {
    emit_and_run::real_blog().run_spinel(FORM_ERROR_CLASSES).assert_passes();
}

/// Rails' default `field_error_proc`: a label or control whose field has
/// errors renders inside `<div class="field_with_errors">`; a check box's
/// hidden companion stays outside it.
fn field_with_errors_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog().edit(
        "app/views/articles/_form.html.erb",
        "    <%= form.text_field :title,",
        "    <%= form.check_box :title %>\n    <%= form.text_field :title,",
    )
}

const FIELD_WITH_ERRORS: &str = r#"a = Article.new(title: "", body: "long enough body")
a.valid?
html = Views::Articles.form(a)
raise html unless html.include?("<div class=\"field_with_errors\"><label for=\"article_title\">Title</label></div>")
raise html unless html.include?("<input name=\"article[title]\" type=\"hidden\" value=\"0\"><div class=\"field_with_errors\"><input type=\"checkbox\" value=\"1\" name=\"article[title]\" id=\"article_title\"></div>")
raise html unless html.include?("<div class=\"field_with_errors\"><input type=\"text\" name=\"article[title]\" id=\"article_title\"")
raise html unless html.include?("<label for=\"article_body\">Body</label>\n")
raise html unless html.include?("<textarea name=\"article[body]\"")
raise html if html.include?("<div class=\"field_with_errors\"><textarea")
clean = Views::Articles.form(Article.new(title: "t", body: "long enough body"))
raise clean if clean.include?("field_with_errors")
"#;

#[test]
fn a_field_with_errors_renders_inside_field_with_errors() {
    field_with_errors_app().run_ruby(FIELD_WITH_ERRORS).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_field_with_errors_renders_inside_field_with_errors_on_spinel() {
    field_with_errors_app().run_spinel(FIELD_WITH_ERRORS).assert_passes();
}

/// An app that replaces `field_error_proc` keeps its form fields as errors.
#[test]
fn a_custom_field_error_proc_stays_an_error() {
    let (_emitted, errors) = emit_and_run::real_blog()
        .write(
            "config/initializers/field_error_proc.rb",
            "Rails.application.config.action_view.field_error_proc = proc { |html_tag, _instance| html_tag }\n",
        )
        .emit(roundhouse::project::BuildTarget::Ruby);
    assert!(errors.iter().any(|e| e.contains("field_error_proc")), "{errors:?}");
}

#[test]
fn a_mention_of_field_error_proc_keeps_the_default_wrapper() {
    let (_emitted, errors) = emit_and_run::real_blog()
        .write(
            "config/initializers/notes.rb",
            "Rails.application.config.x.note = \"field_error_proc\" # field_error_proc = custom\n",
        )
        .emit(roundhouse::project::BuildTarget::Ruby);
    assert!(!errors.iter().any(|e| e.contains("field_error_proc")), "{errors:?}");
}

/// `I18n.t` and a template's `t` answer the locale at compile time:
/// interpolation, plural forms chosen by a run-time count, `scope:`,
/// `default:` and a view's lazy `.key`.
fn translate_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write(
            "config/locales/en.yml",
            "en:\n  hello: \"Hello world\"\n  greet: \"Hi %{name}\"\n  inbox:\n    zero: \"No messages\"\n    one: \"1 message\"\n    other: \"%{count} messages\"\n  shop:\n    title: \"Store\"\n  intro_html: \"<b>Hi</b> %{name}\"\n  pct: \"100%% sure %%{x} %{name}\"\n  mood:\n    happy: \"Glad\"\n    sad: \"Down %{name}\"\n  visibility:\n    public: \"Everyone\"\n    private: \"Only me\"\n    notice_html: \"<b>Note</b> %{name}\"\n  box:\n    inbox:\n      one: \"1 letter\"\n      other: \"%{count} letters\"\n  tally_html:\n    one: \"<b>one</b>\"\n    other: \"<b>%{count}</b> items\"\n  articles:\n    index:\n      heading: \"All the articles\"\n",
        )
        .edit(
            "app/models/article.rb",
            "  validates :title, presence: true\n",
            "  validates :title, presence: true

  def self.greeting(name)
    I18n.t(\"greet\", name: name)
  end

  def self.inbox(count)
    I18n.t(\"inbox\", count: count)
  end

  def self.scoped_title
    I18n.t(:title, scope: :shop)
  end

  def self.fallbacks
    [I18n.t(:nope, default: :greet, name: \"Bo\"), I18n.t(:nope, default: \"Plain\")]
  end

  def self.intro
    I18n.t(\"intro_html\", name: \"<i>\")
  end

  def self.counted_inbox(calls)
    I18n.t(\"inbox\", count: (calls << 1).size)
  end

  def self.percents
    [I18n.t(\"pct\", name: \"N\", x: \"X\"), I18n.t(\"greet\")]
  end

  def self.moods(flag)
    [I18n.t(\"mood.#{flag ? 'happy' : 'sad'}\", name: \"Al\"), I18n.t(flag ? :hello : \"shop.title\")]
  end

  def self.visibility(v)
    I18n.t(\"visibility.#{v}\")
  end

  def self.visibility_or(v)
    I18n.t(\"visibility.#{v}\", default: \"Unknown\")
  end

  def self.letters(box, n)
    I18n.t(\"box.#{box}\", count: n)
  end

  def self.missing_text(flag)
    [I18n.t(\"no.such.key\", name: \"x\"), I18n.t(\"shop.#{flag ? 'title' : 'nope'}\")]
  end

  def self.unread_option(calls)
    I18n.t(\"hello\", unused: (calls << 1).size)
  end

  def self.read_before_write
    name = \"before\"
    I18n.t(\"greet\", name: name, unused: (name = \"after\"))
  end

  def self.ordered_options(calls)
    I18n.t(\"greet\", other: (calls << \"a\").size, name: (calls << \"b\").join)
  end
",
        )
        .edit(
            "app/views/articles/index.html.erb",
            "<h1",
            "<p id=\"lazy\"><%= t(\".heading\") %></p><p id=\"inbox\"><%= t(\"inbox\", count: @articles.size) %></p><p id=\"intro\"><%= t(\"intro_html\", name: \"<i>\") %></p><p id=\"tally\"><%= t(\"tally_html\", count: \"<s>\") %></p><p id=\"missing\"><%= t(\"no.such_key\", name: \"<b>\", count: 2) %></p><p id=\"lazy-missing\"><%= t(\".missing_heading\") %></p><% [\"notice_html\", \"public\", \"nope\"].each do |k| %><p class=\"vis\"><%= t(\"visibility.#{k}\", name: \"<i>\") %></p><% end %>\n<h1",
        )
        .write(
            "test/controllers/translations_controller_test.rb",
            "require \"test_helper\"\n\nclass TranslationsControllerTest < ActionDispatch::IntegrationTest\n  test \"templates translate\" do\n    get articles_url\n    assert_includes response.body, \"<p id=\\\"lazy\\\">All the articles</p>\"\n    assert_includes response.body, \" messages</p>\"\n    assert_includes response.body, \"<p id=\\\"intro\\\"><b>Hi</b> &lt;i&gt;</p>\"\n    assert_includes response.body, \"<p id=\\\"tally\\\"><b>&lt;s&gt;</b> items</p>\"\n    assert_includes response.body, \"<p id=\\\"missing\\\"><span class=\\\"translation_missing\\\" title=\\\"translation missing: en.no.such_key, name: &amp;lt;b&amp;gt;, count: 2\\\">Such Key</span></p>\"\n    assert_includes response.body, \"<p class=\\\"vis\\\"><b>Note</b> &lt;i&gt;</p><p class=\\\"vis\\\">Everyone</p><p class=\\\"vis\\\"><span class=\\\"translation_missing\\\" title=\\\"translation missing: en.visibility.nope, name: &amp;lt;i&amp;gt;\\\">Nope</span></p>\"\n    assert_includes response.body, \"<p id=\\\"lazy-missing\\\"><span class=\\\"translation_missing\\\" title=\\\"translation missing: en.articles.index.missing_heading\\\">Missing Heading</span></p>\"\n  end\nend\n",
        )
}

const TRANSLATE_ASSERTIONS: &str = r#"raise "greet: #{Article.greeting("Ann")}" unless Article.greeting("Ann") == "Hi Ann"
raise "zero: #{Article.inbox(0)}" unless Article.inbox(0) == "No messages"
raise "one: #{Article.inbox(1)}" unless Article.inbox(1) == "1 message"
raise "other: #{Article.inbox(3)}" unless Article.inbox(3) == "3 messages"
raise "scope: #{Article.scoped_title}" unless Article.scoped_title == "Store"
raise "fallbacks: #{Article.fallbacks.inspect}" unless Article.fallbacks == ["Hi Bo", "Plain"]
raise "intro: #{Article.intro}" unless Article.intro == "<b>Hi</b> <i>"
calls = []
counted = Article.counted_inbox(calls)
raise "counted: #{counted}" unless counted == "1 message"
raise "count evaluated #{calls.size} times" unless calls.size == 1
raise "percents: #{Article.percents.inspect}" unless Article.percents == ["100% sure %{x} N", "Hi %{name}"]
raise "moods: #{Article.moods(true).inspect}" unless Article.moods(true) == ["Glad", "Hello world"]
raise "moods: #{Article.moods(false).inspect}" unless Article.moods(false) == ["Down Al", "Store"]
raise "visibility: #{Article.visibility("private")}" unless [Article.visibility("public"), Article.visibility("private")] == ["Everyone", "Only me"]
raise "visibility missing: #{Article.visibility("nope")}" unless Article.visibility("nope") == "Translation missing: en.visibility.nope"
raise "visibility default: #{Article.visibility_or("nope")}" unless Article.visibility_or("nope") == "Unknown" && Article.visibility_or("public") == "Everyone"
raise "letters: #{Article.letters("inbox", 2)}" unless Article.letters("inbox", 2) == "2 letters" && Article.letters("inbox", 1) == "1 letter"
begin
  Article.visibility("")
  raise "subtree did not raise"
rescue NotImplementedError
end
raise "missing: #{Article.missing_text(false).inspect}" unless Article.missing_text(false) == ["Translation missing: en.no.such.key", "Translation missing: en.shop.nope"]
calls = []
raise "unread: #{calls.inspect}" unless Article.unread_option(calls) == "Hello world" && calls == [1]
raise "read before write: #{Article.read_before_write}" unless Article.read_before_write == "Hi before"
calls = []
ordered = Article.ordered_options(calls)
raise "ordered: #{ordered} #{calls.inspect}" unless ordered == "Hi ab" && calls == ["a", "b"]
puts "translate passed"
"#;

#[test]
fn i18n_t_answers_the_locale() {
    translate_app().run_ruby(TRANSLATE_ASSERTIONS).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn i18n_t_answers_the_locale_on_spinel() {
    translate_app().run_spinel(TRANSLATE_ASSERTIONS).assert_passes();
}

#[test]
fn template_t_answers_the_locale() {
    translate_app().run_test("test/controllers/translations_controller_test.rb").assert_passes();
}

/// A `Rails::Engine` gem's `config/locales` load between Rails' English
/// and the app's, as Rails assembles `I18n.load_path`: the gem's key
/// resolves, and the app overrides one the gem also defines.
fn engine_locale_app() -> emit_and_run::Overlay {
    let gem = "vendor/bundle/ruby/3.4.0/gems/shelf-1.2.0";
    emit_and_run::real_blog()
        .write(".bundle/config", "---\nBUNDLE_PATH: \"vendor/bundle\"\n")
        .edit("Gemfile.lock", "  specs:\n", "  specs:\n    shelf (1.2.0)\n")
        .write(&format!("{gem}/lib/shelf/engine.rb"), "module Shelf\n  class Engine < ::Rails::Engine\n  end\nend\n")
        .write(&format!("{gem}/config/locales/shelf.en.yml"), "en:\n  shelf:\n    edit: \"Edit shelf\"\n    title: \"Gem title\"\n")
        .write("config/locales/en.yml", "en:\n  hello: \"Hello world\"\n  shelf:\n    title: \"App title\"\n")
        .edit(
            "app/models/article.rb",
            "  validates :title, presence: true\n",
            "  validates :title, presence: true\n\n  def self.shelf_texts\n    [I18n.t(\"shelf.edit\"), I18n.t(\"shelf.title\")]\n  end\n",
        )
}

#[test]
fn an_engine_gem_s_locales_answer_beneath_the_app_s() {
    engine_locale_app()
        .run_ruby("raise Article.shelf_texts.inspect unless Article.shelf_texts == [\"Edit shelf\", \"App title\"]\nputs \"engine locales passed\"\n")
        .assert_passes();
}

/// `raise_on_missing_translations` in the running environment's file
/// makes a missing key raise `I18n::MissingTranslationData`.
#[test]
fn a_missing_translation_raises_where_the_environment_says_so() {
    translate_app()
        .edit(
            "config/environments/development.rb",
            "  # config.i18n.raise_on_missing_translations = true",
            "  config.i18n.raise_on_missing_translations = true",
        )
        .run_ruby("begin\n  Article.missing_text(true)\n  raise \"no raise\"\nrescue I18n::MissingTranslationData => e\n  raise e.message unless e.message == \"Translation missing: en.no.such.key\"\nend\nraise \"found\" unless Article.scoped_title == \"Store\"\nputs \"raise passed\"\n")
        .assert_passes();
}

/// A `:strict` setting or a call with a block stays an error; a key
/// computed at run time is read from the catalog then (`i18n_runtime`).
#[test]
fn unresolvable_translations_stay_errors() {
    let (_emitted, errors) = emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "  validates :title, presence: true\n",
            "  validates :title, presence: true\n\n  def self.dynamic(key)\n    I18n.t(key)\n  end\n\n  def self.blocked\n    I18n.t(\"hello\") { |text| text }\n  end\n",
        )
        .emit(roundhouse::project::BuildTarget::Ruby);
    let i18n: Vec<&String> = errors.iter().filter(|e| e.contains("I18n not supported")).collect();
    assert!(i18n.len() == 1 && i18n[0].contains("a block on the call"), "{errors:?}");
    let (_emitted, errors) = emit_and_run::real_blog()
        .edit(
            "config/environments/test.rb",
            "  # config.i18n.raise_on_missing_translations = true",
            "  config.i18n.raise_on_missing_translations = :strict",
        )
        .edit("app/models/article.rb", "  validates :title, presence: true\n", "  validates :title, presence: true\n\n  def self.missing\n    I18n.t(\"no.such.key\")\n  end\n")
        .emit(roundhouse::project::BuildTarget::Ruby);
    assert!(errors.iter().any(|e| e.contains(":strict")), "{errors:?}");
}

/// `errors[:field]` picks a message by the attribute it was added on,
/// not by its text: `title_note`'s "Title note …" is not a `title`
/// error, and a format that names no attribute still tells fields apart.
fn errors_by_attribute_app(format: &str) -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write("config/locales/en.yml", &format!("en:\n  hello: \"Hello world\"\n  errors:\n    format: \"{format}\"\n"))
        .edit(
            "app/models/article.rb",
            "  validates :title, presence: true\n",
            "  validates :title, presence: true
  validate :note_check

  def note_check
    errors.add(:title_note, \"is odd\") if body == \"an odd body\"
  end

  def title_errors
    errors[:title]
  end

  def base_errors
    errors[:base]
  end

  def recheck
    errors.clear
    errors.add(:body, \"is odd\")
  end
",
        )
}

const ERRORS_BY_ATTRIBUTE: &str = r#"a = Article.new(title: "", body: "an odd body")
a.valid?
raise a.title_errors.inspect unless a.title_errors == ["can't be blank"]
raise a.base_errors.inspect unless a.base_errors.empty?
details = a.error_details.map { |e| e.attribute + "/" + e.kind + "/" + e.message }
raise details.inspect unless details == ["title/blank/can't be blank", "title_note/is odd/is odd"]
html = Views::Articles.form(a)
raise html unless html.include?("<div class=\"field_with_errors\"><label for=\"article_title\">Title</label></div>")
raise html if html.include?("<div class=\"field_with_errors\"><textarea")
a.recheck
raise a.title_errors.inspect unless a.title_errors.empty?
raise a.errors.inspect unless a.errors.length == 1
"#;

#[test]
fn errors_index_picks_a_field_by_its_attribute() {
    errors_by_attribute_app("%{attribute} %{message}").run_ruby(ERRORS_BY_ATTRIBUTE).assert_passes();
}

#[test]
fn errors_index_picks_a_field_under_a_format_that_names_no_attribute() {
    errors_by_attribute_app("Error: %{message}").run_ruby(ERRORS_BY_ATTRIBUTE).assert_passes();
}

/// Each error keeps its message apart from the full text, so the format can put the message anywhere.
#[test]
fn errors_index_picks_a_field_under_a_format_that_ends_elsewhere() {
    errors_by_attribute_app("%{message} (%{attribute})").run_ruby(ERRORS_BY_ATTRIBUTE).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn errors_index_picks_a_field_by_its_attribute_on_spinel() {
    errors_by_attribute_app("%{attribute} %{message}").run_spinel(ERRORS_BY_ATTRIBUTE).assert_passes();
}

/// `errors.to_hash` maps each attribute with errors to its messages, in
/// the order Rails gives: a `validates` rule, a `validate` method's
/// `errors.add(:base, …)`, on the record itself and on a record a class
/// method builds.
fn errors_to_hash_app(format: &str) -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write("config/locales/en.yml", &format!("en:\n  hello: \"Hello world\"\n  errors:\n    format: \"{format}\"\n"))
        .edit(
            "app/models/article.rb",
            "  validates :body, presence: true, length: { minimum: 10 }\n",
            "  validates :body, presence: true, length: { minimum: 10 }
  validate :odd_body

  def odd_body
    errors.add(:base, \"is odd\") if body == \"odd\"
  end

  def errors_hash
    errors.to_hash
  end

  def self.draft_errors(title, body)
    article = Article.new(title: title, body: body)
    article.valid?
    article.errors.to_hash
  end
",
        )
}

const ERRORS_TO_HASH: &str = r#"a = Article.new(title: "", body: "odd")
a.valid?
expected = { title: ["can't be blank"], body: ["is too short (minimum is 10 characters)"], base: ["is odd"] }
raise a.errors_hash.inspect unless a.errors_hash == expected && a.errors_hash.keys == [:title, :body, :base]
raise Article.draft_errors("", "odd").inspect unless Article.draft_errors("", "odd") == expected
b = Article.new(title: "A title", body: "A long enough body")
b.valid?
raise b.errors_hash.inspect unless b.errors_hash.empty?
puts "errors.to_hash passed"
"#;

#[test]
fn errors_to_hash_groups_messages_by_attribute() {
    errors_to_hash_app("%{attribute} %{message}").run_ruby(ERRORS_TO_HASH).assert_passes();
}

#[test]
fn errors_to_hash_does_not_depend_on_the_message_format() {
    errors_to_hash_app("%{message} (%{attribute})").run_ruby(ERRORS_TO_HASH).assert_passes();
}

/// `errors.include?` / `key?` / `has_key?`, `attribute_names` and a
/// `messages` read as a whole answer what Rails 8.1 does: the Hash
/// answers `[]` for an attribute with no errors, and both it and the
/// names are frozen.
fn errors_readers_app() -> emit_and_run::Overlay {
    errors_to_hash_app("%{attribute} %{message}").edit(
        "app/models/article.rb",
        "  def errors_hash\n",
        "  def error_questions
    [errors.include?(:title), errors.key?(\"body\"), errors.has_key?(:base), errors.include?(:nothing)]
  end

  def error_names
    errors.attribute_names
  end

  def all_messages
    errors.messages
  end

  def messages_on(field)
    errors.messages[field]
  end

  def errors_hash\n",
    )
}

const ERRORS_READERS: &str = r#"a = Article.new(title: "", body: "odd")
a.valid?
raise a.error_questions.inspect unless a.error_questions == [true, true, true, false]
raise a.error_names.inspect unless a.error_names == [:title, :body, :base] && a.error_names.frozen?
expected = { title: ["can't be blank"], body: ["is too short (minimum is 10 characters)"], base: ["is odd"] }
raise a.all_messages.inspect unless a.all_messages == expected && a.all_messages.frozen?
raise a.all_messages[:nothing].inspect unless a.all_messages[:nothing] == [] && a.all_messages[:nothing].frozen?
raise a.messages_on(:base).inspect unless a.messages_on(:base) == ["is odd"] && a.messages_on(:nothing) == []
b = Article.new(title: "A title", body: "A long enough body")
b.valid?
raise b.error_questions.inspect unless b.error_questions == [false, false, false, false]
raise b.error_names.inspect unless b.error_names.empty? && b.all_messages.empty?
puts "errors readers passed"
"#;

#[test]
fn errors_readers_answer_from_the_error_details() {
    errors_readers_app().run_ruby(ERRORS_READERS).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn errors_readers_answer_from_the_error_details_on_spinel() {
    errors_readers_app().run_spinel(ERRORS_READERS).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn errors_to_hash_groups_messages_by_attribute_on_spinel() {
    errors_to_hash_app("%{attribute} %{message}").run_spinel(ERRORS_TO_HASH).assert_passes();
}

/// Iterating `errors` yields each `ActiveModel::Error`, as Rails 8.1
/// does: its attribute is a Symbol, its `message` the bare message and
/// its `full_message` the one `full_messages` lists.
fn errors_each_app() -> emit_and_run::Overlay {
    errors_to_hash_app("%{attribute} %{message}").edit(
        "app/models/article.rb",
        "  def errors_hash\n",
        "  def error_rows
    rows = []
    errors.each { |error| rows << [error.attribute, error.message, error.full_message] }
    rows
  end

  def error_full_messages
    errors.map(&:full_message)
  end

  def error_attributes
    errors.collect { |error| error.attribute }
  end

  def base_message
    found = errors.find { |error| error.attribute == :base }
    found ? found.message : \"none\"
  end

  def first_attribute
    errors.first ? errors.first.attribute : :none
  end

  def body_messages
    errors.select { |error| error.attribute == :body }.map(&:message)
  end

  def unattributed_count
    errors.reject { |error| error.attribute == :base }.size
  end

  def all_have_messages
    errors.all? { |error| error.message.length > 0 }
  end

  def errors_hash\n",
    )
}

const ERRORS_EACH: &str = r#"a = Article.new(title: "", body: "odd")
a.valid?
rows = [[:title, "can't be blank", "Title can't be blank"], [:body, "is too short (minimum is 10 characters)", "Body is too short (minimum is 10 characters)"], [:base, "is odd", "is odd"]]
raise a.error_rows.inspect unless a.error_rows == rows
raise a.error_full_messages.inspect unless a.error_full_messages == rows.map(&:last)
raise a.error_attributes.inspect unless a.error_attributes == [:title, :body, :base]
raise a.base_message.inspect unless a.base_message == "is odd"
raise a.first_attribute.inspect unless a.first_attribute == :title
raise a.body_messages.inspect unless a.body_messages == ["is too short (minimum is 10 characters)"]
raise a.unattributed_count.inspect unless a.unattributed_count == 2
raise a.all_have_messages.inspect unless a.all_have_messages == true
b = Article.new(title: "A title", body: "A long enough body")
b.valid?
raise b.error_rows.inspect unless b.error_rows.empty? && b.error_full_messages.empty?
raise b.base_message.inspect unless b.base_message == "none" && b.first_attribute == :none
puts "errors iteration passed"
"#;

#[test]
fn iterating_errors_yields_each_error() {
    errors_each_app().run_ruby(ERRORS_EACH).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn iterating_errors_yields_each_error_on_spinel() {
    errors_each_app().run_spinel(ERRORS_EACH).assert_passes();
}

/// Each `&.` of a chain binds its own receiver: one shared local held
/// `errors` and then its full messages, so `&.join` was asked of `errors`.
fn safe_navigation_chain_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog().edit(
        "app/models/article.rb",
        "  validates :title, presence: true\n",
        "  validates :title, presence: true

  def self.error_line(title)
    Article.find_by(title: title)&.errors&.full_messages&.join(\", \")
  end

  def error_sentence
    errors&.full_messages&.to_sentence
  end

  def shout
    title&.strip&.upcase&.length
  end
",
    )
}

const SAFE_NAVIGATION_CHAIN: &str = r#"a = Article.new(title: " hi ", body: "")
a.valid?
raise a.error_sentence.inspect unless a.error_sentence == "Body can't be blank and Body is too short (minimum is 10 characters)"
raise a.shout.inspect unless a.shout == 2
puts "safe navigation chain passed"
"#;

#[test]
fn each_link_of_a_safe_navigation_chain_reads_its_own_receiver() {
    let found = r#"Article.create!(title: "Saved", body: "A long enough body")
raise Article.error_line("Saved").inspect unless Article.error_line("Saved") == ""
raise Article.error_line("Missing").inspect unless Article.error_line("Missing").nil?
"#;
    safe_navigation_chain_app().run_ruby(&format!("{found}{SAFE_NAVIGATION_CHAIN}")).assert_passes();
}

/// The native consumer boots no database, so only the chains on a built record.
#[test]
#[ignore = "requires the Spinel toolchain"]
fn each_link_of_a_safe_navigation_chain_reads_its_own_receiver_on_spinel() {
    safe_navigation_chain_app().run_spinel(SAFE_NAVIGATION_CHAIN).assert_passes();
}

/// `error.type` is a Symbol, or the message when `errors.add` was given a
/// String, and `of_kind?` and `where` ask it, as Rails 8.1 does.
fn error_types_app() -> emit_and_run::Overlay {
    errors_each_app().edit(
        "app/models/article.rb",
        "  def error_rows\n",
        "  def error_types
    errors.map { |error| error.type }
  end

  def first_type
    errors.first ? errors.first.type : :none
  end

  def body_errors
    errors.where(:body).map(&:full_message)
  end

  def where_sizes
    [errors.where(:title, :blank).size, errors.where(:base, \"is odd\").size, errors.where(:base, :invalid).size,
     errors.where(\"title\").reject { |error| error.type == :blank }.size]
  end

  def kinds
    [errors.of_kind?(:title, :blank), errors.of_kind?(:body, :blank), errors.of_kind?(\"body\", :too_short),
     errors.of_kind?(:base, \"is odd\"), errors.of_kind?(:base, :invalid), errors.of_kind?(:title)]
  end

  def error_rows\n",
    )
}

const ERROR_TYPES: &str = r#"a = Article.new(title: "", body: "odd")
a.valid?
raise a.error_types.inspect unless a.error_types == [:blank, :too_short, "is odd"]
raise a.first_type.inspect unless a.first_type == :blank
raise a.kinds.inspect unless a.kinds == [true, false, true, true, false, false]
raise a.body_errors.inspect unless a.body_errors == ["Body is too short (minimum is 10 characters)"]
raise a.where_sizes.inspect unless a.where_sizes == [1, 1, 0, 0]
b = Article.new(title: "A title", body: "A long enough body")
b.valid?
raise b.kinds.inspect unless b.kinds == [false, false, false, false, false, false] && b.first_type == :none
puts "error types passed"
"#;

#[test]
fn an_error_s_type_is_rails_type() {
    error_types_app().run_ruby(ERROR_TYPES).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn an_error_s_type_is_rails_type_on_spinel() {
    error_types_app().run_spinel(ERROR_TYPES).assert_passes();
}

/// `errors.delete(:field)` drops the field's errors from the full
/// messages and the details alike, and answers their messages or nil.
fn errors_delete_app() -> emit_and_run::Overlay {
    errors_each_app().edit(
        "app/models/article.rb",
        "  def error_rows\n",
        "  def drop_errors(field)
    field == \"body\" ? errors.delete(:body) : errors.delete(\"nothing\")
  end

  def listed_messages
    [errors.full_messages, errors.count]
  end

  def error_rows\n",
    )
}

const ERRORS_DELETE: &str = r#"a = Article.new(title: "", body: "odd")
a.valid?
raise a.drop_errors("nothing").inspect unless a.drop_errors("nothing").nil?
deleted = a.drop_errors("body")
raise deleted.inspect unless deleted == ["is too short (minimum is 10 characters)"]
raise a.error_full_messages.inspect unless a.error_full_messages == ["Title can't be blank", "is odd"]
raise a.error_attributes.inspect unless a.error_attributes == [:title, :base]
raise a.listed_messages.inspect unless a.listed_messages == [["Title can't be blank", "is odd"], 2]
raise a.drop_errors("body").inspect unless a.drop_errors("body").nil?
puts "errors.delete passed"
"#;

#[test]
fn errors_delete_drops_the_field_s_errors() {
    errors_delete_app().run_ruby(ERRORS_DELETE).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn errors_delete_drops_the_field_s_errors_on_spinel() {
    errors_delete_app().run_spinel(ERRORS_DELETE).assert_passes();
}

#[test]
fn errors_delete_is_a_gap_on_strict_targets() {
    let (_emitted, errors) = errors_delete_app().emit(roundhouse::project::BuildTarget::Crystal);
    assert_eq!(
        errors.iter().filter(|e| e.contains("ActiveSupport.errors_delete is only available on the ruby family and spinel")).count(),
        2,
        "{errors:#?}"
    );
}

/// A `belongs_to` checks its record where it is declared, among the
/// `validates`, and its error's type is `:blank`: Rails validates the
/// association's presence with `message: :required`.
fn belongs_to_errors_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog().edit(
        "app/models/comment.rb",
        "  validates :body, presence: true\n",
        "  validates :body, presence: true

  def error_rows
    errors.map { |error| [error.attribute, error.type, error.full_message] }
  end
",
    )
}

const BELONGS_TO_ERRORS: &str = r#"comment = Comment.new
comment.valid?
rows = [[:article, :blank, "Article must exist"], [:commenter, :blank, "Commenter can't be blank"], [:body, :blank, "Body can't be blank"]]
raise comment.error_rows.inspect unless comment.error_rows == rows
puts "belongs_to errors passed"
"#;

#[test]
fn a_belongs_to_error_lists_where_it_is_declared() {
    belongs_to_errors_app().run_ruby(BELONGS_TO_ERRORS).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_belongs_to_error_lists_where_it_is_declared_on_spinel() {
    belongs_to_errors_app().run_spinel(BELONGS_TO_ERRORS).assert_passes();
}

/// `details` groups the errors by attribute, an entry's `[:error]` is its
/// type, and `added?` matches only an error Rails keeps no options for.
fn errors_details_app() -> emit_and_run::Overlay {
    error_types_app().edit(
        "app/models/article.rb",
        "  def error_rows\n",
        "  def detail_types
    out = []
    errors.details.each_pair { |attribute, entries| entries.each { |entry| out << [attribute, entry[:error]] } }
    out
  end

  def short_body
    [errors.details[:body].any? { |detail| detail[:error] == :too_short }, errors.details[:nothing].size]
  end

  def addeds
    [errors.added?(:title, :blank), errors.added?(:body, :too_short), errors.added?(:base, \"is odd\"), errors.added?(:title),
     errors.added?(:base, :invalid)]
  end

  def error_rows\n",
    )
}

const ERRORS_DETAILS: &str = r#"a = Article.new(title: "", body: "odd")
a.valid?
raise a.detail_types.inspect unless a.detail_types == [[:title, :blank], [:body, :too_short], [:base, "is odd"]]
raise a.short_body.inspect unless a.short_body == [true, 0]
raise a.addeds.inspect unless a.addeds == [true, false, true, false, false]
b = Article.new(title: "A title", body: "A long enough body")
b.valid?
raise b.detail_types.inspect unless b.detail_types.empty? && b.short_body == [false, 0]
puts "errors.details passed"
"#;

#[test]
fn errors_details_reads_each_error_s_type() {
    errors_details_app().run_ruby(ERRORS_DETAILS).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn errors_details_reads_each_error_s_type_on_spinel() {
    errors_details_app().run_spinel(ERRORS_DETAILS).assert_passes();
}

#[test]
fn errors_details_is_a_gap_on_strict_targets() {
    let (_emitted, errors) = errors_details_app().emit(roundhouse::project::BuildTarget::Crystal);
    assert!(
        errors.iter().any(|e| e.contains("ActiveSupport.errors_details is only available on the ruby family and spinel")),
        "{errors:#?}"
    );
}

/// `errors.merge!` and `errors.import` file another record's errors here,
/// each full message naming its attribute as this model does.
fn errors_import_app() -> emit_and_run::Overlay {
    errors_details_app()
        .write(
            "config/locales/en.yml",
            "en:\n  hello: \"Hello world\"\n  activerecord:\n    attributes:\n      article:\n        commenter: \"Reviewer\"\n      comment:\n        body: \"Remark\"\n",
        )
        .edit(
            "app/models/article.rb",
            "  def error_rows\n",
            "  def self.comment_messages
    comment = Comment.new
    comment.valid?
    comment.errors.full_messages
  end

  def merged_messages
    comment = Comment.new
    comment.valid?
    errors.merge!(comment)
    errors.full_messages
  end

  def merged_errors
    comment = Comment.new
    comment.valid?
    errors.merge!(comment.errors)
    errors.map { |error| [error.attribute, error.type, error.message, error.full_message] }
  end

  def imported_errors
    comment = Comment.new
    comment.valid?
    [:title].each { |field| comment.errors.each { |error| errors.import(error, attribute: field) } }
    errors.map { |error| [error.attribute, error.type, error.message, error.full_message] }
  end

  def imported_own
    comment = Comment.new
    comment.valid?
    comment.errors.each { |error| errors.import(error) }
    errors.full_messages
  end

  def merged_into_itself
    before = errors.size
    errors.merge!(errors)
    errors.size == before
  end

  def error_rows\n",
        )
}

const ERRORS_IMPORT: &str = r#"raise Article.comment_messages.inspect unless Article.comment_messages == ["Article must exist", "Commenter can't be blank", "Remark can't be blank"]
fresh = -> { Article.new(title: "A title", body: "A long enough body") }
messages = fresh.call.merged_messages
raise messages.inspect unless messages == ["Article must exist", "Reviewer can't be blank", "Body can't be blank"]
rows = [[:article, :blank, "must exist", "Article must exist"], [:commenter, :blank, "can't be blank", "Reviewer can't be blank"], [:body, :blank, "can't be blank", "Body can't be blank"]]
merged = fresh.call
raise merged.merged_errors.inspect unless merged.merged_errors == rows
raise merged.addeds.inspect unless merged.addeds == [false, false, false, false, false]
raise merged.merged_into_itself.inspect unless merged.merged_into_itself
titled = [[:title, :blank, "must exist", "Title must exist"], [:title, :blank, "can't be blank", "Title can't be blank"], [:title, :blank, "can't be blank", "Title can't be blank"]]
raise fresh.call.imported_errors.inspect unless fresh.call.imported_errors == titled
own = fresh.call.imported_own
raise own.inspect unless own == ["Article must exist", "Reviewer can't be blank", "Body can't be blank"]
puts "errors.import passed"
"#;

#[test]
fn imported_errors_are_named_as_the_importing_model_names_them() {
    errors_import_app().run_ruby(ERRORS_IMPORT).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn imported_errors_are_named_as_the_importing_model_names_them_on_spinel() {
    errors_import_app().run_spinel(ERRORS_IMPORT).assert_passes();
}

#[test]
fn errors_import_is_a_gap_on_strict_targets() {
    let (_emitted, errors) = errors_import_app().emit(roundhouse::project::BuildTarget::Crystal);
    for helper in ["errors_merge", "errors_import"] {
        assert!(
            errors.iter().any(|e| e.contains(&format!("ActiveSupport.{helper} is only available on the ruby family and spinel"))),
            "{helper}: {errors:#?}"
        );
    }
}

/// A `details` entry's other keys are its options, which no error keeps.
#[test]
fn a_details_entry_s_options_are_an_error() {
    let (_emitted, errors) = errors_each_app()
        .edit(
            "app/models/article.rb",
            "  def error_rows\n",
            "  def body_minimum
    errors.details[:body].map { |detail| detail[:count] }
  end

  def error_rows\n",
        )
        .emit(roundhouse::project::BuildTarget::Ruby);
    assert!(errors.iter().any(|e| e.contains("no known method `[]`")), "{errors:#?}");
}

/// `type` reads the error more than once, so a found error is an error.
#[test]
fn the_type_of_an_error_that_is_not_a_plain_read_is_an_error() {
    let (_emitted, errors) = errors_each_app()
        .edit(
            "app/models/article.rb",
            "  def error_rows\n",
            "  def base_type
    errors.find { |error| error.attribute == :base }.type
  end

  def error_rows\n",
        )
        .emit(roundhouse::project::BuildTarget::Ruby);
    assert!(errors.iter().any(|e| e.contains("read the error three times")), "{errors:#?}");
}

/// The helper is the shared Ruby runtime's, which no strict target loads.
#[test]
fn errors_to_hash_is_a_gap_on_strict_targets() {
    let (_emitted, errors) = errors_to_hash_app("%{attribute} %{message}").emit(roundhouse::project::BuildTarget::Crystal);
    assert_eq!(
        errors.iter().filter(|e| e.contains("ActiveSupport.errors_to_hash is only available on the ruby family and spinel")).count(),
        2,
        "{errors:#?}"
    );
}

/// The membership questions read `error_messages_for`, which every target has; the Hash and the names do not reach a strict target.
#[test]
fn errors_readers_beyond_membership_are_a_gap_on_strict_targets() {
    let (_emitted, errors) = errors_readers_app().emit(roundhouse::project::BuildTarget::Crystal);
    let gated = |helper: &str| {
        errors.iter().filter(|e| e.contains(&format!("ActiveSupport.{helper} is only available on the ruby family and spinel"))).count()
    };
    assert_eq!((gated("errors_to_hash"), gated("errors_messages"), gated("errors_attribute_names")), (2, 2, 1), "{errors:#?}");
    assert_eq!(errors.len(), 5, "{errors:#?}");
}

/// A record built for its `errors` is still built: Rails runs
/// `Article.create!` before it answers the full message.
fn full_message_on_a_built_record_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog().edit(
        "app/models/article.rb",
        "  validates :title, presence: true\n",
        "  validates :title, presence: true

  def self.taken_message(reason)
    error = Article.create!(title: \"Built\", body: \"A long enough body\").errors.full_message(:title, reason.strip)
    skipped = reason.empty? || Article.create!(title: \"Skipped\", body: \"A long enough body\").errors.full_message(:base, \"is skipped\")
    [error, skipped, Article.new.errors.full_message(:base, \"is odd\")]
  end
",
    )
}

#[test]
fn full_message_on_a_built_record_builds_it() {
    full_message_on_a_built_record_app()
        .run_ruby(
            r#"messages = Article.taken_message(" is taken ")
raise messages.inspect unless messages == ["Title is taken", "is skipped", "is odd"]
messages = Article.taken_message("")
raise messages.inspect unless messages == ["Title ", true, "is odd"]
raise Article.count.inspect unless Article.where(title: "Built").count == 2 && Article.where(title: "Skipped").count == 1
puts "full message on a built record passed"
"#,
        )
        .assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn full_message_on_a_built_record_builds_it_on_spinel() {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "  validates :title, presence: true\n",
            "  validates :title, presence: true

  def self.odd_message(reason)
    reason.empty? || Article.new.errors.full_message(:title, reason.strip)
  end
",
        )
        .run_spinel(
            r#"raise Article.odd_message(" is odd ").inspect unless Article.odd_message(" is odd ") == "Title is odd"
raise Article.odd_message("").inspect unless Article.odd_message("") == true
puts "full message on a built record passed"
"#,
        )
        .assert_passes();
}

/// A field the lowering cannot humanize at compile time keeps its error.
#[test]
fn errors_full_message_on_a_dynamic_field_stays_an_error() {
    let (_emitted, errors) = emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "  validates :title, presence: true\n",
            "  validates :title, presence: true

  def message_for(field)
    errors.full_message(field, \"is taken\")
  end
",
        )
        .emit(roundhouse::project::BuildTarget::Ruby);
    assert!(errors.iter().any(|e| e.contains("full_message lacks")), "{errors:?}");
}

/// A Sidekiq worker's class-side entries run its `perform` inline, as an
/// ActiveJob's `perform_later` does: `include Sidekiq::Job` (or `Worker`)
/// and `sidekiq_options` leave the emitted class, `perform_in` /
/// `perform_at` drop their delay, and a subclass of a worker is one too.
#[test]
fn sidekiq_workers_perform_inline() {
    let run = emit_and_run::real_blog()
        .write(
            "app/workers/title_worker.rb",
            "class TitleWorker
  include Sidekiq::Job
  sidekiq_options queue: :low_priority, retry: 3

  def perform(title)
    Article.create!(title: title, body: \"A sufficiently long body.\")
  end
end
",
        )
        .write(
            "app/workers/loud_title_worker.rb",
            "class LoudTitleWorker < TitleWorker
  def perform(title)
    Article.create!(title: title.upcase, body: \"A sufficiently long body.\")
  end
end
",
        )
        .write(
            "app/workers/legacy_title_worker.rb",
            "class LegacyTitleWorker
  include Sidekiq::Worker

  def perform(title)
    Article.create!(title: title + \"?\", body: \"A sufficiently long body.\")
  end
end
",
        )
        .edit(
            "app/models/article.rb",
            "  validates :title, presence: true\n",
            "  validates :title, presence: true

  def self.enqueue_all
    TitleWorker.perform_async(\"now\")
    TitleWorker.perform_in(5.minutes, \"later\")
    LoudTitleWorker.perform_async(\"loud\")
    LegacyTitleWorker.perform_at(1.hour.from_now, \"legacy\")
  end
",
        )
        .run_ruby(
            r#"Article.enqueue_all
titles = Article.all.map(&:title).sort
raise "titles: #{titles.inspect}" unless titles == ["LOUD", "later", "legacy?", "now"]
puts "sidekiq passed"
"#,
        );
    run.assert_passes();
    assert!(run.stdout.contains("sidekiq passed"));
}

/// An abstract base worker with no `perform` anywhere in its own chain
/// (just the Sidekiq include, `sidekiq_options`, and a private helper)
/// isn't an entry point: it gets no wrapper and raises no analysis
/// error, and its concrete subclass's own positional `perform` is
/// still wrapped and still runs `perform_async` inline.
#[test]
fn an_abstract_base_worker_with_no_perform_is_not_an_entry_point() {
    let run = emit_and_run::real_blog()
        .write(
            "app/workers/base_worker.rb",
            "class BaseWorker
  include Sidekiq::Worker
  sidekiq_options queue: \"default\"

  private

  def helper
    \"unused\"
  end
end
",
        )
        .write(
            "app/workers/concrete_worker.rb",
            "class ConcreteWorker < BaseWorker
  def perform(title)
    Article.create!(title: title, body: \"A sufficiently long body.\")
  end
end
",
        )
        .run_ruby(
            r#"ConcreteWorker.perform_async("abstract base worked")
titles = Article.all.map(&:title)
raise "titles: #{titles.inspect}" unless titles.include?("abstract base worked")
puts "abstract base worker passed"
"#,
        );
    run.assert_passes();
    assert!(run.stdout.contains("abstract base worker passed"));
}

/// A class object and its instances that define the same names: each
/// side's call types and runs as that side's method.
#[test]
fn same_named_class_and_instance_methods_run_on_their_own_side() {
    let run = emit_and_run::real_blog()
        .write(
            "app/models/greeter.rb",
            "class Greeter
  def self.call(name)
    new(name).call
  end

  def initialize(name)
    @name = name
  end

  def call
    \"hello \" + @name
  end
end
",
        )
        .edit(
            "app/models/article.rb",
            "  validates :title, presence: true\n",
            "  validates :title, presence: true

  def self.headline
    [:class]
  end

  def headline
    title.to_s
  end

  def shout
    headline.upcase
  end
",
        )
        .run_ruby(
            "raise \"class call\" unless Greeter.call(\"a\") == \"hello a\"
raise \"instance call\" unless Greeter.new(\"b\").call == \"hello b\"
raise \"class headline\" unless Article.headline == [:class]
raise \"instance headline\" unless Article.new(title: \"hi\").shout == \"HI\"
puts \"same-named sides passed\"
",
        );
    run.assert_passes();
    assert!(run.stdout.contains("same-named sides passed"));
}

#[path = "support/engine_mount.rs"]
mod engine_mount;

#[test]
fn literal_isolated_engine_mount_runs_over_cruby_http() {
    let run = engine_mount::overlay().run_ruby(engine_mount::CRUBY_ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("literal engine mount CRuby HTTP contract passed"));
}

#[test]
fn engine_helper_calls_are_explicit_instead_of_using_host_helpers() {
    let controller = r#"module Catalog
  class ProductsController < ActionController::Base
    def index
      render plain: products_path
    end
  end
end
"#;
    let (_emitted, app, errors) = engine_mount::overlay()
        .write("vendor/catalog/app/controllers/catalog/products_controller.rb", controller)
        .emit_with_app(roundhouse::project::BuildTarget::Ruby);
    assert!(
        engine_mount::has_error_in_source(&app, &errors, "vendor/catalog/app/controllers/catalog/products_controller.rb", "products_path"),
        "engine helper collision was not explicit:\n{}",
        engine_mount::describe_errors(&app, &errors)
    );
}

#[test]
fn reflective_engine_helpers_and_proxy_calls_are_explicit() {
    let engine_controller = r#"module Catalog
  class ProductsController < ActionController::Base
    def reflective_symbol
      send(:products_path)
    end

    def reflective_string
      public_send("items_path")
    end
  end
end
"#;
    let host_controller = r#"class HostController < ApplicationController
  def host_products_reflection
    public_send(:products_path)
  end

  def reflective_mount_helper
    self.send(:catalog_path)
  end

  def reflective_symbol_proxy
    catalog.send(:products_path)
  end

  def reflective_string_proxy
    self.catalog.public_send("items_path")
  end

  def reflective_private_proxy
    self.catalog.__send__("products_path")
  end

  def reflective_proxy_then_direct_helper
    public_send(:catalog).items_path
  end

  def reflective_proxy_and_helper_chain
    self.public_send("catalog").public_send(:items_path)
  end

  def reflective_send_proxy_and_helper
    send(:catalog).send("products_path")
  end
end
"#;
    let engine_routes = "Catalog::Engine.routes.draw do\n  get \"/products\", to: \"products#index\"\n  get \"/items\", to: \"products#index\", as: :items\nend\n";
    let host_routes = "Rails.application.routes.draw do\n  mount Catalog::Engine, at: \"/catalog\"\n  get \"/host/products\", to: \"host#reflective_symbol_proxy\", as: :products\nend\n";
    let (_emitted, app, errors) = engine_mount::overlay()
        .write("vendor/catalog/app/controllers/catalog/products_controller.rb", engine_controller)
        .write("vendor/catalog/config/routes.rb", engine_routes)
        .write("app/controllers/host_controller.rb", host_controller)
        .write("config/routes.rb", host_routes)
        .emit_with_app(roundhouse::project::BuildTarget::Ruby);
    for (path, needle) in [
        ("vendor/catalog/app/controllers/catalog/products_controller.rb", "send(:products_path)"),
        ("vendor/catalog/app/controllers/catalog/products_controller.rb", "public_send(\"items_path\")"),
        ("app/controllers/host_controller.rb", "catalog.send(:products_path)"),
        ("app/controllers/host_controller.rb", "self.catalog.public_send(\"items_path\")"),
        ("app/controllers/host_controller.rb", "self.catalog.__send__(\"products_path\")"),
        ("app/controllers/host_controller.rb", "public_send(:catalog).items_path"),
        ("app/controllers/host_controller.rb", "self.public_send(\"catalog\").public_send(:items_path)"),
        ("app/controllers/host_controller.rb", "send(:catalog).send(\"products_path\")"),
        ("app/controllers/host_controller.rb", "self.send(:catalog_path)"),
    ] {
        assert!(
            engine_mount::has_error_on_source_line(&app, &errors, path, needle),
            "reflective helper use {needle} in {path} was not a located mount error:\n{}",
            engine_mount::describe_errors(&app, &errors)
        );
    }
    assert!(
        !engine_mount::has_error_on_source_line(
            &app,
            &errors,
            "app/controllers/host_controller.rb",
            "public_send(:products_path)"
        ),
        "host-authored products_path reflection was rejected:\n{}",
        engine_mount::describe_errors(&app, &errors)
    );
}

#[test]
fn host_engine_helper_proxy_calls_are_explicit() {
    let host = r#"class HostController < ApplicationController
  def index
    render plain: "host root"
  end

  def root_helper
    render plain: root_path
  end

  def products_helper
    render plain: products_path
  end

  def engine_proxy
    render plain: catalog.products_path
  end

  def self_engine_proxy
    render plain: self.catalog.items_path
  end

  def after
    render plain: "host after"
  end
end
"#;
    let (_emitted, app, errors) = engine_mount::overlay()
        .write("app/controllers/host_controller.rb", host)
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\n  root \"host#index\"\n  get \"/root-helper\", to: \"host#root_helper\"\n  get \"/products-helper\", to: \"host#products_helper\"\n  get \"/engine-proxy\", to: \"host#engine_proxy\"\n  get \"/self-engine-proxy\", to: \"host#self_engine_proxy\"\n  get \"/products\", to: \"products#index\", as: :products\n  get \"/catalog/products\", to: \"products#index\", as: :host_catalog_products\n  mount Catalog::Engine, at: \"/catalog\"\n  get \"/catalog/items\", to: \"items#index\", as: :host_catalog_items\n  get \"/after\", to: \"host#after\"\nend\n",
        )
        .emit_with_app(roundhouse::project::BuildTarget::Ruby);
    assert!(
        engine_mount::has_error_in_source(&app, &errors, "app/controllers/host_controller.rb", "products_path"),
        "mounted engine proxy calls were not explicit:\n{}",
        engine_mount::describe_errors(&app, &errors)
    );
    assert!(
        engine_mount::has_error_on_source_line(&app, &errors, "app/controllers/host_controller.rb", "self.catalog.items_path"),
        "self.catalog.items_path receiver was not rejected:\n{}",
        engine_mount::describe_errors(&app, &errors)
    );
}

#[test]
fn host_root_helper_is_kept_while_engine_root_helper_is_an_error() {
    let home = r#"module Catalog
  class HomeController < ActionController::Base
    def index
      render plain: root_path
    end
  end
end
"#;
    let (_emitted, app, errors) = engine_mount::overlay()
        .write("vendor/catalog/app/controllers/catalog/home_controller.rb", home)
        .emit_with_app(roundhouse::project::BuildTarget::Ruby);
    assert!(
        engine_mount::has_error_in_source(&app, &errors, "vendor/catalog/app/controllers/catalog/home_controller.rb", "root_path"),
        "engine root helper did not retain an explicit error:\n{}",
        engine_mount::describe_errors(&app, &errors)
    );
    assert!(
        !engine_mount::has_error_in_source(&app, &errors, "app/controllers/host_controller.rb", "root_path"),
        "the legitimate host root_path call was rejected: {errors:?}"
    );
}

#[test]
fn isolated_engine_paths_do_not_create_or_shadow_host_helpers() {
    let host = r#"class HostController < ApplicationController
  def index
    render plain: "host root"
  end

  def fake_engine_helper
    render plain: "host action"
  end
end
"#;
    let (emitted, app, errors) = engine_mount::overlay()
        .write("app/controllers/host_controller.rb", host)
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\n  root \"host#index\"\n  mount Catalog::Engine, at: \"/catalog\"\n  get \"/host/catalog/products\", to: \"host#fake_engine_helper\", as: :catalog_products\nend\n",
        )
        .emit_with_app(roundhouse::project::BuildTarget::Ruby);
    assert!(
        errors.is_empty(),
        "literal engine mount reported unexpected errors:\n{}",
        engine_mount::describe_errors(&app, &errors)
    );
    let helpers = std::fs::read_to_string(emitted.join("app/route_helpers.rb")).expect("route helpers");
    assert_eq!(
        helpers.matches("def self.catalog_products_path").count(),
        1,
        "the host-authored route helper must remain available exactly once: {helpers}"
    );
    assert!(
        helpers.contains("\"/host/catalog/products\""),
        "catalog_products_path must retain the host route's path: {helpers}"
    );
    assert!(
        !helpers.contains("def self.catalog_items_path"),
        "the isolated engine's catalog_items_path leaked into host helpers: {helpers}"
    );
}

#[test]
fn helper_proxy_uses_in_library_jbuilder_and_configured_test_sources_are_explicit() {
    let sources = [
        (
            "lib/engine_proxy_reference.rb",
            "class EngineProxyReference\n  def self.path\n    catalog.products_path\n  end\nend\n",
        ),
        (
            "app/views/host/index.json.jbuilder",
            "json.url catalog.products_path\n",
        ),
        (
            "spec/engine_proxy_reference_spec.rb",
            "class EngineProxyReferenceSpec\n  def path\n    catalog.products_path\n  end\nend\n",
        ),
        (
            "vendor/catalog/lib/catalog/path_reference.rb",
            r#"module Catalog
  class PathReference
    def path
      products_path
    end

    def self.engine_home
      products_path
    end

    class Engine
      def self.reference
        "plain nested helper class"
      end
    end
  end
end
"#,
        ),
    ];
    for (path, source) in sources {
        let mut overlay = engine_mount::overlay().write(path, source);
        if path.starts_with("spec/") {
            overlay = overlay.write("roundhouse.yml", "test_paths:\n  - spec\n");
        }
        let (_emitted, app, errors) = overlay.emit_with_app(roundhouse::project::BuildTarget::Ruby);
        assert!(
            engine_mount::has_error_in_source(&app, &errors, path, "products_path"),
            "route helper call in {path} was not an explicit mount gap:\n{}",
            engine_mount::describe_errors(&app, &errors)
        );
    }
}

#[test]
fn engine_haml_helper_calls_keep_their_source_boundary() {
    let path = "vendor/catalog/app/views/home/index.html.haml";
    let (_emitted, app, errors) = engine_mount::overlay()
        .write(path, "= root_path\n")
        .emit_with_app(roundhouse::project::BuildTarget::Ruby);
    assert!(
        engine_mount::has_error_in_source(&app, &errors, path, "root_path"),
        "bare engine helper in HAML was not rejected:\n{}",
        engine_mount::describe_errors(&app, &errors)
    );
}

#[test]
fn dynamic_engine_route_targets_keep_their_source_boundary() {
    let path = "vendor/catalog/config/routes.rb";
    let (_emitted, app, errors) = engine_mount::overlay()
        .write(
            path,
            "Catalog::Engine.routes.draw do\n  root to: redirect(root_path)\n  get \"/products\", to: \"products#index\"\nend\n",
        )
        .emit_with_app(roundhouse::project::BuildTarget::Ruby);
    assert!(
        engine_mount::has_error_in_source(&app, &errors, path, "dynamic targets"),
        "dynamic engine route target was not rejected:\n{}",
        engine_mount::describe_errors(&app, &errors)
    );
}

#[path = "emit_and_run/integer_query_exists.rs"]
mod integer_query_exists;

#[test]
fn critic_corrections_preserve_class_objects_reflection_and_operators() {
    emit_and_run::real_blog()
        .write("app/helpers/protocol_control.rb", r#"class FirstOperand
  def +(other)
    other + 41
  end
end
class SecondOperand
  def +(other)
    other + 42
  end
end
class ProtocolControl
  def initialize
    @value = 7
  end
  def self.implicit_eval
    class_eval { 31 }
  end
  def union_operator_control(flag)
    value = flag ? FirstOperand.new : SecondOperand.new
    value + 1
  end
  def reflection
    instance_variable_get(:@value)
  end
  def include?(value)
    value == 3
  end
  def +(other)
    @value + other
  end
def reflective_override(value)
  value
end
def override_control
  ProtocolControl.new.reflective_override("ok").upcase
end
def operator_control
    ProtocolControl.new + 2
  end
  def install
    klass = ProtocolControl
    alias_klass = klass
    alias_klass.define_method(:installed) { 19 }
    ProtocolControl.new.installed
  end
end
"#)
        .run_ruby(r#"
control = ProtocolControl.new
raise "implicit class identity lost" unless ProtocolControl.implicit_eval == 31
raise "first union operator lost" unless control.union_operator_control(true) == 42
raise "second union operator lost" unless control.union_operator_control(false) == 43
raise "reflection changed" unless control.reflection == 7
raise "override changed" unless control.include?(3)
raise "app return inference changed" unless control.override_control == "OK"
raise "operator changed" unless control.operator_control == 9
raise "class alias identity lost" unless control.install == 19
raise "JSON support changed" unless control.to_json.is_a?(String)
puts "critic positive controls passed"
"#)
        .assert_passes();
}

#[test]
fn critic_corrections_preserve_generated_model_narrowing() {
    emit_and_run::real_blog()
        .edit("app/models/article.rb", "class Article < ApplicationRecord", "class Article < ApplicationRecord\n  def critic_item\n    self\n  end")
        .write("app/views/articles/_critic_narrow.html.erb", "<% case article.critic_item %>\n<% when Article %>\n<%= link_to 'narrowed', article.critic_item %>\n<% end %>\n")
        .run_ruby("article = Article.new(id: 7, title: 'narrowed title'); raise 'model narrowing changed' unless Views::Articles.critic_narrow(article).include?('/articles/7')")
        .assert_passes();
}

#[test]
fn critic_corrections_preserve_native_module_callback_identity() {
    emit_and_run::real_blog()
        .write("app/models/concerns/native_hook.rb", "module NativeHook\n  def self.included(base)\n    base.define_method(:hook_value) { 23 }\n  end\nend\n")
        .edit("app/models/article.rb", "class Article < ApplicationRecord", "class Article < ApplicationRecord\n  include NativeHook")
        .run_ruby("raise 'native callback identity lost' unless Article.new.hook_value == 23")
        .assert_passes();
}

#[test]
fn critic_corrections_preserve_errors_message_projections() {
    emit_and_run::real_blog()
        .edit("app/models/article.rb", "class Article < ApplicationRecord", "class Article < ApplicationRecord\n  def critic_title_messages\n    errors[:title]\n  end\n  def critic_full_messages\n    errors.full_messages\n  end")
        .run_ruby(r#"
article = Article.new
article.valid?
raise "message indexing changed" unless article.critic_title_messages.include?("can't be blank")
raise "full messages changed" unless article.critic_full_messages.any?
puts "error message runtime controls passed"
"#)
        .assert_passes();
}

#[test]
fn defined_method_operands_are_not_invoked() {
    emit_and_run::empty_app()
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"probes\" do |t|\n    t.string \"name\"\n  end\nend\n")
        .write("app/helpers/defined_probe.rb", r#"class DefinedProbe
  def present
    raise "defined? invoked its operand"
  end

  def present_definition
    defined?(self.present)
  end

  def missing_definition
    defined?(self.missing)
  end
end
"#)
        .run_ruby(r#"
probe = DefinedProbe.new
raise "existing method lost" unless probe.present_definition == "method"
raise "missing method admitted" unless probe.missing_definition.nil?
puts "defined? did not invoke either operand"
"#)
        .assert_passes();
}

/// Build each query case independently: declaring a model class method
/// must not accidentally open the old gate for the order/where.not cases.
fn scope_free_query_app(action: &str) -> emit_and_run::Overlay {
    let (model, query) = match action {
        "index" => ("class Widget < ApplicationRecord\nend\n", "Widget.order(:name).limit(1)"),
        "named" => ("class Widget < ApplicationRecord\nend\n", "Widget.where.not(name: nil).order(:name)"),
        "recent" => (
            "class Widget < ApplicationRecord\n  def self.recent\n    order(:name).limit(1)\n  end\nend\n",
            "Widget.recent",
        ),
        _ => panic!("unknown scope-free query action: {action}"),
    };
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("db/schema.rb", r#"ActiveRecord::Schema.define do
  create_table "widgets", force: :cascade do |t|
    t.string "name"
  end
end
"#)
        .write("app/models/widget.rb", model)
        .write("config/routes.rb", &format!(
            "Rails.application.routes.draw do\n  get \"/widgets\", to: \"widgets#{action}\"\nend\n"
        ))
        .write("app/controllers/widgets_controller.rb", &format!(
            "class WidgetsController < ApplicationController\n  def {action}\n    render plain: {query}.map {{ |w| w.name }}.join(\",\")\n  end\nend\n"
        ))
}

/// Exercise one action independently, so an order failure cannot mask
/// where.not or the model class method's implicit-self query root.
fn scope_free_query_assertions(action: &str, expected: &str) -> String {
    let nullable_row = if action == "named" { "Widget.create!(name: nil)" } else { "" };
    format!(r#"
require_relative "app/controllers/widgets_controller"
Widget.create!(name: "beta")
Widget.create!(name: "alpha")
{nullable_row}
controller = WidgetsController.new
controller.process_action(:{action})
raise "{action} lost its relation: #{{controller.body}}" unless controller.body == "{expected}"
puts "scope-free {action} passed"
"#)
}

/// Class-root order needs a Relation even without any declared scope.
#[test]
fn scope_free_model_order_runs() {
    scope_free_query_app("index")
        .run_ruby(&scope_free_query_assertions("index", "alpha"))
        .assert_passes();
}

/// Zero-argument where reaches WhereChain independently of the order case.
#[test]
fn scope_free_model_where_not_runs() {
    scope_free_query_app("named")
        .run_ruby(&scope_free_query_assertions("named", "alpha,beta"))
        .assert_passes();
}

/// A model class method's bare order root remains supported without named
/// scopes; this control is independent of other class-root query demands.
#[test]
fn scope_free_model_bare_root_class_method_runs() {
    scope_free_query_app("recent")
        .run_ruby(&scope_free_query_assertions("recent", "alpha"))
        .assert_passes();
}

/// Compile and execute the same three controller actions after booting
/// their emitted in-memory SQLite database.
#[test]
#[ignore = "requires the Spinel toolchain"]
fn scope_free_model_query_builders_run_on_spinel() {
    for (action, expected) in [("index", "alpha"), ("named", "alpha,beta"), ("recent", "alpha")] {
        let script = format!(
            "Db.configure(\":memory:\")\nSchema.statements.each {{ |sql| Db.exec(sql) }}\nActiveRecord.adapter = SqliteAdapter\n{}",
            scope_free_query_assertions(action, expected)
        );
        scope_free_query_app(action).run_spinel(&script).assert_passes();
    }
}

/// The runtime-provided cable mount survives strict analysis and Rack dispatch.
#[test]
fn builtin_cable_mount_runs_with_supported_sibling_routes() {
    emit_and_run::real_blog()
        .write("app/channels/application_cable/connection.rb", "module ApplicationCable\n  class Connection < ActionCable::Connection::Base\n    def connect\n      reject_unauthorized_connection\n    end\n  end\nend\n")
        .write("app/controllers/widgets_controller.rb", "class WidgetsController < ActionController::Base\n  def index\n    render plain: 'widgets'\n  end\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\n  get '/widgets', to: 'widgets#index'\n  mount ActionCable.server => '/cable'\nend\n")
        .run_ruby(r##"
require "rack"
require "rack/mock"
# As in overlay_cable_identity, leave the unused reactor dependencies empty.
# The actual app authorization must refuse before any reactor API is called.
%w[nio websocket/driver].each { |feature| $LOADED_FEATURES << "#{feature}.rb" }
module NIO; end
module WebSocket; end
builder = Rack::Builder.new
builder.instance_eval(File.read("config.ru"), "config.ru")
client = Rack::MockRequest.new(builder.to_app)
widgets = client.get("/widgets")
raise "sibling route failed: #{widgets.status} #{widgets.body}" unless widgets.status == 200 && widgets.body == "widgets"
cable = client.get("/cable", "HTTP_HOST" => "example.test", "HTTP_ORIGIN" => "http://example.test")
raise "cable endpoint disappeared: #{cable.status} #{cable.body}" unless cable.status == 401 && cable.body == "Unauthorized\n"
puts "runtime cable endpoint preserved"
"##)
        .assert_passes();
}

/// config.ru serves public/ ahead of the routes, from the file list it
/// takes at boot: nested files, HEAD, a 304 for a repeated
/// Last-Modified; a POST to a public path and a path that is not a file
/// reach the app; `..` reaches nothing outside public/.
#[test]
fn public_files_are_served_from_the_boot_listing() {
    emit_and_run::real_blog()
        .write("public/robots.txt", "User-agent: *\n")
        .write("public/nested/deep/note.txt", "deep\n")
        .run_ruby(r##"
require "rack"
require "rack/mock"
%w[nio websocket/driver].each { |feature| $LOADED_FEATURES << "#{feature}.rb" }
module NIO; end
module WebSocket; end
builder = Rack::Builder.new
builder.instance_eval(File.read("config.ru"), "config.ru")
client = Rack::MockRequest.new(builder.to_app)
check = ->(cond, what) { raise "FAILED: #{what}" unless cond }

r = client.get("/robots.txt")
check.(r.status == 200 && r.body == "User-agent: *\n", "a public file is served: #{r.status}")
check.(client.get("/nested/deep/note.txt").body == "deep\n", "a nested public file is served")
h = client.request("HEAD", "/robots.txt")
check.(h.status == 200 && h.body.empty?, "HEAD of a public file has no body")
m = client.get("/robots.txt", "HTTP_IF_MODIFIED_SINCE" => r.headers["last-modified"])
check.(m.status == 304, "a repeated Last-Modified is a 304: #{m.status}")
p = client.post("/robots.txt")
check.(p.status != 405 && p.body != "User-agent: *\n", "a POST to a public path reaches the app: #{p.status}")
check.(client.get("/nested").status != 200 || client.get("/nested").body != "deep\n", "a directory is not a file")
check.(client.get("/missing.txt").status == 404, "a path that is no file reaches the app's 404")
t = client.get("/../Gemfile")
check.(!t.body.include?("source"), "`..` serves nothing outside public/")
puts "public files contract passed"
"##)
        .assert_passes();
}

#[test]
fn class_configuration_shares_a_name_with_an_instance_method() {
    let run = class_configuration::same_name_overlay().run_ruby(class_configuration::SAME_NAME_ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("class and instance configuration names passed"));
}

#[test]
fn finite_concern_class_configuration_runs_without_replaying_rails() {
    for (overlay, assertions) in [
        (class_configuration::overlay(), class_configuration::ASSERTIONS),
        (class_configuration::empty_overlay(), class_configuration::EMPTY_ASSERTIONS),
    ] {
        let run = overlay.run_ruby(assertions);
        run.assert_passes();
        assert!(run.stdout.contains("finite class configuration contract passed"));
    }
}

/// `Dry::Struct` classes construct, coerce and refuse as dry-struct does,
/// with no dry-struct in the emitted tree.
#[test]
fn dry_struct_classes_run_lowered() {
    let run = dry_struct::ruby_overlay().run_ruby(&format!(
        "{}\n{}\n{}",
        dry_struct::ASSERTIONS,
        dry_struct::STAMP_ASSERTIONS,
        dry_struct::CLOCK_ASSERTIONS
    ));
    run.assert_passes();
    assert!(run.stdout.contains("dry-struct contract passed"));
    assert!(run.stdout.contains("dry-struct stamp contract passed"));
    assert!(run.stdout.contains("dry-struct clock contract passed"));
}

/// Oj under `Oj.mimic_JSON` + `Oj.optimize_rails` (the initializer a Rails
/// app writes): generate as `JSON.generate` (`<>&` raw, `Infinity` and
/// `NaN` a `JSON::GeneratorError`), load as
/// `JSON.parse` (nil and a non-String as given, `symbol_keys:`), and a
/// parse failure a `JSON::ParserError` that `Oj::ParseError` does not
/// catch. Checked against oj 3.17 with ActiveSupport.
#[test]
fn oj_under_mimic_json_runs_as_json() {
    let run = oj_overlay(emit_and_run::real_blog()).run_ruby(OJ_ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("oj passed"));
}

/// A call the stand-in does not model is a refusal of its own, ledgered
/// (strict ingest fails on it): `Oj.dump` (its compat and rails encoding
/// both unmodeled), `Oj.safe_load` (the strict parser). In survey mode
/// `Oj.load` beside it is still lowered.
#[test]
fn oj_unmodeled_calls_are_refused_one_by_one() {
    for (call, expected) in [
        ("Oj.dump(v)", "`Oj.dump`: neither its compat nor its rails encoding is modeled"),
        ("Oj.dump(v, mode: :rails)", "`Oj.dump`: neither its compat nor its rails encoding is modeled"),
        ("Oj.safe_load(v)", "`Oj.safe_load`"),
    ] {
        let tree: std::collections::HashMap<std::path::PathBuf, Vec<u8>> = [
            ("config/initializers/oj.rb".to_string(), "Oj.mimic_JSON\n".to_string()),
            (
                "lib/codec.rb".to_string(),
                format!("class Codec\n  def self.out(v)\n    {call}\n  end\n\n  def self.in(v)\n    Oj.load(v)\n  end\nend\n"),
            ),
            ("config/routes.rb".to_string(), "Rails.application.routes.draw do\nend\n".to_string()),
        ]
        .into_iter()
        .map(|(p, s)| (std::path::PathBuf::from(p), s.into_bytes()))
        .collect();
        assert!(roundhouse::ingest::ingest_app_from_tree(tree.clone()).is_err(), "{call}: strict ingest passed");
        roundhouse::ingest::survey::activate();
        let app = roundhouse::ingest::ingest_app_from_tree(tree).expect("survey ingest");
        let ledger: Vec<String> = roundhouse::ingest::survey::drain().iter().map(|e| e.to_string()).collect();
        assert!(ledger.iter().any(|e| e.contains(expected)), "{call}: {ledger:#?}");
        let oj = app.library_classes.iter().find(|lc| lc.name.0.as_str() == "Oj").expect("load still lowered");
        assert!(oj.methods.iter().all(|m| !matches!(m.name.as_str(), "dump" | "safe_load")), "{call}: modeled");
    }
}

/// Without `Oj.mimic_JSON` Oj is in its `:object` mode, not modeled: its
/// uses stay errors.
#[test]
fn oj_without_mimic_json_stays_unsupported() {
    let (_emitted, errors) = emit_and_run::real_blog()
        .write("lib/payload.rb", OJ_LIB)
        .emit(roundhouse::project::BuildTarget::Ruby);
    assert!(errors.iter().any(|e| e.contains("Oj")), "{errors:#?}");
}

/// An option the lowering does not read is a refusal (ledgered, strict
/// ingest fails); a mode that may not be set, or `optimize_rails` alone,
/// leaves Oj unmodeled.
#[test]
fn oj_unmodeled_options_are_refused() {
    let tree: std::collections::HashMap<std::path::PathBuf, Vec<u8>> = [
        ("config/initializers/oj.rb", "Oj.mimic_JSON\n"),
        ("lib/pretty.rb", "class Pretty\n  def self.show(v)\n    Oj.load(v, bigdecimal_load: :float)\n  end\nend\n"),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ]
    .into_iter()
    .map(|(p, s)| (std::path::PathBuf::from(p), s.as_bytes().to_vec()))
    .collect();
    assert!(roundhouse::ingest::ingest_app_from_tree(tree.clone()).is_err(), "strict ingest passed a refusal");
    roundhouse::ingest::survey::activate();
    let app = roundhouse::ingest::ingest_app_from_tree(tree).expect("survey ingest");
    let ledger: Vec<String> = roundhouse::ingest::survey::drain().iter().map(|e| e.to_string()).collect();
    assert!(ledger.iter().any(|e| e.contains("Oj not lowered: `Oj.load` option `bigdecimal_load: :float`")), "{ledger:#?}");
    let _ = app;
    // A mode that may not be set or that this does not read leaves Oj
    // unmodeled; an option value oj rejects is a refusal of that call.
    for (init, use_, expected, unmodeled) in [
        ("if ENV[\"X\"]\n  Oj.mimic_JSON\nend\n", "Oj.load(v)", "`Oj.mimic_JSON` not at an initializer's top level", true),
        ("Oj.mimic_JSON if ENV[\"X\"]\n", "Oj.load(v)", "`Oj.mimic_JSON` not at an initializer's top level", true),
        ("Oj.mimic_JSON rescue nil\n", "Oj.load(v)", "`Oj.mimic_JSON` not at an initializer's top level", true),
        ("Oj.optimize_rails\n", "Oj.load(v)", "`Oj.optimize_rails` without `Oj.mimic_JSON`", true),
        ("Oj.mimic_JSON(mode: :compat)\n", "Oj.load(v)", "`Oj.mimic_JSON` not at an initializer's top level", true),
        ("Oj.mimic_JSON\n", "Oj.load(v, symbol_keys: \"true\")", "`Oj.load` option `symbol_keys: \"true\"`", false),
    ] {
        let tree: std::collections::HashMap<std::path::PathBuf, Vec<u8>> = [
            ("config/initializers/oj.rb".to_string(), init.to_string()),
            ("lib/reader.rb".to_string(), format!("class Reader\n  def self.read(v)\n    {use_}\n  end\nend\n")),
            ("config/routes.rb".to_string(), "Rails.application.routes.draw do\nend\n".to_string()),
        ]
        .into_iter()
        .map(|(p, s)| (std::path::PathBuf::from(p), s.into_bytes()))
        .collect();
        roundhouse::ingest::survey::activate();
        let app = roundhouse::ingest::ingest_app_from_tree(tree).expect("survey ingest");
        let ledger: Vec<String> = roundhouse::ingest::survey::drain().iter().map(|e| e.to_string()).collect();
        assert!(ledger.iter().any(|e| e.contains(expected)), "{init}: {ledger:#?}");
        if unmodeled {
            assert!(app.library_classes.iter().all(|lc| lc.name.0.as_str() != "Oj"), "{init}: lowered");
        }
    }
    // Empty parentheses are the same top-level call as the bare form.
    let tree: std::collections::HashMap<std::path::PathBuf, Vec<u8>> = [
        ("config/initializers/oj.rb".to_string(), "Oj.mimic_JSON()\n".to_string()),
        ("lib/reader.rb".to_string(), "class Reader\n  def self.read(v)\n    Oj.load(v)\n  end\nend\n".to_string()),
        ("config/routes.rb".to_string(), "Rails.application.routes.draw do\nend\n".to_string()),
    ]
    .into_iter()
    .map(|(p, s)| (std::path::PathBuf::from(p), s.into_bytes()))
    .collect();
    roundhouse::ingest::survey::activate();
    let app = roundhouse::ingest::ingest_app_from_tree(tree).expect("survey ingest");
    let ledger: Vec<String> = roundhouse::ingest::survey::drain().iter().map(|e| e.to_string()).collect();
    assert!(
        ledger.iter().all(|e| !e.contains("mimic_JSON")),
        "empty parentheses were refused: {ledger:#?}"
    );
    assert!(
        app.library_classes.iter().any(|lc| lc.name.0.as_str() == "Oj"),
        "Oj.mimic_JSON() did not install the stand-in"
    );
}

const OJ_LIB: &str = r##"class Payload
  def self.generated(value)
    Oj.generate(value)
  end

  def self.generated_or(value)
    Oj.generate(value)
  rescue JSON::GeneratorError
    :not_finite
  end

  def self.decode(text)
    Oj.load(text)
  end

  def self.decode_symbols(text)
    Oj.load(text, symbol_keys: true)
  end

  def self.guarded(text)
    Oj.load(text)
  rescue Oj::ParseError
    :oj_rescued
  rescue JSON::ParserError
    :json_rescued
  end

  def self.guarded_symbols(text)
    Oj.load(text, symbol_keys: true)
  rescue JSON::ParserError
    :bad
  end
end
"##;

const OJ_ASSERTIONS: &str = r##"require "stringio"
raise "generate" unless Payload.generated({a: :b, h: "<&>", n: [1.5, nil, true]}) == %({"a":"b","h":"<&>","n":[1.5,null,true]})
raise "generate infinity" unless Payload.generated_or({x: Float::INFINITY}) == :not_finite
raise "generate nan" unless Payload.generated_or({x: Float::NAN}) == :not_finite
raise "load" unless Payload.decode(%({"a":[1,2.5,null,true],"b":{"c":"d"}})) == {"a" => [1, 2.5, nil, true], "b" => {"c" => "d"}}
raise "load nil" unless Payload.decode(nil).nil?
raise "load integer" unless Payload.decode(5) == 5
raise "load symbols" unless Payload.decode_symbols(%({"a":{"b":1}})) == {a: {b: 1}}
raise "parse error" unless Payload.guarded("{x") == :json_rescued
raise "load array" unless Payload.decode([1]) == [1]
raise "load array nil" unless Payload.guarded([nil]) == :json_rescued
raise "load array symbol" unless Payload.guarded([:a]) == :json_rescued
raise "load array hash" unless Payload.guarded([{"a" => 1}]) == :json_rescued
raise "load array symbols" unless Payload.decode_symbols(%([{"a":1}])) == [{a: 1}]
raise "load bad array symbols" unless Payload.guarded_symbols([{"a" => 1}]) == :bad
raise "load rational" unless Payload.guarded(Rational(1, 2)) == :json_rescued
raise "load infinity" unless Payload.decode(Float::INFINITY).infinite?
raise "load nan" unless Payload.decode(Float::NAN).nan?
require "bigdecimal"
raise "load bigdecimal" unless Payload.decode(BigDecimal("2.5")) == 2.5
raise "load bigdecimal class" unless Payload.decode(BigDecimal("2.5")).is_a?(Float)
raise "load hash" unless Payload.guarded({"a" => 1}) == :json_rescued
raise "load symbol" unless Payload.guarded(:x) == :json_rescued
raise "load io" unless Payload.decode(StringIO.new(%([1,{"a":null}]))) == [1, {"a" => nil}]
puts "oj passed"
"##;

fn oj_overlay(overlay: emit_and_run::Overlay) -> emit_and_run::Overlay {
    overlay
        .write("config/initializers/oj.rb", "# frozen_string_literal: true\n\nOj.mimic_JSON\nOj.optimize_rails\n")
        .write("lib/payload.rb", OJ_LIB)
}

/// The stand-in is one Ruby class. Ruby emits it and stays clean. Python,
/// Crystal, TypeScript and the other strict targets refuse the call in one
/// place: no `Oj.load` / `Oj.generate` written against a missing class, and
/// not the incidental `String#read` or missing-`JSON` errors the class body
/// would raise.
#[test]
fn oj_stand_in_is_ruby_only() {
    let dir = std::env::temp_dir().join(format!("rh-oj-targets-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for (path, text) in [
        ("config/initializers/oj.rb", "Oj.mimic_JSON\nOj.optimize_rails\n"),
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"posts\", force: :cascade do |t|\n    t.string \"title\"\n    t.bigint \"author_id\"\n  end\n  create_table \"comments\", force: :cascade do |t|\n    t.text \"body\"\n    t.bigint \"post_id\"\n  end\nend\n",
        ),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"/posts\", to: \"posts#index\"\n  direct :payload do\n    Oj.load('{\"a\":1}', symbol_keys: true)\n  end\nend\n",
        ),
        (
            "app/models/post.rb",
            "class Post < ApplicationRecord\n  has_many :comments, -> { where(body: Oj.load('{\"a\":1}')) }\n  has_one :pin, -> { where(body: Oj.load('{\"b\":2}')) }, class_name: \"Comment\"\n  belongs_to :author, class_name: \"Post\", optional: true, default: -> { Oj.generate({a: \"<&>\"}) }\nend\n",
        ),
        (
            "app/models/comment.rb",
            "class Comment < ApplicationRecord\n  belongs_to :post\nend\n",
        ),
        (
            "app/controllers/posts_controller.rb",
            "class PostsController < ApplicationController\n  def index\n    Oj.generate({a: \"<&>\"})\n    Oj.load('{\"a\":1}', symbol_keys: true)\n    render plain: \"ok\"\n  end\n\n  def self.only_rescue\n    raise \"x\"\n  rescue Oj::ParseError\n    :oj\n  end\nend\n",
        ),
        (
            "test/models/post_test.rb",
            "class PostTest < ActiveSupport::TestCase\n  PAYLOAD = Oj.generate({a: \"<&>\"})\n  class Inner\n    def self.read\n      Oj.load('{\"a\":1}', symbol_keys: true)\n    end\n  end\nend\n",
        ),
        (
            "test/fixtures/posts.yml",
            "one:\n  title: <%= Oj.generate({a: 1}) %>\n",
        ),
    ] {
        let full = dir.join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, text).unwrap();
    }
    let mut app = roundhouse::ingest::ingest_app(&dir).expect("ingest");
    let lower_diags = roundhouse::session::analyze_and_lower(&mut app);
    assert!(
        lower_diags.iter().all(|d| d.severity != roundhouse::diagnostic::Severity::Error),
        "ingest lowered with errors: {lower_diags:?}"
    );
    let ruby = oj_target_errors_and_text(&app, &dir, roundhouse::project::BuildTarget::Ruby);
    assert!(ruby.0.is_empty(), "ruby refused a stand-in it emits:\n{}", ruby.0.join("\n"));
    let stand_in = ruby.1.get("app/models/oj.rb").map(String::as_str).unwrap_or("");
    assert!(stand_in.contains("def self.load"), "ruby emit dropped the stand-in:\n{stand_in}");
    assert!(stand_in.contains("JSON.parse(source)"), "ruby stand-in is not the one-argument parse:\n{stand_in}");
    assert!(stand_in.contains("json.is_a?(Integer)"), "ruby stand-in still passes every Numeric through:\n{stand_in}");
    assert!(!stand_in.contains("json.is_a?(Array)"), "ruby stand-in still returns every Array:\n{stand_in}");
    assert!(!stand_in.contains("def self.dump"), "ruby stand-in gained dump:\n{stand_in}");
    for target in [
        roundhouse::project::BuildTarget::Python,
        roundhouse::project::BuildTarget::Crystal,
        roundhouse::project::BuildTarget::Typescript,
        roundhouse::project::BuildTarget::Rust,
        roundhouse::project::BuildTarget::Go,
        roundhouse::project::BuildTarget::Elixir,
        roundhouse::project::BuildTarget::CSharp,
        roundhouse::project::BuildTarget::Kotlin,
        roundhouse::project::BuildTarget::Swift,
    ] {
        let (errors, files) = oj_target_errors_and_text(&app, &dir, target);
        assert!(
            errors.iter().any(|e| e.contains("does not emit") && e.contains("missing class")),
            "{target:?} did not refuse Oj:\n{}",
            errors.join("\n")
        );
        assert!(
            !errors.iter().any(|e| e.contains("String#read") || e.contains("JSON is not available")),
            "{target:?} refused Oj for the wrong reason:\n{}",
            errors.join("\n")
        );
        let hits: Vec<_> = files
            .iter()
            .filter(|(path, text)| {
                !path.ends_with(".map")
                    && (text.contains("Oj.load") || text.contains("Oj.generate") || text.contains("Oj::"))
            })
            .map(|(path, _)| path.clone())
            .collect::<Vec<_>>();
        assert!(hits.is_empty(), "{target:?} emitted an Oj call in {hits:?}");
        for site in ["Oj.load", "Oj.generate", "Oj::ParseError"] {
            assert!(
                errors.iter().any(|e| e.contains(site)),
                "{target:?} did not refuse {site}:\n{}",
                errors.join("\n")
            );
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

fn oj_target_errors_and_text(
    app: &roundhouse::App,
    dir: &std::path::Path,
    target: roundhouse::project::BuildTarget,
) -> (Vec<String>, std::collections::HashMap<String, String>) {
    let (files, diags) = roundhouse::emit::diagnostics::scope(|| {
        roundhouse::project::target_files(app, dir, target)
    });
    let errors = diags
        .into_iter()
        .filter(|d| d.severity == roundhouse::diagnostic::Severity::Error)
        .map(|d| d.message)
        .collect();
    let files = files.unwrap_or_default().into_iter().collect();
    (errors, files)
}

const FARADAY_LIB: &str = "class PingClient
  def self.fetch(base, path)
    conn = Faraday.new(url: base) do |c|
      c.response :raise_error
      c.options.timeout = 5
      c.adapter Faraday.default_adapter
    end
    response = conn.get(path)
    [response.status, response.body, response.success?]
  rescue Faraday::ResourceNotFound => e
    [e.response_status, :not_found]
  rescue Faraday::Error => e
    [nil, e.class.name]
  end
end
";

/// Ruby runs the real faraday gem: a connection with middleware, a
/// response, and the error hierarchy it raises, against a local server.
#[test]
fn faraday_runs_as_the_real_gem_on_ruby() {
    emit_and_run::real_blog()
        .write("app/models/ping_client.rb", FARADAY_LIB)
        .run_ruby(
            r#"require "socket"
server = TCPServer.new("127.0.0.1", 0)
port = server.addr[1]
Thread.new do
  loop do
    client = server.accept
    line = client.gets.to_s
    while (header = client.gets) && header != "\r\n"; end
    if line.include?("/ok")
      client.write "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok"
    else
      client.write "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    end
    client.close
  end
end
base = "http://127.0.0.1:#{port}"
r = PingClient.fetch(base, "/ok")
raise r.inspect unless r == [200, "ok", true]
r = PingClient.fetch(base, "/missing")
raise r.inspect unless r == [404, :not_found]
closed = TCPServer.new("127.0.0.1", 0)
closed_port = closed.addr[1]
closed.close
r = PingClient.fetch("http://127.0.0.1:#{closed_port}", "/x")
raise r.inspect unless r == [nil, "Faraday::ConnectionFailed"]"#,
        )
        .assert_passes();
}

/// faraday-multipart and faraday-retry run too: their middleware is named
/// by a symbol, and the Gemfile declares the gems it needs.
#[test]
fn faraday_multipart_and_retry_run_as_the_real_gems_on_ruby() {
    let run = emit_and_run::real_blog()
        .write(
            "lib/uploader.rb",
            "class Uploader
  def self.upload(base, text)
    conn = Faraday.new(url: base) do |c|
      c.request :multipart
      c.adapter Faraday.default_adapter
    end
    part = Faraday::Multipart::FilePart.new(StringIO.new(text), \"text/plain\", \"a.txt\")
    conn.post(\"/upload\", { file: part }).body
  end

  def self.flaky(base)
    conn = Faraday.new(url: base) do |c|
      c.request(:retry, max: 2, interval: 0, retry_statuses: [503])
      c.adapter Faraday.default_adapter
    end
    conn.get(\"/flaky\").body
  end
end
",
        )
        .run_ruby(
            r#"require "socket"
require "stringio"
server = TCPServer.new("127.0.0.1", 0)
port = server.addr[1]
hits = 0
Thread.new do
  loop do
    client = server.accept
    line = client.gets.to_s
    headers = {}
    while (header = client.gets) && header != "\r\n"
      name, value = header.split(":", 2)
      headers[name.downcase] = value.strip
    end
    body = client.read(headers["content-length"].to_i)
    reply = if line.include?("/upload")
      headers["content-type"].start_with?("multipart/form-data") && body.include?("filename=\"a.txt\"") && body.include?("hello") ? "multipart" : "plain"
    else
      hits += 1
      hits == 1 ? nil : "recovered"
    end
    if reply
      client.write "HTTP/1.1 200 OK\r\nContent-Length: #{reply.bytesize}\r\nConnection: close\r\n\r\n#{reply}"
    else
      client.write "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    end
    client.close
  end
end
base = "http://127.0.0.1:#{port}"
r = Uploader.upload(base, "hello")
raise r.inspect unless r == "multipart"
r = Uploader.flaky(base)
raise [r, hits].inspect unless r == "recovered" && hits == 2"#,
        );
    run.assert_passes();
    let gemfile = std::fs::read_to_string(run.emitted.join("Gemfile")).expect("Gemfile");
    for gem in ["faraday", "faraday-multipart", "faraday-retry"] {
        assert!(gemfile.contains(&format!("gem {gem:?}")), "Gemfile does not declare {gem}:\n{gemfile}");
    }
}

/// A bare `Faraday` value (assignment / pass-through) still demands the
/// gem on Ruby — `names_constant` would miss it; the RUNTIME_GEMS needle
/// does not.
#[test]
fn faraday_as_a_value_declares_the_gem_on_ruby() {
    let run = emit_and_run::real_blog()
        .write(
            "app/models/http_client_holder.rb",
            "class HttpClientHolder\n  CLIENT = Faraday\n  def self.client\n    CLIENT\n  end\nend\n",
        )
        .run_ruby(
            r#"c = HttpClientHolder.client
raise c.inspect unless c.equal?(Faraday)"#,
        );
    run.assert_passes();
    let gemfile = std::fs::read_to_string(run.emitted.join("Gemfile")).expect("Gemfile");
    assert!(
        gemfile.contains("gem \"faraday\""),
        "Gemfile does not declare faraday for a bare Faraday value:\n{gemfile}"
    );
}

/// An identifier that merely starts with `Faraday` must not demand the gem.
#[test]
fn faraday_holder_name_does_not_declare_the_gem_on_ruby() {
    let run = emit_and_run::real_blog()
        .write(
            "app/models/faraday_holder.rb",
            "class FaradayHolder\n  def self.label\n    \"FaradayHolder\"\n  end\nend\n",
        )
        .run_ruby(r#"raise FaradayHolder.label.inspect unless FaradayHolder.label == "FaradayHolder""#);
    run.assert_passes();
    let gemfile = std::fs::read_to_string(run.emitted.join("Gemfile")).expect("Gemfile");
    assert!(
        !gemfile.contains("gem \"faraday\""),
        "Gemfile declared faraday for FaradayHolder alone:\n{gemfile}"
    );
}

/// A superclass that is only `Faraday::…` (no call in a body) is refused
/// off the shipping targets — `gem_refusal` used to see only emit bodies.
#[test]
fn faraday_superclass_is_refused_off_ruby_and_jruby() {
    let dir = std::env::temp_dir().join(format!("rh-faraday-super-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for (path, text) in [
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"posts\", force: :cascade do |t|\n    t.string \"title\"\n  end\nend\n",
        ),
        ("config/routes.rb", "Rails.application.routes.draw do\n  get \"/posts\", to: \"posts#index\"\nend\n"),
        ("app/models/post.rb", "class Post < ApplicationRecord\nend\n"),
        (
            "app/models/ping_client.rb",
            "class PingClient < Faraday::Connection\nend\n",
        ),
        (
            "app/controllers/posts_controller.rb",
            "class PostsController < ApplicationController\n  def index\n    render plain: PingClient.name\n  end\nend\n",
        ),
    ] {
        let full = dir.join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, text).unwrap();
    }
    let mut app = roundhouse::ingest::ingest_app(&dir).expect("ingest");
    let lower_diags = roundhouse::session::analyze_and_lower(&mut app);
    assert!(
        lower_diags.iter().all(|d| d.severity != roundhouse::diagnostic::Severity::Error),
        "check is not clean: {lower_diags:?}"
    );
    for target in [roundhouse::project::BuildTarget::Ruby, roundhouse::project::BuildTarget::Jruby] {
        let (errors, files) = oj_target_errors_and_text(&app, &dir, target);
        assert!(!errors.iter().any(|e| e.contains("Faraday")), "{target:?} refused a superclass it ships:\n{}", errors.join("\n"));
        assert!(
            files.get("Gemfile").is_some_and(|g| g.contains("gem \"faraday\"")),
            "{target:?} Gemfile does not declare faraday for Faraday::Connection superclass"
        );
    }
    for target in [
        roundhouse::project::BuildTarget::Spinel,
        roundhouse::project::BuildTarget::Roda,
        roundhouse::project::BuildTarget::Python,
    ] {
        let (errors, files) = oj_target_errors_and_text(&app, &dir, target);
        assert!(
            errors.iter().any(|e| e.contains("`Faraday::Connection`") && e.contains("only the ruby and jruby trees ship")),
            "{target:?} did not refuse Faraday::Connection superclass:\n{}",
            errors.join("\n")
        );
        let hits: Vec<_> = files
            .iter()
            .filter(|(path, text)| !path.ends_with(".map") && path.contains("ping_client") && text.contains("Faraday"))
            .map(|(path, _)| path.clone())
            .collect();
        assert!(hits.is_empty(), "{target:?} emitted a Faraday superclass in {hits:?}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// A controller sibling (or the controller itself) whose only Faraday
/// reference is the superclass is refused off the shipping targets —
/// those parents live on `Controller`, not on a LibraryClass yet.
#[test]
fn faraday_controller_superclass_is_refused_off_ruby_and_jruby() {
    let dir = std::env::temp_dir().join(format!("rh-faraday-ctrl-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for (path, text) in [
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"posts\", force: :cascade do |t|\n    t.string \"title\"\n  end\nend\n",
        ),
        ("config/routes.rb", "Rails.application.routes.draw do\n  get \"/posts\", to: \"posts#index\"\nend\n"),
        ("app/models/post.rb", "class Post < ApplicationRecord\nend\n"),
        (
            "app/controllers/posts_controller.rb",
            "class ApiBoom < Faraday::Error; end\nclass PostsController < ApplicationController\n  def index\n    render plain: ApiBoom.name\n  end\nend\n",
        ),
    ] {
        let full = dir.join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, text).unwrap();
    }
    let mut app = roundhouse::ingest::ingest_app(&dir).expect("ingest");
    let lower_diags = roundhouse::session::analyze_and_lower(&mut app);
    assert!(
        lower_diags.iter().all(|d| d.severity != roundhouse::diagnostic::Severity::Error),
        "check is not clean: {lower_diags:?}"
    );
    for target in [roundhouse::project::BuildTarget::Ruby, roundhouse::project::BuildTarget::Jruby] {
        let (errors, _) = oj_target_errors_and_text(&app, &dir, target);
        assert!(!errors.iter().any(|e| e.contains("Faraday")), "{target:?} refused a sibling superclass it ships:\n{}", errors.join("\n"));
    }
    for target in [
        roundhouse::project::BuildTarget::Spinel,
        roundhouse::project::BuildTarget::Typescript,
    ] {
        let (errors, files) = oj_target_errors_and_text(&app, &dir, target);
        assert!(
            errors.iter().any(|e| e.contains("`Faraday::Error`") && e.contains("only the ruby and jruby trees ship")),
            "{target:?} did not refuse Faraday::Error sibling superclass:\n{}",
            errors.join("\n")
        );
        let hits: Vec<_> = files
            .iter()
            .filter(|(path, text)| !path.ends_with(".map") && text.contains("Faraday"))
            .map(|(path, _)| path.clone())
            .collect();
        assert!(hits.is_empty(), "{target:?} emitted a Faraday sibling superclass in {hits:?}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// A test class whose only Faraday reference is the superclass (or an
/// include / inner class) is refused on Spinel — test modules are not
/// LibraryClasses.
#[test]
fn faraday_test_module_superclass_is_refused_on_spinel() {
    let dir = std::env::temp_dir().join(format!("rh-faraday-tm-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for (path, text) in [
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"posts\", force: :cascade do |t|\n    t.string \"title\"\n  end\nend\n",
        ),
        ("config/routes.rb", "Rails.application.routes.draw do\n  get \"/posts\", to: \"posts#index\"\nend\n"),
        ("app/models/post.rb", "class Post < ApplicationRecord\nend\n"),
        (
            "app/controllers/posts_controller.rb",
            "class PostsController < ApplicationController\n  def index\n    render plain: \"ok\"\n  end\nend\n",
        ),
        (
            "test/models/post_test.rb",
            "require \"test_helper\"\nclass PostTest < Faraday::Error\n  include Faraday::FlatParamsEncoder\n  class Boom < Faraday::Error; end\n  test \"name\" do\n    assert_equal \"PostTest\", PostTest.name\n  end\nend\n",
        ),
    ] {
        let full = dir.join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, text).unwrap();
    }
    let mut app = roundhouse::ingest::ingest_app(&dir).expect("ingest");
    let lower_diags = roundhouse::session::analyze_and_lower(&mut app);
    assert!(
        lower_diags.iter().all(|d| d.severity != roundhouse::diagnostic::Severity::Error),
        "check is not clean: {lower_diags:?}"
    );
    let (errors, files) = oj_target_errors_and_text(
        &app,
        &dir,
        roundhouse::project::BuildTarget::Spinel,
    );
    for site in ["`Faraday::Error`", "`Faraday::FlatParamsEncoder`"] {
        assert!(
            errors.iter().any(|e| e.contains(site) && e.contains("only the ruby and jruby trees ship")),
            "Spinel did not refuse test-module {site}:\n{}",
            errors.join("\n")
        );
    }
    let hits: Vec<_> = files
        .iter()
        .filter(|(path, text)| !path.ends_with(".map") && path.contains("post_test") && text.contains("Faraday"))
        .map(|(path, _)| path.clone())
        .collect();
    assert!(hits.is_empty(), "Spinel emitted Faraday in test module {hits:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

const RETRYABLE_LIB: &str = "class Contact
  def self.flaky(failures)
    attempts = 0
    value = Retryable.retryable(tries: 3, on: ArgumentError, sleep: 0) do
      attempts += 1
      raise ArgumentError, \"again\" if attempts <= failures
      :done
    end
    [value, attempts]
  rescue ArgumentError
    [:gave_up, attempts]
  end
end
";

/// Retryable runs as the real gem on Ruby, as Faraday does: the block is
/// retried on the exceptions `on:` names, and its value is the call's.
#[test]
fn retryable_runs_as_the_real_gem_on_ruby() {
    let run = emit_and_run::real_blog()
        .write("lib/contact.rb", RETRYABLE_LIB)
        .run_ruby(
            r#"r = Contact.flaky(2)
raise r.inspect unless r == [:done, 3]
r = Contact.flaky(5)
raise r.inspect unless r == [:gave_up, 3]"#,
        );
    run.assert_passes();
    let gemfile = std::fs::read_to_string(run.emitted.join("Gemfile")).expect("Gemfile");
    assert!(gemfile.contains("gem \"retryable\""), "Gemfile does not declare retryable:\n{gemfile}");
}

/// Off Ruby and JRuby, each Retryable reference is refused.
#[test]
fn retryable_is_refused_off_ruby_and_jruby() {
    let dir = std::env::temp_dir().join(format!("rh-retryable-targets-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for (path, text) in [
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"posts\", force: :cascade do |t|\n    t.string \"title\"\n  end\nend\n",
        ),
        ("config/routes.rb", "Rails.application.routes.draw do\n  get \"/posts\", to: \"posts#index\"\nend\n"),
        ("app/models/post.rb", "class Post < ApplicationRecord\nend\n"),
        ("app/models/contact.rb", RETRYABLE_LIB),
        (
            "app/controllers/posts_controller.rb",
            "class PostsController < ApplicationController\n  def index\n    render plain: Contact.flaky(1).inspect\n  end\nend\n",
        ),
    ] {
        let full = dir.join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, text).unwrap();
    }
    let mut app = roundhouse::ingest::ingest_app(&dir).expect("ingest");
    let lower_diags = roundhouse::session::analyze_and_lower(&mut app);
    assert!(
        lower_diags.iter().all(|d| d.severity != roundhouse::diagnostic::Severity::Error),
        "check is not clean: {lower_diags:?}"
    );
    for target in [roundhouse::project::BuildTarget::Ruby, roundhouse::project::BuildTarget::Jruby] {
        let (errors, files) = oj_target_errors_and_text(&app, &dir, target);
        assert!(
            !errors.iter().any(|e| e.contains("Retryable")),
            "{target:?} refused a gem it ships:\n{}",
            errors.join("\n")
        );
        let gemfile = files.get("Gemfile").map(String::as_str).unwrap_or("");
        assert!(gemfile.contains("gem \"retryable\""), "{target:?} Gemfile:\n{gemfile}");
    }
    for target in [
        roundhouse::project::BuildTarget::Spinel,
        roundhouse::project::BuildTarget::Roda,
        roundhouse::project::BuildTarget::Python,
        roundhouse::project::BuildTarget::Crystal,
        roundhouse::project::BuildTarget::Typescript,
        roundhouse::project::BuildTarget::Rust,
        roundhouse::project::BuildTarget::Go,
        roundhouse::project::BuildTarget::Elixir,
        roundhouse::project::BuildTarget::CSharp,
        roundhouse::project::BuildTarget::Kotlin,
        roundhouse::project::BuildTarget::Swift,
    ] {
        let (errors, files) = oj_target_errors_and_text(&app, &dir, target);
        for site in ["`Retryable.retryable`"] {
            assert!(
                errors.iter().any(|e| e.contains(site) && e.contains("only the ruby and jruby trees ship")),
                "{target:?} did not refuse {site}:\n{}",
                errors.join("\n")
            );
        }
        let hits: Vec<_> = files
            .iter()
            .filter(|(path, text)| {
                !path.ends_with(".map") && path.contains("contact") && text.contains("Retryable")
            })
            .map(|(path, _)| path.clone())
            .collect();
        assert!(hits.is_empty(), "{target:?} emitted a Retryable reference in {hits:?}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

const JWT_LIB: &str = "class Token
  SECRET = \"s3cret\"

  def self.issue(user_id, exp)
    JWT.encode({ user_id: user_id, exp: exp }, SECRET, \"HS256\")
  end

  def self.read(token, key = SECRET)
    payload, _header = JWT.decode(token, key, true, { algorithm: \"HS256\" })
    payload[\"user_id\"]
  rescue JWT::ExpiredSignature
    :expired
  rescue JWT::VerificationError
    :forged
  rescue JWT::DecodeError
    :malformed
  end
end
";

/// JWT runs as the real gem on Ruby: a token round-trips, and a wrong key,
/// an expired token and garbage each raise their own `JWT::DecodeError`.
#[test]
fn jwt_runs_as_the_real_gem_on_ruby() {
    let run = emit_and_run::real_blog()
        .write("lib/token.rb", JWT_LIB)
        .run_ruby(
            r#"fresh = Token.issue(7, Time.now.to_i + 60)
raise "round trip" unless Token.read(fresh) == 7
raise "forged" unless Token.read(fresh, "other") == :forged
raise "expired" unless Token.read(Token.issue(7, Time.now.to_i - 60)) == :expired
raise "malformed" unless Token.read("not-a-token") == :malformed"#,
        );
    run.assert_passes();
    let gemfile = std::fs::read_to_string(run.emitted.join("Gemfile")).expect("Gemfile");
    assert!(gemfile.contains("gem \"jwt\""), "Gemfile does not declare jwt:\n{gemfile}");
}

/// Off Ruby and JRuby, each JWT reference is refused.
#[test]
fn jwt_is_refused_off_ruby_and_jruby() {
    let dir = std::env::temp_dir().join(format!("rh-jwt-targets-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for (path, text) in [
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"posts\", force: :cascade do |t|\n    t.string \"title\"\n  end\nend\n",
        ),
        ("config/routes.rb", "Rails.application.routes.draw do\n  get \"/posts\", to: \"posts#index\"\nend\n"),
        ("app/models/post.rb", "class Post < ApplicationRecord\nend\n"),
        ("app/models/token.rb", JWT_LIB),
        (
            "app/controllers/posts_controller.rb",
            "class PostsController < ApplicationController\n  def index\n    render plain: Token.read(Token.issue(1, 0)).inspect\n  end\nend\n",
        ),
    ] {
        let full = dir.join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, text).unwrap();
    }
    let mut app = roundhouse::ingest::ingest_app(&dir).expect("ingest");
    let lower_diags = roundhouse::session::analyze_and_lower(&mut app);
    assert!(
        lower_diags.iter().all(|d| d.severity != roundhouse::diagnostic::Severity::Error),
        "check is not clean: {lower_diags:?}"
    );
    for target in [roundhouse::project::BuildTarget::Ruby, roundhouse::project::BuildTarget::Jruby] {
        let (errors, files) = oj_target_errors_and_text(&app, &dir, target);
        assert!(!errors.iter().any(|e| e.contains("JWT")), "{target:?} refused a gem it ships:\n{}", errors.join("\n"));
        let gemfile = files.get("Gemfile").map(String::as_str).unwrap_or("");
        assert!(gemfile.contains("gem \"jwt\""), "{target:?} Gemfile:\n{gemfile}");
    }
    for target in [
        roundhouse::project::BuildTarget::Spinel,
        roundhouse::project::BuildTarget::Roda,
        roundhouse::project::BuildTarget::Python,
        roundhouse::project::BuildTarget::Crystal,
        roundhouse::project::BuildTarget::Typescript,
        roundhouse::project::BuildTarget::Rust,
        roundhouse::project::BuildTarget::Go,
        roundhouse::project::BuildTarget::Elixir,
        roundhouse::project::BuildTarget::CSharp,
        roundhouse::project::BuildTarget::Kotlin,
        roundhouse::project::BuildTarget::Swift,
    ] {
        let (errors, files) = oj_target_errors_and_text(&app, &dir, target);
        for site in ["`JWT.encode`", "`JWT.decode`", "`JWT::ExpiredSignature`", "`JWT::DecodeError`"] {
            assert!(
                errors.iter().any(|e| e.contains(site) && e.contains("only the ruby and jruby trees ship")),
                "{target:?} did not refuse {site}:\n{}",
                errors.join("\n")
            );
        }
        let hits: Vec<_> = files
            .iter()
            .filter(|(path, text)| !path.ends_with(".map") && path.contains("token") && text.contains("JWT"))
            .map(|(path, _)| path.clone())
            .collect();
        assert!(hits.is_empty(), "{target:?} emitted a JWT reference in {hits:?}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

const ELASTIC_APM_LIB: &str = "module Traced
  include ElasticAPM::SpanHelpers

  def double(x)
    x * 2
  end
  span_method :double, 'traced:double'
end

class Pricer
  include Traced
  include ElasticAPM::SpanHelpers

  def call(x, scale: 2)
    ElasticAPM.set_custom_context(x: x)
    ElasticAPM.set_label(:pricer, \"on\")
    block_given? ? yield(x * scale) : x * scale
  end
  span_method :call, 'pricer:call'

  def self.quote(x)
    new.call(x, scale: 3)
  end

  def self.rate
    3
  end
  span_class_method :rate, 'pricer:rate'

  def traced
    current = ElasticAPM.current_transaction
    ElasticAPM.with_span(\"traced\", \"app\") { current.nil? ? :none : :some }
  end

  def report(error)
    ElasticAPM.report(error)
  end
end
";

/// ElasticAPM runs as the real gem on Ruby. With no agent started, as an
/// app that never configures one: `span_method` / `span_class_method` wrap
/// the method and pass its arguments, keywords and block through, in a
/// class and in a module; `with_span` answers its block and there is no
/// transaction.
#[test]
fn elastic_apm_runs_as_the_real_gem_on_ruby() {
    let run = emit_and_run::real_blog()
        .write("lib/pricer.rb", ELASTIC_APM_LIB)
        .run_ruby(
            r#"pricer = Pricer.new
raise "call" unless Pricer.quote(5) == 15
raise "block" unless pricer.call(5) { |v| v + 1 } == 11
raise "module" unless pricer.double(4) == 8
raise "class side" unless Pricer.rate == 3
raise "not wrapped" if Pricer.ancestors.first == Pricer
raise "not wrapped in the module" if Traced.ancestors.first == Traced
raise "singleton not wrapped" if Pricer.singleton_class.ancestors.first == Pricer.singleton_class
raise "span" unless pricer.traced == :none
pricer.report(RuntimeError.new("boom"))"#,
        );
    run.assert_passes();
    let gemfile = std::fs::read_to_string(run.emitted.join("Gemfile")).expect("Gemfile");
    assert!(gemfile.contains("gem \"elastic-apm\""), "Gemfile does not declare elastic-apm:\n{gemfile}");
}

/// `span_method` and an app module's class-body macro replay in source
/// order: a macro written after `span_method` sees the method wrapped.
#[test]
fn span_method_replays_in_source_order_with_app_macros() {
    emit_and_run::real_blog()
        .write(
            "lib/worker.rb",
            "module Audit
  def audited(name)
    raise \"span_method must run first\" if ancestors.first == self
    name
  end
end

class Worker
  include ElasticAPM::SpanHelpers

  def work
    1
  end
  span_method :work, 'worker:work'

  extend Audit
  audited :work
end
",
        )
        .run_ruby(r#"raise "work" unless Worker.new.work == 1"#)
        .assert_passes();
}

/// Off Ruby and JRuby, each ElasticAPM reference and the `SpanHelpers`
/// include are refused.
#[test]
fn elastic_apm_is_refused_off_ruby_and_jruby() {
    let dir = std::env::temp_dir().join(format!("rh-elastic-apm-targets-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for (path, text) in [
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"posts\", force: :cascade do |t|\n    t.string \"title\"\n  end\nend\n",
        ),
        ("config/routes.rb", "Rails.application.routes.draw do\n  get \"/posts\", to: \"posts#index\"\nend\n"),
        ("app/models/post.rb", "class Post < ApplicationRecord\nend\n"),
        ("app/models/pricer.rb", ELASTIC_APM_LIB),
        (
            "app/controllers/posts_controller.rb",
            "class PostsController < ApplicationController\n  def index\n    render plain: Pricer.new.call(1).inspect\n  end\nend\n",
        ),
    ] {
        let full = dir.join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, text).unwrap();
    }
    let mut app = roundhouse::ingest::ingest_app(&dir).expect("ingest");
    let lower_diags = roundhouse::session::analyze_and_lower(&mut app);
    assert!(
        lower_diags.iter().all(|d| d.severity != roundhouse::diagnostic::Severity::Error),
        "check is not clean: {lower_diags:?}"
    );
    for target in [roundhouse::project::BuildTarget::Ruby, roundhouse::project::BuildTarget::Jruby] {
        let (errors, files) = oj_target_errors_and_text(&app, &dir, target);
        assert!(
            !errors.iter().any(|e| e.contains("ElasticAPM")),
            "{target:?} refused a gem it ships:\n{}",
            errors.join("\n")
        );
        let gemfile = files.get("Gemfile").map(String::as_str).unwrap_or("");
        assert!(gemfile.contains("gem \"elastic-apm\""), "{target:?} Gemfile:\n{gemfile}");
    }
    for target in [
        roundhouse::project::BuildTarget::Spinel,
        roundhouse::project::BuildTarget::Roda,
        roundhouse::project::BuildTarget::Python,
        roundhouse::project::BuildTarget::Crystal,
        roundhouse::project::BuildTarget::Typescript,
        roundhouse::project::BuildTarget::Rust,
        roundhouse::project::BuildTarget::Go,
        roundhouse::project::BuildTarget::Elixir,
        roundhouse::project::BuildTarget::CSharp,
        roundhouse::project::BuildTarget::Kotlin,
        roundhouse::project::BuildTarget::Swift,
    ] {
        let (errors, files) = oj_target_errors_and_text(&app, &dir, target);
        for site in ["`ElasticAPM.set_label`", "`ElasticAPM.with_span`", "`ElasticAPM::SpanHelpers`"] {
            assert!(
                errors.iter().any(|e| e.contains(site) && e.contains("only the ruby and jruby trees ship")),
                "{target:?} did not refuse {site}:\n{}",
                errors.join("\n")
            );
        }
        let hits: Vec<_> = files
            .iter()
            .filter(|(path, text)| !path.ends_with(".map") && path.contains("pricer") && text.contains("ElasticAPM"))
            .map(|(path, _)| path.clone())
            .collect();
        assert!(hits.is_empty(), "{target:?} emitted an ElasticAPM reference in {hits:?}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// ActiveSupport's `Digest::UUID.uuid_v4` runs as `SecureRandom.uuid`: a
/// fresh random (version 4) uuid on each call.
#[test]
fn digest_uuid_v4_runs_as_a_random_uuid() {
    emit_and_run::real_blog()
        .write("lib/correlation.rb", "class Correlation\n  def self.id\n    ::Digest::UUID.uuid_v4\n  end\nend\n")
        .run_ruby(
            "a = Correlation.id\nb = Correlation.id\n\
             raise a.inspect unless a.match?(/\\A\\h{8}-\\h{4}-4\\h{3}-[89ab]\\h{3}-\\h{12}\\z/)\n\
             raise \"repeated\" if a == b",
        )
        .assert_passes();
}

const REST_CLIENT_LIB: &str = "class Remote
  def self.fetch(base, path)
    response = RestClient.get(base + path, params: { q: \"x\" })
    [response.code, response.body]
  rescue RestClient::NotFound => e
    [e.http_code, :not_found]
  end

  def self.send_form(base)
    RestClient.post(base + \"/echo\", { name: \"a\" }).body
  end

  def self.execute(base)
    RestClient::Request.execute(method: :get, url: base + \"/ok\", timeout: 5).body
  end

  def self.status_of(base, path)
    RestClient.get(base + path) { |response, _request, _result| [response.code, response.body] }.first
  end

  CLIENT = ::RestClient

  def self.built(base)
    ::RestClient::Request.new(method: :get, url: base + \"/built\").execute.body
  end
end
";

/// RestClient runs as the real gem on Ruby: requests against a local
/// server, a 404 rescued as `RestClient::NotFound`, and a verb with a
/// block answering the block's value, a 404 included.
#[test]
fn rest_client_runs_as_the_real_gem_on_ruby() {
    let run = emit_and_run::real_blog()
        .write("lib/remote.rb", REST_CLIENT_LIB)
        .run_ruby(
            r##"require "socket"
server = TCPServer.new("127.0.0.1", 0)
port = server.addr[1]
Thread.new do
  loop do
    client = server.accept
    line = client.gets.to_s
    headers = {}
    while (header = client.gets) && header != "\r\n"
      name, value = header.split(":", 2)
      headers[name.downcase] = value.strip
    end
    body = client.read(headers["content-length"].to_i)
    reply = if line.include?("/missing") then nil elsif line.include?("/echo") then body else line.split(" ")[1] end
    if reply
      client.write "HTTP/1.1 200 OK\r\nContent-Length: #{reply.bytesize}\r\nConnection: close\r\n\r\n#{reply}"
    else
      client.write "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    end
    client.close
  end
end
base = "http://127.0.0.1:#{port}"
r = Remote.fetch(base, "/ok")
raise r.inspect unless r == [200, "/ok?q=x"]
r = Remote.fetch(base, "/missing")
raise r.inspect unless r == [404, :not_found]
raise Remote.send_form(base) unless Remote.send_form(base) == "name=a"
raise Remote.execute(base) unless Remote.execute(base) == "/ok"
raise Remote.built(base) unless Remote.built(base) == "/built"
raise "block 200" unless Remote.status_of(base, "/ok") == 200
raise "block 404" unless Remote.status_of(base, "/missing") == 404
"##,
        );
    run.assert_passes();
    let gemfile = std::fs::read_to_string(run.emitted.join("Gemfile")).expect("Gemfile");
    assert!(gemfile.contains("gem \"rest-client\""), "Gemfile does not declare rest-client:\n{gemfile}");
}

/// Off Ruby and JRuby, each RestClient reference is refused.
#[test]
fn rest_client_is_refused_off_ruby_and_jruby() {
    let dir = std::env::temp_dir().join(format!("rh-rest-client-targets-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for (path, text) in [
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"posts\", force: :cascade do |t|\n    t.string \"title\"\n  end\nend\n",
        ),
        ("config/routes.rb", "Rails.application.routes.draw do\n  get \"/posts\", to: \"posts#index\"\nend\n"),
        ("app/models/post.rb", "class Post < ApplicationRecord\nend\n"),
        ("app/models/remote.rb", REST_CLIENT_LIB),
        (
            "app/controllers/posts_controller.rb",
            "class PostsController < ApplicationController\n  def index\n    render plain: Remote.fetch(\"http://example.test\", \"/\").inspect\n  end\nend\n",
        ),
    ] {
        let full = dir.join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, text).unwrap();
    }
    let mut app = roundhouse::ingest::ingest_app(&dir).expect("ingest");
    let lower_diags = roundhouse::session::analyze_and_lower(&mut app);
    assert!(
        lower_diags.iter().all(|d| d.severity != roundhouse::diagnostic::Severity::Error),
        "check is not clean: {lower_diags:?}"
    );
    for target in [roundhouse::project::BuildTarget::Ruby, roundhouse::project::BuildTarget::Jruby] {
        let (errors, files) = oj_target_errors_and_text(&app, &dir, target);
        assert!(
            !errors.iter().any(|e| e.contains("RestClient")),
            "{target:?} refused a gem it ships:\n{}",
            errors.join("\n")
        );
        let gemfile = files.get("Gemfile").map(String::as_str).unwrap_or("");
        assert!(gemfile.contains("gem \"rest-client\""), "{target:?} Gemfile:\n{gemfile}");
    }
    for target in [
        roundhouse::project::BuildTarget::Spinel,
        roundhouse::project::BuildTarget::Roda,
        roundhouse::project::BuildTarget::Python,
        roundhouse::project::BuildTarget::Crystal,
        roundhouse::project::BuildTarget::Typescript,
        roundhouse::project::BuildTarget::Rust,
        roundhouse::project::BuildTarget::Go,
        roundhouse::project::BuildTarget::Elixir,
        roundhouse::project::BuildTarget::CSharp,
        roundhouse::project::BuildTarget::Kotlin,
        roundhouse::project::BuildTarget::Swift,
    ] {
        let (errors, files) = oj_target_errors_and_text(&app, &dir, target);
        for site in ["`RestClient.get`", "`RestClient::NotFound`", "`RestClient::Request`"] {
            assert!(
                errors.iter().any(|e| e.contains(site) && e.contains("only the ruby and jruby trees ship")),
                "{target:?} did not refuse {site}:\n{}",
                errors.join("\n")
            );
        }
        let hits: Vec<_> = files
            .iter()
            .filter(|(path, text)| {
                !path.ends_with(".map") && path.contains("remote") && text.contains("RestClient")
            })
            .map(|(path, _)| path.clone())
            .collect();
        assert!(hits.is_empty(), "{target:?} emitted a RestClient reference in {hits:?}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// No other target has an HTTP client behind `Faraday`: each call and
/// constant is refused there, and no `Faraday` reference is emitted.
#[test]
fn faraday_is_refused_off_ruby_and_jruby() {
    let dir = std::env::temp_dir().join(format!("rh-faraday-targets-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for (path, text) in [
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"posts\", force: :cascade do |t|\n    t.string \"title\"\n  end\nend\n",
        ),
        ("config/routes.rb", "Rails.application.routes.draw do\n  get \"/posts\", to: \"posts#index\"\nend\n"),
        ("app/models/post.rb", "class Post < ApplicationRecord\nend\n"),
        ("app/models/ping_client.rb", FARADAY_LIB),
        (
            "app/controllers/posts_controller.rb",
            "class PostsController < ApplicationController\n  def index\n    render plain: PingClient.fetch(\"http://example.test\", \"/ok\").inspect\n  end\nend\n",
        ),
    ] {
        let full = dir.join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, text).unwrap();
    }
    let mut app = roundhouse::ingest::ingest_app(&dir).expect("ingest");
    let lower_diags = roundhouse::session::analyze_and_lower(&mut app);
    assert!(
        lower_diags.iter().all(|d| d.severity != roundhouse::diagnostic::Severity::Error),
        "check is not clean: {lower_diags:?}"
    );
    for target in [roundhouse::project::BuildTarget::Ruby, roundhouse::project::BuildTarget::Jruby] {
        let (errors, files) = oj_target_errors_and_text(&app, &dir, target);
        assert!(!errors.iter().any(|e| e.contains("Faraday")), "{target:?} refused the gem it ships:\n{}", errors.join("\n"));
        assert!(
            files.get("Gemfile").is_some_and(|g| g.contains("gem \"faraday\"")),
            "{target:?} Gemfile does not declare faraday"
        );
    }
    for target in [
        roundhouse::project::BuildTarget::Spinel,
        roundhouse::project::BuildTarget::Roda,
        roundhouse::project::BuildTarget::Python,
        roundhouse::project::BuildTarget::Crystal,
        roundhouse::project::BuildTarget::Typescript,
        roundhouse::project::BuildTarget::Rust,
        roundhouse::project::BuildTarget::Go,
        roundhouse::project::BuildTarget::Elixir,
        roundhouse::project::BuildTarget::CSharp,
        roundhouse::project::BuildTarget::Kotlin,
        roundhouse::project::BuildTarget::Swift,
    ] {
        let (errors, files) = oj_target_errors_and_text(&app, &dir, target);
        for site in ["`Faraday.new`", "`Faraday::ResourceNotFound`", "`Faraday::Error`"] {
            assert!(
                errors.iter().any(|e| e.contains(site) && e.contains("only the ruby and jruby trees ship")),
                "{target:?} did not refuse {site}:\n{}",
                errors.join("\n")
            );
        }
        let hits: Vec<_> = files
            .iter()
            .filter(|(path, text)| !path.ends_with(".map") && path.contains("ping_client") && text.contains("Faraday"))
            .map(|(path, _)| path.clone())
            .collect();
        assert!(hits.is_empty(), "{target:?} emitted a Faraday reference in {hits:?}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// `/(?<name>..)/ =~ str` assigns each named group to a local (nil when
/// nothing matches) and answers the match's index: as a statement, as an
/// `if` condition, and as a method's value. Checked against CRuby.
#[test]
fn named_capture_matches_assign_their_locals() {
    let run = emit_and_run::real_blog()
        .write("lib/matcher.rb", r##"class Matcher
  def self.map_name(html)
    /usemap=\s*['"]#(?<map_name>\S+?)['"]/m =~ html
    map_name
  end

  def self.pair(text)
    if /(?<a>\d+)-(?<b>\d+)/ =~ text
      [a, b]
    else
      [a, b, :none]
    end
  end

  def self.index(text)
    /(?<w>x)/ =~ text
  end
end
"##)
        .run_ruby(r##"raise "map" unless Matcher.map_name(%(<img usemap="#m1">)) == "m1"
raise "map none" unless Matcher.map_name("none").nil?
raise "pair" unless Matcher.pair("1-2") == ["1", "2"]
raise "pair none" unless Matcher.pair("x") == [nil, nil, :none]
raise "index" unless Matcher.index("abx") == 2
raise "index none" unless Matcher.index("q").nil?
puts "named captures passed"
"##);
    run.assert_passes();
    assert!(run.stdout.contains("named captures passed"));
}

#[test]
fn dry_monads_results_run_lowered() {
    let run = dry_monads::overlay().run_ruby(&dry_monads::assertions());
    run.assert_passes();
    assert!(run.stdout.contains("dry-monads contract passed"));
}

/// What dry-monads lowering leaves as it was, ledgered: a monad it does
/// not model that the app builds (`Some(1)`), a called form it does not
/// read, and a do-notation method that takes a block (called with one,
/// the gem's wrapper steps aside). Outside survey mode each fails the
/// ingest.
#[test]
fn dry_monads_refusals_are_ledgered() {
    let tree: std::collections::HashMap<std::path::PathBuf, Vec<u8>> = [
        ("app/services/maybe_service.rb", "class MaybeService\n  include Dry::Monads[:maybe]\n\n  def one\n    Some (1)\n  end\nend\n"),
        ("app/services/typed_service.rb", "class TypedService\n  include Dry::Monads::Result(StandardError)\nend\n"),
        (
            "app/services/block_service.rb",
            "class BlockService\n  include Dry::Monads[:result, :do]\n\n  def each_item\n    return to_enum(:each_item) unless block_given?\n\n    yield Success(1)\n  end\n\n  def call\n    v = yield Success(2)\n    Success(v)\n  end\nend\n",
        ),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ]
    .into_iter()
    .map(|(p, s)| (std::path::PathBuf::from(p), s.as_bytes().to_vec()))
    .collect();
    assert!(roundhouse::ingest::ingest_app_from_tree(tree.clone()).is_err(), "strict ingest passed a refusal");
    roundhouse::ingest::survey::activate();
    let app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest");
    let ledger: Vec<String> = roundhouse::ingest::survey::drain().iter().map(|e| e.to_string()).collect();
    assert!(ledger.iter().any(|e| e.contains("Dry::Monads not lowered: `:maybe`")), "{ledger:#?}");
    assert!(ledger.iter().any(|e| e.contains("Dry::Monads not lowered: `include Dry::Monads") && e.contains("Result(StandardError)`")), "{ledger:#?}");
    assert!(ledger.iter().any(|e| e.contains("`each_item` takes a block")), "{ledger:#?}");
    let class = |name: &str| app.library_classes.iter().find(|lc| lc.name.0.as_str() == name).expect(name);
    assert!(class("MaybeService").unknown_calls.iter().any(|c| roundhouse::emit::ruby::emit_expr(c).contains("Dry::Monads[:maybe]")));
    let body = |name: &str| {
        roundhouse::emit::ruby::emit_expr(&class("BlockService").methods.iter().find(|m| m.name.as_str() == name).expect(name).body)
    };
    assert!(body("each_item").contains("yield"), "{}", body("each_item"));
    assert!(body("call").contains("Do.bind"), "{}", body("call"));
}

#[test]
fn dry_validation_contracts_run_lowered() {
    let run = dry_validation::overlay().run_ruby(&dry_validation::assertions());
    run.assert_passes();
    assert!(run.stdout.contains("dry-validation contract passed"));
}

/// A message file is read from the app only: a `load_paths` entry that
/// is absolute or climbs out refuses the contract. Outside survey mode a
/// refusal fails the ingest instead of passing as supported.
#[test]
fn dry_validation_load_paths_stay_in_the_app_and_refusals_fail_strict_ingest() {
    let tree = |path: &str| -> std::collections::HashMap<std::path::PathBuf, Vec<u8>> {
        [
            (
                "app/contracts/far_contract.rb".to_string(),
                format!("class FarContract < Dry::Validation::Contract\n  config.messages.load_paths << \"{path}\"\n\n  params do\n    required(:a).filled(:string)\n  end\nend\n"),
            ),
            ("config/routes.rb".to_string(), "Rails.application.routes.draw do\nend\n".to_string()),
        ]
        .into_iter()
        .map(|(p, s)| (std::path::PathBuf::from(p), s.into_bytes()))
        .collect()
    };
    for path in ["/etc/hostname", "../outside.yml", "config/../../outside.yml"] {
        roundhouse::ingest::survey::activate();
        let app = roundhouse::ingest::ingest_app_from_tree(tree(path)).expect("survey ingest");
        let ledger: Vec<String> = roundhouse::ingest::survey::drain().iter().map(|e| e.to_string()).collect();
        assert!(ledger.iter().any(|e| e.contains("outside the app")), "{path}: {ledger:#?}");
        let contract = app.library_classes.iter().find(|lc| lc.name.0.as_str() == "FarContract").expect("contract");
        assert!(contract.methods.iter().all(|m| m.name.as_str() != "call"), "{path}: lowered");
        assert!(roundhouse::ingest::ingest_app_from_tree(tree(path)).is_err(), "{path}: strict ingest passed");
    }
    roundhouse::ingest::survey::activate();
    roundhouse::ingest::ingest_app_from_tree(tree("config/locales/x.yml")).expect("an app path");
    let ledger: Vec<String> = roundhouse::ingest::survey::drain().iter().map(|e| e.to_string()).collect();
    assert!(ledger.iter().all(|e| !e.contains("FarContract")), "{ledger:#?}");
}

/// A schema reference that does not resolve (`filled(MissingContract.schema)`)
/// is ledgered as the missing schema, not as an unread type.
#[test]
fn dry_validation_an_unresolved_schema_reference_names_the_schema() {
    let tree: std::collections::HashMap<std::path::PathBuf, Vec<u8>> = [
        (
            "app/contracts/checkout_contract.rb",
            "class CheckoutContract < Dry::Validation::Contract\n  params do\n    required(:address).filled(MissingContract.schema)\n  end\nend\n",
        ),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ]
    .into_iter()
    .map(|(p, s)| (std::path::PathBuf::from(p), s.as_bytes().to_vec()))
    .collect();
    roundhouse::ingest::survey::activate();
    roundhouse::ingest::ingest_app_from_tree(tree).expect("survey ingest");
    let ledger: Vec<String> = roundhouse::ingest::survey::drain().iter().map(|e| e.to_string()).collect();
    assert!(
        ledger.iter().any(|e| e.contains("CheckoutContract") && e.contains("schema `MissingContract.schema`")),
        "{ledger:#?}"
    );
}

/// `:time` and `:date_time` coerce with `Time.parse` and `DateTime`,
/// which Spinel's tree does not have: reported for it, not emitted to
/// fail. The fixture's dates and decimals are not.
#[test]
fn dry_validation_stdlib_coercions_are_reported_for_spinel() {
    let (_emitted, errors) = dry_validation::overlay().emit(roundhouse::project::BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:#?}");
    let (_emitted, errors) = dry_validation::overlay()
        .write(
            "app/contracts/clock_contract.rb",
            "class ClockContract < Dry::Validation::Contract\n  params do\n    optional(:at).value(:time)\n    optional(:stamp).value(:date_time)\n  end\nend\n",
        )
        .emit(roundhouse::project::BuildTarget::Spinel);
    for what in ["`Time.parse`", "`DateTime`"] {
        assert!(errors.iter().any(|e| e.contains(what)), "{what} not reported: {errors:#?}");
    }
}

/// A contract the lowering cannot read (here, an `option`) stays as it was,
/// with the topmost contract it inherits a schema from and everything
/// under that: a lowered parent's `call` would answer for it. Its
/// declarations are dropped, not replayed onto a gem that is not there,
/// so the tree boots and another contract is lowered.
#[test]
fn dry_validation_refusal_takes_the_schema_hierarchy() {
    let refused = || {
        dry_validation::overlay().write(
            "app/contracts/ruled_contract.rb",
            "class RuledBase < Dry::Validation::Contract\n  params do\n    required(:a).filled(:integer)\n  end\nend\n\nclass RuledChild < RuledBase\n  option :repo\nend\n\nclass RuledSibling < RuledBase\n  params do\n    required(:b).filled(:string)\n  end\nend\n",
        )
    };
    // Survey mode, as `--survey` runs: strict ingest fails on the refusal
    // (`dry_validation_load_paths_stay_in_the_app_and_refusals_fail_strict_ingest`).
    roundhouse::ingest::survey::activate();
    let (emitted, _errors) = refused().emit(roundhouse::project::BuildTarget::Ruby);
    let ledger: Vec<String> = roundhouse::ingest::survey::drain().iter().map(|e| e.to_string()).collect();
    assert!(ledger.iter().any(|e| e.contains("RuledChild") && e.contains("`option`")), "{ledger:#?}");
    for kept in ["ruled_base.rb", "ruled_child.rb", "ruled_sibling.rb"] {
        let source = std::fs::read_to_string(emitted.join("app/models").join(kept)).expect(kept);
        assert!(!source.contains("def call"), "{kept} lowered:\n{source}");
        assert!(!source.contains("params do"), "{kept} replays the DSL:\n{source}");
    }
    roundhouse::ingest::survey::activate();
    let run = refused().run_ruby("raise \"lowered\" unless UserContract.new.call(\"name\" => \"a\", \"age\" => \"1\").success?\nputs \"refusal contract passed\"\n");
    roundhouse::ingest::survey::drain();
    run.assert_passes();
    assert!(run.stdout.contains("refusal contract passed"));
}

#[test]
fn dry_initializer_classes_run_lowered() {
    let run = dry_initializer::ruby_overlay().run_ruby(&format!(
        "{}{}",
        dry_initializer::ASSERTIONS,
        dry_initializer::CLOCK_ASSERTIONS
    ));
    run.assert_passes();
    assert!(run.stdout.contains("dry-initializer contract passed"));
    assert!(run.stdout.contains("dry-initializer clock contract passed"));
}

/// A class the lowering cannot read keeps its whole hierarchy on the gem
/// (ledgered): a refused class under a lowered ancestor would inherit a
/// constructor that ignores its own options. Constructing one is an
/// error; another hierarchy is lowered, and the tree still boots with the
/// stand-ins the lowered classes raise.
#[test]
fn dry_initializer_refusal_keeps_the_hierarchy_on_the_gem() {
    let refused = || dry_initializer::overlay()
        .write(
            "lib/svc/odd.rb",
            "module Svc\n  class Odd < Base\n    option :size, ::Svc::Types::Strict::String.constrained(uuid_v4: true)\n  end\n\n  class Odder < Odd\n  end\nend\n",
        )
        .write(
            "lib/svc/lone.rb",
            "module Svc\n  class Lone\n    extend Dry::Initializer\n\n    option :count, ::Svc::Types::Coercible::Integer\n  end\nend\n",
        );
    let (emitted, _errors) = refused().emit(roundhouse::project::BuildTarget::Ruby);
    for kept in ["svc/base.rb", "svc/charge.rb", "svc/quote.rb", "svc/odd.rb", "svc/odder.rb"] {
        let source = std::fs::read_to_string(emitted.join("app/models").join(kept)).expect(kept);
        assert!(!source.contains("dry_initializer_assign"), "{kept} lowered:\n{source}");
    }
    let lone = std::fs::read_to_string(emitted.join("app/models/svc/lone.rb")).expect("emitted Svc::Lone");
    assert!(lone.contains("def dry_initializer_assign"), "other hierarchy not lowered:\n{lone}");

    let (_, errors) = refused()
        .write("app/models/odd_caller.rb", "class OddCaller\n  def self.size\n    Svc::Odd.new(size: 3).size\n  end\nend\n")
        .emit(roundhouse::project::BuildTarget::Ruby);
    assert!(
        errors.iter().any(|e| e.contains("unmodeled constructor lookup")),
        "a refused class constructs without an error:\n{}",
        errors.join("\n")
    );

    let run = refused().run_ruby(
        r#"
raise "lone" unless Svc::Lone.new(count: "3").count == 3
begin
  Svc::Lone.new(count: "x")
  raise "coercible integer accepted a word"
rescue Dry::Types::CoercionError
end
raise "undefined" unless defined?(Dry::Initializer::UNDEFINED)
puts "refusal contract passed"
"#,
    );
    run.assert_passes();
    assert!(run.stdout.contains("refusal contract passed"));
}

/// `Params::Time` coerces with `Time.parse`, which Spinel's tree does not
/// have: reported for it, as for a struct, not emitted to fail.
#[test]
fn dry_initializer_stdlib_coercions_are_reported_for_spinel() {
    let (_emitted, errors) = dry_initializer::ruby_overlay().emit(roundhouse::project::BuildTarget::Spinel);
    assert!(errors.iter().any(|e| e.contains("`Time.parse`")), "Time.parse not reported: {errors:#?}");
    let (_emitted, errors) = dry_initializer::overlay().emit(roundhouse::project::BuildTarget::Spinel);
    assert!(errors.iter().all(|e| !e.contains("Time.parse")), "{errors:#?}");
}

/// The initializer lowering reads type constants before the `Dry::Struct`
/// shell strips them: an option typed by one is lowered next to a struct.
#[test]
fn dry_initializer_reads_type_constants_beside_a_lowered_struct() {
    let run = dry_initializer::overlay()
        .write(
            "lib/svc/amounts.rb",
            "module Svc\n  module Amounts\n    COUNT = ::Svc::Types::Coercible::Integer\n  end\nend\n",
        )
        .write(
            "lib/svc/point.rb",
            "module Svc\n  class Point < Dry::Struct\n    attribute :x, ::Svc::Types::Coercible::Integer\n  end\nend\n",
        )
        .write(
            "lib/svc/tally.rb",
            "module Svc\n  class Tally\n    extend Dry::Initializer\n\n    option :count, ::Svc::Amounts::COUNT\n  end\nend\n",
        )
        .run_ruby(
            r#"
raise "struct" unless Svc::Point.new(x: "2").x == 2
raise "constant type" unless Svc::Tally.new(count: "4").count == 4
puts "type constant contract passed"
"#,
        );
    run.assert_passes();
    assert!(run.stdout.contains("type constant contract passed"));
}

/// Spinel's tree has its own `Date` and `BigDecimal()`, but no
/// `DateTime`, `Time.parse` or `BigDecimal.interpret_loosely`: a lowered
/// struct using those is reported for it, not emitted to fail when first
/// reached, and one using only the former is not.
#[test]
fn dry_struct_stdlib_coercions_are_reported_for_spinel() {
    let (emitted, errors) = dry_struct::overlay().emit(roundhouse::project::BuildTarget::Spinel);
    assert!(errors.iter().all(|e| !e.contains("Dry::Struct")), "{errors:#?}");
    // ... and the struct emitted with its date and decimal coercions.
    let stamp = std::fs::read_to_string(emitted.join("app/models/shop/stamp.rb")).expect("emitted Shop::Stamp");
    for coercion in ["::Date.parse(", "is_a?(::Date)", "BigDecimal("] {
        assert!(stamp.contains(coercion), "{coercion} not emitted:\n{stamp}");
    }
    let (_emitted, errors) = dry_struct::clock_overlay(dry_struct::overlay())
        .emit(roundhouse::project::BuildTarget::Spinel);
    for what in ["`DateTime`", "`Time.parse`", "`BigDecimal.interpret_loosely`"] {
        assert!(errors.iter().any(|e| e.contains(what)), "{what} not reported: {errors:#?}");
    }
    // A shared default is a class constant, scanned like the methods.
    let (_emitted, errors) = dry_struct::overlay()
        .write(
            "lib/shop/since.rb",
            "module Shop\n  class Since < Dry::Struct\n    attribute? :at, ::Shop::Types::Any.default(Time.parse(\"2026-01-01\"))\n  end\nend\n",
        )
        .emit(roundhouse::project::BuildTarget::Spinel);
    assert!(errors.iter().any(|e| e.contains("`Time.parse`")), "default not scanned: {errors:#?}");
    // An app's own `interpret_loosely` is not BigDecimal's.
    let (_emitted, errors) = dry_struct::overlay()
        .write(
            "lib/shop/loose.rb",
            "module Shop\n  class Loose < Dry::Struct\n    attribute :v, ::Shop::Types::Any.constructor { |v| v.interpret_loosely }\n  end\nend\n",
        )
        .emit(roundhouse::project::BuildTarget::Spinel);
    assert!(errors.iter().all(|e| !e.contains("interpret_loosely")), "{errors:#?}");
}

/// A struct the lowering cannot read keeps its whole `Dry::Struct`
/// hierarchy as it was: a lowered parent under an unlowered child would
/// leave `attribute` calls on a plain class, and the stand-in
/// `Dry::Struct` would end the child's ancestry at a class without them.
#[test]
fn dry_struct_hierarchy_is_lowered_whole_or_not_at_all() {
    let tree = [
        ("lib/shop/types.rb", "module Shop\n  module Types\n    include Dry.Types()\n  end\nend\n"),
        ("lib/shop/base.rb", "module Shop\n  class Base < Dry::Struct\n  end\nend\n"),
        ("lib/shop/good.rb", "module Shop\n  class Good < Base\n    attribute :id, ::Shop::Types::Coercible::String\n  end\nend\n"),
        (
            "lib/shop/bad.rb",
            "module Shop\n  class Bad < Base\n    PIECES = ::Shop::Types::Array.of(::Shop::Piece)\n    attribute :amount, ::Shop::Types::Strict::String.constrained(uuid_v4: true)\n    attribute :other, Other\n    attribute :pieces, PIECES\n    attribute :inner do\n      attribute :n, ::Shop::Types::Coercible::Integer\n    end\n  end\nend\n",
        ),
        ("lib/shop/other.rb", "module Shop\n  class Other < Dry::Struct\n    attribute :id, ::Shop::Types::Coercible::String\n  end\nend\n"),
        (
            "lib/shop/kept.rb",
            "module Shop\n  class Kept < Dry::Struct\n    CODE = ::Shop::Types::Coercible::String\n    attribute :id, CODE\n  end\nend\n",
        ),
        ("lib/shop/piece.rb", "module Shop\n  class Piece < Dry::Struct\n    attribute :id, ::Shop::Types::Coercible::String\n  end\nend\n"),
        ("lib/shop/bulk.rb", "module Shop\n  class Bulk < Dry::Struct\n    attributes(id: ::Shop::Types::Coercible::String)\n  end\nend\n"),
        ("lib/types.rb", "module Types\n  include Dry.Types()\nend\n"),
        (
            "lib/legacy.rb",
            "class Legacy < Dry::Struct\n  module Types\n    include Dry.Types(default: :nominal)\n  end\nend\n\nclass LegacyChild < Legacy\n  attribute :id, Types::String\nend\n",
        ),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ]
    .into_iter()
    .map(|(p, s)| (std::path::PathBuf::from(p), s.as_bytes().to_vec()))
    .collect();
    roundhouse::ingest::survey::activate();
    let app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest");
    let class = |name: &str| app.library_classes.iter().find(|lc| lc.name.0.as_str() == name).expect(name);
    assert_eq!(class("Shop::Base").parent.as_ref().map(|p| p.0.as_str()), Some("Dry::Struct"));
    assert!(class("Shop::Good").methods.iter().all(|m| m.name.as_str() != "id"), "sibling lowered");
    assert!(class("Shop::Good").unknown_calls.iter().any(|c| roundhouse::emit::ruby::emit_expr(c).starts_with("attribute")));
    // Empty `class Dry::Struct` + `Error` so nest emit stays Class-safe
    // while some hierarchies remain unlowered; the shell has no methods.
    let dry_struct = class("Dry::Struct");
    assert!(!dry_struct.is_module, "Dry::Struct shell must be a class");
    assert!(dry_struct.methods.is_empty(), "Dry::Struct shell grew methods");
    assert!(
        app.library_classes.iter().any(|lc| lc.name.0.as_str() == "Dry::Struct::Error"),
        "Error stand-in missing while Kept lowered"
    );
    assert!(class("Shop::Kept").methods.iter().any(|m| m.name.as_str() == "id"), "Kept not lowered");
    // The nested struct goes with its refused owner, which names it as a
    // type on the gem.
    assert_eq!(class("Shop::Bad::Inner").parent.as_ref().map(|p| p.0.as_str()), Some("Dry::Struct"));
    assert!(class("Shop::Bad::Inner").methods.iter().all(|m| m.name.as_str() != "n"), "nested lowered");
    // A struct a refused class names as a type stays one, in its own
    // hierarchy too.
    assert_eq!(class("Shop::Other").parent.as_ref().map(|p| p.0.as_str()), Some("Dry::Struct"));
    // ... also when it names it through a constant holding a type.
    assert_eq!(class("Shop::Piece").parent.as_ref().map(|p| p.0.as_str()), Some("Dry::Struct"));
    // Class-safe shell: dry-type constants are stripped even under partial
    // refusal — refused siblings still require Types modules, and residual
    // `Types.Constructor` / `Coercible` would crash the boot.
    assert!(
        class("Shop::Kept").constants.iter().all(|(n, _)| n.as_str() != "CODE"),
        "dry-type constant kept under Class-safe shell"
    );
    // Unmodeled class DSL refuses rather than being dropped.
    assert_eq!(class("Shop::Bulk").parent.as_ref().map(|p| p.0.as_str()), Some("Dry::Struct"));
    // `Types::String` in a subclass may be the parent's nominal `Types`,
    // which Ruby finds before the strict top-level one: refused.
    assert!(class("LegacyChild").methods.iter().all(|m| m.name.as_str() != "id"), "inherited Types guessed");
}

/// `class << self; prepend …` wraps `attribute`; lowering would drop the
/// override. Refuse the class instead.
#[test]
fn dry_struct_singleton_prepend_refuses_lowering() {
    let tree = [
        ("lib/shop/types.rb", "module Shop\n  module Types\n    include Dry.Types()\n  end\nend\n"),
        (
            "lib/shop/strict.rb",
            "module Shop\n  module StrictAttribute\n    def attribute(name, type = nil, **opts, &block)\n      super\n    end\n  end\n\n  class Strict < Dry::Struct\n    class << self\n      prepend StrictAttribute\n    end\n    attribute :id, ::Shop::Types::Coercible::String\n  end\nend\n",
        ),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ]
    .into_iter()
    .map(|(p, s)| (std::path::PathBuf::from(p), s.as_bytes().to_vec()))
    .collect();
    roundhouse::ingest::survey::activate();
    let app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest");
    let strict = app.library_classes.iter().find(|lc| lc.name.0.as_str() == "Shop::Strict").expect("Shop::Strict");
    assert_eq!(strict.parent.as_ref().map(|p| p.0.as_str()), Some("Dry::Struct"));
    assert!(strict.origin.is_none(), "prepend override was silently lowered");
    assert!(strict.methods.iter().all(|m| m.name.as_str() != "id"), "reader synthesized despite prepend");
}

/// Mixed refusal emits empty `class Dry::Struct` + `Error` so a lowered
/// sibling can raise it and a refused sibling still subclasses a Class.
#[test]
fn dry_struct_error_standin_with_partial_refusal() {
    // Minimal tree: full dry_struct::overlay leaves Types constants that
    // cannot emit while any hierarchy stays on the gem.
    let run = emit_and_run::real_blog()
        .write(
            "lib/shop/types.rb",
            "module Shop\n  module Types\n    include Dry.Types()\n  end\nend\n",
        )
        .write(
            "lib/shop/bulk.rb",
            "module Shop\n  class Bulk < Dry::Struct\n    attributes(id: ::Shop::Types::Coercible::String)\n  end\nend\n",
        )
        .write(
            "lib/shop/kept.rb",
            "module Shop\n  class Kept < Dry::Struct\n    attribute :id, ::Shop::Types::Coercible::String\n  end\nend\n",
        )
        .run_ruby(
            r#"
raise "Dry::Struct must be a Class" unless Dry::Struct.is_a?(Class)
raise "Error missing" unless defined?(Dry::Struct::Error)
raise "Bulk must subclass Dry::Struct" unless Shop::Bulk < Dry::Struct
begin
  Shop::Kept.new({})
  raise "missing id should raise"
rescue Dry::Struct::Error
end
ok = Shop::Kept.new(id: "1")
raise "Kept failed" unless ok.id == "1"
puts "dry-struct partial-refusal Error contract passed"
"#,
        );
    run.assert_passes();
    assert!(run.stdout.contains("dry-struct partial-refusal Error contract passed"));
}

/// Production-shaped partial refusal: full dry_struct overlay (Types with
/// `Constructor` / dry-type constants) plus one refused `attributes(...)`
/// sibling. Class-safe shell must strip those residuals so `models.rb`
/// boots and lowered structs still construct / raise `Dry::Struct::Error`.
#[test]
fn dry_struct_prod_shape_partial_refusal_boots() {
    let run = dry_struct::ruby_overlay()
        .write(
            "lib/shop/bulk.rb",
            "module Shop\n  class Bulk < Dry::Struct\n    attributes(id: ::Shop::Types::Coercible::String)\n  end\nend\n",
        )
        .run_ruby(
            r#"
raise "Dry::Struct must be a Class" unless Dry::Struct.is_a?(Class)
raise "Error missing" unless defined?(Dry::Struct::Error)
raise "Bulk must subclass Dry::Struct" unless Shop::Bulk < Dry::Struct
raise "plain Types value dropped" unless Shop::Kinds::LABEL == "shop"
begin
  Shop::Refund.new({})
  raise "missing id should raise"
rescue Dry::Struct::Error
end
r = Shop::Refund.new(id: "1", amount: "10", paid: true)
raise "Refund failed" unless r.id == "1" && r.amount == 10 && r.paid == true
d = Shop::Delivery.new(to: { city: "NYC" }, volume: "hi")
raise "Loud coercion lost" unless d.volume == "HI"
puts "dry-struct prod-shape partial-refusal boot passed"
"#,
        );
    run.assert_passes();
    assert!(run.stdout.contains("dry-struct prod-shape partial-refusal boot passed"));
}

/// A Concern macro that writes a `class_attribute` runs when the
/// includer loads, rather than being evaluated at compile time.
#[test]
fn concern_class_attribute_macros_run_at_class_load() {
    let run = class_attribute::overlay().run_ruby(class_attribute::ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("class_attribute contract passed"));
}

/// An explicit nil on a subclass is its value; unset reads the parent's.
#[test]
fn concern_class_attribute_set_to_nil_is_not_unset() {
    let run = class_attribute::nil_overlay().run_ruby(class_attribute::NIL_ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("class_attribute nil contract passed"));
}

/// The same Concern types without a diagnostic of any severity: the
/// macro parameters from the class-body calls (a subclass's included),
/// the helper's keywords through `**options`, `Array(...)`'s elements,
/// and the attribute from the values its methods store.
#[test]
fn concern_class_attribute_macros_are_fully_typed() {
    let controllers = [
        ("probe_controller.rb", "class ProbeController < ApplicationController\n  include PreloadableConfigurationConcern\n  preload_site_configs %w[a b], only: :show\nend\n"),
        ("own_controller.rb", "class OwnController < ProbeController\n  preload_feature_flags %w[f], only: %i[index show]\nend\n"),
        ("inherit_controller.rb", "class InheritController < ProbeController\n  def show\n    render plain: self.class._preload_definitions.length.to_s\n  end\nend\n"),
    ];
    let tree = [
        ("app/controllers/concerns/preloadable_configuration_concern.rb".to_string(), class_attribute::CONCERN.to_string()),
        ("app/controllers/application_controller.rb".to_string(), "class ApplicationController < ActionController::Base\nend\n".to_string()),
    ]
    .into_iter()
    .chain(controllers.iter().map(|(f, s)| (format!("app/controllers/{f}"), s.to_string())))
    .map(|(p, s)| (std::path::PathBuf::from(p), s.into_bytes()))
    .collect();
    let mut app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest");
    let _ = roundhouse::session::analyze_and_lower(&mut app);
    let concern: Vec<String> = roundhouse::analyze::diagnose(&app)
        .into_iter()
        .filter(|d| {
            (d.span.file.0 as usize)
                .checked_sub(1)
                .and_then(|i| app.sources.get(i))
                .is_some_and(|f| f.path.ends_with("preloadable_configuration_concern.rb"))
        })
        .map(|d| d.message)
        .collect();
    assert!(concern.is_empty(), "concern diagnostics: {concern:#?}");
    // The signature declares the parameter's own type, not the slot's.
    let probe = app.controllers.iter().find(|c| c.name.0.as_str() == "ProbeController").unwrap();
    let preload = probe.class_methods().find(|m| m.name.as_str() == "preload_site_configs").unwrap();
    let Some(roundhouse::ty::Ty::Fn { params, .. }) = &preload.signature else {
        panic!("unsigned: {:?}", preload.signature)
    };
    assert_eq!(
        params[0].ty,
        roundhouse::ty::Ty::Array { elem: std::sync::Arc::new(roundhouse::ty::Ty::Str) }.into(),
        "codes"
    );
    // `only: nil` is nil when absent, which the signature has to say.
    let add = probe.class_methods().find(|m| m.name.as_str() == "add_preload_definition").unwrap();
    let Some(roundhouse::ty::Ty::Fn { params, .. }) = &add.signature else {
        panic!("unsigned: {:?}", add.signature)
    };
    let only = params.iter().find(|p| p.name.as_str() == "only").expect("only");
    assert!(
        matches!(&*only.ty, roundhouse::ty::Ty::Union { variants } if variants.contains(&roundhouse::ty::Ty::Nil)),
        "only: {:?}",
        only.ty
    );
}

/// The harness itself: the unedited blog emits and its controller
/// suite, which renders every page, passes.
/// `reorder`, `skip_preloading!` and `preload_associations`, the three
/// Relation methods campfire's message paging (basecamp/once-campfire#292)
/// leans on. Preloading is observed by deleting the parts after loading:
/// only a widget whose parts were preloaded still sees them. An
/// `includes` load that did not skip runs the batch before that delete,
/// so both widgets still see their parts.
fn relation_paging_hooks_app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("db/schema.rb", r#"ActiveRecord::Schema.define do
  create_table "widgets", force: :cascade do |t|
    t.string "name"
  end
  create_table "parts", force: :cascade do |t|
    t.integer "widget_id"
    t.string "label"
  end
end
"#)
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\n  has_many :parts\nend\n")
        .write("app/models/part.rb", "class Part < ApplicationRecord\n  belongs_to :widget\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\n  get \"/widgets\", to: \"widgets#index\"\nend\n")
        .write("app/controllers/widgets_controller.rb", r##"class WidgetsController < ApplicationController
  def index
    reordered = Widget.order(:name).reorder(name: :desc).map { |w| w.name }.join(",")

    relation = Widget.includes(:parts).order(:name)
    widgets = relation.skip_preloading!.to_a
    relation.preload_associations(widgets.first(1))
    eager = Widget.includes(:parts).order(:name).to_a
    Part.delete_all
    counts = widgets.map { |w| w.parts.size }.join(",")
    eager_counts = eager.map { |w| w.parts.size }.join(",")

    render plain: "#{reordered}|#{counts}|#{eager_counts}"
  end
end
"##)
}

fn relation_paging_hooks_assertions() -> &'static str {
    r##"
require_relative "app/controllers/widgets_controller"
alpha = Widget.create!(name: "alpha")
beta = Widget.create!(name: "beta")
Part.create!(widget: alpha, label: "a1")
Part.create!(widget: beta, label: "b1")
controller = WidgetsController.new
controller.process_action(:index)
raise "paging hooks: #{controller.body}" unless controller.body == "beta,alpha|1,0|1,1"
puts "relation paging hooks passed"
"##
}

#[test]
fn relation_reorder_and_deferred_preloading_run() {
    relation_paging_hooks_app()
        .run_ruby(relation_paging_hooks_assertions())
        .assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn relation_reorder_and_deferred_preloading_run_on_spinel() {
    let script = format!(
        "Db.configure(\":memory:\")\nSchema.statements.each {{ |sql| Db.exec(sql) }}\nActiveRecord.adapter = SqliteAdapter\n{}",
        relation_paging_hooks_assertions()
    );
    relation_paging_hooks_app().run_spinel(&script).assert_passes();
}

/// A class method runs against the relation it is reached from: called at
/// implicit self inside a scope body (basecamp/once-campfire#292's
/// `scope :last_page, -> { last_page_of(PAGE_SIZE) }`), and called on a
/// relation no syntactic channel recognizes, here a through-association
/// (campfire's `Current.user.reachable_messages.search(q)
/// .last_page_of_matches(n)`). `a0` sorts first in the whole table but
/// belongs to the other widget and the other shop, so an unscoped page
/// answers it.
fn class_method_relation_scope_app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("db/schema.rb", r#"ActiveRecord::Schema.define do
  create_table "shops", force: :cascade do |t|
    t.string "name"
  end
  create_table "widgets", force: :cascade do |t|
    t.integer "shop_id"
    t.string "name"
  end
  create_table "parts", force: :cascade do |t|
    t.integer "widget_id"
    t.string "label"
  end
end
"#)
        .write("app/models/shop.rb", "class Shop < ApplicationRecord\n  has_many :widgets\n  has_many :shop_parts, through: :widgets, source: :parts\nend\n")
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\n  belongs_to :shop\n  has_many :parts\nend\n")
        .write("app/models/part.rb", r#"class Part < ApplicationRecord
  belongs_to :widget

  scope :by_label, -> { order(:label) }
  scope :first_label, -> { labels_page(1) }

  def self.labels_page(size)
    by_label.limit(size).pluck(:label)
  end
end
"#)
        .write("config/routes.rb", "Rails.application.routes.draw do\n  get \"/parts\", to: \"parts#index\"\nend\n")
        .write("app/controllers/parts_controller.rb", r##"class PartsController < ApplicationController
  def index
    widget = Widget.find_by(name: "alpha")
    shop = Shop.find_by(name: "north")
    render plain: "#{widget.parts.first_label.join(",")}|#{shop.shop_parts.where.not(label: nil).labels_page(1).join(",")}"
  end
end
"##)
}

fn class_method_relation_scope_assertions() -> &'static str {
    r##"
require_relative "app/controllers/parts_controller"
north = Shop.create!(name: "north")
south = Shop.create!(name: "south")
alpha = Widget.create!(name: "alpha", shop: north)
beta = Widget.create!(name: "beta", shop: south)
Part.create!(widget: alpha, label: "a2")
Part.create!(widget: alpha, label: "a1")
Part.create!(widget: beta, label: "a0")
controller = PartsController.new
controller.process_action(:index)
raise "class method lost its relation: #{controller.body}" unless controller.body == "a1|a1"
puts "class method relation scope passed"
"##
}

#[test]
fn class_methods_run_against_the_relation_they_are_reached_from() {
    class_method_relation_scope_app()
        .run_ruby(class_method_relation_scope_assertions())
        .assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn class_methods_run_against_the_relation_they_are_reached_from_on_spinel() {
    let script = format!(
        "Db.configure(\":memory:\")\nSchema.statements.each {{ |sql| Db.exec(sql) }}\nActiveRecord.adapter = SqliteAdapter\n{}",
        class_method_relation_scope_assertions()
    );
    class_method_relation_scope_app().run_spinel(&script).assert_passes();
}

/// `relation.public_send(direction, size)` where `direction` is a
/// parameter every caller passes as a Symbol literal — campfire's
/// `Page.load(relation, :first | :last, size)` after
/// basecamp/once-campfire#292. The literals are the name set, so the
/// send grounds into a static dispatch, and the counted `last(n)` lands
/// on the runtime's `last_n` because the receiver is a typed Relation.
fn param_selector_dispatch_app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("db/schema.rb", r#"ActiveRecord::Schema.define do
  create_table "widgets", force: :cascade do |t|
    t.string "name"
  end
end
"#)
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n")
        .write("app/models/pager.rb", r#"class Pager
  def self.load(relation, direction, size)
    relation.public_send(direction, size).map { |widget| widget.name }
  end
end
"#)
        .write("config/routes.rb", "Rails.application.routes.draw do\n  get \"/widgets\", to: \"widgets#index\"\nend\n")
        .write("app/controllers/widgets_controller.rb", r##"class WidgetsController < ApplicationController
  def index
    oldest = Pager.load(Widget.order(:name), :first, 2).join(",")
    newest = Pager.load(Widget.order(:name), :last, 1).join(",")
    render plain: "#{oldest}|#{newest}"
  end
end
"##)
}

fn param_selector_dispatch_assertions() -> &'static str {
    r##"
require_relative "app/controllers/widgets_controller"
%w[ beta alpha gamma ].each { |name| Widget.create!(name: name) }
controller = WidgetsController.new
controller.process_action(:index)
raise "selector dispatch: #{controller.body}" unless controller.body == "alpha,beta|gamma"
puts "param selector dispatch passed"
"##
}

#[test]
fn a_send_whose_selector_every_caller_names_dispatches_statically() {
    param_selector_dispatch_app()
        .run_ruby(param_selector_dispatch_assertions())
        .assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_send_whose_selector_every_caller_names_dispatches_statically_on_spinel() {
    let script = format!(
        "Db.configure(\":memory:\")\nSchema.statements.each {{ |sql| Db.exec(sql) }}\nActiveRecord.adapter = SqliteAdapter\n{}",
        param_selector_dispatch_assertions()
    );
    param_selector_dispatch_app().run_spinel(&script).assert_passes();
}

/// assert_select attribute operators (`$=`, `^=`, `*=`) and `:not([…])`,
/// and assert_response's failure message. Rails 8.2-era tests write all
/// of them: basecamp/once-campfire#301 checks `img[src*='install-edge']`,
/// #303 checks `input[type=checkbox]:not([checked])` and passes a message
/// to assert_response. A `?` in a predicate is Rails' substitution value
/// (campfire's sidebar test: `"#row[data-sorted-list-number=?]", epoch`).
#[test]
fn assert_select_attribute_operators_and_negation_run() {
    emit_and_run::real_blog()
        .write(
            "test/controllers/article_selectors_controller_test.rb",
            r#"require "test_helper"

class ArticleSelectorsControllerTest < ActionDispatch::IntegrationTest
  test "attribute operators, negation and a response message" do
    article = Article.create!(title: "Selectors", body: "Body text here")
    get article_url(article)
    assert_response :success, "the article page"
    assert_select "a[href$='/edit']"
    assert_select "a[href^='/articles/']"
    assert_select "a[href*='articles']"
    assert_select "h1[class]:not([hidden])"
    assert_select "a[href=?]", edit_article_path(article)
  end
end
"#,
        )
        .run_test("test/controllers/article_selectors_controller_test.rb")
        .assert_passes();
}

/// `redirect(path: ...)` is Rails' options form, and unlike the
/// positional `redirect("/x")` it keeps the request's query string: an
/// empty query leaves the path alone, a path that already has a `?` is
/// joined with `&`, and the query goes ahead of a fragment. The last two
/// diverge from Rails on purpose, since Rails builds `/articles?sort=new?page=2`
/// and `/articles#top?page=2` (see `synthesize_redirect_controller`).
/// A `%{id}` is decoded by the router and path-escaped again, in both
/// redirect forms, as Rails does: an encoded `#` or `?` stays in the
/// path, `%25` survives, and a non-ASCII or control sequence the router
/// leaves encoded is not escaped twice. Each expectation is Rails 8.1.4's.
/// Jumpstart Pro routes its Devise-era `/users/sign_in` this way.
#[test]
fn a_path_option_redirect_keeps_the_request_query() {
    emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  get \"/posts\", to: redirect(path: \"/articles\")\n  get \"/filtered\", to: redirect(path: \"/articles?sort=new\")\n  get \"/step\", to: redirect(path: \"/articles#top\")\n  get \"/old/:id\", to: redirect(path: \"/articles/%{id}\", status: 302)\n  get \"/legacy/:id\", to: redirect(\"/articles/%{id}\")\n",
        )
        .write(
            "test/controllers/path_redirects_controller_test.rb",
            r#"require "test_helper"

class PathRedirectsControllerTest < ActionDispatch::IntegrationTest
  test "the options form keeps the query" do
    get "/posts"
    assert_response 301
    assert_redirected_to "/articles"
    get "/posts?page=2"
    assert_redirected_to "/articles?page=2"
    get "/filtered?page=2"
    assert_redirected_to "/articles?sort=new&page=2"
    get "/step?page=2"
    assert_redirected_to "/articles?page=2#top"
    get "/filtered"
    assert_redirected_to "/articles?sort=new"
    get "/old/7?page=2"
    assert_response 302
    assert_redirected_to "/articles/7?page=2"
    get "/old/a%23top?page=2"
    assert_redirected_to "/articles/a%23top?page=2"
    get "/legacy/a%20b%3Fc"
    assert_redirected_to "/articles/a%20b%3Fc"
    get "/old/jos%C3%A9"
    assert_redirected_to "/articles/jos%C3%A9"
    get "/old/100%25"
    assert_redirected_to "/articles/100%25"
    get "/legacy/a%2Fb%7e"
    assert_redirected_to "/articles/a/b~"
    get "/old/x%0Ay"
    assert_redirected_to "/articles/x%0Ay"
  end
end
"#,
        )
        .run_test("test/controllers/path_redirects_controller_test.rb")
        .assert_passes();
}

/// A path parameter is percent-decoded the way Rails' router decodes it
/// (`CGI.unescapeURIComponent`): `%20` is a space, `%2F`/`%2f` a slash
/// inside the one segment, `%25` a percent, and a `+` stays a `+`. A
/// glob capture is decoded too. Every expectation is what Rails 8.1.4
/// answers for the same request.
#[test]
fn a_path_parameter_is_percent_decoded() {
    emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  get \"/echo/:id\", to: \"echoes#show\"\n  get \"/files/*rest\", to: \"echoes#glob\"\n",
        )
        .write(
            "app/controllers/echoes_controller.rb",
            "class EchoesController < ApplicationController\n  def show\n    render plain: params[:id]\n  end\n\n  def glob\n    render plain: params[:rest]\n  end\nend\n",
        )
        .write(
            "test/controllers/echoes_controller_test.rb",
            r#"require "test_helper"

class EchoesControllerTest < ActionDispatch::IntegrationTest
  test "path parameters are percent-decoded" do
    get "/echo/a%20b+c"
    assert_equal "a b+c", response.body
    get "/echo/a%2Fb"
    assert_equal "a/b", response.body
    get "/echo/a%2fb%3F%23"
    assert_equal "a/b?#", response.body
    get "/echo/100%25"
    assert_equal "100%", response.body
    get "/echo/plain"
    assert_equal "plain", response.body
    get "/files/a%20b/c%2Fd"
    assert_equal "a b/c/d", response.body
  end
end
"#,
        )
        .run_test("test/controllers/echoes_controller_test.rb")
        .assert_passes();
}

/// `Date.parse(params[:from])` types the way a String argument does: a
/// request parameter is a union whose other arms (nil, an Array, nested
/// params) raise in Rails as well. The calendar sends after it run too:
/// a week that starts on a named day, and `in_time_zone` with a zone
/// that may be nil. Expected values are Rails 8.1's.
#[test]
fn date_parse_of_a_request_parameter_runs() {
    emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  get \"/week\", to: \"weeks#show\"\n",
        )
        .write(
            "app/controllers/weeks_controller.rb",
            r#"class WeeksController < ApplicationController
  def show
    from = Date.parse(params[:from])
    zone = Article.new.title
    at = Time.utc(2024, 2, 15, 10, 0, 0)
    render plain: [
      from.iso8601, from.beginning_of_month.iso8601, Date.parse(params.require(:from)).year,
      from.beginning_of_week.iso8601, from.beginning_of_week(:sunday).iso8601, from.end_of_week(:sunday).iso8601,
      at.beginning_of_week(:sunday).day, at.end_of_week(:wednesday).day,
      from.in_time_zone(zone).strftime("%H:%M")
    ].join(" ")
  end
end
"#,
        )
        .write(
            "test/controllers/weeks_controller_test.rb",
            r#"require "test_helper"

class WeeksControllerTest < ActionDispatch::IntegrationTest
  test "a request parameter parses as a date" do
    get "/week", params: { from: "2024-02-15" }
    assert_equal "2024-02-15 2024-02-01 2024 2024-02-12 2024-02-11 2024-02-17 11 20 00:00", response.body
  end
end
"#,
        )
        .run_test("test/controllers/weeks_controller_test.rb")
        .assert_passes();
}

/// `Result = Struct.new(…, keyword_init: true)` in a class body is the
/// class it defines: its members, their nil default, and the methods
/// its block adds. Expected values are Ruby's own.
#[test]
fn a_struct_constant_runs_as_its_class() {
    emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  get \"/render\", to: \"renders#show\"\n",
        )
        .write(
            "app/services/renderer.rb",
            r#"class Renderer
  Result = Struct.new(:title, :html, keyword_init: true)
  Pair = Struct.new(:left, :right, keyword_init: true) do
    def total
      left + right
    end
  end

  def call(text)
    Result.new(title: text, html: "<p>#{text}</p>")
  end

  def pair
    Pair.new(left: 1, right: 2)
  end

  def half
    Pair.new(left: 3)
  end
end
"#,
        )
        .write(
            "app/controllers/renders_controller.rb",
            r#"class RendersController < ApplicationController
  def show
    result = Renderer.new.call(params[:text].to_s)
    result.title = result.title.upcase
    pair = Renderer.new.pair
    render plain: [result.title, result.html, pair.total, pair.left, Renderer.new.half.right.nil?].join(" ")
  end
end
"#,
        )
        .write(
            "test/controllers/renders_controller_test.rb",
            r#"require "test_helper"

class RendersControllerTest < ActionDispatch::IntegrationTest
  test "struct constants build their classes" do
    get "/render", params: { text: "hi" }
    assert_equal "HI <p>hi</p> 3 1 true", response.body
  end
end
"#,
        )
        .run_test("test/controllers/renders_controller_test.rb")
        .assert_passes();
}

/// The positional `Result = Struct.new(:a, :b)` in a class body is the
/// class it defines too: members in order, a missing one nil, writers,
/// and the methods its block adds. Expected values are Ruby's own.
#[test]
fn a_positional_struct_constant_runs_as_its_class() {
    emit_and_run::real_blog()
        .write(
            "app/services/phone_check.rb",
            r#"class PhoneCheck
  Normalized = Struct.new(:number, :valid, :error)
  Point = Struct.new(:x, :y) do
    def sum
      x + y
    end
  end

  def self.call(raw)
    return Normalized.new(nil, false, "blank") if raw.empty?
    Normalized.new(raw.delete("-"), true)
  end

  def self.point
    Point.new(1, 2)
  end
end
"#,
        )
        .run_ruby(
            r#"ok = PhoneCheck.call("912-345")
raise ok.to_s unless ok.number == "912345" && ok.valid == true && ok.error.nil?
bad = PhoneCheck.call("")
raise bad.to_s unless bad.number.nil? && bad.valid == false && bad.error == "blank"
point = PhoneCheck.point
point.y = 40
raise point.sum.to_s unless point.sum == 41"#,
        )
        .assert_passes();
}

/// `CONST.each do |k[, v]| define_method(…) { … } end` over a literal
/// constant is the `def`s it makes: an Array's names, a Hash's names and
/// values (read back from the constant), instance and class side, and a
/// literal Array written in place. Expected values are Ruby's own.
#[test]
fn define_method_loops_over_literal_constants_run() {
    emit_and_run::real_blog()
        .write(
            "app/services/labels.rb",
            r##"class Labels
  KINDS = %i[draft live].freeze
  PREFIXES = { short: "S", long: "L" }.freeze

  KINDS.each do |kind|
    define_method(:"#{kind}?") do
      @kind == kind
    end
  end

  %w[red blue].each do |color|
    define_method("#{color}_label") do |text|
      "#{color}:#{text}"
    end
  end

  class << self
    PREFIXES.each do |slug, prefix|
      define_method(:"#{slug}_prefix") do
        "#{prefix}-#{slug}"
      end
    end
  end

  def initialize(kind)
    @kind = kind
  end
end
"##,
        )
        .run_ruby(
            r##"draft = Labels.new(:draft)
raise "draft?" unless draft.draft? && !draft.live?
raise draft.red_label("x") unless draft.red_label("x") == "red:x" && draft.blue_label(1) == "blue:1"
raise Labels.short_prefix unless Labels.short_prefix == "S-short" && Labels.long_prefix == "L-long""##,
        )
        .assert_passes();
}

/// Since Ruby 3.2 a positional Struct also takes keywords, by member name,
/// which the emitted positional constructor would bind to its first
/// member: refused. A block defining its own `initialize` is not read as
/// the class at all: its `super` reaches the Struct parent in Ruby, which
/// the synthesized class does not have.
#[test]
fn a_positional_struct_constant_refuses_keyword_construction() {
    let errors = emit_and_run::real_blog()
        .write(
            "app/services/phone_check.rb",
            "class PhoneCheck\n  Normalized = Struct.new(:number, :valid)\n\n  def self.call(raw)\n    Normalized.new(number: raw, valid: true)\n  end\nend\n",
        )
        .check_errors();
    assert!(errors.iter().any(|e| e.contains("Struct keyword construction")), "{errors:?}");

    // `keyword_init: false` written out: Ruby passes the keywords as one Hash
    // to the first member, which inference does not model: refused too.
    let errors = emit_and_run::real_blog()
        .write(
            "app/services/phone_check.rb",
            "class PhoneCheck\n  Normalized = Struct.new(:number, :valid, keyword_init: false)\n\n  def self.call(raw)\n    Normalized.new(number: raw, valid: true)\n  end\nend\n",
        )
        .check_errors();
    assert!(errors.iter().any(|e| e.contains("Struct keyword construction")), "{errors:?}");

    // A subclass with a class-side `new` of its own decides what `new` takes.
    let errors = emit_and_run::real_blog()
        .write(
            "app/services/phone_check.rb",
            "class PhoneCheck\n  Normalized = Struct.new(:number, :valid)\n\n  class Named < Normalized\n    def self.new(number:, valid:)\n      super(number, valid)\n    end\n  end\n\n  def self.call(raw)\n    Named.new(number: raw, valid: true)\n  end\nend\n",
        )
        .check_errors();
    assert!(!errors.iter().any(|e| e.contains("Struct keyword construction")), "{errors:?}");

    let errors = emit_and_run::real_blog()
        .write(
            "app/services/phone_check.rb",
            "class PhoneCheck\n  Normalized = Struct.new(:number, :valid) do\n    def initialize(number:, valid:)\n      super(number, valid)\n    end\n  end\n\n  def self.call(raw)\n    Normalized.new(number: raw, valid: true)\n  end\nend\n",
        )
        .check_errors();
    assert!(!errors.is_empty(), "a Struct block with its own initialize checked clean");
}

/// The same loop in a concern's `included do class << self … end` is the
/// includer's class methods, beside the block's plain `def`s.
#[test]
fn define_method_loops_in_an_included_class_body_reach_the_includer() {
    emit_and_run::real_blog()
        .write(
            "app/models/concerns/lookups.rb",
            r##"module Lookups
  extend ActiveSupport::Concern

  MAP = { short: "S", long: "L" }.freeze

  included do
    class << self
      MAP.each do |slug, value|
        define_method(:"#{slug}_code") do
          "#{value}#{value}"
        end
      end

      def plain
        "p"
      end
    end
  end
end
"##,
        )
        .edit("app/models/article.rb", "class Article < ApplicationRecord\n", "class Article < ApplicationRecord\n  include Lookups\n")
        .run_ruby(r##"raise [Article.short_code, Article.long_code, Article.plain].inspect unless [Article.short_code, Article.long_code, Article.plain] == ["SS", "LL", "p"]"##)
        .assert_passes();
}

/// A loop whose body the substitution cannot keep is not unrolled: no
/// `def` is invented for it.
#[test]
fn define_method_loops_the_substitution_cannot_keep_are_not_unrolled() {
    for body in [
        // writes the loop parameter
        "KINDS.each do |kind|\n    define_method(:\"#{kind}?\") do\n      kind = :other\n      kind\n    end\n  end",
        // reads a local outside the loop
        "suffix = \"!\"\n  KINDS.each do |kind|\n    define_method(:\"#{kind}?\") do\n      suffix\n    end\n  end",
        // yields the defining method's block
        "KINDS.each do |kind|\n    define_method(:\"#{kind}?\") do\n      yield\n    end\n  end",
        // a constant whose value is not a literal
        "LIST.each do |kind|\n    define_method(:\"#{kind}?\") do\n      kind\n    end\n  end",
        // `next` at the block's own level does not parse inside a `def`
        "KINDS.each do |kind|\n    define_method(:\"#{kind}?\") do\n      next false if kind == :draft\n      true\n    end\n  end",
        // a named visibility marker sets a generated method's visibility
        "def draft?\n    true\n  end\n\n  KINDS.each do |kind|\n    define_method(:\"#{kind}?\") do\n      kind\n    end\n  end\n  private :draft?",
    ] {
        let (emitted, _errors) = emit_and_run::real_blog()
            .write(
                "app/services/labels.rb",
                &format!("class Labels\n  KINDS = %i[draft live].freeze\n  LIST = KINDS.map(&:itself)\n\n  {body}\nend\n"),
            )
            .emit(roundhouse::project::BuildTarget::Ruby);
        let labels = std::fs::read_to_string(emitted.join("app/models/labels.rb")).expect("labels.rb");
        assert!(!labels.contains("def live?"), "{body}:\n{labels}");
    }
}

/// `Float#to_d` / `Integer#to_d` build a BigDecimal without
/// `bigdecimal/util`, and the decimal arithmetic after them runs:
/// Float and Integer operands, comparison, `round`, and `floor`/`ceil`
/// with and without digits. `0.1 + 0.2` keeps `Float#to_d`'s 16 digits,
/// and exponent-form floats (`Float#to_s`'s `1.0e-05`) convert as well.
/// Expected values are CRuby 3.4's with bigdecimal 4.1.
#[test]
fn float_to_d_and_decimal_arithmetic_run() {
    emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  get \"/decimal\", to: \"decimals#show\"\n",
        )
        .write(
            "app/controllers/decimals_controller.rb",
            r#"class DecimalsController < ApplicationController
  def show
    f = params[:f].to_s.to_f
    i = params[:i].to_s.to_i
    g = params[:a].to_s.to_f + params[:b].to_s.to_f
    a = BigDecimal("1.555")
    price = f.to_d * i.to_d
    render plain: [
      price.to_s, (price - a).to_s, (a / 7).floor(2).to_s, a.round(2).to_s, a.round, a.floor,
      a.ceil(1).to_s, (f - a).to_s, (i * a).to_s, a.to_f, price > a, g.to_d.to_s, (-a).abs.to_s, a.round(0),
      a.to_d.to_s,
      params[:exps].to_s.split(",").map { |e| e.to_f.to_d.to_s }.join(",")
    ].join(" ")
  end
end
"#,
        )
        .write(
            "test/controllers/decimals_controller_test.rb",
            r#"require "test_helper"

class DecimalsControllerTest < ActionDispatch::IntegrationTest
  test "decimal arithmetic matches CRuby" do
    get "/decimal", params: { f: "2.5", i: "3", a: "0.1", b: "0.2", exps: "1.0e-5,1e20,-1.0e-10,1.2345678901234567e20,-1.2345678901234567e-20" }
    assert_equal "0.75e1 0.5945e1 0.22e0 0.156e1 2 1 0.16e1 0.945e0 0.4665e1 1.555 true 0.3e0 0.1555e1 2 0.1555e1 " \
      "0.1e-4,0.1e21,-0.1e-9,0.1234567890123456e21,-0.1234567890123456e-19", response.body
  end
end
"#,
        )
        .run_test("test/controllers/decimals_controller_test.rb")
        .assert_passes();
}

/// An Integer range's Enumerable calls (`map`, `filter_map`, `select`,
/// `reduce`, `flat_map`, `each_with_object`) run through `to_a`, on an
/// inclusive and an exclusive range and a constant bound.
#[test]
fn integer_range_enumerable_calls_run() {
    emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  get \"/months\", to: \"months#show\"\n",
        )
        .write(
            "app/controllers/months_controller.rb",
            r#"class MonthsController < ApplicationController
  MAX = 3

  def show
    a = (1..12).map { |m| "month#{m}" }
    b = (1..MAX).filter_map { |i| i.even? ? i * 2 : nil }
    c = (1..5).select { |i| i > 2 }
    d = (1..4).reduce(0) { |s, i| s + i }
    e = (1..3).flat_map { |i| [i, i] }
    f = (1..3).each_with_object({}) { |i, h| h[i] = i * i }
    g = (2...5).map { |i| i * 10 }
    render plain: [a.first, a.size, b.sum, c.size, d, e.size, f[3], g.join("-")].join(" ")
  end
end
"#,
        )
        .write(
            "test/controllers/months_controller_test.rb",
            r#"require "test_helper"

class MonthsControllerTest < ActionDispatch::IntegrationTest
  test "range enumerable calls" do
    get "/months"
    assert_equal "month1 12 4 3 10 6 9 20-30-40", response.body
  end
end
"#,
        )
        .run_test("test/controllers/months_controller_test.rb")
        .assert_passes();
}

/// The range reaches those calls as an Array, the shape every target
/// answers: the emitted Ruby reads `(1..n).to_a.filter_map`, and a call
/// that answers its receiver (`each_with_index`) is left on the range.
#[test]
fn integer_range_enumerable_calls_go_through_to_a() {
    let (emitted, _errors) = emit_and_run::real_blog()
        .write(
            "app/models/calendar.rb",
            "class Calendar\n  def self.even_doubles(n)\n    (1..n).filter_map { |i| i.even? ? i * 2 : nil }\n  end\n\n  def self.walk\n    (1..3).each_with_index { |i, at| i + at }\n  end\nend\n",
        )
        .emit(roundhouse::project::BuildTarget::Ruby);
    let source = walk_files(&emitted)
        .into_iter()
        .find(|text| text.contains("def self.even_doubles"))
        .expect("Calendar emitted");
    assert!(source.contains("(1..n).to_a.filter_map"), "{source}");
    assert!(source.contains("(1..3).each_with_index"), "{source}");
}

/// A test body takes the same rewrites as app code: the analyzer types
/// `(1..3).filter_map` and `2.5.to_d` there too, so without them the
/// emitted test would call a range method or a `bigdecimal/util`
/// reopen no target provides.
#[test]
fn range_and_to_d_rewrites_reach_test_bodies() {
    emit_and_run::real_blog()
        .write(
            "test/models/rewrites_in_tests_test.rb",
            r#"require "test_helper"

class RewritesInTestsTest < ActiveSupport::TestCase
  test "range and decimal calls in a test body" do
    assert_equal [4], (1..3).filter_map { |i| i.even? ? i * 2 : nil }
    assert_equal "0.25e1", 2.5.to_d.to_s
  end
end
"#,
        )
        .run_test("test/models/rewrites_in_tests_test.rb")
        .assert_passes();
    let (emitted, _errors) = emit_and_run::real_blog()
        .write(
            "test/models/rewrites_in_tests_test.rb",
            "require \"test_helper\"\n\nclass RewritesInTestsTest < ActiveSupport::TestCase\n  test \"range\" do\n    assert_equal [4], (1..3).filter_map { |i| i.even? ? i * 2 : nil }\n  end\nend\n",
        )
        .emit(roundhouse::project::BuildTarget::Ruby);
    let test = std::fs::read_to_string(emitted.join("test/models/rewrites_in_tests_test.rb")).expect("emitted test");
    assert!(test.contains("(1..3).to_a.filter_map"), "{test}");
}

fn walk_files(dir: &std::path::Path) -> Vec<String> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walk_files(&path));
        } else if path.extension().is_some_and(|e| e == "rb") {
            out.extend(std::fs::read_to_string(&path).ok());
        }
    }
    out
}

/// Rails' own exception classes the app raises answer the status
/// Rails' rescue_responses give them, through the production
/// dispatcher: `ActionController::BadRequest` 400,
/// `ActiveRecord::RecordNotSaved` / `RecordInvalid` /
/// `ActionController::InvalidAuthenticityToken` 422. A rescued
/// `ActiveRecord::ReadOnlyRecord` carries its message.
#[test]
fn rails_exception_classes_answer_their_status() {
    emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  get \"/fail/:kind\", to: \"failures#show\"\n",
        )
        .write(
            "app/controllers/failures_controller.rb",
            r#"class FailuresController < ApplicationController
  def show
    case params[:kind]
    when "bad" then raise ActionController::BadRequest, "nope"
    when "not_saved" then raise ActiveRecord::RecordNotSaved.new("not saved", Article.new)
    when "invalid" then Article.new(title: "").save!
    when "token" then raise ActionController::InvalidAuthenticityToken
    when "readonly"
      begin
        raise ActiveRecord::ReadOnlyRecord, "frozen"
      rescue ActiveRecord::ReadOnlyRecord => e
        render plain: e.message
      end
    else
      render plain: "ok"
    end
  end
end
"#,
        )
        .run_ruby(
            r##"def call(path)
  status, _headers, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => path, "QUERY_STRING" => "", "rack.input" => StringIO.new(""))
  [status, body.respond_to?(:join) ? body.join : body.to_s]
end
{
  "/fail/bad" => 400,
  "/fail/not_saved" => 422,
  "/fail/invalid" => 422,
  "/fail/token" => 422,
  "/fail/none" => 200,
  "/fail/readonly" => 200,
}.each do |path, want|
  got, = call(path)
  raise "#{path} answered #{got}, want #{want}" unless got == want
end
_, body = call("/fail/readonly")
raise "readonly body #{body.inspect}" unless body.include?("frozen")
"##,
        )
        .assert_passes();
}

/// `ActiveRecord::Type::Boolean` casts as `ActiveModel::Type::Boolean`
/// does (its parent), and `deserialize` casts the same way for a
/// boolean. `ActiveSupport::StringInquirer.new(x)` is `x.inquiry`: a
/// method returning it answers `<label>?` as a comparison with the
/// label. Expected values are Rails 8.1's.
#[test]
fn ar_boolean_type_and_string_inquirer_run() {
    emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  get \"/flags\", to: \"flags#show\"\n",
        )
        .write(
            "app/services/deploy_env.rb",
            r#"class DeployEnv
  def self.current(name)
    ActiveSupport::StringInquirer.new(name.presence || "development")
  end

  def self.live?(name)
    current(name).production?
  end
end
"#,
        )
        .write(
            "app/controllers/flags_controller.rb",
            r#"class FlagsController < ApplicationController
  def show
    cast = ActiveRecord::Type::Boolean.new.cast(params[:a])
    stored = ActiveRecord::Type::Boolean.new.deserialize(params[:b])
    blank = ActiveRecord::Type::Boolean.new.cast(params[:c])
    render plain: [cast, stored, blank.nil?, DeployEnv.live?(params[:env].to_s), DeployEnv.live?(""), DeployEnv.current("").development?].join(" ")
  end
end
"#,
        )
        .write(
            "test/controllers/flags_controller_test.rb",
            r#"require "test_helper"

class FlagsControllerTest < ActionDispatch::IntegrationTest
  test "boolean casts and inquirer predicates" do
    get "/flags", params: { a: "0", b: "t", c: "", env: "production" }
    assert_equal "false true true true false true", response.body
  end
end
"#,
        )
        .run_test("test/controllers/flags_controller_test.rb")
        .assert_passes();
}

/// `ActiveRecord::Associations::Preloader.new(records:, associations:).call`
/// is the model's own `preload_associations`, over an Array of one model
/// or a relation. The association reads after it answer what they
/// would have without the preload.
#[test]
fn associations_preloader_call_runs() {
    emit_and_run::real_blog()
        .write(
            "app/models/comment_digest.rb",
            r#"class CommentDigest
  def self.titles
    comments = Comment.order(:id).to_a
    ActiveRecord::Associations::Preloader.new(records: comments, associations: :article).call
    ActiveRecord::Associations::Preloader.new(records: Article.all, associations: [:comments]).call
    comments.map { |c| c.article.title }.join(",") + " " + Article.order(:id).map { |a| a.comments.size }.join(",")
  end
end
"#,
        )
        .run_ruby(
            r##"a = Article.create!(title: "First", body: "A body long enough")
b = Article.create!(title: "Second", body: "Another body long enough")
Comment.create!(article_id: a.id, commenter: "x", body: "one")
Comment.create!(article_id: b.id, commenter: "y", body: "two")
Comment.create!(article_id: b.id, commenter: "z", body: "three")
got = CommentDigest.titles
raise "got #{got.inspect}" unless got == "First,Second,Second 1,2"
"##,
        )
        .assert_passes();
}

/// Records of several models, handed over as a parameter analysis does
/// not type, are grouped by class and each group preloaded, as Rails'
/// preloader does: reading the association afterwards runs no query.
fn mixed_preload_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "  add_foreign_key \"comments\", \"articles\"\n",
            "  create_table \"notes\", force: :cascade do |t|\n    t.integer \"article_id\", null: false\n    t.string \"text\"\n  end\n\n  add_foreign_key \"comments\", \"articles\"\n",
        )
        .write("app/models/note.rb", "class Note < ApplicationRecord\n  belongs_to :article\nend\n")
        .write(
            "app/models/preload_digest.rb",
            r#"class PreloadDigest
  def self.preload(records)
    ActiveRecord::Associations::Preloader.new(records: records, associations: :article).call
    nil
  end
end
"#,
        )
}

#[test]
fn associations_preloader_groups_mixed_records() {
    mixed_preload_app()
        .run_ruby(
            r##"a = Article.create!(title: "First", body: "A body long enough")
b = Article.create!(title: "Second", body: "Another body long enough")
Comment.create!(article_id: a.id, commenter: "x", body: "one")
Comment.create!(article_id: b.id, commenter: "y", body: "two")
Note.create!(article_id: b.id, text: "n")
records = Comment.all.to_a + Note.all.to_a
PreloadDigest.preload(records)
titles = nil
queries = Db.capture_sql { titles = records.map { |r| r.article.title }.join(",") }
raise "got #{titles.inspect}" unless titles == "First,Second,Second"
raise "reads ran #{queries.length} queries: #{queries.inspect}" unless queries.empty?
"##,
        )
        .assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn associations_preloader_groups_mixed_records_on_spinel() {
    let script = r#"Db.configure(":memory:")
Schema.statements.each { |sql| Db.exec(sql) }
ActiveRecord.adapter = SqliteAdapter
a = Article.create!(title: "First", body: "A body long enough")
b = Article.create!(title: "Second", body: "Another body long enough")
Comment.create!(article_id: a.id, commenter: "x", body: "one")
Comment.create!(article_id: b.id, commenter: "y", body: "two")
Note.create!(article_id: b.id, text: "n")
records = Comment.all.to_a + Note.all.to_a
PreloadDigest.preload(records)
titles = ""
queries = Db.capture_sql { titles = records.map { |r| r.article.title }.join(",") }
puts titles
puts queries.length
"#;
    let run = mixed_preload_app().run_spinel(script);
    run.assert_passes();
    assert_eq!(run.stdout, "First,Second,Second\n0\n");
}

/// `Model.where(…).delete_all` / `update_all` / `destroy_all` /
/// `touch_all` run against the relation: lifted into a SELECT, the
/// receiver would be the hydrated rows, which answer none of them.
fn bulk_writes_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "  has_many :comments, dependent: :destroy\n",
            concat!(
                "  has_many :comments, dependent: :destroy\n\n",
                "  def rename_comments\n    Comment.where(article_id: @id).update_all(commenter: \"renamed\")\n    Comment.where(article_id: @id).touch_all\n  end\n\n",
                "  def drop_comments\n    Comment.where(article_id: @id).delete_all\n  end\n\n",
                "  def drop_self\n    Article.where(id: @id).destroy_all\n  end\n",
            ),
        )
}

const BULK_WRITES_SCRIPT: &str = r##"a = Article.create!(title: "Keep", body: "A body long enough")
b = Article.create!(title: "Drop", body: "Another body long enough")
Comment.create!(article_id: a.id, commenter: "x", body: "one")
Comment.create!(article_id: b.id, commenter: "y", body: "two")
a.rename_comments
b.drop_comments
b.drop_self
raise "comments: #{Comment.count}" unless Comment.count == 1
raise "renamed: #{Comment.first.commenter}" unless Comment.first.commenter == "renamed"
raise "articles: #{Article.count}" unless Article.count == 1
puts "ok"
"##;

#[test]
fn bulk_writes_on_a_where_run_against_the_relation() {
    bulk_writes_app().run_ruby(BULK_WRITES_SCRIPT).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn bulk_writes_on_a_where_run_against_the_relation_on_spinel() {
    let run = bulk_writes_app().run_spinel(&format!("Db.configure(\":memory:\")\nSchema.statements.each {{ |sql| Db.exec(sql) }}\nActiveRecord.adapter = SqliteAdapter\n{BULK_WRITES_SCRIPT}"));
    run.assert_passes();
    assert_eq!(run.stdout, "ok\n");
}

/// `dependent: :delete_all`, `has_one … dependent: :delete` and
/// `dependent: :nullify` act on the children in one statement each,
/// running none of their callbacks, as Rails' do.
fn dependent_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "  add_foreign_key \"comments\", \"articles\"\n",
            concat!(
                "  create_table \"notes\", force: :cascade do |t|\n    t.integer \"article_id\"\n    t.string \"text\"\n  end\n\n",
                "  create_table \"summaries\", force: :cascade do |t|\n    t.integer \"article_id\"\n    t.string \"text\"\n  end\n\n",
                "  add_foreign_key \"comments\", \"articles\"\n",
            ),
        )
        .edit("app/models/article.rb", "  has_many :comments, dependent: :destroy\n", "  has_many :comments, dependent: :delete_all\n  has_many :tagged_notes, -> { where(text: \"tag\") }, class_name: \"Note\", dependent: :delete_all\n  has_many :notes, dependent: :nullify\n  has_one :summary, dependent: :delete\n")
        .edit("app/models/comment.rb", "  belongs_to :article\n", "  belongs_to :article\n  before_destroy { raise \"a delete_all child runs no callback\" }\n")
        .write("app/models/note.rb", "class Note < ApplicationRecord\n  belongs_to :article, optional: true\nend\n")
        .write("app/models/summary.rb", "class Summary < ApplicationRecord\n  belongs_to :article, optional: true\nend\n")
}

const DEPENDENT_SCRIPT: &str = r##"a = Article.create!(title: "Gone", body: "A body long enough")
b = Article.create!(title: "Kept", body: "Another body long enough")
Comment.create!(article_id: a.id, commenter: "x", body: "one")
Comment.create!(article_id: b.id, commenter: "y", body: "two")
n = Note.create!(article_id: a.id, text: "n")
Note.create!(article_id: a.id, text: "tag")
Summary.create!(article_id: a.id, text: "s")
Summary.create!(article_id: b.id, text: "t")
a.destroy
raise "comments left: #{Comment.count}" unless Comment.count == 1
raise "summaries left: #{Summary.count}" unless Summary.count == 1
raise "note kept its key" unless Note.find(n.id).article_id.nil?
raise "notes left: #{Note.count}" unless Note.count == 1
puts "ok"
"##;

#[test]
fn dependent_delete_all_delete_and_nullify_run() {
    dependent_app().run_ruby(DEPENDENT_SCRIPT).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn dependent_delete_all_delete_and_nullify_run_on_spinel() {
    let run = dependent_app().run_spinel(&format!("Db.configure(\":memory:\")\nSchema.statements.each {{ |sql| Db.exec(sql) }}\nActiveRecord.adapter = SqliteAdapter\n{DEPENDENT_SCRIPT}"));
    run.assert_passes();
    assert_eq!(run.stdout, "ok\n");
}

/// A strict target loads no `runtime/ruby/active_record/preloading.rb`, so
/// mixed-model records keep the call unsupported there rather than emit a
/// call to nothing.
#[test]
fn mixed_records_preloading_is_unsupported_on_strict_targets() {
    let (_emitted, errors) = mixed_preload_app().emit(roundhouse::project::BuildTarget::Rust);
    assert!(
        errors.iter().any(|e| e.contains("ActiveRecord::Preloading.records is only available on the ruby family and spinel")),
        "{errors:#?}"
    );
}

/// A Preloader call whose value is dropped by a `tap` block or by an
/// `if` branch whose `if` is a statement preloads like one standing alone.
fn tap_digest_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write(
            "app/models/tap_digest.rb",
            r#"class TapDigest
  def self.tapped
    Comment.all.to_a.tap do |records|
      ActiveRecord::Associations::Preloader.new(records: records, associations: :article).call
    end
  end

  def self.branched(records)
    if records.any?
      ActiveRecord::Associations::Preloader.new(records: records, associations: :article).call
    end
    records
  end
end
"#,
        )
}

const TAP_DIGEST_SCRIPT: &str = r##"a = Article.create!(title: "First", body: "A body long enough")
Comment.create!(article_id: a.id, commenter: "x", body: "one")
Comment.create!(article_id: a.id, commenter: "y", body: "two")
tapped = TapDigest.tapped
branched = TapDigest.branched(Comment.all.to_a)
titles = nil
queries = Db.capture_sql { titles = (tapped + branched).map { |c| c.article.title }.join(",") }
raise "got #{titles.inspect}" unless titles == "First,First,First,First"
raise "reads ran #{queries.length} queries" unless queries.empty?
puts "ok"
"##;

#[test]
fn associations_preloader_in_a_tap_block_or_a_dropped_branch_runs() {
    tap_digest_app().run_ruby(TAP_DIGEST_SCRIPT).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn associations_preloader_in_a_tap_block_or_a_dropped_branch_runs_on_spinel() {
    let run = tap_digest_app().run_spinel(&format!("Db.configure(\":memory:\")\nSchema.statements.each {{ |sql| Db.exec(sql) }}\nActiveRecord.adapter = SqliteAdapter\n{TAP_DIGEST_SCRIPT}"));
    run.assert_passes();
    assert_eq!(run.stdout, "ok\n");
}

/// `render … and return`, `redirect_to … and return` and `head … and return` return, as Rails' truthy answers make them.
#[test]
fn a_response_call_and_return_returns() {
    emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  get \"/drafts/check\", to: \"drafts#check\"\n",
        )
        .write(
            "app/controllers/drafts_controller.rb",
            r#"class DraftsController < ApplicationController
  def check
    render plain: "stopped", status: :unprocessable_entity and return if params[:stop] == "render"
    redirect_to articles_path and return if params[:stop] == "redirect"
    head :no_content and return if params[:stop] == "head"
    render plain: "fell through"
  end
end
"#,
        )
        .write(
            "test/controllers/drafts_controller_test.rb",
            r#"require "test_helper"

class DraftsControllerTest < ActionDispatch::IntegrationTest
  test "each response call returns" do
    get "/drafts/check", params: { stop: "render" }
    assert_response :unprocessable_entity
    assert_equal "stopped", response.body
    get "/drafts/check", params: { stop: "redirect" }
    assert_redirected_to articles_path
    get "/drafts/check", params: { stop: "head" }
    assert_response :no_content
    get "/drafts/check"
    assert_equal "fell through", response.body
  end
end
"#,
        )
        .run_test("test/controllers/drafts_controller_test.rb")
        .assert_passes();
}

/// `ActiveSupport::TimeZone[name]`, `.all`, and a zone's `name`,
/// `to_s`, `utc_offset`, `formatted_offset` and `now` answer what Rails
/// answers. Only offsets no daylight saving moves are pinned here; the
/// runtime port reads `utc_offset` as Rails does, as the period's base
/// `render json: record.errors` writes the errors keyed by attribute, as
/// Rails' `errors.as_json` (`to_hash`) does, not the Array of full
/// messages: the scaffold's JSON failure arm (#586), the same inside a
/// Hash, and `errors.as_json` itself.
#[test]
fn render_json_errors_is_keyed_by_attribute() {
    emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  get \"/drafts/check\", to: \"drafts#check\"\n",
        )
        .write(
            "app/controllers/drafts_controller.rb",
            r#"class DraftsController < ApplicationController
  def check
    article = Article.new(title: "", body: "x")
    article.valid?
    if params[:as] == "hash"
      render json: article.errors.as_json
    else
      render json: { errors: article.errors }, status: :unprocessable_entity
    end
  end
end
"#,
        )
        .write(
            "test/controllers/drafts_controller_test.rb",
            r#"require "test_helper"

class DraftsControllerTest < ActionDispatch::IntegrationTest
  test "the scaffold's json failure arm" do
    post "/articles.json", params: { article: { title: "", body: "x" } }
    assert_response :unprocessable_entity
    assert_equal %({"title":["can't be blank"],"body":["is too short (minimum is 10 characters)"]}), response.body
  end

  test "errors inside a hash, and as_json" do
    get "/drafts/check"
    assert_equal %({"errors":{"title":["can't be blank"],"body":["is too short (minimum is 10 characters)"]}}), response.body
    get "/drafts/check", params: { as: "hash" }
    assert_equal %({"title":["can't be blank"],"body":["is too short (minimum is 10 characters)"]}), response.body
  end
end
"#,
        )
        .run_test("test/controllers/drafts_controller_test.rb")
        .assert_passes();
}

/// `render json:` of a table-backed record, an Array of them and a Relation.
fn records_json_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  get \"/records/one\", to: \"records#one\"\n  get \"/records/all\", to: \"records#all\"\n  get \"/records/comments\", to: \"records#comments\"\n",
        )
        .write(
            "app/controllers/records_controller.rb",
            r#"class RecordsController < ApplicationController
  def one
    render json: Article.find(params[:id])
  end

  def all
    render json: Article.order(:id).to_a
  end

  def comments
    render json: Comment.where(article_id: params[:id])
  end
end
"#,
        )
}

/// Rails' `serializable_hash`: every column in schema order, timestamps as
/// `TimeWithZone#as_json` writes them, strings HTML-escaped.
#[test]
fn render_json_record_writes_its_columns() {
    records_json_app()
        .write(
            "test/controllers/records_controller_test.rb",
            r#"require "test_helper"

class RecordsControllerTest < ActionDispatch::IntegrationTest
  TIME = '"\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{3}Z"'

  setup do
    @article = Article.create!(title: "A <b> & co", body: "Long enough body text")
    @comment = @article.comments.create!(commenter: "Ann", body: "Nice")
  end

  test "one record" do
    get "/records/one", params: { id: @article.id }
    assert_equal "application/json", response.media_type
    assert_match(/\A\{"id":#{@article.id},"title":"A \\u003cb\\u003e \\u0026 co","body":"Long enough body text","created_at":#{TIME},"updated_at":#{TIME}\}\z/, response.body)
  end

  test "an Array of records" do
    get "/records/all"
    assert_match(/\A\[\{"id":\d+,.*\}\]\z/, response.body)
    assert_equal Article.count, response.body.scan('"id":').size
  end

  test "a Relation of records" do
    get "/records/comments", params: { id: @article.id }
    assert_match(/\A\[\{"id":#{@comment.id},"article_id":#{@article.id},"commenter":"Ann","body":"Nice","created_at":#{TIME},"updated_at":#{TIME}\}\]\z/, response.body)
  end
end
"#,
        )
        .run_test("test/controllers/records_controller_test.rb")
        .assert_passes();
}

/// A strict target reports the written-down encoder as unsupported rather than emitting code no toolchain has built.
#[test]
fn render_json_record_writer_is_unsupported_on_strict_targets() {
    use roundhouse::project::BuildTarget;
    let (_emitted, errors) = records_json_app().emit(BuildTarget::TypescriptWorker);
    assert!(errors.iter().any(|e| e.contains("json_writer")), "{errors:?}");
    // Rust builds the writer, `methods:` included (`render_json_record_writer_runs_on_rust`).
    let (_emitted, errors) = json_writer_app().emit(BuildTarget::Rust);
    assert!(errors.is_empty(), "{errors:?}");
    for target in [BuildTarget::Ruby, BuildTarget::Spinel] {
        let (_emitted, errors) = records_json_app().emit(target);
        assert!(!errors.iter().any(|e| e.contains("json_writer")), "{target:?}: {errors:?}");
    }
    // An app's own method of that name is not the compiler's writer.
    let own = emit_and_run::real_blog()
        .edit("app/models/article.rb", "  validates :title, presence: true\n", "  validates :title, presence: true\n\n  def as_json_str\n    title.to_s\n  end\n")
        .edit("config/routes.rb", "  root \"articles#index\"\n", "  root \"articles#index\"\n  get \"/own/:id\", to: \"own#show\"\n")
        .write("app/controllers/own_controller.rb", "class OwnController < ApplicationController\n  def show\n    render plain: Article.find(params[:id]).as_json_str\n  end\nend\n");
    let (_emitted, errors) = own.emit(BuildTarget::TypescriptWorker);
    assert!(!errors.iter().any(|e| e.contains("json_writer")), "{errors:?}");
}

/// `render json:` of a record, of a has_many and of a loaded Relation, with `only:`, `except:` and `methods:`.
/// One action each, in `ArticlesController`: the Rust runtime builds `params` from the path alone, and builds a controller only in the shape real-blog's take.
fn json_writer_app() -> emit_and_run::Overlay {
    const CONTROLLER: &str = "app/controllers/articles_controller.rb";
    emit_and_run::real_blog()
        .edit(CONTROLLER, "  def show\n  end\n", "  def show\n    render json: @article\n  end\n")
        .edit(CONTROLLER, "  def edit\n  end\n", "  def edit\n    render json: @article, only: [:title, :id]\n  end\n")
        .edit(
            CONTROLLER,
            "    @articles = Article.includes(:comments).order(created_at: :desc)\n",
            "    @articles = Article.includes(:comments).order(created_at: :desc)\n    render json: @articles, methods: [:headline]\n",
        )
        .edit("app/models/article.rb", "  validates :title, presence: true\n", "  validates :title, presence: true\n\n  def headline\n    \"Big\"\n  end\n")
        .edit(
            CONTROLLER,
            "    respond_to do |format|\n      if @article.update(article_params)\n        format.html { redirect_to @article, notice: \"Article was successfully updated.\", status: :see_other }\n        format.json { render :show, status: :ok, location: @article }\n      else\n        format.html { render :edit, status: :unprocessable_content }\n        format.json { render json: @article.errors, status: :unprocessable_content }\n      end\n    end\n",
            "    render json: @article, except: [:body, :updated_at], status: :ok\n",
        )
        .edit(
            CONTROLLER,
            "    @article.destroy!\n\n    respond_to do |format|\n      format.html { redirect_to articles_path, notice: \"Article was successfully destroyed.\", status: :see_other }\n      format.json { head :no_content }\n    end\n",
            "    render json: @article.comments\n",
        )
        .write("test/fixtures/articles.yml", JSON_WRITER_ARTICLES)
        .write("test/fixtures/comments.yml", JSON_WRITER_COMMENTS)
        .write("test/controllers/articles_controller_test.rb", JSON_WRITER_TEST)
        .remove("test/models/article_test.rb")
}

const JSON_WRITER_ARTICLES: &str = "one:\n  title: A <b> & co\n  body: Long enough body text\n  created_at: 2026-01-02 03:04:05.678901\n  updated_at: 2026-01-03 00:00:00\n\ntwo:\n  title: Second\n  body: Another body text\n  created_at: 2026-01-04 00:00:00\n  updated_at: 2026-01-04 00:00:00\n";

const JSON_WRITER_COMMENTS: &str = "one:\n  article: one\n  commenter: Ann\n  body: Nice\n  created_at: 2026-01-05 00:00:00\n  updated_at: 2026-01-05 00:00:00\n";

/// What ActiveRecord 8.1.4 writes for these fixtures.
const JSON_WRITER_TEST: &str = r##"require "test_helper"

class ArticlesControllerTest < ActionDispatch::IntegrationTest
  setup do
    @article = articles(:one)
    @second = articles(:two)
    @comment = comments(:one)
  end

  test "a record is its columns" do
    get article_url(@article)
    assert_equal %({"id":#{@article.id},"title":"A \\u003cb\\u003e \\u0026 co","body":"Long enough body text","created_at":"2026-01-02T03:04:05.678Z","updated_at":"2026-01-03T00:00:00.000Z"}), response.body
  end

  test "only: writes its own order" do
    get edit_article_url(@article)
    assert_equal %({"title":"A \\u003cb\\u003e \\u0026 co","id":#{@article.id}}), response.body
  end

  test "except: leaves out its columns" do
    patch article_url(@article), params: { article: { title: "unused" } }
    assert_equal %({"id":#{@article.id},"title":"A \\u003cb\\u003e \\u0026 co","created_at":"2026-01-02T03:04:05.678Z"}), response.body
  end

  test "a has_many is its records" do
    delete article_url(@article)
    assert_equal %([{"id":#{@comment.id},"article_id":#{@article.id},"commenter":"Ann","body":"Nice","created_at":"2026-01-05T00:00:00.000Z","updated_at":"2026-01-05T00:00:00.000Z"}]), response.body
  end

  test "a Relation with methods: is its records and the methods" do
    get articles_url
    assert_equal %([{"id":#{@second.id},"title":"Second","body":"Another body text","created_at":"2026-01-04T00:00:00.000Z","updated_at":"2026-01-04T00:00:00.000Z","headline":"Big"},{"id":#{@article.id},"title":"A \\u003cb\\u003e \\u0026 co","body":"Long enough body text","created_at":"2026-01-02T03:04:05.678Z","updated_at":"2026-01-03T00:00:00.000Z","headline":"Big"}]), response.body
  end
end
"##;

/// `notice.present?` in the scaffold views lowers to the shared nil
/// guard `!(notice || "").empty?`. Each target has to keep the coalesce
/// whole as the receiver and leave `notice` readable for the paragraph
/// after it: Kotlin printed `notice ?: "".isEmpty()`, Elixir
/// `notice || "" == ""` (never true for a notice), and Rust moved
/// `notice` into `unwrap_or`.
fn notice_guard_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog().edit(
        CONTROLLER_TEST,
        "    assert_redirected_to article_url(Article.last)\n",
        "    assert_redirected_to article_url(Article.last)\n    \
           get article_url(Article.last)\n    \
           assert_select \"#notice\", \"Article was successfully created.\"\n",
    )
}

#[test]
fn a_nullable_present_guard_test_passes() {
    notice_guard_app().run_test(CONTROLLER_TEST).assert_passes();
}

/// Rust and Elixir test clients do not carry the flash to the next
/// request, so these render the index view with and without a notice.
#[test]
#[ignore = "requires the Rust toolchain; compiles and executes an emitted app"]
fn a_nullable_present_guard_runs_on_rust() {
    let (emitted, errors) = emit_and_run::real_blog().emit(roundhouse::project::BuildTarget::Rust);
    assert!(errors.is_empty(), "{errors:?}");
    std::fs::create_dir_all(emitted.join("tests")).unwrap();
    std::fs::write(emitted.join("tests/notice_guard.rs"), r#"
use app::views::Articles;

#[test]
fn a_present_notice_is_shown_and_an_absent_one_is_not() {
    let shown = Articles::index(vec![], Some("Saved".to_string()), None);
    assert!(shown.contains("id=\"notice\">Saved</p>"), "{shown}");
    for absent in [None, Some(String::new())] {
        let page = Articles::index(vec![], absent, None);
        assert!(!page.contains("id=\"notice\""), "{page}");
    }
}
"#).unwrap();
    let output = std::process::Command::new("cargo")
        .args(["test", "--test", "notice_guard"])
        .env_remove("CARGO_TARGET_DIR")
        .current_dir(&*emitted)
        .output()
        .expect("run the emitted Rust project's test");
    assert!(output.status.success(), "{}\n{}\n{}", emitted.display(),
        String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
}

#[test]
#[ignore = "requires Gradle; builds and runs an emitted app"]
fn a_nullable_present_guard_runs_on_kotlin() {
    let (emitted, errors) = notice_guard_app().emit(roundhouse::project::BuildTarget::Kotlin);
    assert!(errors.is_empty(), "{errors:?}");
    let mut gradle = std::process::Command::new("gradle");
    gradle.args(["test", "--console=plain", "--no-daemon"]).current_dir(&*emitted);
    if let Ok(java_home) = std::env::var("KOTLIN_JAVA_HOME").or_else(|_| std::env::var("JAVA_HOME")) {
        gradle.env("JAVA_HOME", java_home);
    }
    let output = gradle.output().expect("run gradle test");
    assert!(output.status.success(), "{}\n{}\n{}", emitted.display(),
        String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
}

#[test]
#[ignore = "requires Elixir; builds and runs an emitted app"]
fn a_nullable_present_guard_runs_on_elixir() {
    let (emitted, errors) = emit_and_run::real_blog().emit(roundhouse::project::BuildTarget::Elixir);
    assert!(errors.is_empty(), "{errors:?}");
    std::fs::write(emitted.join("test/notice_guard_test.exs"), r#"defmodule NoticeGuardTest do
  use ExUnit.Case

  test "a present notice is shown and an absent one is not" do
    shown = IO.iodata_to_binary(Views.Articles.index([], "Saved"))
    assert shown =~ ~s(id="notice">Saved</p>)
    for absent <- [nil, ""] do
      refute IO.iodata_to_binary(Views.Articles.index([], absent)) =~ ~s(id="notice")
    end
  end
end
"#).unwrap();
    for (args, env) in [(&["deps.get"][..], Some("test")), (&["test"][..], None)] {
        let mut mix = std::process::Command::new("mix");
        mix.args(args).current_dir(&*emitted).env("MIX_HOME", emitted.join(".mix"));
        if let Some(env) = env {
            mix.env("MIX_ENV", env);
        }
        let output = mix.output().expect("run mix");
        assert!(output.status.success(), "mix {args:?} in {}\n{}\n{}", emitted.display(),
            String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    }
}

/// The Rust project builds the writer and answers what Rails' `serializable_hash` writes.
#[test]
#[ignore = "requires the Rust toolchain; compiles and executes an emitted app"]
fn render_json_record_writer_runs_on_rust() {
    let (emitted, errors) = json_writer_app().emit(roundhouse::project::BuildTarget::Rust);
    assert!(errors.is_empty(), "{errors:?}");
    std::fs::create_dir_all(emitted.join("tests")).unwrap();
    std::fs::write(emitted.join("tests/json_writer.rs"), r##"
use app::db::{self, Db};
use app::{router, schema_sql};

#[tokio::test(flavor = "multi_thread")]
async fn record_json_is_rails_serializable_hash() {
    db::setup_test_db(schema_sql::CREATE_TABLES);
    Db::exec("INSERT INTO articles (title, body, created_at, updated_at) VALUES ('A <b> & co', 'Long enough body text', '2026-01-02 03:04:05.678901', '2026-01-03 00:00:00')");
    Db::exec("INSERT INTO articles (title, body, created_at, updated_at) VALUES ('Second', 'Another body text', '2026-01-04 00:00:00', '2026-01-04 00:00:00')");
    Db::exec("INSERT INTO comments (article_id, commenter, body, created_at, updated_at) VALUES (1, 'Ann', 'Nice', '2026-01-05 00:00:00', '2026-01-05 00:00:00')");
    let server = axum_test::TestServer::new(router::router()).unwrap();
    let one = r#"{"id":1,"title":"A \u003cb\u003e \u0026 co","body":"Long enough body text","created_at":"2026-01-02T03:04:05.678Z","updated_at":"2026-01-03T00:00:00.000Z"}"#;
    let second = r#"{"id":2,"title":"Second","body":"Another body text","created_at":"2026-01-04T00:00:00.000Z","updated_at":"2026-01-04T00:00:00.000Z"}"#;
    for (method, path, want) in [
        ("GET", "/articles/1", one.to_string()),
        ("GET", "/articles/1/edit", r#"{"title":"A \u003cb\u003e \u0026 co","id":1}"#.to_string()),
        ("PATCH", "/articles/1", r#"{"id":1,"title":"A \u003cb\u003e \u0026 co","created_at":"2026-01-02T03:04:05.678Z"}"#.to_string()),
        ("DELETE", "/articles/1", r#"[{"id":1,"article_id":1,"commenter":"Ann","body":"Nice","created_at":"2026-01-05T00:00:00.000Z","updated_at":"2026-01-05T00:00:00.000Z"}]"#.to_string()),
        ("GET", "/articles", format!("[{},{}]", second.replace("Z\"}", "Z\",\"headline\":\"Big\"}"), one.replace("Z\"}", "Z\",\"headline\":\"Big\"}"))),
    ] {
        let response = match method {
            "PATCH" => server.patch(path).form(&[("article[title]", "unused")]).await,
            "DELETE" => server.delete(path).await,
            _ => server.get(path).await,
        };
        assert_eq!(response.status_code(), 200, "{method} {path}");
        assert_eq!(response.header("content-type"), "application/json", "{method} {path}");
        assert_eq!(response.text(), want, "{method} {path}");
    }
}
"##).unwrap();
    let output = std::process::Command::new("cargo")
        .args(["test", "--test", "json_writer"])
        .env_remove("CARGO_TARGET_DIR")
        .current_dir(&*emitted)
        .output()
        .expect("run the emitted Rust project's test");
    assert!(output.status.success(), "{}\n{}\n{}", emitted.display(),
        String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
}

/// The fixtures' test, as ActiveRecord 8.1.4 answers it.
#[test]
fn render_json_record_writer_test_passes() {
    json_writer_app().run_test("test/controllers/articles_controller_test.rb").assert_passes();
}

/// The same test, translated to TypeScript, against the writer the TypeScript project builds.
#[test]
#[ignore = "requires Node; installs and runs an emitted app"]
fn render_json_record_writer_runs_on_typescript() {
    let (emitted, errors) = json_writer_app().emit(roundhouse::project::BuildTarget::Typescript);
    assert!(errors.is_empty(), "{errors:?}");
    for args in [&["install", "--silent", "--no-audit", "--no-fund"][..], &["test"][..]] {
        let output = std::process::Command::new("npm").args(args).current_dir(&*emitted).output().expect("run npm");
        assert!(output.status.success(), "npm {args:?} in {}\n{}\n{}", emitted.display(),
            String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    }
}

/// The same test, translated to Python.
#[test]
#[ignore = "requires Python; runs an emitted app"]
fn render_json_record_writer_runs_on_python() {
    let (emitted, errors) = json_writer_app().emit(roundhouse::project::BuildTarget::Python);
    assert!(errors.is_empty(), "{errors:?}");
    let output = std::process::Command::new("python3")
        .args(["-m", "unittest", "discover", "-s", "tests", "-t", "."])
        .current_dir(&*emitted)
        .output()
        .expect("run python unittest");
    assert!(output.status.success(), "{}\n{}\n{}", emitted.display(),
        String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
}

/// The same test, translated to Crystal.
#[test]
#[ignore = "requires Crystal; builds and runs an emitted app"]
fn render_json_record_writer_runs_on_crystal() {
    let (emitted, errors) = json_writer_app().emit(roundhouse::project::BuildTarget::Crystal);
    assert!(errors.is_empty(), "{errors:?}");
    for (program, args) in [("shards", &["install"][..]), ("crystal", &["spec", "--no-color"][..])] {
        let output = std::process::Command::new(program).args(args).current_dir(&*emitted).output().expect("run crystal");
        assert!(output.status.success(), "{program} {args:?} in {}\n{}\n{}", emitted.display(),
            String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    }
}

/// The same test, translated to C#.
#[test]
#[ignore = "requires .NET; builds and runs an emitted app"]
fn render_json_record_writer_runs_on_csharp() {
    let (emitted, errors) = json_writer_app().emit(roundhouse::project::BuildTarget::CSharp);
    assert!(errors.is_empty(), "{errors:?}");
    let output = std::process::Command::new("dotnet")
        .args(["test", "tests", "--nologo"])
        .current_dir(&*emitted)
        .output()
        .expect("run dotnet test");
    assert!(output.status.success(), "{}\n{}\n{}", emitted.display(),
        String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
}

/// The same test, translated to Kotlin.
#[test]
#[ignore = "requires Gradle; builds and runs an emitted app"]
fn render_json_record_writer_runs_on_kotlin() {
    let (emitted, errors) = json_writer_app().emit(roundhouse::project::BuildTarget::Kotlin);
    assert!(errors.is_empty(), "{errors:?}");
    let mut gradle = std::process::Command::new("gradle");
    gradle.args(["test", "--console=plain", "--no-daemon"]).current_dir(&*emitted);
    if let Ok(java_home) = std::env::var("KOTLIN_JAVA_HOME").or_else(|_| std::env::var("JAVA_HOME")) {
        gradle.env("JAVA_HOME", java_home);
    }
    let output = gradle.output().expect("run gradle test");
    assert!(output.status.success(), "{}\n{}\n{}", emitted.display(),
        String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
}

/// The same test, translated to Swift. macOS needs a full Xcode for XCTest (`DEVELOPER_DIR`).
#[test]
#[ignore = "requires Swift and XCTest; builds and runs an emitted app"]
fn render_json_record_writer_runs_on_swift() {
    let (emitted, errors) = json_writer_app().emit(roundhouse::project::BuildTarget::Swift);
    assert!(errors.is_empty(), "{errors:?}");
    let output = std::process::Command::new("swift")
        .arg("test")
        .current_dir(&*emitted)
        .output()
        .expect("run swift test");
    assert!(output.status.success(), "{}\n{}\n{}", emitted.display(),
        String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
}

/// The same test, translated to Elixir.
#[test]
#[ignore = "requires Elixir; builds and runs an emitted app"]
fn render_json_record_writer_runs_on_elixir() {
    let (emitted, errors) = json_writer_app().emit(roundhouse::project::BuildTarget::Elixir);
    assert!(errors.is_empty(), "{errors:?}");
    for (args, env) in [(&["deps.get"][..], Some("test")), (&["test"][..], None)] {
        let mut mix = std::process::Command::new("mix");
        mix.args(args).current_dir(&*emitted).env("MIX_HOME", emitted.join(".mix"));
        if let Some(env) = env {
            mix.env("MIX_ENV", env);
        }
        let output = mix.output().expect("run mix");
        assert!(output.status.success(), "mix {args:?} in {}\n{}\n{}", emitted.display(),
            String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    }
}

/// The same test, translated to Go.
#[test]
#[ignore = "requires Go; builds and runs an emitted app"]
fn render_json_record_writer_runs_on_go() {
    let (emitted, errors) = json_writer_app().emit(roundhouse::project::BuildTarget::Go);
    assert!(errors.is_empty(), "{errors:?}");
    for args in [&["mod", "tidy"][..], &["test", "./..."][..]] {
        let output = std::process::Command::new("go").args(args).current_dir(&*emitted).output().expect("run go");
        assert!(output.status.success(), "go {args:?} in {}\n{}\n{}", emitted.display(),
            String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    }
}

/// The same writer compiled by Spinel, where the runtime encoder does not exist.
#[test]
#[ignore = "requires the Spinel toolchain"]
fn render_json_record_writer_runs_on_spinel() {
    let script = r#"Db.configure(":memory:")
Schema.statements.each { |sql| Db.exec(sql) }
ActiveRecord.adapter = SqliteAdapter
a = Article.create!(title: "A <b> & co", body: "Long enough body text")
c = Comment.create!(article_id: a.id, commenter: "Ann", body: "Nice")
time = '"\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{3}Z"'
want = Regexp.new('\A\{"id":' + a.id.to_s + ',"title":"A \\\\u003cb\\\\u003e \\\\u0026 co","body":"Long enough body text","created_at":' + time + ',"updated_at":' + time + '\}\z')
raise "article: #{a.as_json_str}" unless want.match?(a.as_json_str)
raise "comment: #{c.as_json_str}" unless c.as_json_str.start_with?('{"id":' + c.id.to_s + ',"article_id":' + a.id.to_s + ',"commenter":"Ann","body":"Nice","created_at":"')
puts "record json passed"
"#;
    let run = records_json_app().run_spinel(script);
    run.assert_passes();
    assert!(run.stdout.contains("record json passed"));
}

/// `render json: { error: record.errors.messages }` writes the same Hash Rails does.
fn render_json_errors_messages_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  get \"/drafts/check\", to: \"drafts#check\"\n",
        )
        .write(
            "app/controllers/drafts_controller.rb",
            r#"class DraftsController < ApplicationController
  def check
    article = Article.new(title: "", body: "x")
    if article.valid?
      head :ok
    else
      render json: { error: article.errors.messages }, status: :unprocessable_entity
    end
  end
end
"#,
        )
        .write(
            "test/controllers/drafts_controller_test.rb",
            r#"require "test_helper"

class DraftsControllerTest < ActionDispatch::IntegrationTest
  test "messages inside a hash" do
    get "/drafts/check"
    assert_response :unprocessable_entity
    assert_equal %({"error":{"title":["can't be blank"],"body":["is too short (minimum is 10 characters)"]}}), response.body
  end
end
"#,
        )
}

#[test]
fn render_json_errors_messages_is_keyed_by_attribute() {
    render_json_errors_messages_app().run_test("test/controllers/drafts_controller_test.rb").assert_passes();
}

/// Not `errors_messages`, which a strict target does not have: the rendered Hash is `errors_json`, which every target has.
#[test]
fn render_json_errors_messages_reaches_strict_targets() {
    let (_emitted, errors) = render_json_errors_messages_app().emit(roundhouse::project::BuildTarget::Crystal);
    assert!(errors.is_empty(), "{errors:#?}");
}

/// `render json:` of values holding records and Times: Hash and Array literals, a typed Hash.
fn json_values_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "  validates :title, presence: true\n",
            "  validates :title, presence: true\n\n  def headline\n    title.to_s.upcase\n  end\n",
        )
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  get \"/values/:id\", to: \"values#show\"\n",
        )
        .write(
            "app/controllers/values_controller.rb",
            r#"class ValuesController < ApplicationController
  def show
    article = Article.find(params[:id])
    at = Time.utc(2026, 1, 2, 3, 4, 5, 678901)
    case params[:shape]
    when "hash"
      render json: { article: article, ok: true, count: 2, note: nil, at: at }
    when "list"
      render json: { articles: Article.where(id: article.id), total: 1 }
    when "array"
      render json: [article, nil, "x"]
    when "splat"
      render json: [*Article.where(id: article.id).to_a, "x"]
    when "string"
      render json: { a: 1 }.to_json
    when "raw"
      render json: "[1,2]"
    when "maybe_string"
      text = params[:other] ? "[3]" : nil
      render json: text
    when "to_json"
      render plain: article.to_json + "|" + { a: article }.to_json
    when "maybe"
      found = Article.find_by(id: params[:other])
      render json: { article: found }
    when "float"
      render json: { at: at, x: Float::INFINITY, y: 1.5 }
    when "float_only"
      ratios = [1.5, Float::NAN]
      render json: { x: Float::INFINITY, y: 1.5, ratios: ratios, nested: [-Float::INFINITY] }
    when "float_to_json"
      render plain: { x: Float::NAN, y: 2.5 }.to_json
    when "float_splat"
      ratios = [1.5, Float::NAN]
      render json: [*ratios, 2.5]
    when "only"
      render json: article, only: [:title, :id]
    when "except"
      render json: article, except: [:body, :updated_at], status: :ok
    when "methods"
      render plain: article.to_json(only: [:id], methods: [:headline])
    when "relation_only"
      render json: Article.where(id: article.id), only: :id
    else
      stamps = { "at" => at }
      render json: { stamps: stamps, times: [at] }
    end
  end
end
"#,
        )
}

/// Each value as Rails' `to_json` writes it: a record as its columns, a
/// Time as `xmlschema(3)` with `Z` for UTC, nil as `null`.
#[test]
fn render_json_values_hold_records_and_times() {
    json_values_app()
        .write(
            "test/controllers/values_controller_test.rb",
            r##"require "test_helper"

class ValuesControllerTest < ActionDispatch::IntegrationTest
  setup do
    @article = Article.create!(title: "A <b>", body: "Long enough body text")
    @record = %({"id":#{@article.id},"title":"A \\u003cb\\u003e","body":"Long enough body text","created_at":"#{@article.created_at.utc.strftime("%Y-%m-%dT%H:%M:%S.%LZ")}","updated_at":"#{@article.updated_at.utc.strftime("%Y-%m-%dT%H:%M:%S.%LZ")}"})
  end

  test "a hash literal" do
    get "/values/#{@article.id}", params: { shape: "hash" }
    assert_equal "application/json", response.media_type
    assert_equal %({"article":#{@record},"ok":true,"count":2,"note":null,"at":"2026-01-02T03:04:05.678Z"}), response.body
  end

  test "a relation inside a hash" do
    get "/values/#{@article.id}", params: { shape: "list" }
    assert_equal %({"articles":[#{@record}],"total":1}), response.body
  end

  test "an array literal" do
    get "/values/#{@article.id}", params: { shape: "array" }
    assert_equal %([#{@record},null,"x"]), response.body
  end

  test "a splat keeps the runtime encoder and its elements" do
    get "/values/#{@article.id}", params: { shape: "splat" }
    assert_match(/\A\[\{"id":#{@article.id},.*\},"x"\]\z/, response.body)
  end

  test "a String is the body as it is" do
    get "/values/#{@article.id}", params: { shape: "string" }
    assert_equal %({"a":1}), response.body
    get "/values/#{@article.id}", params: { shape: "raw" }
    assert_equal "[1,2]", response.body
    get "/values/#{@article.id}", params: { shape: "maybe_string", other: 1 }
    assert_equal "[3]", response.body
    get "/values/#{@article.id}", params: { shape: "maybe_string" }
    assert_equal "null", response.body
  end

  test "to_json writes what render json: does" do
    get "/values/#{@article.id}", params: { shape: "to_json" }
    assert_equal %(#{@record}|{"a":#{@record}}), response.body
  end

  test "a record that may be nil" do
    get "/values/#{@article.id}", params: { shape: "maybe", other: @article.id }
    assert_equal %({"article":#{@record}}), response.body
    get "/values/#{@article.id}", params: { shape: "maybe", other: 0 }
    assert_equal %({"article":null}), response.body
  end

  test "only:, except: and methods: as Rails' serializable_hash reads them" do
    get "/values/#{@article.id}", params: { shape: "only" }
    assert_equal %({"title":"A \\u003cb\\u003e","id":#{@article.id}}), response.body
    get "/values/#{@article.id}", params: { shape: "except" }
    assert_match(/\A\{"id":#{@article.id},"title":"A \\u003cb\\u003e","created_at":"[^"]+"\}\z/, response.body)
    get "/values/#{@article.id}", params: { shape: "methods" }
    assert_equal %({"id":#{@article.id},"headline":"A \\u003cB\\u003e"}), response.body
    get "/values/#{@article.id}", params: { shape: "relation_only" }
    assert_equal %([{"id":#{@article.id}}]), response.body
  end

  test "a non-finite Float is null" do
    get "/values/#{@article.id}", params: { shape: "float" }
    assert_equal %({"at":"2026-01-02T03:04:05.678Z","x":null,"y":1.5}), response.body
    get "/values/#{@article.id}", params: { shape: "float_only" }
    assert_equal %({"x":null,"y":1.5,"ratios":[1.5,null],"nested":[null]}), response.body
    get "/values/#{@article.id}", params: { shape: "float_to_json" }
    assert_equal %({"x":null,"y":2.5}), response.body
    get "/values/#{@article.id}", params: { shape: "float_splat" }
    assert_equal %([1.5,null,2.5]), response.body
  end

  test "a typed hash and array of times" do
    get "/values/#{@article.id}"
    assert_equal %({"stamps":{"at":"2026-01-02T03:04:05.678Z"},"times":["2026-01-02T03:04:05.678Z"]}), response.body
  end
end
"##,
        )
        .run_test("test/controllers/values_controller_test.rb")
        .assert_passes();
}

/// Not Go's `[]float64`: a literal Array whose Floats `finite_float` wraps holds `Float?`, and Go builds the slice from the literal's type.
#[test]
fn a_wrapped_float_array_literal_is_nullable_on_go() {
    let (emitted, errors) = emit_and_run::real_blog()
        .edit("config/routes.rb", "  root \"articles#index\"\n", "  root \"articles#index\"\n  get \"/f\", to: \"f#show\"\n")
        .write("app/controllers/f_controller.rb", "class FController < ApplicationController\n  def show\n    r = 2.5\n    render json: { nested: [r, 1.0] }\n  end\nend\n")
        .emit(roundhouse::project::BuildTarget::Go);
    assert!(errors.is_empty(), "{errors:?}");
    let controller = std::fs::read_to_string(emitted.join("app/v2/fcontroller.go")).expect("controller");
    assert!(controller.contains("[]interface{}{JsonBuilder_finite_float(r), JsonBuilder_finite_float(1.0)}"), "{controller}");
}

/// The same values compiled by Spinel, where the runtime encoder cannot type them.
#[test]
#[ignore = "requires the Spinel toolchain"]
fn render_json_values_run_on_spinel() {
    let script = r##"Db.configure(":memory:")
Schema.statements.each { |sql| Db.exec(sql) }
ActiveRecord.adapter = SqliteAdapter
a = Article.create!(title: "A <b>", body: "Long enough body text")
["hash", "list", "array", "times", "string", "raw", "to_json", "maybe", "float", "only", "methods", "relation_only", "float_only", "float_to_json"].each do |shape|
  c = ValuesController.new
  c.params = { "id" => a.id.to_s, "shape" => shape }
  c.process_action(:show)
  puts c.body
end
"##;
    let run = json_values_app().run_spinel(script);
    run.assert_passes();
    let lines: Vec<&str> = run.stdout.lines().collect();
    assert_eq!(lines.len(), 14, "{}", run.stdout);
    // The record's timestamps are the run's own; read them back and build the exact text around them.
    let stamp = |key: &str| {
        let at = lines[0].find(&format!("\"{key}\":\"")).expect(key) + key.len() + 4;
        lines[0][at..at + 24].to_string()
    };
    let record = format!(
        r#"{{"id":1,"title":"A \u003cb\u003e","body":"Long enough body text","created_at":"{}","updated_at":"{}"}}"#,
        stamp("created_at"),
        stamp("updated_at")
    );
    assert_eq!(lines[0], format!(r#"{{"article":{record},"ok":true,"count":2,"note":null,"at":"2026-01-02T03:04:05.678Z"}}"#));
    assert_eq!(lines[1], format!(r#"{{"articles":[{record}],"total":1}}"#));
    assert_eq!(lines[2], format!(r#"[{record},null,"x"]"#));
    assert_eq!(lines[3], r#"{"stamps":{"at":"2026-01-02T03:04:05.678Z"},"times":["2026-01-02T03:04:05.678Z"]}"#);
    assert_eq!(lines[4], r#"{"a":1}"#);
    assert_eq!(lines[5], "[1,2]");
    assert!(lines[6].starts_with(r#"{"id":1,"#) && lines[6].contains(r#"Z"}|{"a":{"id":1,"#), "{}", lines[6]);
    assert_eq!(lines[7], r#"{"article":null}"#);
    assert_eq!(lines[8], r#"{"at":"2026-01-02T03:04:05.678Z","x":null,"y":1.5}"#);
    assert_eq!(lines[9], r#"{"title":"A \u003cb\u003e","id":1}"#);
    assert_eq!(lines[10], r#"{"id":1,"headline":"A \u003cB\u003e"}"#);
    assert_eq!(lines[11], r#"[{"id":1}]"#);
    assert_eq!(lines[12], r#"{"x":null,"y":1.5,"ratios":[1.5,null],"nested":[null]}"#);
    assert_eq!(lines[13], r#"{"x":null,"y":2.5}"#);
    // Not "float_splat": spinel spreads a map-built `Float?` Array into a literal with its nil as 0.0 (matz/spinel#9114).
}

/// A Date value as Rails writes it: the ISO date, and `null` for nil.
fn json_dates_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit("db/schema.rb", "t.string \"commenter\"", "t.string \"commenter\"\n    t.date \"archived_on\"")
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  get \"/days\", to: \"days#show\"\n",
        )
        .write(
            "app/controllers/days_controller.rb",
            "class DaysController < ApplicationController\n  def show\n    day = params[:none] ? nil : Date.new(2026, 1, 2)\n    render json: { day: day, days: [Date.new(2026, 3, 4)] }\n  end\nend\n",
        )
}

#[test]
fn render_json_date_values() {
    json_dates_app()
        .write(
            "test/controllers/days_controller_test.rb",
            "require \"test_helper\"\n\nclass DaysControllerTest < ActionDispatch::IntegrationTest\n  test \"dates\" do\n    get \"/days\"\n    assert_equal %({\"day\":\"2026-01-02\",\"days\":[\"2026-03-04\"]}), response.body\n    get \"/days\", params: { none: \"1\" }\n    assert_equal %({\"day\":null,\"days\":[\"2026-03-04\"]}), response.body\n  end\nend\n",
        )
        .run_test("test/controllers/days_controller_test.rb")
        .assert_passes();
}

/// A strict target reports a written-down value as unsupported rather than emitting code no toolchain has built.
#[test]
fn render_json_values_are_unsupported_on_strict_targets() {
    let app = || {
        emit_and_run::real_blog()
            .edit("config/routes.rb", "  root \"articles#index\"\n", "  root \"articles#index\"\n  get \"/pair/:id\", to: \"pair#show\"\n")
            .write(
                "app/controllers/pair_controller.rb",
                "class PairController < ApplicationController\n  def show\n    render json: { article: Article.find(params[:id]), ok: true }\n  end\nend\n",
            )
    };
    for (target, name) in [(roundhouse::project::BuildTarget::Rust, "rust"), (roundhouse::project::BuildTarget::Go, "go")] {
        let (_emitted, errors) = app().emit(target);
        let want = format!("bundled_constant not supported ({name}): ActiveSupport::JSON is not available as a class/module value on {name}");
        assert!(errors.iter().any(|e| e.contains(&want)), "{target:?}: {errors:?}");
    }
    let (_emitted, errors) = app().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn render_json_date_values_run_on_spinel() {
    let script = r#"Db.configure(":memory:")
Schema.statements.each { |sql| Db.exec(sql) }
ActiveRecord.adapter = SqliteAdapter
[{ "x" => "1" }, { "none" => "1" }].each do |params|
  c = DaysController.new
  c.params = params
  c.process_action(:show)
  puts c.body
end
"#;
    let run = json_dates_app().run_spinel(script);
    run.assert_passes();
    assert_eq!(
        run.stdout,
        "{\"day\":\"2026-01-02\",\"days\":[\"2026-03-04\"]}\n{\"day\":null,\"days\":[\"2026-03-04\"]}\n"
    );
}

/// In a non-UTC zone at offset zero (London in winter) a zoned Time is
/// `+00:00`, as `TimeWithZone#as_json` writes it; a `Time.utc` is still `Z`.
fn london_json_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit(
            "config/application.rb",
            "    # config.time_zone = \"Central Time (US & Canada)\"",
            "    config.time_zone = \"London\"",
        )
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  get \"/zoned\", to: \"zoned#show\"\n",
        )
        .write(
            "app/controllers/zoned_controller.rb",
            "class ZonedController < ApplicationController\n  def show\n    at = Time.utc(2026, 1, 2, 3, 4, 5)\n    render json: { utc: at, london: at.in_time_zone(\"London\") }\n  end\nend\n",
        )
}

#[test]
fn render_json_time_in_a_zone_at_offset_zero() {
    london_json_app()
        .write(
            "test/controllers/zoned_controller_test.rb",
            "require \"test_helper\"\n\nclass ZonedControllerTest < ActionDispatch::IntegrationTest\n  test \"london in winter\" do\n    get \"/zoned\"\n    assert_equal %({\"utc\":\"2026-01-02T03:04:05.000Z\",\"london\":\"2026-01-02T03:04:05.000+00:00\"}), response.body\n  end\nend\n",
        )
        .run_test("test/controllers/zoned_controller_test.rb")
        .assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn render_json_time_in_a_zone_at_offset_zero_on_spinel() {
    let script = "c = ZonedController.new\nc.params = {}\nc.process_action(:show)\nputs c.body\n";
    let run = london_json_app().run_spinel(script);
    run.assert_passes();
    assert_eq!(run.stdout, "{\"utc\":\"2026-01-02T03:04:05.000Z\",\"london\":\"2026-01-02T03:04:05.000+00:00\"}\n");
}

/// A failed statement raises ActiveRecord's own class, in Rails' hierarchy:
/// a unique index is RecordNotUnique, a NOT NULL column NotNullViolation,
/// a bad statement StatementInvalid, and every one rescues as
/// StatementInvalid and ActiveRecordError.
fn statement_errors_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit("db/schema.rb", "    t.index [\"article_id\"], name: \"index_comments_on_article_id\"\n", "    t.index [\"article_id\"], name: \"index_comments_on_article_id\"\n    t.index [\"commenter\"], name: \"index_comments_on_commenter\", unique: true\n")
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  get \"/statement_errors\", to: \"statement_errors#show\"\n",
        )
        .write(
            "app/controllers/statement_errors_controller.rb",
            r#"class StatementErrorsController < ApplicationController
  def show
    article = Article.create!(title: "Errors", body: "Long enough body text")
    out = []
    Comment.create!(article_id: article.id, commenter: "Ann", body: "first")
    begin
      Comment.create!(article_id: article.id, commenter: "Ann", body: "second")
    rescue ActiveRecord::StatementInvalid => e
      out << e.class.name
    end
    begin
      ActiveRecord::Base.connection.execute("INSERT INTO comments (article_id, commenter, body, created_at, updated_at) VALUES (NULL, 'Bob', 'x', '2026-01-01', '2026-01-01')")
    rescue ActiveRecord::StatementInvalid => e
      out << e.class.name
    end
    begin
      Article.where("no_such_column = 1").to_a
    rescue ActiveRecord::ActiveRecordError => e
      out << e.class.name
    end
    begin
      ActiveRecord::Base.connection.execute("INSERT INTO comments (article_id, commenter, body, created_at, updated_at) VALUES (999999, 'Cy', 'x', '2026-01-01', '2026-01-01')")
    rescue ActiveRecord::StatementInvalid => e
      out << e.class.name
    end
    render plain: out.join("|")
  end
end
"#,
        )
}

#[test]
fn statement_errors_raise_activerecords_classes() {
    statement_errors_app()
        .write(
            "test/controllers/statement_errors_controller_test.rb",
            "require \"test_helper\"\n\nclass StatementErrorsControllerTest < ActionDispatch::IntegrationTest\n  test \"each failed statement\" do\n    get \"/statement_errors\"\n    assert_equal \"ActiveRecord::RecordNotUnique|ActiveRecord::NotNullViolation|ActiveRecord::StatementInvalid|ActiveRecord::InvalidForeignKey\", response.body\n  end\nend\n",
        )
        .run_test("test/controllers/statement_errors_controller_test.rb")
        .assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn statement_errors_raise_activerecords_classes_on_spinel() {
    let script = "Db.configure(\":memory:\")\nSchema.statements.each { |sql| Db.exec(sql) }\nActiveRecord.adapter = SqliteAdapter\nc = StatementErrorsController.new\nc.params = {}\nc.process_action(:show)\nputs c.body\n";
    let run = statement_errors_app().run_spinel(script);
    run.assert_passes();
    assert_eq!(run.stdout, "ActiveRecord::RecordNotUnique|ActiveRecord::NotNullViolation|ActiveRecord::StatementInvalid|ActiveRecord::InvalidForeignKey\n");
}

/// A fixture naming a parent no fixture defines fails the test's setup, as Rails' `verify_foreign_keys_for_fixtures` does; the loader itself runs with referential integrity off, so fixture order does not matter.
#[test]
fn a_fixture_naming_a_missing_parent_fails_setup() {
    let run = emit_and_run::real_blog()
        .edit("test/fixtures/comments.yml", "  article: two\n", "  article: missing\n")
        .write(
            "test/models/fk_check_test.rb",
            "require \"test_helper\"\n\nclass FkCheckTest < ActiveSupport::TestCase\n  test \"loads\" do\n    assert Article.count > 0\n  end\nend\n",
        )
        .run_test("test/models/fk_check_test.rb");
    assert!(!run.success, "{}", run.stdout);
    assert!(run.stdout.contains("Foreign key violations found: comments") || run.stderr.contains("Foreign key violations found: comments"), "{}\n{}", run.stdout, run.stderr);
}

/// offset. Expected values are ActiveSupport 8.1's.
#[test]
fn time_zone_lookups_run() {
    emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  get \"/zones\", to: \"zones#show\"\n",
        )
        .write(
            "app/controllers/zones_controller.rb",
            r#"class ZonesController < ApplicationController
  def show
    all = ActiveSupport::TimeZone.all
    render plain: [
      ActiveSupport::TimeZone[params[:tz].to_s]&.name,
      ActiveSupport::TimeZone["Asia/Tokyo"].to_s,
      ActiveSupport::TimeZone["Nope"].nil?,
      ActiveSupport::TimeZone["Tokyo"].now.formatted_offset,
      ActiveSupport::TimeZone["UTC"].utc_offset,
      all.size,
      all.first.to_s,
      all.last.to_s,
      ActiveSupport::TimeZone["Tokyo"].formatted_offset
    ].join("|")
  end
end
"#,
        )
        .write(
            "test/controllers/zones_controller_test.rb",
            r#"require "test_helper"

class ZonesControllerTest < ActionDispatch::IntegrationTest
  test "time zone lookups" do
    get "/zones", params: { tz: "Tokyo" }
    assert_equal "Tokyo|(GMT+09:00) Asia/Tokyo|true|+09:00|0|152|(GMT-12:00) International Date Line West|(GMT+13:00) Tokelau Is.|+09:00", response.body
  end
end
"#,
        )
        .run_test("test/controllers/zones_controller_test.rb")
        .assert_passes();
}

/// A class method `ApplicationRecord` defines runs on a subclass's relation,
/// as Rails' scoping runs it: a relation-returning one filters that
/// relation, and one that maps `pluck` rows to indifferent hashes answers
/// that Array (not a relation of the hash class), so `first[:title]` reads
/// a hash. Called on the subclass directly, it queries the subclass. A
/// model's own class method after `where(...)` runs on that relation too,
/// rather than on the rows the chain would otherwise be folded into. An
/// inherited `scope` chains on a subclass relation, and an inherited class
/// method runs on an association of the subclass.
#[test]
fn an_inherited_class_method_runs_on_a_subclass_relation() {
    emit_and_run::real_blog()
        .edit(
            "app/models/application_record.rb",
            "  primary_abstract_class\n",
            "  primary_abstract_class\n\n  scope :newest_first, -> { order(id: :desc) }\n\n  def self.id_list\n    pluck(:id)\n  end\n\n  def self.titled\n    where.not(title: \"Quiet\")\n  end\n\n  def self.hash_titles\n    pluck(:id, :title).map { |id, title| { id: id, title: title }.with_indifferent_access }\n  end\n",
        )
        .edit(
            "app/models/article.rb",
            "  validates :body, presence: true, length: { minimum: 10 }\n",
            "  validates :body, presence: true, length: { minimum: 10 }\n\n  def self.titled_count\n    order(:id).titled.to_a.size\n  end\n\n  def self.first_hash_title\n    order(:id).hash_titles.first[:title]\n  end\n\n  def self.hash_title_ids\n    all.hash_titles.map { |h| h[\"id\"] }\n  end\n\n  def self.direct_count\n    titled.count\n  end\n\n  def self.title_list\n    pluck(:title)\n  end\n\n  def self.where_titles\n    where(title: \"One\").title_list\n  end\n\n  def self.newest_title\n    where.not(title: nil).newest_first.first.title\n  end\n\n  def comment_ids\n    comments.id_list\n  end\n",
        )
        .run_ruby(
            r#"a = Article.create!(title: "One", body: "A sufficiently long body.")
b = Article.create!(title: "Quiet", body: "A sufficiently long body.")
raise Article.titled_count.inspect unless Article.titled_count == 1
raise Article.direct_count.inspect unless Article.direct_count == 1
raise Article.first_hash_title.inspect unless Article.first_hash_title == "One"
raise Article.hash_title_ids.inspect unless Article.hash_title_ids.sort == [a.id, b.id].sort
raise Article.where_titles.inspect unless Article.where_titles == ["One"]
raise Article.newest_title.inspect unless Article.newest_title == "Quiet"
c = Comment.create!(article: a, commenter: "Reader", body: "Comment body")
raise a.comment_ids.inspect unless a.comment_ids == [c.id]"#,
        )
        .assert_passes();
}

/// The nearest definition wins across the model hierarchy: a subclass
/// scope shadows an ancestor's class method on an association, and a
/// subclass class method shadows an ancestor's scope on a relation.
#[test]
fn a_subclass_definition_shadows_an_ancestor_of_the_same_name() {
    emit_and_run::real_blog()
        .edit(
            "app/models/application_record.rb",
            "  primary_abstract_class\n",
            "  primary_abstract_class\n\n  scope :ordered, -> { order(id: :desc) }\n\n  def self.picked(value)\n    where(id: value)\n  end\n",
        )
        .edit(
            "app/models/article.rb",
            "  validates :body, presence: true, length: { minimum: 10 }\n",
            "  validates :body, presence: true, length: { minimum: 10 }\n\n  def self.ordered\n    \"own\"\n  end\n\n  def self.picked_titles(value)\n    where.not(title: nil).picked(value).map(&:title)\n  end\n\n  def picked_commenters\n    comments.picked.map(&:commenter)\n  end\n",
        )
        .edit(
            "app/models/comment.rb",
            "  belongs_to :article\n",
            "  belongs_to :article\n\n  scope :picked, -> { where(commenter: \"Reader\") }\n",
        )
        .run_ruby(
            r#"a = Article.create!(title: "One", body: "A sufficiently long body.")
Article.create!(title: "Two", body: "A sufficiently long body.")
raise Article.where.not(title: nil).ordered.inspect unless Article.where.not(title: nil).ordered == "own"
raise Article.picked_titles(a.id).inspect unless Article.picked_titles(a.id) == ["One"]
Comment.create!(article: a, commenter: "Reader", body: "Comment body")
Comment.create!(article: a, commenter: "Other", body: "Comment body")
raise a.picked_commenters.inspect unless a.picked_commenters == ["Reader"]"#,
        )
        .assert_passes();
}

/// `ActiveSupport::HashWithIndifferentAccess` reads and writes a Symbol
/// key as its String, nested hashes included, and its conversions
/// (`merge`, `reverse_merge`, `dup`, `to_hash`, `symbolize_keys`) answer
/// what Rails answers.
#[test]
fn hash_with_indifferent_access_runs() {
    emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  get \"/indifferent\", to: \"indifferent#show\"\n  get \"/indifferent/loose\", to: \"indifferent#loose\"\n  get \"/indifferent/copies\", to: \"indifferent#copies\"\n",
        )
        .write(
            "app/controllers/indifferent_controller.rb",
            r#"class IndifferentController < ApplicationController
  OPTIONS = { state: :trusted }.with_indifferent_access.freeze

  def show
    h = ActiveSupport::HashWithIndifferentAccess.new(a: 1, "b" => 2)
    h[:c] = 3
    key = :a
    nested = { outer: { inner: 4 } }.with_indifferent_access
    list = HashWithIndifferentAccess.new(items: [{ id: 5 }])
    w = ActiveSupport::HashWithIndifferentAccess.new
    w[:name] = "x"
    w[:rows] = [{ n: 8 }]
    merged = h.merge(d: 6)
    defaults = h.reverse_merge(a: 0, e: 7)
    copy = h.dup
    copy[:a] = 10
    syms = h.symbolize_keys
    render plain: [
      h[:a], h["a"], h[key], h[:b], h.fetch(:c), h.key?(:b), h.values_at(:a, :c).join(","),
      h.slice(:a, :c).keys.join(","), h.except(:b).keys.join(","),
      nested[:outer][:inner], nested.dig(:outer, :inner), list[:items][0][:id], w["name"], w[:rows][0][:n],
      merged[:d], defaults[:a], defaults[:e], copy[:a], h[:a],
      h.to_hash.keys.join(","), syms[:a], h.is_a?(Hash), h.delete(:c), h.key?(:c), h.keys.join(","),
      h.select { |k, v| v > 1 }.keys.join(","), h.map { |k, v| k + v.to_s }.join(",")
    ].join("|")
  end

  def loose
    h = ActiveSupport::HashWithIndifferentAccess.new(a: 1, "b" => 2)
    t = h.to_h
    t["z"] = 1
    filled = h.with_defaults(a: 0, f: 9)
    loose = ActiveSupport::HashWithIndifferentAccess.new
    loose[:cfg] = { deep: { leaf: "y" } }
    loose.fetch(:cfg)[:deep][:leaf] = "z"
    loose.update(more: 1)
    render plain: [
      h.key?(:z), filled[:a], filled[:f], loose.dig(:cfg, :deep, :leaf), loose[:cfg].key?(:deep), loose[:more]
    ].join("|")
  end

  def copies
    src = ActiveSupport::HashWithIndifferentAccess.new(n: { k: 1 }, drop: 2)
    cp = ActiveSupport::HashWithIndifferentAccess.new(src)
    src[:n][:k] = 2
    bag = ActiveSupport::HashWithIndifferentAccess.new
    bag[:syms] = [:a]
    keys = %i[n drop]
    render plain: [cp[:n][:k], src.without(:drop).keys.join(","), bag[:syms].include?(:a), src.slice(*keys).keys.join(","), OPTIONS[:state], OPTIONS.frozen?, src[:n].keys.join(",")].join("|")
  end
end
"#,
        )
        .write(
            "test/controllers/indifferent_controller_test.rb",
            r#"require "test_helper"

class IndifferentControllerTest < ActionDispatch::IntegrationTest
  test "indifferent access" do
    get "/indifferent"
    assert_equal "1|1|1|2|3|true|1,3|a,c|a,c|4|4|5|x|8|6|1|7|10|1|a,b,c|1|true|3|false|a,b|b|a1,b2", response.body
  end

  test "untyped values read back indifferently" do
    get "/indifferent/loose"
    assert_equal "false|1|9|z|true|1", response.body
  end

  test "an indifferent source is copied shallowly" do
    get "/indifferent/copies"
    assert_equal "2|n|true|n,drop|trusted|true|k", response.body
  end
end
"#,
        )
        .run_test("test/controllers/indifferent_controller_test.rb")
        .assert_passes();
}

/// A job `perform_later` enqueues under the test adapter is held, not
/// dropped, and a blockless `perform_enqueued_jobs only:` runs it
/// (basecamp/once-campfire#296's tests). Its broadcast is JSON encoded
/// once with `ActiveSupport::JSON.encode` and sent `coder: nil`
/// (#292), so the test pubsub hands it back decoded exactly once.
#[test]
fn held_jobs_run_on_demand_and_pre_encoded_broadcasts_arrive_once() {
    emit_and_run::real_blog()
        .write(
            "app/jobs/notice_job.rb",
            r#"class NoticeJob < ApplicationJob
  def perform(article)
    ActionCable.server.broadcast "notices", ActiveSupport::JSON.encode(articleId: article.id, title: article.title), coder: nil
  end
end
"#,
        )
        .write(
            "test/models/notice_job_test.rb",
            r#"require "test_helper"

class NoticeJobTest < ActiveSupport::TestCase
  include ActiveJob::TestHelper

  test "a held job runs when performed, and broadcasts pre-encoded JSON" do
    article = Article.create!(title: "Held", body: "Body text here")
    NoticeJob.perform_later(article)
    assert_enqueued_with job: NoticeJob
    assert_equal 0, ActionCable.server.pubsub.broadcasts("notices").size

    perform_enqueued_jobs only: NoticeJob

    notices = ActionCable.server.pubsub.broadcasts("notices").map { |broadcast| JSON.parse(broadcast) }
    assert_equal [ { "articleId" => article.id, "title" => "Held" } ], notices
  end
end
"#,
        )
        .run_test("test/models/notice_job_test.rb")
        .assert_passes();
}

/// rack-test's `Rack::Test::UploadedFile` built from a StringIO, as
/// campfire's undecodable-image test builds one from half a WebP
/// (basecamp/once-campfire#311). It is the `ActionDispatch::Http::UploadedFile`
/// a controller's params read takes. Undefined, it was a constant error
/// on CRuby and, on Spinel, a refusal that kept the whole test file
/// from compiling.
#[test]
fn rack_test_uploaded_file_from_a_stringio_runs() {
    emit_and_run::real_blog()
        .write(
            "test/models/article_upload_test.rb",
            r#"require "test_helper"

class ArticleUploadTest < ActiveSupport::TestCase
  test "a StringIO upload is the file params carry" do
    upload = Rack::Test::UploadedFile.new(StringIO.new("RIFF half"), "image/webp", original_filename: "broken.webp")
    assert_equal "broken.webp", upload.original_filename
    assert_equal "image/webp", upload.content_type
    assert_equal "RIFF half", upload.read
    assert_equal 9, upload.size
    assert upload.is_a?(ActionDispatch::Http::UploadedFile)
  end
end
"#,
        )
        .run_test("test/models/article_upload_test.rb")
        .assert_passes();
}

/// Named binds in `where` and `having` — `:size` used twice and an Array
/// bound into `IN (:labels)` — in the shape of campfire's direct-room
/// lookup (basecamp/once-campfire#310). Unbound, the placeholders reached
/// SQLite as NULL and the lookup found nothing; campfire then created a
/// second room on every Ping. Plus `relation.to_set`.
fn named_binds_app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("db/schema.rb", r#"ActiveRecord::Schema.define do
  create_table "widgets", force: :cascade do |t|
    t.string "name"
  end
  create_table "parts", force: :cascade do |t|
    t.integer "widget_id"
    t.string "label"
  end
end
"#)
        .write("app/models/widget.rb", r#"class Widget < ApplicationRecord
  has_many :parts

  def self.with_exactly(labels)
    joins(:parts).group(:id)
      .having("COUNT(*) = :size AND COUNT(CASE WHEN parts.label IN (:labels) THEN 1 END) = :size", size: labels.size, labels: labels)
      .first
  end
end
"#)
        .write("app/models/part.rb", "class Part < ApplicationRecord\n  belongs_to :widget\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\n  get \"/widgets\", to: \"widgets#index\"\nend\n")
        .write("app/controllers/widgets_controller.rb", r##"class WidgetsController < ApplicationController
  def index
    exact = Widget.with_exactly(%w[ a1 a2 ])
    named = Widget.where("name = :name", name: "beta").first
    set = Widget.where(name: %w[ alpha beta ]).to_set
    render plain: "#{exact&.name}|#{named&.name}|#{set.size}|#{set.include?(named)}"
  end
end
"##)
}

fn named_binds_assertions() -> &'static str {
    r#"
require_relative "app/controllers/widgets_controller"
alpha = Widget.create!(name: "alpha")
beta = Widget.create!(name: "beta")
Part.create!(widget: alpha, label: "a1")
Part.create!(widget: alpha, label: "a2")
Part.create!(widget: beta, label: "a1")
controller = WidgetsController.new
controller.process_action(:index)
raise "named binds: #{controller.body}" unless controller.body == "alpha|beta|2|true"
puts "named binds passed"
"#
}

#[test]
fn named_binds_reach_the_query() {
    named_binds_app().run_ruby(named_binds_assertions()).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn named_binds_reach_the_query_on_spinel() {
    let script = format!(
        "Db.configure(\":memory:\")\nSchema.statements.each {{ |sql| Db.exec(sql) }}\nActiveRecord.adapter = SqliteAdapter\n{}",
        named_binds_assertions()
    );
    named_binds_app().run_spinel(&script).assert_passes();
}

/// `sanitize_sql_array` is the documented array-form entry point (#400).
fn sanitize_sql_array_app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write(
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table "widgets", force: :cascade do |t|
    t.string "name"
  end
end
"#,
        )
        .write(
            "app/models/widget.rb",
            r#"class Widget < ApplicationRecord
  def self.quoted(value)
    ActiveRecord::Base.sanitize_sql_array(["SELECT ? AS v", value])
  end
end
"#,
        )
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"/widgets\", to: \"widgets#index\"\nend\n",
        )
        .write(
            "app/controllers/widgets_controller.rb",
            r##"class WidgetsController < ApplicationController
  def index
    render plain: Widget.quoted(1)
  end
end
"##,
        )
}

#[test]
fn sanitize_sql_array_is_supported() {
    sanitize_sql_array_app()
        .run_ruby(
            r#"
require_relative "app/controllers/widgets_controller"
sql = Widget.quoted(1)
raise "sanitize_sql_array: #{sql.inspect}" unless sql == "SELECT 1 AS v"
controller = WidgetsController.new
controller.process_action(:index)
raise "controller: #{controller.body}" unless controller.body == "SELECT 1 AS v"
puts "sanitize_sql_array passed"
"#,
        )
        .assert_passes();
}

/// `sanitize_sql_for_order` on a String is the fragment as written, and
/// orders a relation through `Arel.sql`.
#[test]
fn sanitize_sql_for_order_is_supported() {
    sanitize_sql_array_app()
        .write(
            "app/models/widget.rb",
            r#"class Widget < ApplicationRecord
  def self.quoted(value)
    ActiveRecord::Base.sanitize_sql_array(["SELECT ? AS v", value])
  end

  def self.by_fragment
    order(Arel.sql(ActiveRecord::Base.sanitize_sql_for_order("length(name) DESC"))).map(&:name)
  end
end
"#,
        )
        .run_ruby(
            r#"
%w[bb a ccc].each { |n| Widget.create!(name: n) }
raise Widget.by_fragment.inspect unless Widget.by_fragment == %w[ccc bb a]
"#,
        )
        .assert_passes();
}

/// Rails' Array form of `sanitize_sql_for_order` is not modeled: Rails 8.1
/// raises for a `?` fragment there. It keeps a refusal at check.
#[test]
fn sanitize_sql_for_order_refuses_the_array_form() {
    // On Base, and on a model through implicit self.
    for receiver in ["ActiveRecord::Base.", ""] {
        let errors = sanitize_sql_array_app()
            .edit(
                "app/models/widget.rb",
                "class Widget < ApplicationRecord\n",
                &format!("class Widget < ApplicationRecord\n  def self.first_named(name)\n    order(Arel.sql({receiver}sanitize_sql_for_order([\"CASE WHEN name = ? THEN 0 ELSE 1 END\", name])))\n  end\n"),
            )
            .check_errors();
        assert!(
            errors.iter().any(|e| e.contains("sanitize_sql_for_order is modeled on a String only")),
            "{receiver:?}: {errors:?}"
        );
    }
}

/// Relation#ids must preserve uuid / named string keys (#310).
fn relation_ids_uuid_app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write(
            "db/schema.rb",
            r#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "widgets", id: :uuid, force: :cascade do |t|
    t.string "name"
    t.boolean "active", default: true
  end
end
"#,
        )
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n")
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"/widget_ids\", to: \"widgets#ids\"\nend\n",
        )
        .write(
            "app/controllers/widgets_controller.rb",
            r##"class WidgetsController < ApplicationController
  def ids
    render plain: Widget.where(active: true).ids.join(",")
  end
end
"##,
        )
}

#[test]
fn relation_ids_preserves_uuid_keys() {
    relation_ids_uuid_app()
        .run_ruby(
            r#"
require_relative "app/controllers/widgets_controller"
uid = "44444444-4444-4444-8444-444444444441"
Widget.create!(id: uid, name: "a", active: true)
controller = WidgetsController.new
controller.process_action(:ids)
raise "uuid ids: #{controller.body.inspect}" unless controller.body == uid
puts "relation ids uuid passed"
"#,
        )
        .assert_passes();
}

/// Writebook-shaped `ActionText::Markdown < Record` under `module ActionText`
/// in `lib/` is an ordinary model (table `action_text_markdowns`, attr
/// `content`). Storage-only — does not claim `has_markdown`.
#[test]
fn action_text_markdown_saves_and_reloads_content() {
    emit_and_run::empty_app()
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write(
            "db/schema.rb",
            r#"ActiveRecord::Schema.define(version: 1) do
  create_table "action_text_markdowns", force: :cascade do |t|
    t.text "content"
    t.string "name", null: false
    t.bigint "record_id", null: false
    t.string "record_type", null: false
    t.datetime "created_at", null: false
    t.datetime "updated_at", null: false
  end
end
"#,
        )
        .write(
            "lib/rails_ext/action_text_markdown.rb",
            r#"module ActionText
  class Markdown < Record
    belongs_to :record, polymorphic: true
  end
end
"#,
        )
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\nend\n",
        )
        .run_ruby(
            r##"
m = ActionText::Markdown.new
m.content = "# Hello"
m.name = "body"
m.record_type = "Article"
m.record_id = 1
m.save!
reloaded = ActionText::Markdown.find(m.id)
raise "content lost: #{reloaded.content.inspect}" unless reloaded.content == "# Hello"
raise "name lost: #{reloaded.name.inspect}" unless reloaded.name == "body"
puts "action_text markdown storage passed"
"##,
        )
        .assert_passes();
}

/// The pinned Writebook Page tests exercise this exact behavior: a new
/// Page's `markable` method returns the raw Markdown stored by `has_markdown`.
/// Keep the source-shaped `ActionText::Markdown < Record` declaration here so
/// the emitted model resolves the Action Text base against shared runtime code.
#[test]
fn writebook_page_markable_runs_against_emitted_markdown_runtime() {
    emit_and_run::empty_app()
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write(
            "db/schema.rb",
            r#"ActiveRecord::Schema.define(version: 1) do
  create_table "pages", force: :cascade do |t|
    t.datetime "created_at", null: false
    t.datetime "updated_at", null: false
  end

  create_table "action_text_markdowns", force: :cascade do |t|
    t.text "content", default: "", null: false
    t.string "name", null: false
    t.bigint "record_id", null: false
    t.string "record_type", null: false
    t.datetime "created_at", null: false
    t.datetime "updated_at", null: false
  end
end
"#,
        )
        .write(
            "lib/rails_ext/action_text_markdown.rb",
            r#"module ActionText
  class Markdown < Record
    belongs_to :record, polymorphic: true
  end
end
"#,
        )
        .write(
            "app/models/page.rb",
            r#"class Page < ApplicationRecord
  has_markdown :body

  def markable
    body.content.to_s
  end
end
"#,
        )
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .run_ruby(
            r###"
page = Page.new(body: "## Markdown Content\n\nWith **bold** text.")
raise "raw markdown changed: #{page.markable.inspect}" unless page.markable == "## Markdown Content\n\nWith **bold** text."
empty = Page.new(body: "")
raise "empty markdown changed: #{empty.markable.inspect}" unless empty.markable == ""
puts "Writebook Page markable behavior passed"
"###,
        )
        .assert_passes();
}

/// A nested include file must load after the parent class is established,
/// so its namespace can reopen the intended class rather than a placeholder.
#[test]
fn nested_included_module_loads_inside_its_parent_class() {
    emit_and_run::empty_app()
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table :action_text_markdowns do |t|\n    t.text :content\n  end\nend\n",
        )
        .write(
            "app/models/action_text/markdown.rb",
            r#"module ActionText
  class Markdown < ApplicationRecord
    include ActionText::Markdown::Uploads
  end
end
"#,
        )
        .write(
            "app/models/action_text/markdown/uploads.rb",
            r#"module ActionText
  class Markdown < ApplicationRecord
    module Uploads
      def nested_upload_module_loaded?
        true
      end
    end
  end
end
"#,
        )
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .run_ruby(r#"
raise "nested module was not included" unless ActionText::Markdown.new.nested_upload_module_loaded?
puts "nested include namespace passed"
"#)
        .assert_passes();
}

/// Named plain-text association (`has_markdown :body`): assign through
/// the owner, autosave on save, reload scoped by owner/name. Abstract
/// overlay — Writebook `Page#body` is extra fixture coverage only.
#[test]
fn named_plain_text_attr_assign_save_reload() {
    emit_and_run::empty_app()
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write(
            "db/schema.rb",
            r#"ActiveRecord::Schema.define(version: 1) do
  create_table "articles", force: :cascade do |t|
    t.string "title"
    t.datetime "created_at", null: false
    t.datetime "updated_at", null: false
  end

  create_table "action_text_markdowns", force: :cascade do |t|
    t.text "content", default: "", null: false
    t.string "name", null: false
    t.bigint "record_id", null: false
    t.string "record_type", null: false
    t.datetime "created_at", null: false
    t.datetime "updated_at", null: false
  end
end
"#,
        )
        .write(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_markdown :body\nend\n",
        )
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\nend\n",
        )
        .run_ruby(
            r##"
a = Article.new
a.title = "Hello"
a.body = "# Title\n\nParagraph"
a.save!
reloaded = Article.find(a.id)
raise "body missing" unless reloaded.body
raise "content lost: #{reloaded.body.content.inspect}" unless reloaded.body.content == "# Title\n\nParagraph"
raise "name wrong: #{reloaded.body.name.inspect}" unless reloaded.body.name == "body"
raise "record_type wrong: #{reloaded.body.record_type.inspect}" unless reloaded.body.record_type == "Article"
raise "record_id wrong: #{reloaded.body.record_id.inspect}" unless reloaded.body.record_id == reloaded.id
# Ordinary autosave includes blank content (unlike RichText blank suppression).
b = Article.create!(title: "Empty")
b.body = ""
b.save!
blank = Article.find(b.id)
raise "blank content not saved: #{blank.body.content.inspect}" unless blank.body.content == ""
raise "predicate false on blank row" unless blank.body?
puts "named plain text attr assign/save/reload passed"
"##,
        )
        .assert_passes();
}

/// `delegated_type` singular reader (`entry.page`) must stay a record
/// reader at runtime — not collide with Relation pagination `page` —
/// and compose with a plain-text attr on the delegated target.
#[test]
fn delegated_type_singular_reader_plain_text_body_runs() {
    emit_and_run::empty_app()
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write(
            "db/schema.rb",
            r#"ActiveRecord::Schema.define(version: 1) do
  create_table "entries", force: :cascade do |t|
    t.string "entryable_type", null: false
    t.integer "entryable_id", null: false
    t.datetime "created_at", null: false
    t.datetime "updated_at", null: false
  end

  create_table "pages", force: :cascade do |t|
    t.datetime "created_at", null: false
    t.datetime "updated_at", null: false
  end

  create_table "sections", force: :cascade do |t|
    t.text "body"
    t.datetime "created_at", null: false
    t.datetime "updated_at", null: false
  end

  create_table "action_text_markdowns", force: :cascade do |t|
    t.text "content", default: "", null: false
    t.string "name", null: false
    t.bigint "record_id", null: false
    t.string "record_type", null: false
    t.datetime "created_at", null: false
    t.datetime "updated_at", null: false
  end
end
"#,
        )
        .write(
            "app/models/page.rb",
            "class Page < ApplicationRecord\n  has_markdown :body\n\n  def searchable_content\n    body.content\n  end\nend\n",
        )
        .write(
            "app/models/section.rb",
            "class Section < ApplicationRecord\n  def searchable_content\n    body\n  end\nend\n",
        )
        .write(
            "app/models/entry.rb",
            r#"class Entry < ApplicationRecord
  delegated_type :entryable, types: %w[ Page Section ]
  delegate :searchable_content, to: :entryable
end
"#,
        )
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\nend\n",
        )
        .run_ruby(
            r##"
page = Page.new
page.body = "# Hello"
page.save!
entry = Entry.create!(entryable: page)
raise "page? false" unless entry.page?
raise "page reader nil" unless entry.page
raise "body content lost: #{entry.page.body.content.inspect}" unless entry.page.body.content == "# Hello"
# Zero-arg `page` on a record is the delegated_type reader, not pagination.
raise "page reader must be Page, got #{entry.page.class}" unless entry.page.is_a?(Page)
raise "delegated method lost" unless entry.searchable_content == "# Hello"
section = Section.create!(body: "Section content")
section_entry = Entry.create!(entryable: section)
raise "delegated method lost on second type" unless section_entry.searchable_content == "Section content"
puts "delegated_type singular reader plain text body passed"
"##,
        )
        .assert_passes();
}

/// Rails 7.2's query assertions and the notification they are built on,
/// over the runtime's statement capture, with `connection.select_rows`
/// answering Arrays: campfire's tests count queries, assert none match a
/// pattern, and read an `EXPLAIN QUERY PLAN` through a `->(*, payload)`
/// callback (basecamp/once-campfire#295, #304, #310, #312).
#[test]
fn query_assertions_and_sql_notifications_run() {
    emit_and_run::real_blog()
        .write(
            "test/models/article_queries_test.rb",
            r#"require "test_helper"

class ArticleQueriesTest < ActiveSupport::TestCase
  test "query assertions count, match and explain" do
    article = Article.create!(title: "Counted", body: "Body text here")

    found = assert_queries_count(1) { Article.find(article.id) }
    assert_equal "Counted", found.title
    assert_no_queries { found.title }
    assert_no_queries_match(/comments/) { Article.find(article.id) }
    assert_queries_match(/articles/) { Article.where(title: "Counted").to_a }

    statements = []
    callback = ->(*, payload) { statements << payload[:sql] }
    ActiveSupport::Notifications.subscribed(callback, "sql.active_record") do
      Article.where(title: "Counted").to_a
    end
    assert_equal 1, statements.size
    # Rails' quoting, which apps' tests filter statements by
    # (`start_with?(%(SELECT "messages"))`, basecamp/once-campfire#312).
    assert statements.first.start_with?(%(SELECT "articles")), statements.first
    plan = Article.connection.select_rows("EXPLAIN QUERY PLAN #{statements.first}").map(&:last).join(" | ")
    assert_match(/articles/, plan)
  end
end
"#,
        )
        .run_test("test/models/article_queries_test.rb")
        .assert_passes();
}

/// A template `form_with` enters `ActionView::Base#form_with` when the
/// site runs, and a fragment-cache hit does not enter it again. The
/// markup stays the lowered expansion's; the instance call is what
/// `any_instance.expects(:form_with)` counts (campfire's
/// `FragmentRenderingTest` message-controls example).
#[test]
fn cached_form_with_enters_action_view_base_only_on_miss() {
    emit_and_run::real_blog()
        .write(
            "app/views/articles/_cached_form.html.erb",
            "<% cache article do %><%= form_with model: article do |form| %><%= form.text_field :title %><% end %><% end %>\n",
        )
        .run_ruby(
            r#"
module ActionView
  class Base
    def self.entries
      @entries.nil? ? 0 : @entries
    end

    def self.note!
      @entries = entries + 1
      nil
    end

    def form_with
      ActionView::Base.note!
    end
  end
end

article = Article.create!(title: "Cached form", body: "Body text here")
ActionController::Base.perform_caching = true
ActionController::Base.cache_store = ActiveSupport::Cache::MemoryStore.new
ActionController::Current.controller = ActionController::Base.new

first = Views::Articles.cached_form(article)
raise "form missing on miss: #{first.inspect}" unless first.include?("<form") && first.include?("Cached form")
raise "miss did not enter form_with (#{ActionView::Base.entries})" unless ActionView::Base.entries == 1

second = Views::Articles.cached_form(article)
raise "form missing on hit: #{second.inspect}" unless second.include?("<form") && second.include?("Cached form")
raise "cache hit re-entered form_with (#{ActionView::Base.entries})" unless ActionView::Base.entries == 1

article.update!(title: "Cached form edited")
third = Views::Articles.cached_form(article)
raise "edited form missing: #{third.inspect}" unless third.include?("Cached form edited")
raise "key change did not enter form_with (#{ActionView::Base.entries})" unless ActionView::Base.entries == 2
puts "cached form_with entry passed"
"#,
        )
        .assert_passes();
}

/// `offset(n).exists?` asks for a row past the first n (campfire's
/// `paged?`, basecamp/once-campfire#297) where a COUNT ignored the offset;
/// and the caching knobs campfire's messages caching test turns
/// (`Rails.cache =`, `ActiveSupport::Cache::MemoryStore.new`,
/// `ActionView::PartialRenderer.collection_cache`, a controller's
/// `cache_store` and `perform_caching`).
#[test]
fn offset_exists_and_caching_knobs_run() {
    emit_and_run::real_blog()
        .write(
            "test/models/article_paging_test.rb",
            r#"require "test_helper"

class ArticlePagingTest < ActiveSupport::TestCase
  test "a row past the offset, and the caching knobs" do
    Comment.delete_all
    Article.delete_all
    2.times { |i| Article.create!(title: "Paged #{i}", body: "Body text here") }
    assert Article.offset(1).exists?
    assert_not Article.offset(2).exists?

    store = ActiveSupport::Cache::MemoryStore.new
    Rails.cache = store
    ActionView::PartialRenderer.collection_cache = Rails.cache
    ArticlesController.cache_store = Rails.cache
    ArticlesController.perform_caching = true
    assert_same store, Rails.cache
    assert_same store, ActionView::PartialRenderer.collection_cache
    assert_same store, ArticlesController.cache_store
    assert ArticlesController.perform_caching
  end
end
"#,
        )
        .run_test("test/models/article_paging_test.rb")
        .assert_passes();
}

#[test]
fn the_unedited_blog_runs() {
    emit_and_run::real_blog()
        .run_test("test/controllers/articles_controller_test.rb")
        .assert_passes();
}

/// A builder wrapper's private argument computation belongs to its helper,
/// not to the view into which the form and builder body are spliced.
#[test]
fn a_form_wrapper_keeps_its_private_argument_computation_in_its_owner() {
    emit_and_run::real_blog()
        .write(
            "app/helpers/articles_helper.rb",
            r#"module ArticlesHelper
  def article_form_with!(article, suffix = "!", &)
    form_with model: article, class: "contents", data: private_options(article, suffix), &
  end
  def __rh_form_article_form_with_0; 91; end
  def article_public_form_with(model, &)
    form_with model: model, data: { controller: public_label, label: @article.title }, &
  end
  def public_label; "public-owner"; end
  private
  def private_options(article, suffix)
    article.title = article.title + suffix
    { controller: private_controller, label: article.title }
  end
  def private_controller; "owner-composer"; end
end
"#,
        )
        .write(
            "app/helpers/zzz_helper.rb",
            r#"module ZzzHelper
  def private_options(article, suffix); { controller: "wrong-owner" }; end
  def private_controller; "wrong-controller"; end
end
"#,
        )
        .edit(
            "app/views/articles/_form.html.erb",
            "form_with(model: article, class: \"contents\")",
            "article_form_with!(article)",
        )
        .write(
            "app/views/articles/_public_owner_form.html.erb",
            "<%= article_public_form_with(article) do |form| %><%= form.text_field :title %><% end %>",
        )
        .run_ruby(r#"
article = Article.new(title: "seed", body: "body")
raise "unexpected controller context" unless ActionController::Current.controller.nil?
public_html = Views::Articles.public_owner_form(article)
raise "direct helper ivar lost view binding" unless public_html.include?('data-label="seed"')
raise "public helper lost owner" unless public_html.include?('data-controller="public-owner"')
html = Views::Articles.form(article)
raise "missing form" unless html.include?("<form") && html.include?("</form>")
raise "wrong helper owner" unless html.include?('data-controller="owner-composer"')
raise "argument evaluated incorrectly" unless html.include?('data-label="seed!"') && article.title == "seed!"
raise "builder lost record" unless html.include?('name="article[title]"') && html.include?('value="seed!"')
raise "generated name collision" unless ArticlesHelper.__rh_form_article_form_with_0 == 91
[:private_options, :private_controller].each do |name|
  raise "private helper made public" if ArticlesHelper.respond_to?(name)
  begin
    name == :private_options ? ArticlesHelper.public_send(name, article, "?") : ArticlesHelper.public_send(name)
    raise "public_send exposed private helper"
  rescue NoMethodError
  end
end
puts "form wrapper owner checks passed"
"#)
        .assert_passes();
}

/// Visibility is observable runtime behavior, not just an IR annotation.
/// Cover models, POROs, and a concern's instance and copied class sides.
#[test]
fn local_method_visibility_survives_emission_and_reflective_dispatch() {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord",
            r#"class Article < ApplicationRecord
  include VisibilityHelpers
  def visibility_wrapper; visibility_helper; end
  def visibility_helper; 41; end
  private :visibility_helper
  def visibility_later; 42; end
  private
  def self.visibility_public_class; 43; end
  public
  class << self
    def visibility_class_wrapper; visibility_class_helper; end
    private
    def visibility_class_helper; 44; end
  end
  def self.visibility_class_later; 45; end
"#,
        )
        .write(
            "app/lib/visibility_probe.rb",
            r#"class VisibilityProbe
  def initialize; @value = 51; end
  def visibility_wrapper; visibility_helper; end
  private def visibility_helper; @value; end
  def visibility_later; 52; end
  protected def guarded; 53; end
  class << self
    protected
    def guarded; 54; end
  end
end
class PublicInitializeProbe
  def initialize; @value = 71; end
  public :initialize
  def self.initialize; 72; end
end
"#,
        )
        .write(
            "app/models/concerns/visibility_helpers.rb",
            r#"module VisibilityHelpers
  extend ActiveSupport::Concern
  def concern_wrapper; concern_helper; end
  private
  def concern_helper; 61; end
  class_methods do
    def concern_class_wrapper; concern_class_helper; end
    def concern_class_helper; 62; end
    private :concern_class_helper
    def concern_class_later; 63; end
  end
end
"#,
        )
        .run_ruby(r#"
def assert_equal(expected, actual)
  raise "expected #{expected.inspect}, got #{actual.inspect}" unless expected == actual
end
def rejects_public_send(receiver, name)
  begin
    receiver.public_send(name)
  rescue NoMethodError
    return
  end
  raise "public_send reached #{name}"
end
article = Article.new
probe = VisibilityProbe.new
[
  [article, :visibility_wrapper, :visibility_helper, 41],
  [probe, :visibility_wrapper, :visibility_helper, 51],
  [article, :concern_wrapper, :concern_helper, 61],
  [Article, :visibility_class_wrapper, :visibility_class_helper, 44],
  [Article, :concern_class_wrapper, :concern_class_helper, 62]
].each do |receiver, wrapper, helper, expected|
  assert_equal expected, receiver.public_send(wrapper)
  assert_equal expected, receiver.send(helper)
  rejects_public_send receiver, helper
  assert_equal false, receiver.respond_to?(helper)
  assert_equal false, receiver.respond_to?(helper, false)
  assert_equal true, receiver.respond_to?(helper, true)
end
assert_equal 42, article.public_send(:visibility_later)
assert_equal 52, probe.public_send(:visibility_later)
assert_equal 43, Article.public_send(:visibility_public_class)
assert_equal 45, Article.public_send(:visibility_class_later)
assert_equal 63, Article.public_send(:concern_class_later)
assert_equal false, article.respond_to?(:initialize)
assert_equal true, article.respond_to?(:initialize, true)
assert_equal false, probe.respond_to?(:initialize)
rejects_public_send probe, :initialize
assert_equal true, PublicInitializeProbe.new.respond_to?(:initialize)
assert_equal 71, PublicInitializeProbe.new.public_send(:initialize)
assert_equal 72, PublicInitializeProbe.public_send(:initialize)
assert_equal true, VisibilityProbe.protected_instance_methods(false).include?(:guarded)
assert_equal true, VisibilityProbe.singleton_class.protected_instance_methods(false).include?(:guarded)
rejects_public_send probe, :guarded
rejects_public_send VisibilityProbe, :guarded
puts "visibility dispatch checks passed"
"#)
        .assert_passes();
}

/// Module-function promotion retains a public singleton copy; extend self
/// instead retains the original visibility, even across bare markers.
#[test]
fn module_function_and_extend_self_have_distinct_runtime_visibility() {
    emit_and_run::real_blog()
        .write(
            "app/lib/module_visibility.rb",
            r#"module NamedVisibility
  private
  def helper; 81; end
  module_function :helper
end
module BareVisibility
  private
  module_function
  def helper; 82; end
  private :helper
  private def inline_helper; 83; end
  def later_copy; 84; end
  public
  def instance_later; 85; end
end
module ExtendedVisibility
  extend self
  private
  def helper; 91; end
  protected
  def guarded; 92; end
  public
  def later; 93; end
end
"#,
        )
        .run_ruby(r#"
def assert_equal(expected, actual)
  raise "expected #{expected.inspect}, got #{actual.inspect}" unless expected == actual
end
[
  [NamedVisibility, :helper, 81],
  [BareVisibility, :helper, 82],
  [BareVisibility, :inline_helper, 83],
  [BareVisibility, :later_copy, 84],
  [ExtendedVisibility, :later, 93]
].each do |receiver, name, expected|
  assert_equal expected, receiver.public_send(name)
  assert_equal true, receiver.respond_to?(name)
end
assert_equal false, BareVisibility.respond_to?(:instance_later, true)
assert_equal true, BareVisibility.public_instance_methods(false).include?(:instance_later)
[[ExtendedVisibility, :helper, 91], [ExtendedVisibility, :guarded, 92]].each do |receiver, name, expected|
  assert_equal expected, receiver.send(name)
  assert_equal false, receiver.respond_to?(name)
  assert_equal true, receiver.respond_to?(name, true)
  begin
    receiver.public_send(name)
  rescue NoMethodError
    next
  end
  raise "public_send reached #{name}"
end
puts "module visibility checks passed"
"#)
        .assert_passes();
}

/// A test class that a `module` wraps is emitted and runs. Ingest used
/// to read only the top-level classes of a test file. It lost this
/// class, and `check` reported nothing. The emit names the file after
/// the full class name, as it does for `class Models::ArticleTest`.
#[test]
fn a_test_class_inside_a_module_runs() {
    emit_and_run::real_blog()
        .write(
            "test/models/models_article_test.rb",
            r#"require "test_helper"

module Models
  class ArticleTest < ActiveSupport::TestCase
    test "reads a fixture" do
      assert_equal "Getting Started with Rails", articles(:one).title
    end
  end
end
"#,
        )
        .run_test("test/models/models_article_test.rb")
        .assert_passes();
}

/// An app with no jobs still runs its tests. The test helper switches
/// `ActiveJob` to enqueue at load, so it needs the runtime even when no
/// app file names `ActiveJob`.
#[test]
fn an_app_without_jobs_runs_its_tests() {
    emit_and_run::real_blog()
        .remove("app/jobs/application_job.rb")
        .run_test("test/models/article_test.rb")
        .assert_passes();
}

/// `thread_state` replaces the job queue methods with locked, per-thread
/// versions. Boot loads `active_job` first, so a job class that loads it
/// again later does not put the unlocked versions back.
#[test]
fn the_job_queue_keeps_its_thread_safe_methods() {
    emit_and_run::real_blog()
        .run_ruby(
            r#"%i[drain pending_count performed enqueue_locked record_performed_for_tests].each do |m|
  file = ActiveJob.method(m).source_location[0]
  raise "ActiveJob.#{m} comes from #{file}" unless file.end_with?("runtime/thread_state.rb")
end
# The serving drain wraps these two, and calls the locked ones above.
%i[enqueue record_performed].each do |m|
  file = ActiveJob.method(m).source_location[0]
  raise "ActiveJob.#{m} comes from #{file}" unless file.end_with?("runtime/active_job_cruby.rb")
end
puts "ok"
"#,
        )
        .assert_passes();
}

/// Source inference must preserve Ruby parameter binding and the existing
/// test lowering; inferred signatures are not permission to rewrite calls.
#[test]
fn original_test_helper_defaults_and_keyword_forwarding_run() {
    let run = emit_and_run::real_blog()
        .write(
            "test/models/source_helper_test.rb",
            r#"require "test_helper"

class SourceHelperTest < ActiveSupport::TestCase
  setup { @prefix = "scope:" }

  test "defaults and forwarding preserve original bindings" do
    assert_equal ["FIRST"], normalized("first")
    assert_equal ["chosen"], normalized("second", "chosen")
    assert_equal ["explicit"], normalized("third", "unused", ["explicit"])
    visited = 0
    ["first", "last"].each do |value|
      values_from(value: value).each do |inner|
        assert_equal @prefix + value, inner
        visited += 1
      end
    end
    assert_equal 4, visited
    assert_equal [13, "forwarded"], forwarding(13, value: "forwarded")
    assert_equal [17, "direct"], target(17, value: "direct")
    assert_equal({ value: "kept" }, whole_hash(19, value: "kept"))
    puts "PASS original source helper assertions"
  end

  def normalized(value, upper = upper_for(value), values = [upper])
    values
  end

  def upper_for(input)
    input.upcase
  end

  def values_from(**details)
    values_for(**details)
  end

  def values_for(value:)
    [@prefix + value, @prefix + value]
  end

  def forwarding(number, **details)
    target(number, **details)
  end

  def target(number, value:)
    [number, value]
  end

  def whole_hash(*numbers, **details)
    details
  end
end
"#,
        )
        .run_test("test/models/source_helper_test.rb");
    run.assert_passes();
    assert!(run.stdout.contains("PASS original source helper assertions"));
}

#[test]
fn native_date_column_crud_and_month_shifts() {
    let output = std::process::Command::new("ruby")
        .args(["tests/date_columns_runtime.rb", "native"])
        .output()
        .expect("native Ruby");
    assert!(output.status.success(), "{}\n{}",
        String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    assert!(String::from_utf8_lossy(&output.stdout).contains("Date CRUD and month shifts OK"));
}

#[test]
fn date_column_crud_and_month_shifts_run() {
    date_blog()
        .run_ruby(include_str!("date_columns_runtime.rb"))
        .assert_passes();
}

#[test]
fn date_storage_and_integer_enum_labels_coexist() {
    date_blog()
        .edit("db/schema.rb", "    t.date \"due_on\"", "    t.date \"due_on\"\n    t.integer \"state\"")
        .edit("app/models/calendar_entry.rb", "class CalendarEntry < ApplicationRecord", "class CalendarEntry < ApplicationRecord\n  enum :state, { draft: 7, published: 42 }")
        .run_ruby(r#"
entry = CalendarEntry.create!(due_on: Date.new(2024, 1, 31), state: :published)
entry.reload
raise entry.due_on.inspect unless entry.due_on.class == Date && entry.due_on.iso8601 == "2024-01-31"
raise entry.shifted(2).inspect unless entry.shifted(2).iso8601 == "2024-03-31"
raise entry.state.inspect unless entry.state == "published"
raise entry[:state].inspect unless entry[:state] == "published"
raise entry.attributes.inspect unless entry.attributes["state"] == "published"
raise "enum predicates changed" unless entry.published? && !entry.draft?
stored = CalendarEntry.connection.select_all("SELECT due_on, state FROM calendar_entries WHERE id = #{entry.id}").first
raise stored.inspect unless stored == {"due_on" => "2024-01-31", "state" => 42}
actual = ActionController::JsonRender.encode(entry.as_json(only: [:due_on, :state]))
raise actual.inspect unless actual == '{"due_on":"2024-01-31","state":"published"}'
entry.update!(due_on: nil, state: nil)
entry.reload
raise "nullable readers changed" unless entry.due_on.nil? && entry.state.nil? && entry[:state].nil? && entry.attributes["state"].nil?
stored = CalendarEntry.connection.select_all("SELECT due_on, state FROM calendar_entries WHERE id = #{entry.id}").first
raise stored.inspect unless stored == {"due_on" => nil, "state" => nil}
actual = ActionController::JsonRender.encode(entry.as_json(only: [:due_on, :state]))
raise actual.inspect unless actual == '{"due_on":null,"state":null}'
puts "Date storage and nullable sparse integer enum labels coexist"
"#)
        .assert_passes();
}

fn date_blog() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit("db/schema.rb", "  create_table \"articles\"", "  create_table \"calendar_entries\" do |t|\n    t.date \"due_on\"\n    t.datetime \"observed_at\"\n    t.time \"opens_at\"\n  end\n\n  create_table \"articles\"")
        .write("app/models/calendar_entry.rb", include_str!("date_columns_model.rb"))
}

/// ActiveSupport Date calendar: constructors, date-preserving edges, and
/// Date→Time / Integer→Time zone conversions must both type-clean and run.
#[test]
fn activesupport_date_calendar_runs() {
    emit_and_run::empty_app()
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write(
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table "events", force: :cascade do |t|
    t.date "due_on"
  end
end
"#,
        )
        .write(
            "app/models/event.rb",
            r#"class Event < ApplicationRecord
  def month_span
    due_on.beginning_of_month..due_on.end_of_month
  end

  def prior_day
    due_on.yesterday
  end

  def zoned
    due_on.in_time_zone("UTC")
  end
end
"#,
        )
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"/probe\", to: \"probe#show\"\nend\n",
        )
        .write(
            "app/controllers/probe_controller.rb",
            r#"class ProbeController < ApplicationController
  def show
    event = Event.create!(due_on: Date.new(2024, 1, 31))
    cur = Date.current
    yday = Date.yesterday
    span = event.month_span
    prior = event.prior_day
    # Non-nilable Date literal: column readers are Date? and binop gate
    # refuses Date? + Integer (see #394); day arithmetic on a known Date
    # still grounds through date_days_since.
    shifted = Date.new(2024, 1, 31) + 2
    zoned = event.zoned
    # Today must not be past? (Rails Date#past? is self < Date.current).
    today_past = Date.current.past?
    old_past = Date.new(2020, 1, 1).past?
    epoch = 1_704_067_200.in_time_zone("UTC")
    render plain: [
      cur.class.name,
      yday.class.name,
      span.begin.iso8601,
      span.end.iso8601,
      prior.iso8601,
      shifted.iso8601,
      zoned.year,
      today_past,
      old_past,
      epoch.year
    ].join(",")
  end
end
"#,
        )
        .run_ruby(
            r#"
require_relative "app/controllers/probe_controller"
controller = ProbeController.new
controller.process_action(:show)
parts = controller.body.split(",")
raise "Date.current class: #{parts[0]}" unless parts[0] == "Date"
raise "Date.yesterday class: #{parts[1]}" unless parts[1] == "Date"
raise "beginning_of_month: #{parts[2]}" unless parts[2] == "2024-01-01"
raise "end_of_month: #{parts[3]}" unless parts[3] == "2024-01-31"
raise "yesterday: #{parts[4]}" unless parts[4] == "2024-01-30"
raise "Date+2: #{parts[5]}" unless parts[5] == "2024-02-02"
raise "in_time_zone year: #{parts[6]}" unless parts[6] == "2024"
raise "today.past?: #{parts[7]}" unless parts[7] == "false"
raise "old.past?: #{parts[8]}" unless parts[8] == "true"
raise "Integer#in_time_zone year: #{parts[9]}" unless parts[9] == "2024"
puts "ActiveSupport Date calendar OK"
"#,
        )
        .assert_passes();
}

/// Spinel has no native `Date#+`; grounding must carry constructors,
/// day arithmetic, and calendar-day `past?` — CRuby stdlib can mask that.
#[test]
#[ignore = "requires the Spinel toolchain"]
fn activesupport_date_calendar_runs_on_spinel() {
    date_blog()
        .edit(
            "app/models/calendar_entry.rb",
            "\nend\n",
            "\n  def prior_day\n    due_on.yesterday\n  end\n\n  def self.probe\n    entry = create!(due_on: Date.new(2024, 1, 31))\n    [\n      Date.current.class.name,\n      entry.due_on.beginning_of_month.iso8601,\n      entry.due_on.end_of_month.iso8601,\n      entry.prior_day.iso8601,\n      (Date.new(2024, 1, 31) + 2).iso8601,\n      Date.current.past?,\n      Date.new(2020, 1, 1).past?,\n    ]\n  end\nend\n",
        )
        .run_spinel(
            r#"
Db.configure(":memory:")
Schema.statements.each { |sql| Db.exec(sql) }
ActiveRecord.adapter = SqliteAdapter
parts = CalendarEntry.probe
raise "Date.current class: #{parts[0]}" unless parts[0] == "Date"
raise "beginning_of_month: #{parts[1]}" unless parts[1] == "2024-01-01"
raise "end_of_month: #{parts[2]}" unless parts[2] == "2024-01-31"
raise "yesterday: #{parts[3]}" unless parts[3] == "2024-01-30"
raise "Date+2: #{parts[4]}" unless parts[4] == "2024-02-02"
raise "today.past?: #{parts[5]}" unless parts[5] == false
raise "old.past?: #{parts[6]}" unless parts[6] == true
puts "ActiveSupport Date calendar OK on Spinel"
"#,
        )
        .assert_passes();
}

/// ActiveSupport's Date calendar extensions and `Date.current`, which
/// reads today in the app's zone rather than the host's.
#[test]
fn date_calendar_extensions_and_current_run() {
    date_blog()
        .edit(
            "app/models/calendar_entry.rb",
            "\nend\n",
            "\n  def month_span\n    [due_on.beginning_of_month, due_on.end_of_month]\n  end\n\n  def day_edges\n    [due_on.beginning_of_day, due_on.end_of_day]\n  end\n\n  def self.current_day\n    Date.current\n  end\nend\n",
        )
        .run_ruby(r#"
entry = CalendarEntry.create!(due_on: Date.new(2024, 2, 10))
first, last = entry.month_span
raise first.inspect unless first == Date.new(2024, 2, 1)
raise last.inspect unless last == Date.new(2024, 2, 29)
ActiveSupport.use_zone("Asia/Tokyo") do
  b, e = entry.day_edges
  raise b.inspect unless [b.year, b.month, b.day, b.hour, b.min, b.sec] == [2024, 2, 10, 0, 0, 0]
  raise e.inspect unless [e.year, e.month, e.day, e.hour, e.min, e.sec] == [2024, 2, 10, 23, 59, 59]
  raise b.utc_offset.inspect unless b.utc_offset == 9 * 3600
end
east = ActiveSupport.use_zone("Pacific/Kiritimati") { CalendarEntry.current_day }
west = ActiveSupport.use_zone("Pacific/Pago_Pago") { CalendarEntry.current_day }
raise [east, west].inspect unless east.is_a?(Date) && east > west
"#)
        .assert_passes();
}

fn date_json_blog() -> emit_and_run::Overlay {
    date_blog()
        .edit("app/models/calendar_entry.rb", "\nend\n", "\n  def as_json(options = {})\n    attrs = [:due_on, :observed_at]\n    json = super(only: attrs)\n    json\n  end\nend\n")
        .write("app/controllers/calendar_entries_controller.rb", "class CalendarEntriesController < ApplicationController\n  def show\n    entry = CalendarEntry.find(params[:id])\n    render json: entry\n  end\nend\n")
        .edit("config/routes.rb", "  resources :articles do", "  resources :calendar_entries, only: [:show]\n  resources :articles do")
}

#[test]
fn specialized_date_json_preserves_dates_and_zoned_timestamps() {
    date_json_blog()
        .run_ruby(r#"
require_relative "app/controllers/calendar_entries_controller"
entry = CalendarEntry.create!(due_on: Date.new(2024, 1, 31), observed_at: Time.utc(2024, 1, 31, 23, 47, 19, 123456))
raise "specialization was not exercised" unless entry.respond_to?(:as_json_str)
ActiveSupport.use_zone("Pacific/Auckland") do
  controller = CalendarEntriesController.new
  controller.params = {"id" => entry.id.to_s}
  controller.process_action(:show)
  expected = '{"due_on":"2024-01-31","observed_at":"2024-02-01T12:47:19.123+13:00"}'
  raise controller.body.inspect unless controller.body == expected
  raise controller.content_type.inspect unless controller.content_type == "application/json"
  actual = ActionController::JsonRender.encode(entry.as_json)
  raise actual.inspect unless actual == expected
  entry.update!(due_on: nil)
  controller = CalendarEntriesController.new
  controller.params = {"id" => entry.id.to_s}
  controller.process_action(:show)
  expected = '{"due_on":null,"observed_at":"2024-02-01T12:47:19.123+13:00"}'
  raise controller.body.inspect unless controller.body == expected
  actual = ActionController::JsonRender.encode(entry.as_json)
  raise actual.inspect unless actual == expected
end
puts "Specialized Date JSON and timestamp control OK"
"#)
        .assert_passes();
}

#[test]
fn specialized_date_json_normalizes_unset_nonnullable_storage() {
    date_json_blog()
        .edit("db/schema.rb", "t.date \"due_on\"", "t.date \"due_on\", null: false")
        .run_ruby(r#"
entry = CalendarEntry.new
raise entry.due_on_raw.inspect unless entry.due_on_raw == ""
raise entry.due_on.inspect unless entry.due_on.nil?
raise entry[:due_on].inspect unless entry[:due_on].nil?
raise "unset Date alias changed" unless entry.shifted_attribute(1).nil?
expected = '{"due_on":null,"observed_at":null}'
raise entry.as_json_str.inspect unless entry.as_json_str == expected
actual = ActionController::JsonRender.encode(entry.as_json)
raise actual.inspect unless actual == expected
puts "Unset nonnullable Date JSON is null in both paths"
"#)
        .assert_passes();
}

/// A jbuilder view over a date column renders what Rails 8.1 + jbuilder
/// 2.15 render: the ISO date, or `null` — both through `json.extract!`
/// and a bare `json.key record.col` pair. The view used to send the
/// date's stored text through `encode_datetime`, which quoted the ""
/// an unset nonnullable slot holds (`"due_on":""`). A timestamp column
/// in the same view keeps its `encode_datetime` route.
#[test]
fn jbuilder_date_column_renders_the_iso_date_or_null() {
    date_blog()
        .edit("db/schema.rb", "t.date \"due_on\"", "t.date \"due_on\", null: false")
        .write("app/controllers/calendar_entries_controller.rb", "class CalendarEntriesController < ApplicationController\n  def show\n    @calendar_entry = CalendarEntry.find(params[:id])\n  end\n\n  def fresh\n    @calendar_entry = CalendarEntry.new\n    render :show\n  end\nend\n")
        .write("app/views/calendar_entries/show.json.jbuilder", "json.extract! @calendar_entry, :due_on, :observed_at\njson.due @calendar_entry.due_on\n")
        .edit("config/routes.rb", "  resources :articles do", "  resources :calendar_entries, only: [:show] do\n    get :fresh, on: :collection\n  end\n  resources :articles do")
        .run_ruby(r#"
require_relative "app/controllers/calendar_entries_controller"
entry = CalendarEntry.create!(due_on: Date.new(2024, 1, 31), observed_at: Time.utc(2024, 1, 31, 23, 47, 19, 123456))
controller = CalendarEntriesController.new
controller.params = {"id" => entry.id.to_s}
controller.process_action(:show)
expected = '{"due_on":"2024-01-31","observed_at":"2024-01-31T23:47:19.123Z","due":"2024-01-31"}'
raise controller.body.inspect unless controller.body == expected
controller = CalendarEntriesController.new
controller.params = {}
controller.process_action(:fresh)
expected = '{"due_on":null,"observed_at":null,"due":null}'
raise controller.body.inspect unless controller.body == expected
puts "jbuilder Date JSON is the ISO date or null"
"#)
        .assert_passes();
}

/// Alba's inherited declarations are executable property reads, not just a
/// return-type assertion. Boot loads the generated classes without Alba.
#[test]
fn alba_inherited_attributes_and_one_nested_resource_run() {
    emit_and_run::real_blog()
        .write("app/lib/alba_resources.rb", include_str!("support/alba.rb"))
        .write("app/controllers/alba_probes_controller.rb", r#"
class AlbaProbesController < ApplicationController
  def index
    author = AlbaAuthor.new(9, "Ada")
    article = AlbaArticle.new(7, "Syn", author)
    render json: ArticleResource.new(article).to_h
  end
end
"#)
        .run_ruby(r#"
expected = {"id" => 7, "title" => "Synthetic", "author" => {"id" => 9, "name" => "Ada"}}
actual = SurveyProbe.call
raise actual.inspect unless actual == expected
raise "external Alba loaded" if defined?(Alba::Resource)
puts "PASS portable Alba source-property contract"
"#)
        .assert_passes();
}

/// A delegated setter going from broken (`def behavior=\n  x.behavior=\n
/// end` — a `def` with no parameter and a bare `x.y=` call, two syntax
/// errors) to working is a claim the emitted program actually runs a
/// SET through it, not just that `check` stays clean (invariant 6). A
/// PORO under `app/lib` (the same shape `procore_os/deprecation.rb`
/// declares: `attr_accessor` on the target, `delegate` for a setter)
/// forwards to another object — reusing `Article`, since it already
/// has a `title` column — and a model test sets through the forwarder
/// and reads the value back off the target.
#[test]
fn a_delegated_setter_forwards_through_to_its_target() {
    emit_and_run::real_blog()
        .write(
            "app/lib/deprecation.rb",
            "class Deprecation\n  attr_accessor :inner\n\n  delegate :title=, to: :inner\nend\n",
        )
        .write(
            "test/models/deprecation_test.rb",
            "require \"test_helper\"\n\n\
             class DeprecationTest < ActiveSupport::TestCase\n  \
               test \"a delegated setter forwards through to its target\" do\n    \
                 d = Deprecation.new\n    \
                 d.inner = Article.new\n    \
                 d.title = \"Reused\"\n    \
                 assert_equal \"Reused\", d.inner.title\n  \
               end\n\
             end\n",
        )
        .run_test("test/models/deprecation_test.rb")
        .assert_passes();
}

/// A lambda-target `before_action` — `before_action -> { … }, only:
/// […]`, no Symbol target and no attached block either — going from
/// "controller class-body macro not recognized" to clean is a claim
/// the emitted dispatcher actually RUNS the lambda's body and applies
/// its `only:` scoping (invariant 6), not just that `check` stops
/// complaining. 233 Procore controllers write exactly this shape for
/// a policy guard closing over a receiverless call
/// (`ensure_permission(documents_policy.configure_tab?)`), not a
/// literal condition inline in the lambda — so this pins that exact
/// pattern: the lambda calls a private predicate method, and the
/// existing "should show article" test is updated to expect the
/// redirect the (permanently failing, for the test) predicate causes.
///
/// NOTE ON SCOPE: a lambda body that reads `params[...]` directly
/// (rather than through a named method) is a NARROWER, separate gap —
/// `rewrite_params`, the pass that turns `params[:x]` into the
/// runtime's string-keyed `@params.fetch("x", ...)`, runs over each
/// action/helper body individually and is never reached by a
/// `PreambleStmt::Block`'s body, which is assembled straight into
/// `process_action` after that pass has already run. None of the 233
/// real sites hit it (`ensure_permission`, `policy.can_view_recycle_bin?`,
/// … all delegate to a named method, whose OWN body goes through the
/// normal per-method pipeline and gets `params` rewritten there); a
/// site that inlined `params[...]` directly in the lambda would not.
/// Recorded here rather than silently left for the next person to
/// rediscover.
#[test]
fn a_lambda_target_before_action_gates_the_action_it_guards() {
    emit_and_run::real_blog()
        .edit(
            "app/controllers/articles_controller.rb",
            "before_action :set_article, only: %i[ show edit update destroy ]",
            "before_action :set_article, only: %i[ show edit update destroy ]\n  before_action -> { redirect_to root_path unless allowed_to_view? }, only: [:show]",
        )
        .edit(
            "app/controllers/articles_controller.rb",
            "  private\n",
            "  private\n\n  def allowed_to_view?\n    false\n  end\n",
        )
        .edit(
            "test/controllers/articles_controller_test.rb",
            "test \"should show article\" do\n    get article_url(@article)\n    assert_response :success\n    assert_select \"h1\", @article.title\n    assert_select \"h2\", \"Comments\"\n    assert_select \"#comments .p-4\", minimum: 1\n  end",
            "test \"a lambda-target before_action redirects when its guard fails\" do\n    get article_url(@article)\n    assert_redirected_to root_url\n  end",
        )
        .run_test("test/controllers/articles_controller_test.rb")
        .assert_passes();
}

/// An inner class wins over another class with the same last segment.
#[test]
fn a_bare_inner_class_runs_after_resolution() {
    emit_and_run::real_blog()
        .write(
            "app/services/ui/selector.rb",
            "module UI\n  class Selector\n    class Mode\n      def self.value\n        \"selected\"\n      end\n    end\n    def self.value\n      Mode.value\n    end\n  end\n  class Other\n    class Mode\n    end\n  end\nend\n",
        )
        .run_ruby("raise 'wrong inner class' unless UI::Selector.value == 'selected'")
        .assert_passes();
}

/// A library-only path gem is app code, and its consumer must run in the output.
#[test]
fn a_library_only_path_gem_runs_from_an_app_consumer() {
    emit_and_run::real_blog()
        .edit(
            "Gemfile.lock",
            "GEM\n",
            "PATH\n  remote: components/numbers\n  specs:\n    path_numbers (0.1.0)\n\nGEM\n",
        )
        .write(
            "components/numbers/lib/path_number.rb",
            "class PathNumber\n  def self.value\n    41\n  end\nend\n",
        )
        .write(
            "app/services/path_number_consumer.rb",
            "class PathNumberConsumer\n  def self.value\n    PathNumber.value + 1\n  end\nend\n",
        )
        .run_ruby("raise 'wrong path-gem result' unless PathNumberConsumer.value == 42")
        .assert_passes();
}

/// `class UI::ExplicitSelector` does not lexically include `UI`, even
/// though emitted Ruby nests it there. Keep a top-level same-suffix class
/// distinct from the one inside UI after source-backed resolution.
#[test]
fn a_compact_class_uses_its_source_lexical_constant() {
    emit_and_run::real_blog()
        .write(
            "app/services/ui/explicit_selector.rb",
            "class SourceScopeResolution\n  def self.value\n    \"top-level\"\n  end\nend\nmodule UI\n  class SourceScopeResolution\n    def self.value\n      \"nested\"\n    end\n  end\nend\nclass UI::ExplicitSelector\n  def self.value\n    SourceScopeResolution.value\n  end\nend\n",
        )
        .run_ruby(
            "raise 'wrong lexical constant' unless UI::ExplicitSelector.value == 'top-level'",
        )
        .assert_passes();
}

/// Array `find` is not a scalar lookup or a permissive `where(id: ids)`:
/// it raises on a missing scoped row, and preserves requested order unless
/// the relation carries an explicit order. The terminal cannot poison its
/// receiver's WHERE, limit/offset, or loaded-record cache.
#[test]
fn relation_find_with_array_ids_runs() {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            r#"class Article < ApplicationRecord
  has_many :comments, dependent: :destroy

  def self.find_titles(ids)
    where("title != 'outside'").find(Array(ids)).map(&:title).join("|")
  end"#,
        )
        .run_ruby(
            r##"a = Article.create!(title: "zebra", body: "long enough body")
b = Article.create!(title: "apple", body: "long enough body")
c = Article.create!(title: "outside", body: "long enough body")
raise "requested order" unless Article.find_titles([b.id.to_s, a.id.to_s, b.id.to_s]) == "apple|zebra"
raise "integer ids" unless Article.find_titles([a.id, b.id]) == "zebra|apple"
raise "mixed ids" unless Article.find_titles([b.id.to_s, a.id]) == "apple|zebra"
raise "singleton array" unless Article.find_titles([b.id.to_s]) == "apple"
raise "empty array" unless Article.find_titles([]) == ""
raise "integer casting" unless Article.find_titles(["#{b.id}-slug", a.id.to_s]) == "apple|zebra"
rel = ActiveRecord::Relation.new(Article).where("title != 'outside'")
rel.to_a
raise "loaded scalar" unless rel.find(b.id.to_s).title == "apple"
raise "loaded array" unless rel.find([b.id, a.id]).map(&:title) == ["apple", "zebra"]
begin
  rel.find([a.id, c.id])
  raise "a scoped-out id was accepted"
rescue ActiveRecord::RecordNotFound
end
begin
  rel.find([a.id, 987654321])
  raise "a missing id was accepted"
rescue ActiveRecord::RecordNotFound
end
begin
  rel.find([b.id.to_s, "0#{b.id}"])
  raise "ids were deduplicated after casting"
rescue ActiveRecord::RecordNotFound
end
raise "scope/cache poisoned" unless rel.to_a.map(&:title) == ["zebra", "apple"]
ordered = ActiveRecord::Relation.new(Article).where("title != 'outside'").order(:title)
raise "relation order" unless ordered.find([a.id, b.id]).map(&:title) == ["apple", "zebra"]
offset_only = ActiveRecord::Relation.new(Article).where("title != 'outside'").order(:title).offset(1)
prior_sql = offset_only.to_sql
raise "ordered offset only" unless offset_only.find([a.id, b.id]).map(&:title) == ["zebra"]
raise "offset poisoned" unless offset_only.to_sql == prior_sql
boundary = ActiveRecord::Relation.new(Article).order(:title).offset(2)
raise "offset at size" unless boundary.find([a.id, b.id]) == []
# Writebook's pinned Rails finder raises beyond size (negative expected
# cardinality); newer Rails returns [] here instead.
beyond = ActiveRecord::Relation.new(Article).order(:title).offset(3)
prior_sql = beyond.to_sql
begin
  beyond.find([a.id, b.id])
  raise "pinned Rails beyond-offset behavior changed"
rescue ActiveRecord::RecordNotFound
end
raise "beyond-offset poisoned" unless beyond.to_sql == prior_sql
paged = ActiveRecord::Relation.new(Article).where("title != 'outside'").limit(1).offset(1)
raise "input slicing" unless paged.find([b.id, a.id]).map(&:title) == ["zebra"]
raise "pagination poisoned" unless paged.to_a.map(&:title) == ["apple"]
ordered = ordered.limit(1).offset(1)
raise "ordered pagination" unless ordered.find([b.id, a.id]).map(&:title) == ["zebra"]
selected = ActiveRecord::Relation.new(Article).select(:title)
prior_sql = selected.to_sql
raise "projected key" unless selected.find([b.id, a.id]).map(&:id) == [b.id, a.id]
raise "projection poisoned" unless selected.to_sql == prior_sql
puts "ok"
"##,
        )
        .assert_passes();
}

/// `Relation#last_n` is Rails' SQL tail (`ORDER BY … DESC LIMIT n`, then
/// reverse), not `to_a.last(n)` over the whole history. Campfire's
/// `ordered.last(PAGE_SIZE)` is every room page.
#[test]
fn relation_last_n_limits_in_sql() {
    emit_and_run::real_blog()
        .run_ruby(
            r##"
seen = []
orig = Db.method(:prepare)
Db.define_singleton_method(:prepare) do |sql|
  seen << sql
  orig.call(sql)
end

Article.create!(title: "tail-a", body: "long enough body")
Article.create!(title: "tail-b", body: "long enough body")
Article.create!(title: "tail-c", body: "long enough body")
Article.create!(title: "tail-d", body: "long enough body")
Article.create!(title: "tail-e", body: "long enough body")

rel = ActiveRecord::Relation.new(Article).where("title LIKE 'tail-%'").order(:title)
prior = rel.to_sql
seen.clear
titles = rel.last_n(2).map(&:title)
raise "tail in relation order: #{titles.inspect}" unless titles == ["tail-d", "tail-e"]
raise "last_n poisoned the chain: #{rel.to_sql}" unless rel.to_sql == prior
sql = seen.find { |s| s.include?("FROM articles") && s.include?("LIMIT") }
raise "last_n did not LIMIT in SQL: #{seen.inspect}" if sql.nil?
raise "reversed order missing: #{sql}" unless sql.upcase.include?("TITLE DESC")
raise "LIMIT 2 missing: #{sql}" unless sql.include?("LIMIT 2")
raise "count poisoned" unless rel.count == 5

raise "bare last" unless rel.last.title == "tail-e"
raise "last poisoned the chain" unless rel.to_sql == prior

rel.to_a
seen.clear
loaded = rel.last_n(2).map(&:title)
raise "loaded tail: #{loaded.inspect}" unless loaded == ["tail-d", "tail-e"]
raise "loaded last_n re-queried: #{seen.inspect}" if seen.any? { |s| s.include?("FROM articles") && s.include?("LIMIT") }

off = ActiveRecord::Relation.new(Article).where("title LIKE 'tail-%'").order(:title).offset(1)
raise "offset tail" unless off.last_n(2).map(&:title) == ["tail-d", "tail-e"]

rel = ActiveRecord::Relation.new(Article)
raise "one col" unless rel.reverse_order_term("title DESC") == "title ASC"
raise "hash join" unless rel.reverse_order_term("a ASC, b DESC") == "a DESC, b ASC"
raise "raw pair" unless rel.reverse_order_term("created_at DESC, id DESC") == "created_at ASC, id ASC"
raise "bare" unless rel.reverse_order_term("title") == "title DESC"
puts "ok"
"##,
        )
        .assert_passes();
}

/// Overlay for the `cached: true` collection-cache gate probes.
fn cached_collection_probe() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            r#"class Article < ApplicationRecord
  has_many :comments, dependent: :destroy
  def self.render_count
    @render_count || 0
  end
  def self.reset_render_count
    @render_count = 0
  end
  def self.bump_render
    @render_count = render_count + 1
  end
  def bump_render
    Article.bump_render
    title
  end
"#,
        )
        .edit(
            "app/controllers/articles_controller.rb",
            "  def index\n    @articles = Article.includes(:comments).order(created_at: :desc)\n  end\n",
            "  def index\n    @articles = Article.includes(:comments).order(created_at: :desc)\n  end\n\n  def probe\n    @articles = Article.order(:title)\n  end\n",
        )
        .write(
            "app/views/articles/_probe_row.html.erb",
            "<% probe_row.bump_render %><i><%= probe_row.title %></i>\n",
        )
        .write(
            "app/views/articles/probe.html.erb",
            "<%= render partial: \"articles/probe_row\", collection: @articles, cached: true %>\n",
        )
}

/// `n` rows, `second` bumps on the second render (`0` = store hit).
fn assert_cached_collection_probe(n: i64, second: i64) {
    cached_collection_probe()
        .run_ruby(&format!(
            r#"
Article.delete_all
{n}.times {{ |i| Article.create!(title: "row-#{{i}}", body: "long enough body") }}
rows = ActiveRecord::Relation.new(Article).to_a.sort_by {{ |a| a.title }}
ActionController::Base.perform_caching = true
ActionController::Current.controller = ActionController::Base.new
Article.reset_render_count
a = Views::Articles.probe(rows)
raise "first #{{Article.render_count}}: #{{a}}" unless Article.render_count == {n}
Article.reset_render_count
b = Views::Articles.probe(rows)
raise "second #{{Article.render_count}}: #{{b}}" unless Article.render_count == {second}
raise "html drifted #{{a.inspect}} vs #{{b.inspect}}" unless a == b
puts "ok"
"#
        ))
        .assert_passes();
}

/// `cached: true` on a collection render is one store read of the
/// concatenated partials. A second render of the same records must not
/// run the inner fragment bodies. Needs more than
/// `MAX_UNCACHED_COLLECTION_LENGTH` rows — at or below that the cost
/// gate skips the store.
#[test]
fn cached_true_collection_skips_partial_bodies_on_hit() {
    let n = roundhouse::lower::MAX_UNCACHED_COLLECTION_LENGTH + 1;
    assert_cached_collection_probe(n, 0);
}

/// Small `cached: true` collections skip the store: key-build + read
/// would cost more than rendering (Campfire sidebar after #488).
#[test]
fn cached_true_small_collection_skips_the_store() {
    assert_cached_collection_probe(2, 2);
}

/// The exclusive gate: length == MAX is still uncached.
#[test]
fn cached_true_collection_at_gate_skips_the_store() {
    let n = roundhouse::lower::MAX_UNCACHED_COLLECTION_LENGTH;
    assert_cached_collection_probe(n, n);
}

/// `rel.more_than?(n)` is `SELECT 1 LIMIT 1 OFFSET n` with the same
/// FROM/JOIN/WHERE as COUNT, and the relation is not mutated. Campfire's
/// `Message.paged?` is `count > PAGE_SIZE` rewritten to this method.
#[test]
fn relation_more_than_probes_offset_without_count() {
    emit_and_run::real_blog()
        .write(
            "test/models/article_more_than_test.rb",
            r#"require "test_helper"

class ArticleMoreThanTest < ActiveSupport::TestCase
  test "more_than? offsets without COUNT or mutating the relation" do
    Comment.delete_all
    Article.delete_all
    3.times { |i| Article.create!(title: "more-#{i}", body: "Body text here") }
    rel = Article.where("title LIKE 'more-%'")
    prior = rel.to_sql
    statements = []
    callback = ->(*, payload) { statements << payload[:sql] }
    ActiveSupport::Notifications.subscribed(callback, "sql.active_record") do
      assert rel.more_than?(2)
      assert_not rel.more_than?(3)
    end
    assert_equal prior, rel.to_sql
    sql = statements.find { |s| s.include?("OFFSET 2") }
    assert sql, statements.inspect
    assert_no_match(/COUNT/i, sql)
    assert_match(/LIMIT 1/, sql)
  end
end
"#,
        )
        .run_test("test/models/article_more_than_test.rb")
        .assert_passes();
}

/// The runtime defines this exception in `active_support_ext.rb`.
#[test]
fn framework_exception_resolves_from_real_runtime_source() {
    emit_and_run::real_blog()
        .write(
            "app/services/signature_probe.rb",
            "class SignatureProbe\n  def self.call\n    begin\n      raise ActiveSupport::MessageVerifier::InvalidSignature\n    rescue ActiveSupport::MessageVerifier::InvalidSignature\n      \"handled\"\n    end\n  end\nend\n",
        )
        .run_ruby("raise 'signature error was not caught' unless SignatureProbe.call == 'handled'")
        .assert_passes();
}

/// Rubydex promotes `X = <call>` to a module once code calls a method
/// on `X`. These are still values: `.freeze` and `.map` build a Hash, an
/// Array, and a String, and each read must type and run as that value.
#[test]
fn a_constant_assigned_from_a_call_runs_as_its_value() {
    emit_and_run::real_blog()
        .write(
            "app/services/frozen_table.rb",
            "class FrozenTable\n  STATUSES = { processed: \"processed\" }.freeze\n  NAMES = [\"a\", \"b\"].freeze\n  LABEL = \"label\".freeze\n  DOUBLED = [1, 2].map { |n| n * 2 }\n  def self.summary\n    [STATUSES[:processed].upcase, NAMES.first, LABEL.upcase, DOUBLED.last.to_s].join(\",\")\n  end\nend\n",
        )
        .run_ruby("raise 'frozen constant' unless FrozenTable.summary == 'PROCESSED,a,LABEL,4'")
        .assert_passes();
}

/// Ingest copies a concern's methods into the controller that includes
/// it, and the emitted controller drops the `include`. A class that
/// resolved inside the concern's module must still name that class
/// after the move.
#[test]
fn a_concern_class_reference_survives_the_copy_into_its_controller() {
    emit_and_run::real_blog()
        .write(
            "app/services/price_support.rb",
            "module PriceSupport\n  class RateCalculator\n    def self.value\n      7\n    end\n  end\n\n  def price\n    RateCalculator.value\n  end\nend\n",
        )
        .write(
            "app/controllers/quotes_controller.rb",
            "class QuotesController < ApplicationController\n  include PriceSupport\n\n  def show\n    @value = price\n  end\nend\n",
        )
        .run_ruby(
            "require_relative 'app/controllers/quotes_controller'\nraise 'concern class reference' unless QuotesController.new.price == 7",
        )
        .assert_passes();
}

/// A concern that includes another concern inside its `included do`
/// block: ActiveSupport::Concern runs that block on the includer, so
/// the model gets the inner concern's methods too.
#[test]
fn a_concern_included_from_an_included_block_reaches_the_model() {
    emit_and_run::real_blog()
        .write(
            "app/models/concerns/signing.rb",
            "module Signing\n  extend ActiveSupport::Concern\n\n  included do\n    include Signing::Codes\n  end\nend\n",
        )
        .write(
            "app/models/concerns/signing/codes.rb",
            "module Signing::Codes\n  extend ActiveSupport::Concern\n\n  def shout\n    title.upcase\n  end\nend\n",
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n  include Signing\n",
        )
        .edit(
            "app/controllers/articles_controller.rb",
            "    @articles = Article.includes(:comments).order(created_at: :desc)\n",
            "    @articles = Article.includes(:comments).order(created_at: :desc)\n    @loudest = @articles.first&.shout\n",
        )
        .run_ruby(
            "a = Article.create!(title: \"Hi\", body: \"Body text here\")\nraise a.shout unless a.shout == \"HI\"",
        )
        .assert_passes();
}

/// real-blog with `ApplicationController` including the `Outer` concern
/// (`outer`), the extra concern files in `modules`, and a
/// `StampedController` (`body`) at `/stamped/:id` and `/stamped/:id/plain`.
fn controller_concern_app(outer: &str, modules: &[(&str, &str)], body: &str, test: &str) -> emit_and_run::Overlay {
    let mut app = emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  get \"/stamped/:id\", to: \"stamped#show\"\n  get \"/stamped/:id/plain\", to: \"stamped#plain\"\n",
        )
        .write("app/controllers/concerns/outer.rb", outer)
        .edit(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\n",
            "class ApplicationController < ActionController::Base\n  include Outer\n",
        )
        .write("app/controllers/stamped_controller.rb", body)
        .write(
            "test/controllers/stamped_controller_test.rb",
            &format!("require \"test_helper\"\n\nclass StampedControllerTest < ActionDispatch::IntegrationTest\n{test}end\n"),
        );
    for (path, source) in modules {
        app = app.write(path, source);
    }
    app
}

/// `Outer`'s `included do` includes `Guard`, the way action_policy's setup
/// includes `ActionPolicy::Controller`. The controller gets `Guard`'s
/// method, and `Guard`'s `class_methods` filter macro runs.
#[test]
fn a_concern_included_from_an_included_block_reaches_the_controller() {
    controller_concern_app(
        "module Outer\n  extend ActiveSupport::Concern\n\n  included do\n    include Guard\n  end\nend\n",
        &[(
            "app/controllers/concerns/guard.rb",
            "module Guard\n  extend ActiveSupport::Concern\n\n  def guarded_title\n    \"guarded\"\n  end\n\n  def stamp\n    @stamp = \"stamped\"\n  end\n\n  class_methods do\n    def stamp_before(**options)\n      before_action :stamp, options\n    end\n  end\nend\n",
        )],
        "class StampedController < ApplicationController\n  stamp_before only: :show\n\n  def show\n    render plain: \"#{guarded_title} #{@stamp}\"\n  end\n\n  def plain\n    render plain: \"#{guarded_title} #{@stamp.inspect}\"\n  end\nend\n",
        "  test \"Guard's method and filter macro reach the controller\" do\n    get \"/stamped/1\"\n    assert_equal \"guarded stamped\", response.body\n    get \"/stamped/2/plain\"\n    assert_equal \"guarded nil\", response.body\n  end\n",
    )
    .run_test("test/controllers/stamped_controller_test.rb")
    .assert_passes();
}

/// `Guard` sits ahead of `Outer`, so its `guarded_title` wins, as in Ruby.
#[test]
fn an_include_from_an_included_block_takes_precedence_on_the_controller() {
    controller_concern_app(
        "module Outer\n  extend ActiveSupport::Concern\n\n  included do\n    include Guard\n  end\n\n  def guarded_title\n    \"outer\"\n  end\nend\n",
        &[(
            "app/controllers/concerns/guard.rb",
            "module Guard\n  extend ActiveSupport::Concern\n\n  def guarded_title\n    \"guard\"\n  end\nend\n",
        )],
        "class StampedController < ApplicationController\n  def show\n    render plain: guarded_title\n  end\n\n  def plain\n    render plain: guarded_title\n  end\nend\n",
        "  test \"Guard's method wins over Outer's\" do\n    get \"/stamped/1\"\n    assert_equal \"guard\", response.body\n  end\n",
    )
    .run_test("test/controllers/stamped_controller_test.rb")
    .assert_passes();
}

/// `include First, Second` in an `included do` block puts `First` ahead
/// of `Second`, as Ruby does, so `First`'s `guarded_title` wins.
#[test]
fn a_multi_module_include_in_an_included_block_keeps_its_order_on_the_controller() {
    controller_concern_app(
        "module Outer\n  extend ActiveSupport::Concern\n\n  included do\n    include First, Second\n  end\nend\n",
        &[
            (
                "app/controllers/concerns/first.rb",
                "module First\n  extend ActiveSupport::Concern\n\n  def guarded_title\n    \"first\"\n  end\nend\n",
            ),
            (
                "app/controllers/concerns/second.rb",
                "module Second\n  extend ActiveSupport::Concern\n\n  def guarded_title\n    \"second\"\n  end\nend\n",
            ),
        ],
        "class StampedController < ApplicationController\n  def show\n    render plain: guarded_title\n  end\n\n  def plain\n    render plain: guarded_title\n  end\nend\n",
        "  test \"First's method wins over Second's\" do\n    get \"/stamped/1\"\n    assert_equal \"first\", response.body\n  end\n",
    )
    .run_test("test/controllers/stamped_controller_test.rb")
    .assert_passes();
}

/// `include Guard` written inside `module Outer` names `Outer::Guard`, as
/// Ruby's lexical lookup finds it.
#[test]
fn an_included_block_include_resolves_in_its_concerns_namespace() {
    controller_concern_app(
        "module Outer\n  extend ActiveSupport::Concern\n\n  included do\n    include Guard\n  end\nend\n",
        &[(
            "app/controllers/concerns/outer/guard.rb",
            "module Outer\n  module Guard\n    extend ActiveSupport::Concern\n\n    def guarded_title\n      \"namespaced\"\n    end\n  end\nend\n",
        )],
        "class StampedController < ApplicationController\n  def show\n    render plain: guarded_title\n  end\n\n  def plain\n    render plain: guarded_title\n  end\nend\n",
        "  test \"Outer::Guard's method reaches the controller\" do\n    get \"/stamped/1\"\n    assert_equal \"namespaced\", response.body\n  end\n",
    )
    .run_test("test/controllers/stamped_controller_test.rb")
    .assert_passes();
}

/// On a model too, `include Codes` written inside `module Signing` names
/// `Signing::Codes`.
#[test]
fn an_included_block_include_resolves_in_its_concerns_namespace_on_the_model() {
    emit_and_run::real_blog()
        .write(
            "app/models/concerns/signing.rb",
            "module Signing\n  extend ActiveSupport::Concern\n\n  included do\n    include Codes\n  end\nend\n",
        )
        .write(
            "app/models/concerns/signing/codes.rb",
            "module Signing::Codes\n  extend ActiveSupport::Concern\n\n  def shout\n    title.upcase\n  end\nend\n",
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n  include Signing\n",
        )
        .run_ruby(
            "a = Article.create!(title: \"Hi\", body: \"Body text here\")\nraise a.shout unless a.shout == \"HI\"",
        )
        .assert_passes();
}

/// `Guard`'s own `included do` include, `Deep`, reaches the controller too.
#[test]
fn an_include_two_included_blocks_deep_reaches_the_controller() {
    controller_concern_app(
        "module Outer\n  extend ActiveSupport::Concern\n\n  included do\n    include Guard\n  end\nend\n",
        &[
            (
                "app/controllers/concerns/guard.rb",
                "module Guard\n  extend ActiveSupport::Concern\n\n  included do\n    include Deep\n  end\nend\n",
            ),
            (
                "app/controllers/concerns/deep.rb",
                "module Deep\n  extend ActiveSupport::Concern\n\n  def deep_title\n    \"deep\"\n  end\nend\n",
            ),
        ],
        "class StampedController < ApplicationController\n  def show\n    render plain: deep_title\n  end\n\n  def plain\n    render plain: deep_title\n  end\nend\n",
        "  test \"Deep's method reaches the controller\" do\n    get \"/stamped/1\"\n    assert_equal \"deep\", response.body\n  end\n",
    )
    .run_test("test/controllers/stamped_controller_test.rb")
    .assert_passes();
}

/// Ruby's lookup order for a module included from `included do`: the
/// block runs on the includer after the outer module is appended, so
/// the inner module sits AHEAD of the outer one and its method wins.
#[test]
fn an_include_from_an_included_block_takes_precedence_over_its_concern() {
    emit_and_run::real_blog()
        .write(
            "app/models/concerns/signing.rb",
            "module Signing\n  extend ActiveSupport::Concern\n\n  included do\n    include Signing::Codes\n  end\n\n  def shout\n    \"outer\"\n  end\nend\n",
        )
        .write(
            "app/models/concerns/signing/codes.rb",
            "module Signing::Codes\n  extend ActiveSupport::Concern\n\n  def shout\n    \"inner\"\n  end\nend\n",
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n  include Signing\n",
        )
        .run_ruby(
            "a = Article.create!(title: \"Hi\", body: \"Body text here\")\nraise a.shout unless a.shout == \"inner\"",
        )
        .assert_passes();
}

/// `connected_to(role: :writing)` runs its block on the primary, the one
/// connection an emitted app has: the call is its block.
#[test]
fn connected_to_the_writing_role_runs_its_block() {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n  def self.retitle(id, title)\n    ::ActiveRecord::Base.connected_to(role: :writing) do\n      article = find(id)\n      article.update!(title: title)\n      article.title\n    end\n  end\n",
        )
        .run_ruby(
            "a = Article.create!(title: \"Hi\", body: \"Body text here\")\n\
             raise \"value\" unless Article.retitle(a.id, \"New\") == \"New\"\n\
             raise \"write\" unless Article.find(a.id).title == \"New\"",
        )
        .assert_passes();
}

/// A reading role refuses writes in Rails, and a shard is another
/// database; neither, nor a block whose `next` the call gives meaning,
/// is the plain block.
#[test]
fn connected_to_other_than_the_writing_role_stays_unsupported() {
    for call in [
        "ActiveRecord::Base.connected_to(role: :reading) do\n      count\n    end",
        "ActiveRecord::Base.connected_to(role: :writing, shard: :other) do\n      count\n    end",
        "ActiveRecord::Base.connected_to(role: :writing) do\n      next 0 if none?\n      count\n    end",
    ] {
        let errors = emit_and_run::real_blog()
            .edit(
                "app/models/article.rb",
                "class Article < ApplicationRecord\n",
                &format!("class Article < ApplicationRecord\n  def self.tally\n    {call}\n  end\n"),
            )
            .check_errors();
        assert!(errors.iter().any(|e| e.contains("connected_to")), "{call}: {errors:?}");
    }
}

/// `def`s in a concern's `included do` are includer methods, instance and
/// class side: they keep their visibility, and one over a column reader
/// wins and reads the column through `super`.
#[test]
fn defs_in_an_included_block_run_on_the_includer() {
    emit_and_run::real_blog()
        .write(
            "app/models/concerns/shouty.rb",
            "module Shouty\n  extend ActiveSupport::Concern\n\n  included do\n    class << self\n      def loud\n        \"L\"\n      end\n    end\n\n    def self.quiet\n      \"q\"\n    end\n\n    def shout(times = 1)\n      title.upcase * times\n    end\n\n    def body\n      super + \"!\"\n    end\n\n    def peek\n      secret\n    end\n\n    private\n\n    def secret\n      \"hidden\"\n    end\n  end\nend\n",
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n  include Shouty\n",
        )
        .run_ruby(
            "a = Article.create!(title: \"Hi\", body: \"Body text here\")\n\
             raise a.shout(2) unless a.shout(2) == \"HIHI\"\n\
             raise a.body unless a.body == \"Body text here!\"\n\
             raise a.peek unless a.peek == \"hidden\"\n\
             raise \"secret is public\" if a.respond_to?(:secret)\n\
             raise \"class side\" unless Article.loud + Article.quiet == \"Lq\"",
        )
        .assert_passes();
}

/// Where hoisting would change which definition wins, the def keeps its
/// nested-def refusal: the module defines the name too, the block
/// includes a module that sits ahead of the concern, or a `class << self`
/// holds a statement the hoist would drop.
#[test]
fn included_block_defs_that_would_lose_their_precedence_stay_refused() {
    for concern in [
        "module Shouty\n  extend ActiveSupport::Concern\n  included do\n    def shout\n      \"block\"\n    end\n  end\n\n  def shout\n    \"module\"\n  end\nend\n",
        "module Shouty\n  extend ActiveSupport::Concern\n  included do\n    include Comparable\n    def shout\n      \"block\"\n    end\n  end\nend\n",
        "module Shouty\n  extend ActiveSupport::Concern\n  included do\n    class << self\n      %i[a b].each { |n| define_method(n) { yield } }\n      def shout\n        \"block\"\n      end\n    end\n  end\nend\n",
        "module Shouty\n  extend ActiveSupport::Concern\n  included do\n    class << self\n      private\n      %i[a b].each { |n| define_method(n) { n } }\n      def shout\n        \"block\"\n      end\n    end\n  end\nend\n",
        "module Shouty\n  extend ActiveSupport::Concern\n  class_methods do\n    def loud\n      \"carrier\"\n    end\n  end\n  included do\n    def self.loud\n      \"block\"\n    end\n  end\nend\n",
        "module Shouty\n  extend ActiveSupport::Concern\n  module ClassMethods\n    def loud\n      \"carrier\"\n    end\n  end\n  included do\n    class << self\n      def loud\n        \"block\"\n      end\n    end\n  end\nend\n",
    ] {
        let errors = emit_and_run::real_blog()
            .write("app/models/concerns/shouty.rb", concern)
            .edit("app/models/article.rb", "class Article < ApplicationRecord\n", "class Article < ApplicationRecord\n  include Shouty\n")
            .check_errors();
        assert!(errors.iter().any(|e| e.contains("nested def")), "{concern}: {errors:?}");
    }
}

/// The inner concern's own `included do` runs on the includer too: its
/// scope is declared on the model.
#[test]
fn an_include_from_an_included_block_brings_its_own_included_items() {
    emit_and_run::real_blog()
        .write(
            "app/models/concerns/signing.rb",
            "module Signing\n  extend ActiveSupport::Concern\n\n  included do\n    include Signing::Codes\n  end\nend\n",
        )
        .write(
            "app/models/concerns/signing/codes.rb",
            "module Signing::Codes\n  extend ActiveSupport::Concern\n\n  included do\n    scope :titled, ->(title) { where(title: title) }\n  end\nend\n",
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n  include Signing\n",
        )
        .run_ruby(
            "Article.create!(title: \"Hi\", body: \"Body text here\")\nraise \"scope missing\" unless Article.titled(\"Hi\").count == 1",
        )
        .assert_passes();
}

#[test]
fn a_concerns_unclaimed_attribute_declaration_runs_on_each_includer() {
    emit_and_run::real_blog()
        .write(
            "app/models/concerns/reviewable.rb",
            "module Reviewable\n  extend ActiveSupport::Concern\n  included do\n    attribute :review_note, :string\n  end\nend\n",
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n  include Reviewable\n",
        )
        .edit(
            "app/models/comment.rb",
            "class Comment < ApplicationRecord\n",
            "class Comment < ApplicationRecord\n  include Reviewable\n",
        )
        .run_ruby(
            "article = Article.new\ncomment = Comment.new\narticle.review_note = \"article note\"\ncomment.review_note = \"comment note\"\nraise \"article attribute lost\" unless article.review_note == \"article note\"\nraise \"comment attribute lost\" unless comment.review_note == \"comment note\"\n",
        )
        .assert_passes();
}

#[test]
fn extension_rbi_methods_run_on_the_native_infrastructure_gem() {
    let lock = roundhouse::gems::Lockfile::parse(
        &std::fs::read_to_string(roundhouse::fixtures::real_blog().join("Gemfile.lock")).unwrap(),
    );
    let version = lock.version_of("rake").expect("the Rails fixture resolves rake");
    let rbi_path = format!("sorbet/rbi/gems/rake@{version}.rbi");
    emit_and_run::real_blog()
        .write("roundhouse.yml", "extensions: [config/rh]\n")
        .write("config/rh/gems.yml", "lock_gated_requires:\n  - {require: rake, gem: rake}\n")
        .write(
            &rbi_path,
            "module Rake\n  class Task\n    sig { params(task_name: String).returns(T::Boolean) }\n    def self.task_defined?(task_name); end\n  end\nend\n",
        )
        .write(
            "app/models/rake_probe.rb",
            "class RakeProbe\n  def self.registered?\n    Rake::Task.task_defined?(\"roundhouse_extension_probe_task\")\n  end\nend\n",
        )
        .run_ruby(
            "raise \"unexpected native task\" if RakeProbe.registered?\nRake::Task.define_task(\"roundhouse_extension_probe_task\")\nraise \"the RBI did not reach the native gem\" unless RakeProbe.registered?\n",
        )
        .assert_passes();
}

#[test]
fn extension_rbi_patches_preserve_runtime_constant_ownership() {
    let lock = std::fs::read_to_string(
        roundhouse::fixtures::real_blog().join("Gemfile.lock"),
    ).unwrap().replacen("  specs:\n", "  specs:\n    widget-engine (1.0)\n", 1);
    emit_and_run::real_blog()
        .write("roundhouse.yml", "extensions: [config/rh]\n")
        .write("config/rh/gems.yml", "rails_dependent:\n  - {gem: widget-engine, why: requires the Rails framework}\n")
        .write("Gemfile.lock", &lock)
        .write(
            "sorbet/rbi/gems/widget-engine@1.0.rbi",
            "class ActiveRecord::Base\n  def gem_patch; end\nend\n",
        )
        .write(
            "app/models/runtime_probe.rb",
            "class RuntimeProbe\n  def self.base\n    ActiveRecord::Base\n  end\nend\n",
        )
        .run_ruby(
            "raise \"runtime class ownership changed\" unless RuntimeProbe.base == ActiveRecord::Base\n",
        )
        .assert_passes();
}

/// An action whose whole body is a call to a rendering helper defined
/// on a PARENT controller. The default response appended to the action
/// must be guarded by `performed?`, as it is for a helper on the
/// action's own controller, or it overwrites what the helper rendered.
#[test]
fn an_action_responding_through_an_inherited_helper_keeps_its_response() {
    emit_and_run::real_blog()
        .write(
            "app/controllers/base_reports_controller.rb",
            "class BaseReportsController < ApplicationController\n  private\n\n  def render_title(article)\n    render json: {title: article.title}\n  end\nend\n",
        )
        .write(
            "app/controllers/reports_controller.rb",
            "class ReportsController < BaseReportsController\n  def show\n    render_title(Article.find(params[:id]))\n  end\nend\n",
        )
        .edit(
            "config/routes.rb",
            "  resources :articles do",
            "  get \"/reports/:id\", to: \"reports#show\"\n  resources :articles do",
        )
        .write(
            "test/controllers/reports_controller_test.rb",
            "require \"test_helper\"\n\nclass ReportsControllerTest < ActionDispatch::IntegrationTest\n  test \"a subclass action responds through the base controller's helper\" do\n    article = Article.create!(title: \"Quarterly\", body: \"Body text here\")\n    get \"/reports/#{article.id}\"\n    assert_response :success\n    assert_equal \"Quarterly\", JSON.parse(response.body)[\"title\"]\n  end\nend\n",
        )
        .run_test("test/controllers/reports_controller_test.rb")
        .assert_passes();
}

/// The same, one call further: the action calls an inherited helper
/// that delegates to the inherited helper that renders.
#[test]
fn an_action_responding_through_a_delegating_inherited_helper_keeps_its_response() {
    emit_and_run::real_blog()
        .write(
            "app/controllers/base_reports_controller.rb",
            "class BaseReportsController < ApplicationController\n  private\n\n  def report(article)\n    render_title(article)\n  end\n\n  def render_title(article)\n    render json: {title: article.title}\n  end\nend\n",
        )
        .write(
            "app/controllers/reports_controller.rb",
            "class ReportsController < BaseReportsController\n  def show\n    report(Article.find(params[:id]))\n  end\nend\n",
        )
        .edit(
            "config/routes.rb",
            "  resources :articles do",
            "  get \"/reports/:id\", to: \"reports#show\"\n  resources :articles do",
        )
        .write(
            "test/controllers/reports_controller_test.rb",
            "require \"test_helper\"\n\nclass ReportsControllerTest < ActionDispatch::IntegrationTest\n  test \"a subclass action responds through a delegating helper\" do\n    article = Article.create!(title: \"Quarterly\", body: \"Body text here\")\n    get \"/reports/#{article.id}\"\n    assert_response :success\n    assert_equal \"Quarterly\", JSON.parse(response.body)[\"title\"]\n  end\nend\n",
        )
        .run_test("test/controllers/reports_controller_test.rb")
        .assert_passes();
}

/// The same when the nearest helper is an override that reaches the
/// rendering one through `super`.
#[test]
fn an_action_responding_through_an_override_calling_super_keeps_its_response() {
    emit_and_run::real_blog()
        .write(
            "app/controllers/base_reports_controller.rb",
            "class BaseReportsController < ApplicationController\n  private\n\n  def render_title(article)\n    render json: {title: article.title}\n  end\nend\n",
        )
        .write(
            "app/controllers/reports_controller.rb",
            "class ReportsController < BaseReportsController\n  def show\n    render_title(Article.find(params[:id]))\n  end\n\n  private\n\n  def render_title(article)\n    super\n  end\nend\n",
        )
        .edit(
            "config/routes.rb",
            "  resources :articles do",
            "  get \"/reports/:id\", to: \"reports#show\"\n  resources :articles do",
        )
        .write(
            "test/controllers/reports_controller_test.rb",
            "require \"test_helper\"\n\nclass ReportsControllerTest < ActionDispatch::IntegrationTest\n  test \"a subclass action responds through super\" do\n    article = Article.create!(title: \"Quarterly\", body: \"Body text here\")\n    get \"/reports/#{article.id}\"\n    assert_response :success\n    assert_equal \"Quarterly\", JSON.parse(response.body)[\"title\"]\n  end\nend\n",
        )
        .run_test("test/controllers/reports_controller_test.rb")
        .assert_passes();
}

/// `render_code(size: 2, **opts)` into `def render_code(size:, color:
/// "black")`: a keyword bundle splatted AFTER a literal keyword, in a
/// receiverless call. Ingest desugars it to `{ size: 2 }.merge(opts)`;
/// `kwsplat` recovers the keywords, with the literal as the default
/// the bundle is read against — Ruby lets the later `**` win.
#[test]
fn a_keyword_bundle_after_a_literal_keyword_reaches_a_keyword_callee() {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n  def code_svg(**opts)\n    render_code(size: 2, **opts)\n  end\n\n  def render_code(size:, color: \"black\")\n    \"#{title}:#{size}:#{color}\"\n  end\n",
        )
        .run_ruby(
            "a = Article.create!(title: \"Hi\", body: \"Body text here\")\nraise a.code_svg(color: \"red\") unless a.code_svg(color: \"red\") == \"Hi:2:red\"\nraise a.code_svg unless a.code_svg == \"Hi:2:black\"\nraise a.code_svg(size: 9) unless a.code_svg(size: 9) == \"Hi:9:black\"",
        )
        .assert_passes();
}

/// The same call where the callee comes from an included concern: the
/// receiverless send resolves through the model's includes.
#[test]
fn a_keyword_bundle_reaches_a_keyword_callee_from_an_included_concern() {
    emit_and_run::real_blog()
        .write(
            "app/models/concerns/coded.rb",
            "module Coded\n  extend ActiveSupport::Concern\n\n  def render_code(size:, color: \"black\")\n    \"#{size}:#{color}\"\n  end\nend\n",
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n  include Coded\n\n  def code_svg(**opts)\n    render_code(size: 2, **opts)\n  end\n",
        )
        .run_ruby(
            "a = Article.create!(title: \"Hi\", body: \"Body text here\")\nraise a.code_svg(color: \"red\") unless a.code_svg(color: \"red\") == \"2:red\"\nraise a.code_svg unless a.code_svg == \"2:black\"",
        )
        .assert_passes();
}

/// real-blog's Article with two token purposes declared from a concern's
/// `included do`: `:share` (a day's expiry, the title as its value) and
/// `:plain` (the id alone, never expiring).
fn article_with_shareable_tokens() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write(
            "app/models/concerns/shareable.rb",
            "module Shareable\n  extend ActiveSupport::Concern\n\n  included do\n    generates_token_for :share, expires_in: 1.day do\n      title\n    end\n    generates_token_for :plain\n  end\nend\n",
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n  include Shareable\n",
        )
}

/// `generates_token_for :purpose, expires_in: D do <value> end` (Rails
/// 7.1), synthesized over `ActiveRecord::TokenFor`: a token finds its
/// record, stops verifying when the block's value changes (Rails'
/// contract), rejects tampering and a wrong purpose, the bang form
/// raises InvalidSignature as Rails' does, and the declaration is
/// honored from a concern's `included do` too.
#[test]
fn a_generated_token_finds_its_record_until_its_value_changes() {
    article_with_shareable_tokens()
        .run_ruby(
            "a = Article.create!(title: \"Hello\", body: \"Body text here\")\ntoken = a.generate_token_for(:share)\nraise \"find\" unless Article.find_by_token_for(:share, token)&.id == a.id\nraise \"bang\" unless Article.find_by_token_for!(:share, token).id == a.id\nraise \"purpose\" unless Article.find_by_token_for(:plain, token).nil?\nraise \"tamper\" unless Article.find_by_token_for(:share, token + \"x\").nil?\nplain = a.generate_token_for(:plain)\nraise \"plain\" unless Article.find_by_token_for(:plain, plain)&.id == a.id\na.update!(title: \"Changed\")\nraise \"stale\" unless Article.find_by_token_for(:share, token).nil?\nraise \"plain survives\" unless Article.find_by_token_for(:plain, plain)&.id == a.id\nbegin\n  Article.find_by_token_for!(:share, token)\n  raise \"no raise\"\nrescue ActiveSupport::MessageVerifier::InvalidSignature\nend\nputs \"PASS token_for\"",
        )
        .assert_passes();
}

/// A block value goes into the payload as the JSON Rails' `as_json`
/// writes for its type: an Integer is a number (`[1,1]`) and a boolean
/// is `true`/`false` (`[1,false]`), not a quoted string. Both tokens
/// were minted by Rails 8.1.4 (`SECRET_KEY_BASE=test-secret`) and are
/// unexpiring, so the emitted app must mint the same bytes and accept
/// Rails' own.
#[test]
fn a_non_string_token_value_is_the_json_rails_writes() {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n  generates_token_for :counted do\n    id\n  end\n  generates_token_for :flagged do\n    title.nil?\n  end\n",
        )
        .run_ruby(
            r#"
Rails.secret_key_base = "test-secret"
counted = "eyJfcmFpbHMiOnsiZGF0YSI6WzEsMV0sInB1ciI6IkFydGljbGVcbmNvdW50ZWRcbiJ9fQ==--d02f8c1104bf97b778253f734d7156c48eb98e88"
flagged = "eyJfcmFpbHMiOnsiZGF0YSI6WzEsZmFsc2VdLCJwdXIiOiJBcnRpY2xlXG5mbGFnZ2VkXG4ifX0=--fb27c1ecd775cb15bc0966020341a5c07c7ab57e"
a = Article.create!(title: "Hello", body: "Body text here")
raise "expected the first row" unless a.id == 1
raise "integer value differs from rails" unless a.generate_token_for(:counted) == counted
raise "boolean value differs from rails" unless a.generate_token_for(:flagged) == flagged
raise "rails integer token rejected" unless Article.find_by_token_for(:counted, counted)&.id == 1
raise "rails boolean token rejected" unless Article.find_by_token_for(:flagged, flagged)&.id == 1
puts "PASS typed values"
"#,
        )
        .assert_passes();
}

/// A multi-statement block body must still synthesize: the typed
/// payload writers parenthesize the emitted expression so a Seq (or a
/// modifier-`if`) is legal as a call argument. Without that, prism
/// rejects the synthesized source, methods stay typed but undefined
/// (invariant 6), and `generate_token_for` raises NoMethodError.
#[test]
fn a_multi_statement_token_block_still_synthesizes() {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n  generates_token_for :share do\n    t = title\n    t\n  end\n",
        )
        .run_ruby(
            "a = Article.create!(title: \"Hello\", body: \"Body text here\")\ntoken = a.generate_token_for(:share)\nraise \"find\" unless Article.find_by_token_for(:share, token)&.id == a.id\na.update!(title: \"Changed\")\nraise \"stale\" unless Article.find_by_token_for(:share, token).nil?\nputs \"PASS multi-statement token block\"",
        )
        .assert_passes();
}

/// Declaring a purpose twice keeps the last declaration, as Rails'
/// `token_definitions.merge` does: the token carries the second block's
/// value, so changing the first block's value leaves it valid.
#[test]
fn a_redeclared_token_purpose_uses_the_last_declaration() {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n  generates_token_for :share do\n    title\n  end\n  generates_token_for :share, expires_in: 1.hour do\n    body\n  end\n",
        )
        .run_ruby(
            "a = Article.create!(title: \"Hello\", body: \"Body text here\")\ntoken = a.generate_token_for(:share)\na.update!(title: \"Changed\")\nraise \"first declaration used\" unless Article.find_by_token_for(:share, token)&.id == a.id\na.update!(body: \"Other body text\")\nraise \"last declaration ignored\" unless Article.find_by_token_for(:share, token).nil?\nputs \"PASS redeclared\"",
        )
        .assert_passes();
}

/// The tokens are Rails' own: ones minted by Rails 8.1.4 for the same
/// declarations verify here, with `SECRET_KEY_BASE=test-secret` and the
/// clock at 2100-01-01 so the day's expiry is still ahead:
///
///   {"_rails":{"data":[1,"Hello"],"exp":"2100-01-02T00:00:00.000Z",
///    "pur":"Article\nshare\n86400"}}
///   {"_rails":{"data":[1],"pur":"Article\nplain\n"}}
///
/// and a token minted here is the bytes Rails mints for the same record
/// at the same instant, since the purpose, payload and absent `exp` are
/// all Rails' choices.
#[test]
fn a_generated_token_minted_by_rails_verifies() {
    article_with_shareable_tokens()
        .run_ruby(
            r#"
Rails.secret_key_base = "test-secret"
share = "eyJfcmFpbHMiOnsiZGF0YSI6WzEsIkhlbGxvIl0sImV4cCI6IjIxMDAtMDEtMDJUMDA6MDA6MDAuMDAwWiIsInB1ciI6IkFydGljbGVcbnNoYXJlXG44NjQwMCJ9fQ==--b944fbcc044254e17f4d952e9a1c1e63ec185093"
plain = "eyJfcmFpbHMiOnsiZGF0YSI6WzFdLCJwdXIiOiJBcnRpY2xlXG5wbGFpblxuIn19--5a8e42fe42d686af11a02e972f7e5e1c88bb57db"
a = Article.create!(title: "Hello", body: "Body text here")
raise "expected the first row" unless a.id == 1
raise "rails share token rejected" unless Article.find_by_token_for(:share, share)&.id == 1
raise "rails plain token rejected" unless Article.find_by_token_for!(:plain, plain).id == 1
raise "unexpiring token differs from rails" unless a.generate_token_for(:plain) == plain
a.update!(title: "Changed")
raise "stale rails token accepted" unless Article.find_by_token_for(:share, share).nil?
puts "PASS rails tokens"
"#,
        )
        .assert_passes();
}

/// Integer serialization is not blindly String#to_i: nonnumeric labels
/// must not alias an existing row zero. Invalid IDs still count toward the
/// array finder's required cardinality, except when pagination excludes them.
#[test]
fn relation_find_rejects_nonnumeric_ids_without_aliasing_zero() {
    emit_and_run::real_blog()
        .run_ruby(r#"
Db.exec("INSERT INTO articles (id, title, body, created_at, updated_at) VALUES (0, 'zero', 'long enough body', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)")
Db.exec("INSERT INTO articles (id, title, body, created_at, updated_at) VALUES (31, 'thirty-one', 'long enough body', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)")
rel = ActiveRecord::Relation.new(Article).where("id IN (0, 31)")
prior_sql = rel.to_sql
rel.to_a
raise "valid zero" unless rel.find("0").id == 0
raise "zero prefix" unless rel.find("0x").id == 0
raise "numeric prefix" unless rel.find("31-sarah").id == 31
raise "whitespace/sign" unless rel.find(" \t+31-slug").id == 31
["bogus", "", " ", "+", "-", "٠"].each do |id|
  [id, [id], [id, 31]].each do |input|
    begin
      rel.find(input)
      raise "invalid id aliased a record: #{input.inspect}"
    rescue ActiveRecord::RecordNotFound
    end
  end
end
raise "scope/cache poisoned" unless rel.to_sql == prior_sql && rel.to_a.map(&:id) == [0, 31]
unordered = ActiveRecord::Relation.new(Article).offset(1)
raise "excluded invalid input" unless unordered.find(["bogus", 31]).map(&:id) == [31]
ordered = ActiveRecord::Relation.new(Article).order(:id).limit(1)
raise "ordered limit cardinality" unless ordered.find(["bogus", 31]).map(&:id) == [31]
ordered = ordered.offset(1)
begin
  ordered.find(["bogus", 31])
  raise "invalid input dropped from expected size"
rescue ActiveRecord::RecordNotFound
end
puts "ok"
"#)
        .assert_passes();
}

/// A custom String primary key must not take the default Integer cast.
#[test]
fn relation_find_with_string_keys_runs() {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "create_table \"articles\", force: :cascade do |t|",
            "create_table \"bookmarks\", id: { type: :string, limit: 32 }, primary_key: \"code\", force: :cascade do |t|\n    t.string \"title\", null: false\n    t.integer \"visits\", default: 0\n    t.datetime \"retired_at\"\n    t.index [\"title\"], name: \"index_bookmarks_on_live_title\", unique: true, where: \"(retired_at IS NULL)\"\n  end\n\n  create_table \"articles\", force: :cascade do |t|",
        )
        .write(
            "app/models/bookmark.rb",
            r#"class Bookmark < ApplicationRecord
  self.primary_key = "code"

  def self.find_titles
    where("title != 'outside'").find(["2-apples", "z-9"]).map(&:title).join("|")
  end
end
"#,
        )
        .run_ruby(
            r#"Bookmark.create!(code: "z-9", title: "zebra")
Bookmark.create!(code: "2-apples", title: "apple")
raise "string array cast/order" unless Bookmark.find_titles == "apple|zebra"
rel = ActiveRecord::Relation.new(Bookmark)
raise "string scalar cast" unless rel.find("2-apples").title == "apple"
raise "custom key" unless rel.find(["z-9"]).map(&:id) == ["z-9"]
# The same model must retain both synthesized methods after merging:
# String-key finders and partial-index upsert targets are independent.
Bookmark.upsert_all([{ code: "2-apples", title: "apple", visits: 17 }], unique_by: :title)
raise "partial upsert did not update" unless rel.find("2-apples").visits == 17
raise "partial upsert duplicated the live row" unless Bookmark.where(title: "apple").count == 1
Bookmark.where(code: "2-apples").update_all(retired_at: Time.now)
Bookmark.upsert_all([{ code: "fresh-41", title: "apple", visits: 29 }], unique_by: :title)
raise "partial upsert did not insert past retired row" unless Bookmark.where(title: "apple").count == 2
raise "string keys or partial index lost" unless rel.find(["fresh-41", "2-apples"]).map(&:visits) == [29, 17]
puts "ok"
"#,
        )
        .assert_passes();
}

/// Ingest hoists a file-level constant into the first class of the
/// file, but Ruby declares it on Object. The inferred value must belong
/// to the declaration that the read resolves to.
#[test]
fn a_file_level_constant_runs_from_the_class_below_it() {
    emit_and_run::real_blog()
        .write(
            "app/services/review_probe.rb",
            "ROOT_LIMIT = 7\n\nclass LimitReader\n  def self.value\n    ROOT_LIMIT\n  end\nend\n",
        )
        .run_ruby("raise 'file-level constant' unless LimitReader.value == 7")
        .assert_passes();
}

/// ERB views are not indexed. A qualified class read there must not
/// take the value of an unrelated constant with the same last segment,
/// and a qualified value read takes the value declared at its full name.
#[test]
fn a_qualified_class_in_a_view_ignores_a_same_named_value() {
    emit_and_run::real_blog()
        .write(
            "app/services/archive.rb",
            "module Marker\n  Item = 1\nend\n\nmodule Archive\n  class Item\n    def self.label\n      \"archive\"\n    end\n  end\nend\n",
        )
        .edit(
            "app/views/articles/index.html.erb",
            "<% content_for :title, \"Articles\" %>",
            "<% content_for :title, \"Articles\" %>\n<p id=\"archive-label\"><%= Archive::Item.label %></p>\n<p id=\"marker-item\"><%= Marker::Item + 1 %></p>",
        )
        .edit(
            "test/controllers/articles_controller_test.rb",
            "    assert_select \"h1\", \"Articles\"\n",
            "    assert_select \"h1\", \"Articles\"\n    assert_select \"#archive-label\", \"archive\"\n    assert_select \"#marker-item\", \"2\"\n",
        )
        .run_test("test/controllers/articles_controller_test.rb")
        .assert_passes();
}

/// Each constant in the chain reads the one before it. The value must
/// reach the end of a chain longer than any fixed number of rounds.
#[test]
fn a_long_constant_chain_reaches_its_value() {
    emit_and_run::real_blog()
        .write(
            "app/services/chain.rb",
            "class Chain\n  A = 1\n  B = A\n  C = B\n  D = C\n  E = D\n  F = E\n\n  def self.value\n    F\n  end\nend\n",
        )
        .run_ruby("raise 'constant chain' unless Chain.value == 1")
        .assert_passes();
}

/// Rubydex declares Object, BasicObject, Kernel, Module and Class
/// itself. Those are Ruby's own classes, not unknown constants.
#[test]
fn object_new_runs_as_the_ruby_built_in() {
    emit_and_run::real_blog()
        .write(
            "app/services/object_reader.rb",
            "class ObjectReader\n  def self.value\n    Object.new\n  end\nend\n",
        )
        .write(
            "app/controllers/sentinels_controller.rb",
            "class SentinelsController < ApplicationController\n  def show\n    @sentinel = Object.new\n  end\nend\n",
        )
        .run_ruby("raise 'Object.new' unless ObjectReader.value.instance_of?(Object)")
        .assert_passes();
}

/// Writebook's `pluralize number_with_delimiter(content.split.size), "word"`:
/// keep the formatted label, and recognize a textual one as singular.
#[test]
fn a_formatted_word_count_runs() {
    emit_and_run::real_blog()
        .write(
            "app/helpers/word_counts_helper.rb",
            r#"module WordCountsHelper
  def word_count(content)
    pluralize number_with_delimiter(content.split.size), "word"
  end

  def label(count)
    pluralize count, "word"
  end
end
"#,
        )
        .write(
            "app/views/articles/_word_count.html.erb",
            "<%= pluralize number_with_delimiter(1001), \"word\" %>",
        )
        .run_ruby(
            r#"raise "singular" unless WordCountsHelper.word_count("only") == "1 word"
raise "plural" unless WordCountsHelper.word_count("two words") == "2 words"
raise "delimiter lost" unless WordCountsHelper.word_count((["w"] * 1001).join(" ")) == "1,001 words"
raise "zero" unless WordCountsHelper.word_count("") == "0 words"
raise "text one" unless WordCountsHelper.label("1") == "1 word"
raise "decimal one" unless WordCountsHelper.label("1.00") == "1.00 word"
raise "leading zero" unless WordCountsHelper.label("01") == "01 words"
raise "fraction" unless WordCountsHelper.label("1.01") == "1.01 words"
raise "empty string" unless WordCountsHelper.label("") == " words"
raise "float one" unless WordCountsHelper.label(1.0) == "1.0 word"
raise "view" unless Views::Articles.word_count(nil) == "1,001 words"
puts "ok"
"#,
        )
        .assert_passes();
}

#[test]
fn an_app_pluralize_override_owns_helper_and_view_calls() {
    emit_and_run::real_blog()
        .write(
            "app/helpers/application_helper.rb",
            r#"module ApplicationHelper
  def pluralize(count, word)
    "custom #{count}:#{word}"
  end

  def heading
    pluralize(1, "person")
  end
end
"#,
        )
        .write(
            "app/views/articles/_custom_count.html.erb",
            "<%= pluralize(1, \"person\") %>|<%= \"nested #{pluralize(2, 'person')}\" %>",
        )
        .run_ruby(
            r#"raise "helper override lost" unless ApplicationHelper.heading == "custom 1:person"
raise "view override lost" unless Views::Articles.custom_count(nil) == "custom 1:person|nested custom 2:person"
puts "ok"
"#,
        )
        .assert_passes();
}

#[test]
fn an_app_tag_helper_override_owns_helper_and_view_calls() {
    emit_and_run::real_blog()
        .write(
            "lib/tag_defaults.rb",
            r#"module TagDefaults
  def content_tag(name, text)
    "default tag"
  end

  def safe_join(parts, separator)
    separator
  end
end
"#,
        )
        .write(
            "app/helpers/application_helper.rb",
            r#"module ApplicationHelper
  include TagDefaults

  def content_tag(name, text)
    37
  end

  def override_value
    content_tag(:span, "ignored") + 3
  end
end
"#,
        )
        .write(
            "app/views/articles/_custom_tag.html.erb",
            "<%= content_tag(:span, \"ignored\").div(2) %>|<%= \"nested #{content_tag(:i, 'ignored') + 5}\" %>|<%= override_value %>|<%= safe_join([\"a\", \"b\"], 11).div(2) %>",
        )
        .run_ruby(
            r#"raise "helper override lost" unless ApplicationHelper.override_value == 40
raise "view override lost" unless Views::Articles.custom_tag(nil) == "18|nested 42|40|5"
"#,
        )
        .assert_passes();
}

#[test]
fn promoted_module_helpers_and_controller_helpers_keep_their_view_dispatch() {
    emit_and_run::real_blog()
        .write(
            "app/helpers/bare_helper.rb",
            "module BareHelper\n  module_function\n  def bare_value\n    17\n  end\nend\n",
        )
        .write(
            "app/helpers/named_helper.rb",
            "module NamedHelper\n  def named_value\n    19\n  end\n  module_function :named_value\nend\n",
        )
        .write(
            "app/helpers/extended_helper.rb",
            "module ExtendedHelper\n  extend self\n  def extended_value\n    23\n  end\nend\n",
        )
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\n  helper_method :content_tag\n  def content_tag(name, text)\n    37\n  end\nend\n",
        )
        .write(
            "app/views/articles/_helper_channels.html.erb",
            "<%= bare_value.div(2) %>|<%= named_value.div(2) %>|<%= extended_value.div(2) %>|<%= content_tag(:span, \"ignored\").div(2) %>",
        )
        .run_ruby(
            "raise \"helper dispatch lost\" unless Views::Articles.helper_channels(nil) == \"8|9|11|18\"\n",
        )
        .assert_passes();
}

/// #139 typed `Model.human_attribute_name` as a String, but no runtime
/// defined it, so every page rendering the form raised `undefined method
/// 'human_attribute_name' for class Article` (#147). A literal attribute
/// now folds to the locale's name at compile time.
#[test]
fn human_attribute_name_runs() {
    emit_and_run::real_blog()
        .edit(
            "app/views/articles/_form.html.erb",
            "<%= form.label :title %>",
            "<%= form.label :title %><%= Article.human_attribute_name(:title) %>",
        )
        .run_test("test/controllers/articles_controller_test.rb")
        .assert_passes();
}

/// #140 bound `form_with builder: X`'s block param to `X`, which took a
/// custom builder's own helpers from errors to clean. But the emitted
/// tree cannot load `X` (no runtime `ActionView::Helpers::FormBuilder`
/// to subclass), and the view calls `form.marker_field` on a `form`
/// that no longer exists, because lowering expands the stock builder
/// inline. Passing needs a builder the emitted view can call; until
/// then, the honest state is an error in `check`.
#[test]
#[ignore = "check is clean but the emitted tree fails to load: no runtime FormBuilder, and the inlined form has no builder object (#148)"]
fn a_custom_form_builder_runs() {
    emit_and_run::real_blog()
        .write(
            "app/helpers/custom_form_builder.rb",
            "class CustomFormBuilder < ActionView::Helpers::FormBuilder\n  \
               def marker_field(name)\n    \
                 @template.content_tag(:span, name.to_s, class: \"builder-marker\")\n  \
               end\n\
             end\n",
        )
        .edit(
            "app/views/articles/_form.html.erb",
            "form_with(model: article, class: \"contents\")",
            "form_with(model: article, class: \"contents\", builder: CustomFormBuilder)",
        )
        .edit(
            "app/views/articles/_form.html.erb",
            "<%= form.label :title %>",
            "<%= form.label :title %><%= form.marker_field :title %>",
        )
        .run_test("test/controllers/articles_controller_test.rb")
        .assert_passes();
}

/// `enum :x, CONST.map { |v| [v, v.to_s] }.to_h` — Procore's
/// `bid_package.rb` computes an identity string mapping over a
/// constant instead of writing the hash literal out. Pins that the
/// generated predicate and bang-writer methods actually work against a
/// real column, not just that `check` accepts the declaration.
#[test]
fn computed_enum_map_to_h_runs() {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            // Anchored on the table, not its columns: Rails 8.1 dumps
            // columns alphabetically, older Rails in creation order.
            "create_table \"articles\", force: :cascade do |t|",
            "create_table \"articles\", force: :cascade do |t|\n    t.string \"kind\", default: \"post\", null: false",
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n\n  \
             KINDS = %i[post announcement]\n  \
             enum :kind, KINDS.map { |k| [k, k.to_s] }.to_h",
        )
        .write(
            "test/models/article_enum_test.rb",
            "require \"test_helper\"\n\n\
             class ArticleEnumTest < ActiveSupport::TestCase\n  \
               test \"a computed .map{}.to_h enum mapping generates working predicates\" do\n    \
                 article = articles(:one)\n    \
                 assert article.post?\n    \
                 article.announcement!\n    \
                 assert article.announcement?\n    \
                 assert_equal \"announcement\", article.kind\n  \
               end\n\
             end\n",
        )
        .run_test("test/models/article_enum_test.rb")
        .assert_passes();
}

/// A module constant in another file is the same string-backed enum
/// input as an inline array. Exercise the mapping AND generated methods,
/// including a scope that must exclude the record after a persisted write.
#[test]
fn cross_file_literal_enum_mapping_runs() {
    emit_and_run::real_blog()
        .write(
            "app/services/article_states.rb",
            "module ArticleStates\n  VALUES = %w[draft active archived].freeze\nend\n",
        )
        .edit(
            "db/schema.rb",
            "create_table \"articles\", force: :cascade do |t|",
            "create_table \"articles\", force: :cascade do |t|\n    t.string \"state\", default: \"draft\", null: false",
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n  enum :state, ArticleStates::VALUES.index_with(&:itself)",
        )
        .write(
            "test/models/article_cross_file_enum_test.rb",
            r#"require "test_helper"

class ArticleCrossFileEnumTest < ActiveSupport::TestCase
  test "a cross-file literal constant expands to a working string enum" do
    assert_equal({"draft" => "draft", "active" => "active", "archived" => "archived"}, Article.states)
    assert_equal "active", Article.states[:active]
    article = articles(:one)
    other = articles(:two)
    assert article.draft?
    assert_not article.active?
    article.active!
    assert_equal "active", article.reload.state
    assert article.active?
    assert_not article.draft?
    assert_equal article.id, Article.active.first.id
    assert_equal other.id, Article.draft.first.id
    assert_nil Article.archived.first
  end
end
"#,
        )
        .run_test("test/models/article_cross_file_enum_test.rb")
        .assert_passes();
}

/// `def self.included(klass); class << klass; … end; end` — Procore's
/// shared search concerns (`app/concerns/search_engine/indexed.rb` and
/// more) skip `ActiveSupport::Concern` and open the includer's
/// singleton directly from the vanilla `Module#included` hook. Pins
/// that the resulting class method is actually callable on the
/// including model, not just that `check` no longer reports the
/// `SingletonClassNode` it used to.
#[test]
fn included_hook_class_methods_run() {
    emit_and_run::real_blog()
        .write(
            "app/models/concerns/sluggable.rb",
            "module Sluggable\n  \
               def self.included(klass)\n    \
                 class << klass\n      \
                   def slug_prefix\n        \
                     \"article\"\n      \
                   end\n    \
                 end\n  \
               end\n\
             end\n",
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            "class Article < ApplicationRecord\n  include Sluggable\n\n  \
             has_many :comments, dependent: :destroy",
        )
        .write(
            "test/models/article_sluggable_test.rb",
            "require \"test_helper\"\n\n\
             class ArticleSluggableTest < ActiveSupport::TestCase\n  \
               test \"an included-hook class method is callable on the includer\" do\n    \
                 assert_equal \"article\", Article.slug_prefix\n  \
               end\n\
             end\n",
        )
        .run_test("test/models/article_sluggable_test.rb")
        .assert_passes();
}

/// `scope :x, (lambda do |v| … end)` — Procore's `reports/app/models/
/// report.rb` wraps the spelled-out `lambda`/`proc` scope body in its
/// own parens (`for_tools`, `for_data_sets`, `shared`). Before the fix
/// the parens sat between `parse_scope` and the call it expected, so
/// `check` reported "scope body must be a lambda" — this pins that the
/// emitted scope actually filters, not just that `check` goes quiet.
#[test]
fn parenthesized_lambda_scope_runs() {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "validates :body, presence: true, length: { minimum: 10 }",
            "validates :body, presence: true, length: { minimum: 10 }\n\n  \
             scope :with_title, (lambda do |value|\n    \
               where(title: value)\n  \
             end)",
        )
        .write(
            "test/models/article_scope_test.rb",
            "require \"test_helper\"\n\n\
             class ArticleScopeTest < ActiveSupport::TestCase\n  \
               test \"a parenthesized lambda scope filters by title\" do\n    \
                 found = Article.with_title(\"Getting Started with Rails\").first\n    \
                 assert_not_nil found\n    \
                 assert_equal \"Getting Started with Rails\", found.title\n    \
                 assert_nil Article.with_title(\"No Such Title\").first\n  \
               end\n\
             end\n",
        )
        .run_test("test/models/article_scope_test.rb")
        .assert_passes();
}

/// Invariant 6 pin for transitive filter-target ivar-write discovery
/// (`collect_transitive_filter_ivars` in `src/analyze/mod.rs`): a
/// `before_action` target whose body assigns no ivar itself but calls
/// another private method that does. `set_article` becomes `prepare`,
/// which calls `load_article`, which does the actual
/// `@article = Article.find(...)`, and the `show` view renders
/// `@article.title`.
///
/// `IvarUnresolved` is Error severity (`docs/pipeline/analyze.md`), so
/// this is a genuine before/after, not just an emit-side pin: verified
/// by hand against the pre-change analyzer (`git show
/// origin/main:src/analyze/mod.rs`), this exact edit made `check`
/// report eleven `@article has no known type` errors — `set_article`'s
/// replacement, `prepare`, no longer writes `@article` in its own
/// body, so without this change the write three lines away in
/// `load_article` was invisible. With the change `check` is clean AND
/// the emitted dispatcher actually SEQUENCES `prepare` (which calls
/// `load_article`) before the action body runs — the half of the claim
/// a diagnostic count alone can't make. A chain that silently dropped
/// the transitive write, or that emitted `prepare`'s body without
/// actually invoking `load_article`, would raise `NoMethodError` on
/// `nil.title` when the test hits `GET /articles/:id`.
#[test]
fn a_before_action_that_calls_another_private_method_runs() {
    emit_and_run::real_blog()
        .edit(
            "app/controllers/articles_controller.rb",
            "before_action :set_article, only: %i[ show edit update destroy ]",
            "before_action :prepare, only: %i[ show edit update destroy ]",
        )
        .edit(
            "app/controllers/articles_controller.rb",
            "    # Use callbacks to share common setup or constraints between actions.\n    def set_article\n      @article = Article.find(params.expect(:id))\n    end\n",
            "    # Use callbacks to share common setup or constraints between actions.\n    def prepare\n      load_article\n    end\n\n    def load_article\n      @article = Article.find(params.expect(:id))\n    end\n",
        )
        .run_test("test/controllers/articles_controller_test.rb")
        .assert_passes();
}

const INDEX_VIEW: &str = "app/views/articles/index.html.erb";
const INDEX_HEADING: &str = "<h1 class=\"font-bold text-4xl\">Articles</h1>";
const CONTROLLER_TEST: &str = "test/controllers/articles_controller_test.rb";
const INDEX_ASSERTION: &str = "assert_select \"h1\", \"Articles\"\n";

/// Render `calls` on the articles index, then assert `assertions` in
/// the index test. The expected HTML in each caller is Rails' output.
fn on_the_index(overlay: emit_and_run::Overlay, calls: &str, assertions: &str) -> emit_and_run::Run {
    overlay
        .edit(INDEX_VIEW, INDEX_HEADING, &format!("{INDEX_HEADING}\n{calls}"))
        .edit(CONTROLLER_TEST, INDEX_ASSERTION, &format!("{INDEX_ASSERTION}{assertions}"))
        .run_test(CONTROLLER_TEST)
}

/// A `tag.*` attribute whose value is a method call runs the call once,
/// as Rails does: campfire's layout passes `href: fresh_account_logo_path`,
/// which reads `Current.account` (a query) and formats a timestamp, and
/// the emit's nil guard spelled the value twice, so each tag ran it twice.
/// A value that answers nil still leaves the attribute out.
#[test]
fn a_tag_attribute_value_is_evaluated_once() {
    let run = on_the_index(
        emit_and_run::real_blog().write(
            "app/helpers/application_helper.rb",
            "module ApplicationHelper\n  \
               CALLS = []\n\n  \
               def counted_logo_path\n    \
                 CALLS << 1\n    \
                 \"/logo.png?v=1\"\n  \
               end\n\n  \
               def missing_logo_path\n    \
                 CALLS << 1\n    \
                 nil\n  \
               end\n\
             end\n",
        ),
        "<i id=\"once\"><%= tag.link rel: \"icon\", href: counted_logo_path %></i>\n\
         <i id=\"nil\"><%= tag.link rel: \"apple-touch-icon\", href: missing_logo_path %></i>\n",
        "    assert_match(/<i id=\"once\"><link rel=\"icon\" href=\"\\/logo.png\\?v=1\"><\\/i>/, response.body)\n    \
             assert_match(/<i id=\"nil\"><link rel=\"apple-touch-icon\"><\\/i>/, response.body)\n    \
             assert_equal 2, ApplicationHelper::CALLS.length\n",
    );
    run.assert_passes();
}

/// B1 in NEXUS_BUGS.md: a helper keyword named `class`, read with the
/// `class:` shorthand, emitted a bare `class` and compared it with the
/// String `"nil"`. Rails leaves the attribute out for `class: nil`.
#[test]
fn a_helper_reads_a_reserved_word_keyword_with_the_shorthand_runs() {
    let run = on_the_index(
        emit_and_run::real_blog().write(
            "app/helpers/application_helper.rb",
            "module ApplicationHelper\n  \
               def badge(text, class: \"badge\")\n    \
                 tag.span(text, class:)\n  \
               end\n\n  \
               def merged_badge(text, class: \"badge\", **options)\n    \
                 tag.span(text, **options.merge(class:))\n  \
               end\n\
             end\n",
        ),
        "<i id=\"b1-default\"><%= badge(\"hi\") %></i>\n\
         <i id=\"b1-given\"><%= badge(\"hi\", class: \"big\") %></i>\n\
         <i id=\"b1-nil\"><%= badge(\"hi\", class: nil) %></i>\n\
         <i id=\"b1-merged\"><%= merged_badge(\"hi\", class: \"big\", id: \"b\") %></i>\n",
        "    assert_match(/<i id=\"b1-default\"><span class=\"badge\">hi<\\/span><\\/i>/, response.body)\n    \
             assert_match(/<i id=\"b1-given\"><span class=\"big\">hi<\\/span><\\/i>/, response.body)\n    \
             assert_match(/<i id=\"b1-nil\"><span>hi<\\/span><\\/i>/, response.body)\n    \
             assert_match(/<i id=\"b1-merged\"><span id=\"b\" class=\"big\">hi<\\/span><\\/i>/, response.body)\n",
    );
    run.assert_passes();
}

/// A String keyword that no caller passes as nil keeps Rails' nil
/// rule: `class: "nil"` renders `class="nil"`. The inquiry lowering
/// read `value.nil?` as `StringInquirer#nil?` and emitted
/// `value == "nil"`, which dropped the attribute.
#[test]
fn a_string_keyword_named_nil_keeps_its_attribute() {
    let run = on_the_index(
        emit_and_run::real_blog().write(
            "app/helpers/application_helper.rb",
            "module ApplicationHelper\n  \
               def badge(text, class: \"badge\")\n    \
                 tag.span(text, class: binding.local_variable_get(:class))\n  \
               end\n\
             end\n",
        ),
        "<i id=\"n-literal\"><%= badge(\"hi\", class: \"nil\") %></i>\n\
         <i id=\"n-default\"><%= badge(\"hi\") %></i>\n",
        "    assert_match(/<i id=\"n-literal\"><span class=\"nil\">hi<\\/span><\\/i>/, response.body)\n    \
             assert_match(/<i id=\"n-default\"><span class=\"badge\">hi<\\/span><\\/i>/, response.body)\n",
    );
    run.assert_passes();
}

/// B2 in NEXUS_BUGS.md: strict locals named after reserved words
/// emitted a positional `for` parameter. Nexus reads them with
/// `local_assigns`; the repro reads them with `binding`.
#[test]
fn a_partial_with_reserved_word_strict_locals_runs() {
    let run = on_the_index(
        emit_and_run::real_blog()
            .write(
                "app/views/articles/_empty_la.html.erb",
                "<%# locals: (for:, class: \"\") %>\n\
                 <p id=\"b2-la\" class=\"<%= local_assigns[:class] %>\"><%= local_assigns[:for] %></p>\n",
            )
            .write(
                "app/views/articles/_empty_bind.html.erb",
                "<%# locals: (for:, class: \"\") %>\n\
                 <p id=\"b2-bind\" class=\"<%= binding.local_variable_get(:class) %>\"><%= binding.local_variable_get(:for) %></p>\n",
            ),
        "<%= render \"empty_la\", for: Article, class: \"muted\" %>\n\
         <%= render \"empty_la\", for: Article %>\n\
         <%= render \"empty_bind\", for: Article, class: \"muted\" %>\n",
        "    assert_match(/<p id=\"b2-la\" class=\"muted\">Article<\\/p>/, response.body)\n    \
             assert_match(/<p id=\"b2-la\" class=\"\">Article<\\/p>/, response.body)\n    \
             assert_match(/<p id=\"b2-bind\" class=\"muted\">Article<\\/p>/, response.body)\n",
    );
    run.assert_passes();
}

/// B3 in NEXUS_BUGS.md: `local_assigns[:class]` in a partial without
/// strict locals emitted a positional `class` parameter.
#[test]
fn a_partial_reading_a_reserved_word_local_assign_runs() {
    let run = on_the_index(
        emit_and_run::real_blog().write(
            "app/views/articles/_card.html.erb",
            "<div id=\"b3\" class=\"card <%= local_assigns[:class] %>\"><%= title %></div>\n",
        ),
        "<%= render \"card\", title: \"hi\", class: \"wide\" %>\n\
         <%= render \"card\", title: \"hi\" %>\n",
        "    assert_match(/<div id=\"b3\" class=\"card wide\">hi<\\/div>/, response.body)\n    \
             assert_match(/<div id=\"b3\" class=\"card \">hi<\\/div>/, response.body)\n",
    );
    run.assert_passes();
}

/// `Rails.application.routes.recognize_path` in a helper. `check` was
/// clean, and the emitted app raised NameError on `#routes`. Each
/// expected value is the output of Rails 7.2.4 for the same routes.
/// Rails returns Symbol keys with String values, puts `:format` last,
/// and raises `ActionController::RoutingError` when no route matches.
/// Rails takes a full URL, and normalizes any other path: one leading
/// slash, no doubled slashes and no trailing slash. The error message
/// shows the normalized path.
/// The `:method` of the second argument is the verb, a Symbol or a
/// String in any case, and GET is the default. A verb that Rails does
/// not accept raises `ActionController::UnknownHttpMethod`, which is not
/// a `RoutingError`. The helper rescues it as a `StandardError`: app
/// code that names the class gets a `check` error.
/// The emitted view does not escape the result of a helper call, and
/// Rails does. The `(&quot;|")` alternative in the message check only
/// tolerates that gap, which exists before this test.
#[test]
fn a_helper_recognizes_a_path_with_the_application_routes() {
    let run = on_the_index(
        emit_and_run::real_blog().write(
            "app/helpers/articles_helper.rb",
            r##"module ArticlesHelper
  def route_of(path, environment = {})
    recognized = Rails.application.routes.recognize_path(path, environment)
    recognized.map { |key, value| "#{key}=#{value}" }.join(" ")
  rescue ActionController::RoutingError => e
    "none: #{e.message}"
  rescue StandardError => e
    "#{e.class.name}: #{e.message}"
  end
end
"##,
        ),
        r#"<i id="rp-index"><%= route_of("/articles") %></i>
<i id="rp-show"><%= route_of("/articles/7?tab=comments") %></i>
<i id="rp-fragment"><%= route_of("/articles/7#comments") %></i>
<i id="rp-root"><%= route_of("/") %></i>
<i id="rp-json"><%= route_of("/articles/7.json") %></i>
<i id="rp-missing"><%= route_of("/nowhere") %></i>
<i id="rp-get"><%= route_of("/articles", method: :get) %></i>
<i id="rp-post"><%= route_of("/articles", method: :post) %></i>
<i id="rp-string"><%= route_of("/articles/7/comments/3", method: "Delete") %></i>
<i id="rp-no-verb"><%= route_of("/articles", method: :put) %></i>
<i id="rp-direct"><%= Rails.application.routes.recognize_path("/articles", method: :post)[:action] %></i>
<i id="rp-head"><%= route_of("/articles/7", method: :head) %></i>
<i id="rp-any"><%= route_of("/articles", method: :any) %></i>
<i id="rp-foo"><%= route_of("/articles", method: :foo) %></i>
<i id="rp-url"><%= route_of("http://example.com/articles/7") %></i>
<i id="rp-url-port"><%= route_of("https://example.com:3000/articles?x=1") %></i>
<i id="rp-relative"><%= route_of("articles/7") %></i>
<i id="rp-relative-miss"><%= route_of("nowhere") %></i>
<i id="rp-relative-url-query"><%= route_of("articles/7?next=http://x") %></i>
<i id="rp-doubled"><%= route_of("/articles//7") %></i>
<i id="rp-trailing-miss"><%= route_of("/nowhere/") %></i>
<i id="rp-bad-uri"><%= route_of("/articles/a b") %></i>
<i id="rp-url-trailing"><%= route_of("http://example.com/articles/7/") %></i>
<i id="rp-percent"><%= route_of("/nowhere/%7e") %></i>
<i id="rp-bad-uri-first"><%= route_of("/a b", method: :foo) %></i>
<i id="rp-same"><%= Rails.application.routes.recognize_path("/articles/7.json") == { controller: "articles", action: "show", id: "7", format: "json" } %></i>
"#,
        r#"    assert_match(/<i id="rp-index">controller=articles action=index<\/i>/, response.body)
    assert_match(/<i id="rp-show">controller=articles action=show id=7<\/i>/, response.body)
    assert_match(/<i id="rp-fragment">controller=articles action=show id=7<\/i>/, response.body)
    assert_match(/<i id="rp-root">controller=articles action=index<\/i>/, response.body)
    assert_match(/<i id="rp-json">controller=articles action=show id=7 format=json<\/i>/, response.body)
    assert_match(/<i id="rp-missing">none: No route matches (&quot;|")\/nowhere(&quot;|")<\/i>/, response.body)
    assert_match(/<i id="rp-get">controller=articles action=index<\/i>/, response.body)
    assert_match(/<i id="rp-post">controller=articles action=create<\/i>/, response.body)
    assert_match(/<i id="rp-string">controller=comments action=destroy article_id=7 id=3<\/i>/, response.body)
    assert_match(/<i id="rp-no-verb">none: No route matches (&quot;|")\/articles(&quot;|")<\/i>/, response.body)
    assert_match(/<i id="rp-direct">create<\/i>/, response.body)
    assert_match(/<i id="rp-head">controller=articles action=show id=7<\/i>/, response.body)
    assert_includes(response.body, '<i id="rp-any">ActionController::UnknownHttpMethod: ANY, accepted HTTP methods are OPTIONS, GET, HEAD, POST, PUT, DELETE, TRACE, CONNECT, PROPFIND, PROPPATCH, MKCOL, COPY, MOVE, LOCK, UNLOCK, VERSION-CONTROL, REPORT, CHECKOUT, CHECKIN, UNCHECKOUT, MKWORKSPACE, UPDATE, LABEL, MERGE, BASELINE-CONTROL, MKACTIVITY, ORDERPATCH, ACL, SEARCH, MKCALENDAR, and PATCH</i>')
    assert_match(/<i id="rp-foo">ActionController::UnknownHttpMethod: FOO, accepted HTTP methods are OPTIONS, GET, /, response.body)
    assert_match(/<i id="rp-url">controller=articles action=show id=7<\/i>/, response.body)
    assert_match(/<i id="rp-url-port">controller=articles action=index<\/i>/, response.body)
    assert_match(/<i id="rp-relative">controller=articles action=show id=7<\/i>/, response.body)
    assert_match(/<i id="rp-relative-miss">none: No route matches (&quot;|")\/nowhere(&quot;|")<\/i>/, response.body)
    assert_match(/<i id="rp-relative-url-query">controller=articles action=show id=7<\/i>/, response.body)
    assert_match(/<i id="rp-doubled">controller=articles action=show id=7<\/i>/, response.body)
    assert_match(/<i id="rp-trailing-miss">none: No route matches (&quot;|")\/nowhere(&quot;|")<\/i>/, response.body)
    assert_match(/<i id="rp-bad-uri">none: bad URI \(is not URI\?\): (&quot;|")\/articles\/a b(&quot;|")<\/i>/, response.body)
    assert_match(/<i id="rp-url-trailing">none: No route matches (&quot;|")http:\/\/example.com\/articles\/7\/(&quot;|")<\/i>/, response.body)
    assert_match(/<i id="rp-percent">none: No route matches (&quot;|")\/nowhere\/%7E(&quot;|")<\/i>/, response.body)
    assert_match(/<i id="rp-bad-uri-first">none: bad URI \(is not URI\?\): (&quot;|")\/a b(&quot;|")<\/i>/, response.body)
    assert_match(/<i id="rp-same">true<\/i>/, response.body)
"#,
    );
    run.assert_passes();
}

/// `recognize_path` gives the Rails controller path, not the flat
/// router name, for a namespaced controller, for the health controller
/// and for the Active Storage controllers. It skips a redirect route and
/// tries the later routes, as Rails does. The app has no root route, so
/// `/` matches no route. Each expected value is the output of Rails
/// 7.2.4 for the same routes.
#[test]
fn a_helper_recognizes_controller_paths_and_skips_redirect_routes() {
    let run = on_the_index(
        emit_and_run::real_blog()
            .edit("config/routes.rb", "  root \"articles#index\"\n", "")
            .edit(
                "config/routes.rb",
                "  resources :articles do",
                "  namespace :admin do\n    resources :articles, only: [:index]\n  end\n  \
                 get \"old\", to: redirect(\"/articles\")\n  \
                 get \"moved\", to: redirect(\"/articles\")\n  \
                 get \"moved\", to: \"articles#index\"\n  \
                 get \"up\" => \"rails/health#show\"\n  \
                 resources :articles do",
            )
            .write(
                "app/controllers/admin/articles_controller.rb",
                "class Admin::ArticlesController < ApplicationController\n  def index\n    @articles = Article.all\n  end\nend\n",
            )
            .write("app/views/admin/articles/index.html.erb", "<p>admin</p>\n")
            .write(
                "app/helpers/articles_helper.rb",
                r##"module ArticlesHelper
  def route_of(path)
    recognized = Rails.application.routes.recognize_path(path)
    recognized.map { |key, value| "#{key}=#{value}" }.join(" ")
  rescue ActionController::RoutingError => e
    "none: #{e.message}"
  end
end
"##,
            ),
        r#"<i id="rp-namespaced"><%= route_of("/admin/articles") %></i>
<i id="rp-redirect"><%= route_of("/old") %></i>
<i id="rp-after-redirect"><%= route_of("/moved") %></i>
<i id="rp-health"><%= route_of("/up") %></i>
<i id="rp-disk"><%= route_of("/rails/active_storage/disk/k/x.png") %></i>
<i id="rp-blob"><%= route_of("/rails/active_storage/blobs/redirect/abc/x.png") %></i>
<i id="rp-proxy"><%= route_of("/rails/active_storage/blobs/proxy/abc/x.png") %></i>
<i id="rp-no-root"><%= route_of("/") %></i>
"#,
        r#"    assert_match(/<i id="rp-namespaced">controller=admin\/articles action=index<\/i>/, response.body)
    assert_match(/<i id="rp-redirect">none: No route matches (&quot;|")\/old(&quot;|")<\/i>/, response.body)
    assert_match(/<i id="rp-after-redirect">controller=articles action=index<\/i>/, response.body)
    assert_match(/<i id="rp-health">controller=rails\/health action=show<\/i>/, response.body)
    assert_match(/<i id="rp-disk">controller=active_storage\/disk action=show encoded_key=k filename=x format=png<\/i>/, response.body)
    assert_match(/<i id="rp-blob">controller=active_storage\/blobs\/redirect action=show signed_id=abc filename=x format=png<\/i>/, response.body)
    assert_match(/<i id="rp-proxy">controller=active_storage\/blobs\/proxy action=show signed_id=abc filename=x format=png<\/i>/, response.body)
    assert_match(/<i id="rp-no-root">none: No route matches (&quot;|")\/(&quot;|")<\/i>/, response.body)
"#,
    );
    run.assert_passes();
}

/// B4 in NEXUS_BUGS.md: a partial in `app/views/application/` that a
/// view in another directory renders. Rails looks in the view's own
/// directory first, so a same-name partial there wins.
#[test]
fn a_partial_in_the_application_view_directory_runs_from_another_directory() {
    let run = on_the_index(
        emit_and_run::real_blog()
            .write(
                "app/views/application/_blank_slate.html.erb",
                "<%# locals: (message:) %>\n<p id=\"b4-app\"><%= message %></p>\n",
            )
            .write(
                "app/views/application/_shadowed.html.erb",
                "<%# locals: (message:) %>\n<p id=\"b4-shadow-app\"><%= message %></p>\n",
            )
            .write(
                "app/views/articles/_shadowed.html.erb",
                "<%# locals: (message:) %>\n<p id=\"b4-shadow-own\"><%= message %></p>\n",
            ),
        "<%= render \"blank_slate\", message: \"No articles\" %>\n\
         <%= render \"shadowed\", message: \"own dir\" %>\n",
        "    assert_match(/<p id=\"b4-app\">No articles<\\/p>/, response.body)\n    \
             assert_match(/<p id=\"b4-shadow-own\">own dir<\\/p>/, response.body)\n    \
             assert_no_match(/b4-shadow-app/, response.body)\n",
    );
    run.assert_passes();
}

/// B4 in NEXUS_BUGS.md: `render "x", k: v` looks in the view directory
/// of each controller ancestor, nearest first. The `application`
/// directory is only reached when the chain reaches ApplicationController.
#[test]
fn a_partial_in_a_parent_controller_view_directory_runs() {
    let run = on_the_index(
        emit_and_run::real_blog()
            .write(
                "app/controllers/base_controller.rb",
                "class BaseController < ApplicationController\nend\n",
            )
            .edit(
                "app/controllers/articles_controller.rb",
                "class ArticlesController < ApplicationController",
                "class ArticlesController < BaseController",
            )
            .write(
                "app/views/base/_nav.html.erb",
                "<%# locals: (label:) %>\n<p id=\"b4-base\"><%= label %></p>\n",
            )
            .write(
                "app/views/application/_nav.html.erb",
                "<%# locals: (label:) %>\n<p id=\"b4-app\"><%= label %></p>\n",
            ),
        "<%= render \"nav\", label: \"parent dir\" %>\n",
        "    assert_match(/<p id=\"b4-base\">parent dir<\\/p>/, response.body)\n    \
             assert_no_match(/b4-app/, response.body)\n",
    );
    run.assert_passes();
}

/// The `render partial: "x", locals: { ... }` spelling resolves
/// through the parent controller's view directory too.
#[test]
fn a_partial_keyword_in_a_parent_controller_view_directory_runs() {
    let run = on_the_index(
        emit_and_run::real_blog()
            .write(
                "app/controllers/base_controller.rb",
                "class BaseController < ApplicationController\nend\n",
            )
            .edit(
                "app/controllers/articles_controller.rb",
                "class ArticlesController < ApplicationController",
                "class ArticlesController < BaseController",
            )
            .write(
                "app/views/base/_nav.html.erb",
                "<%# locals: (label:) %>\n<p id=\"b4-base\"><%= label %></p>\n",
            )
            .write(
                "app/views/application/_nav.html.erb",
                "<%# locals: (label:) %>\n<p id=\"b4-app\"><%= label %></p>\n",
            ),
        "<%= render partial: \"nav\", locals: { label: \"hash form\" } %>\n",
        "    assert_match(/<p id=\"b4-base\">hash form<\\/p>/, response.body)\n    \
             assert_no_match(/b4-app/, response.body)\n",
    );
    run.assert_passes();
}

/// A namespaced parent controller (`Admin::BaseController`) has the
/// view directory `admin/base`, which wins over `application`.
#[test]
fn a_partial_in_a_namespaced_parent_controller_view_directory_runs() {
    let run = on_the_index(
        emit_and_run::real_blog()
            .write(
                "app/controllers/admin/base_controller.rb",
                "class Admin::BaseController < ApplicationController\nend\n",
            )
            .edit(
                "app/controllers/articles_controller.rb",
                "class ArticlesController < ApplicationController",
                "class ArticlesController < Admin::BaseController",
            )
            .write(
                "app/views/admin/base/_nav.html.erb",
                "<%# locals: (label:) %>\n<p id=\"b4-admin\"><%= label %></p>\n",
            )
            .write(
                "app/views/application/_nav.html.erb",
                "<%# locals: (label:) %>\n<p id=\"b4-app\"><%= label %></p>\n",
            ),
        "<%= render \"nav\", label: \"admin dir\" %>\n",
        "    assert_match(/<p id=\"b4-admin\">admin dir<\\/p>/, response.body)\n    \
             assert_no_match(/b4-app/, response.body)\n",
    );
    run.assert_passes();
}

/// Not handed to the csv gem's `headers:`: the lowering writes the header row itself, so the output is held to what CSV.generate answers.
#[test]
fn csv_generate_with_written_headers_runs() {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            r##"class Article < ApplicationRecord
  has_many :comments, dependent: :destroy

  def csv_probe
    CSV.generate(headers: ["id", "title"], write_headers: true) do |csv|
      csv << [1, "x"]
      csv << [2, "y,z"]
    end
  end"##,
        )
        .write(
            "test/models/article_csv_test.rb",
            r#"require "test_helper"

class ArticleCsvTest < ActiveSupport::TestCase
  test "CSV.generate writes its headers as the first row" do
    assert_equal "id,title\n1,x\n2,\"y,z\"\n", Article.new.csv_probe
  end
end
"#,
        )
        .run_test("test/models/article_csv_test.rb")
        .assert_passes();
}

/// Not left on the receiver: no ruby-family runtime ships `ActiveModel::Type::Boolean` or the ActiveSupport key conversions.
#[test]
fn boolean_cast_and_key_conversions_run() {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            r##"class Article < ApplicationRecord
  has_many :comments, dependent: :destroy

  def as_hash_boolean_probe(flag)
    on = ActiveModel::Type::Boolean.new.cast(flag)
    h = { a: "x", b: "y" }
    [on.inspect, h.stringify_keys.keys.join(","), h.deep_symbolize_keys.keys.join(","), h.symbolize_keys.size, { "c" => 1 }.stringify_keys.keys.first].join("|")
  end"##,
        )
        .write(
            "test/models/article_as_hash_boolean_test.rb",
            r#"require "test_helper"

class ArticleAsHashBooleanTest < ActiveSupport::TestCase
  test "Boolean#cast and the key conversions answer as ActiveModel and ActiveSupport do" do
    article = Article.new
    assert_equal "false|a,b|a,b|2|c", article.as_hash_boolean_probe("off")
    assert_equal "true|a,b|a,b|2|c", article.as_hash_boolean_probe("1")
    assert_equal "nil|a,b|a,b|2|c", article.as_hash_boolean_probe("")
  end
end
"#,
        )
        .run_test("test/models/article_as_hash_boolean_test.rb")
        .assert_passes();
}

/// `Hash#to_query` is the scalar query string Rails builds: symbol or
/// string keys, a nil value with no `=`, and insertion order. Nested
/// hashes stay on the ruby-family reopen.
#[test]
fn a_hash_to_query_renders_symbol_and_string_keys() {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            r#"class Article < ApplicationRecord
  has_many :comments, dependent: :destroy

  def query_probe
    [
      { name: "Ada", role: nil }.to_query,
      { "name" => "Ada", "role" => "editor" }.to_query,
      {}.to_query
    ].join("|")
  end
"#,
        )
        .write(
            "test/models/article_hash_query_test.rb",
            r#"require "test_helper"

class ArticleHashQueryTest < ActiveSupport::TestCase
  test "Hash#to_query renders symbol keys, string keys, and a nil value" do
    assert_equal "name=Ada&role|name=Ada&role=editor|", Article.new.query_probe
  end
end
"#,
        )
        .run_test("test/models/article_hash_query_test.rb")
        .assert_passes();
}

/// `Array.wrap` is ActiveSupport's class method: nil is empty, an array
/// stays an array, and a scalar becomes a one-element array.
#[test]
fn array_wrap_keeps_nil_an_array_and_a_scalar_distinct() {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            r#"class Article < ApplicationRecord
  has_many :comments, dependent: :destroy

  def wrapped_nil
    Array.wrap(nil).map { |item| item.to_s }.join(",")
  end

  def wrapped_array
    Array.wrap(%w[a b]).map { |item| item.to_s }.join(",")
  end

  def wrapped_string
    Array.wrap("solo").map { |item| item.to_s }.join(",")
  end

  def wrapped_integer
    Array.wrap(7).map { |item| item.to_s }.join(",")
  end

  # One caller passes an array, the default is nil, so the parameter
  # is `Array | Nil`. Folding that to one shape would nest the array
  # or wrap nil. The call stays and answers both.
  def wrapped_either(value = nil)
    Array.wrap(value).map { |item| item.to_s }.join(",")
  end

  def either_from_array
    wrapped_either(%w[a b])
  end
"#,
        )
        .write(
            "test/models/article_array_wrap_test.rb",
            r#"require "test_helper"

class ArticleArrayWrapTest < ActiveSupport::TestCase
  test "Array.wrap keeps nil, an array, and a scalar distinct" do
    article = Article.new
    assert_equal "", article.wrapped_nil
    assert_equal "a,b", article.wrapped_array
    assert_equal "solo", article.wrapped_string
    assert_equal "7", article.wrapped_integer
    assert_equal "", article.wrapped_either
    assert_equal "", article.wrapped_either(nil)
    assert_equal "a,b", article.wrapped_either(%w[a b])
  end
end
"#,
        )
        .run_test("test/models/article_array_wrap_test.rb")
        .assert_passes();
}

/// Not a diagnostic count: each core method's value is held to what CRuby answers.
#[test]
fn core_integer_float_string_array_methods_run() {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            r##"class Article < ApplicationRecord
  has_many :comments, dependent: :destroy

  def core_surface_probe
    n = 17
    ints = [n.clamp(1, 10), n.div(5), n.modulo(5), n.gcd(4), n.lcm(4), n.pow(2), n.bit_length, n.divmod(5).first]
    f = 7.5
    floats = [n.fdiv(2), f.clamp(1.0, 5.0), f.modulo(2.0), f.fdiv(2)]
    ups = 1.upto(3).map { |i| i * 2 }
    downs = 3.downto(1).map { |i| i }
    steps = 0.step(6, 3).map { |i| i }
    seen = []
    3.times { |i| seen << i }
    labels = %w[a b c].map.with_index { |s, i| "#{i}#{s}" }
    arr = [3, 1, 2]
    arr.sort_by! { |x| -x }
    arr.select! { |x| x > 1 }
    arr.insert(1, 9)
    s = "hello".dup
    s.gsub!("l", "L")
    [ints.join(","), floats.join(","), ups.join(","), downs.join(","), steps.join(","), seen.join(","), labels.join(","), arr.join(","), s, Regexp.escape("a.b")].join("|")
  end"##,
        )
        .write(
            "test/models/article_core_surface_test.rb",
            r#"require "test_helper"

class ArticleCoreSurfaceTest < ActiveSupport::TestCase
  test "core Integer, Float, String and Array methods answer as CRuby does" do
    assert_equal "10,3,2,1,68,289,5,3|8.5,5.0,1.5,3.75|2,4,6|3,2,1|0,3,6|0,1,2|0a,1b,2c|3,9,2|heLLo|a\\.b",
                 Article.new.core_surface_probe
  end
end
"#,
        )
        .run_test("test/models/article_core_surface_test.rb")
        .assert_passes();
}

/// Not only the `check` side: an enum, scope and method a namespaced abstract base declares have to run when another model reaches them.
#[test]
fn an_abstract_base_reaches_its_model() {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "create_table \"articles\", force: :cascade do |t|",
            "create_table \"articles\", force: :cascade do |t|\n    t.integer \"state\", default: 0, null: false",
        )
        .write(
            "app/models/base_model/content_base.rb",
            r#"class BaseModel::ContentBase < ApplicationRecord
  self.abstract_class = true
  self.table_name = "articles"
  enum :state, { draft: 0, live: 1 }
  scope :titled, -> { where.not(title: nil) }

  def shout
    title.to_s.upcase
  end
end
"#,
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            "class Article < BaseModel::ContentBase\n  has_many :comments, dependent: :destroy\n  scope :newest_first, -> { order(id: :desc) }",
        )
        .edit(
            "app/models/comment.rb",
            "class Comment < ApplicationRecord",
            r#"class Comment < ApplicationRecord
  def base_probe
    a = Article.find(article_id)
    [article.draft?, a.shout, Article.titled.newest_first.to_a.size, Article.newest_first.titled.first.nil?, Article.draft.count].join("|")
  end
"#,
        )
        .write(
            "test/models/comment_abstract_base_test.rb",
            r#"require "test_helper"

class CommentAbstractBaseTest < ActiveSupport::TestCase
  test "another model reaches an abstract base's enum, scope and method" do
    comment = comments(:one)
    title = comment.article.title.to_s.upcase
    assert_equal "true|#{title}|#{Article.count}|false|#{Article.count}", comment.base_probe
  end
end
"#,
        )
        .run_test("test/models/comment_abstract_base_test.rb")
        .assert_passes();
}

/// Not a plain Hash: Rails' mapping reads `statuses[:paid]` as the `"paid"` entry, so the Symbol keys are held to that.
#[test]
fn an_enum_plural_mapping_runs() {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "create_table \"articles\", force: :cascade do |t|",
            "create_table \"articles\", force: :cascade do |t|\n    t.integer \"state\", default: 0, null: false",
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            r##"class Article < ApplicationRecord
  has_many :comments, dependent: :destroy
  enum :state, { "draft" => 0, "live" => 1, "on hold" => 2 }

  def self.mapping_probe
    [states.keys.join(","), states[:live], self.states["draft"], states.key?(:live), states.fetch(:draft), states.key(1), states.map { |k, v| "#{k}=#{v}" }.join(";")].join("|")
  end

  def live_value
    self.class.states[:live]
  end"##,
        )
        .write(
            "test/models/article_enum_mapping_test.rb",
            r#"require "test_helper"

class ArticleEnumMappingTest < ActiveSupport::TestCase
  test "the plural mapping answers as Rails' indifferent Hash does" do
    assert_equal "draft,live,on hold|1|0|true|0|live|draft=0;live=1;on hold=2", Article.mapping_probe
    assert_equal 1, articles(:one).live_value
    assert_equal 0, Article.states.fetch(:draft)
    assert_equal 2, Article.states[:"on hold"]
  end
end
"#,
        )
        .run_test("test/models/article_enum_mapping_test.rb")
        .assert_passes();
}

/// Not left on the receiver: no ruby-family runtime ships Numeric#to_fs or an errors object with `messages`.
#[test]
fn delimited_numbers_and_errors_messages_run() {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            r##"class Article < ApplicationRecord
  has_many :comments, dependent: :destroy

  def errors_probe
    bad = Article.new(title: "", body: "short")
    bad.valid?
    good = Article.new(title: "t", body: "long enough body")
    good.valid?
    [bad.errors.messages.key?(:title), bad.errors.messages.key?(:created_at), bad.errors.messages.blank?, bad.errors.messages[:body].join(";"),
     good.errors.messages.blank?, good.errors.messages.present?, 1234567.to_fs(:delimited), 1234.5.to_fs(:delimited), -1234.to_fs(:delimited)].join("|")
  end"##,
        )
        .write(
            "test/models/article_errors_messages_test.rb",
            r#"require "test_helper"

class ArticleErrorsMessagesTest < ActiveSupport::TestCase
  test "errors.messages and to_fs(:delimited) answer as ActiveModel and ActiveSupport do" do
    assert_equal "true|false|false|is too short (minimum is 10 characters)|true|false|1,234,567|1,234.5|-1,234",
                 Article.new.errors_probe
  end
end
"#,
        )
        .run_test("test/models/article_errors_messages_test.rb")
        .assert_passes();
}

/// Not one value per process: a `thread_mattr_accessor` written on one thread reads nil on another, as in Rails.
#[test]
fn a_thread_mattr_accessor_runs_per_thread() {
    emit_and_run::real_blog()
        .write(
            "lib/request_context.rb",
            "module RequestContext\n  thread_mattr_accessor :article\nend\n",
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            r#"class Article < ApplicationRecord
  has_many :comments, dependent: :destroy

  def remember
    RequestContext.article = self
  end

  def self.remembered_title
    RequestContext.article.title
  end"#,
        )
        .run_ruby(
            r#"article = Article.new(title: "kept", body: "long enough body")
article.remember
raise "the writing thread lost it" unless Article.remembered_title == "kept"
raise "another thread saw it" unless Thread.new { RequestContext.article.nil? }.value
puts "ok"
"#,
        )
        .assert_passes();
}

/// Not a String: `params.expect(article: [...])` answers the permitted hash, so a `merge` onto it has to run as one.
#[test]
fn an_expected_params_hash_merges() {
    emit_and_run::real_blog()
        .edit(
            "app/controllers/articles_controller.rb",
            "    @article = Article.new(article_params)\n",
            "    @article = Article.new(article_params.merge(title: \"Merged Title\"))\n",
        )
        .edit(
            "test/controllers/articles_controller_test.rb",
            "    assert_equal \"New Title\", Article.last.title\n",
            "    assert_equal \"Merged Title\", Article.last.title\n    assert_equal \"A sufficiently long body for validation.\", Article.last.body\n",
        )
        .run_test("test/controllers/articles_controller_test.rb")
        .assert_passes();
}

/// Not read with the Symbol keys the source writes: every hash in a request's params is String-keyed at run time.
#[test]
fn nested_request_params_read_as_the_request_carried_them() {
    emit_and_run::real_blog()
        .edit(
            "app/controllers/articles_controller.rb",
            "    @article = Article.new(article_params)\n",
            r##"    @article = Article.new(article_params)
    if params[:extra].present? && params[:extra].key?(:suffix)
      @article.title = @article.title.to_s + " " + params[:extra][:suffix].to_s
    end
    if params[:tags].present?
      params[:tags].each { |t| @article.body = @article.body.to_s + " #" + t.to_s }
    end
"##,
        )
        .edit(
            "test/controllers/articles_controller_test.rb",
            "      post articles_url, params: { article: { body: \"A sufficiently long body for validation.\", title: \"New Title\" } }\n",
            "      post articles_url, params: { article: { body: \"A sufficiently long body for validation.\", title: \"New Title\" }, extra: { suffix: \"Extra\" }, tags: [\"a\", \"b\"] }\n",
        )
        .edit(
            "test/controllers/articles_controller_test.rb",
            "    assert_equal \"New Title\", Article.last.title\n",
            "    assert_equal \"New Title Extra\", Article.last.title\n    assert_equal \"A sufficiently long body for validation. #a #b\", Article.last.body\n",
        )
        .run_test("test/controllers/articles_controller_test.rb")
        .assert_passes();
}

/// Not left as `permit`/`to_unsafe_h`/`require`: the emitted request's params are plain hashes, which answer none of them.
#[test]
fn nested_request_params_permit_and_require_run() {
    emit_and_run::real_blog()
        .edit(
            "app/controllers/articles_controller.rb",
            "    @article = Article.new(article_params)\n",
            r##"    @article = Article.new(article_params)
    if params.to_unsafe_h.key?(:extra)
      @article.title = @article.title.to_s + " " + params[:extra].permit(:suffix)[:suffix].to_s
      @article.body = @article.body.to_s + " " + params[:extra].to_unsafe_h.keys.join(",") + " " + params[:extra].require(:suffix).to_s
    end
"##,
        )
        .edit(
            "test/controllers/articles_controller_test.rb",
            "      post articles_url, params: { article: { body: \"A sufficiently long body for validation.\", title: \"New Title\" } }\n",
            "      post articles_url, params: { article: { body: \"A sufficiently long body for validation.\", title: \"New Title\" }, extra: { suffix: \"Extra\", other: \"x\" } }\n",
        )
        .edit(
            "test/controllers/articles_controller_test.rb",
            "    assert_equal \"New Title\", Article.last.title\n",
            "    assert_equal \"New Title Extra\", Article.last.title\n    assert_equal \"A sufficiently long body for validation. suffix,other Extra\", Article.last.body\n",
        )
        .run_test("test/controllers/articles_controller_test.rb")
        .assert_passes();
}

/// Not only a `params[...]` chain: a local assigned from one holds the same request value, and reads through it the same way.
#[test]
fn a_local_holding_a_request_params_value_reads_it() {
    emit_and_run::real_blog()
        .edit(
            "app/controllers/articles_controller.rb",
            "    @article = Article.new(article_params)\n",
            r##"    @article = Article.new(article_params)
    extra = params[:extra]
    tags = params[:tags]
    if extra.present? && extra.key?(:suffix)
      @article.title = @article.title.to_s + " " + extra[:suffix].to_s
    end
    if tags.present?
      tags.each { |t| @article.body = @article.body.to_s + " #" + t.to_s }
    end
"##,
        )
        .edit(
            "test/controllers/articles_controller_test.rb",
            "      post articles_url, params: { article: { body: \"A sufficiently long body for validation.\", title: \"New Title\" } }\n",
            "      post articles_url, params: { article: { body: \"A sufficiently long body for validation.\", title: \"New Title\" }, extra: { suffix: \"Extra\" }, tags: [\"a\", \"b\"] }\n",
        )
        .edit(
            "test/controllers/articles_controller_test.rb",
            "    assert_equal \"New Title\", Article.last.title\n",
            "    assert_equal \"New Title Extra\", Article.last.title\n    assert_equal \"A sufficiently long body for validation. #a #b\", Article.last.body\n",
        )
        .run_test("test/controllers/articles_controller_test.rb")
        .assert_passes();
}

/// Gap F15 took `&method(:name)` from an ingest error (block-argument
/// forms other than `&:symbol`/`&local_var` were unsupported) to
/// clean, emitting `&method(:name)` verbatim (`ExprNode::MethodRef`).
/// Invariant 6: prove the emitted PROGRAM runs it, not just that
/// `check` stays quiet — a PORO under `app/lib` maps an array through
/// a bound-method reference to its own helper, and a model test reads
/// the result back.
#[test]
fn method_ref_block_arg_runs() {
    emit_and_run::real_blog()
        .write(
            "app/lib/doubler.rb",
            "class Doubler\n  \
               def self.doubled(list)\n    \
                 list.map(&method(:double))\n  \
               end\n\n  \
               def self.double(n)\n    \
                 n * 2\n  \
               end\n\
             end\n",
        )
        .write(
            "test/models/doubler_test.rb",
            "require \"test_helper\"\n\n\
             class DoublerTest < ActiveSupport::TestCase\n  \
               test \"&method(:name) as a block argument runs\" do\n    \
                 assert_equal [2, 4, 6], Doubler.doubled([1, 2, 3])\n  \
               end\n\
             end\n",
        )
        .run_test("test/models/doubler_test.rb")
        .assert_passes();
}

/// A `T::Struct` nested in a controller concern must lower and run:
/// keyword construction, readers, and a writable `prop`. Taking that
/// nested class as the controller used to drop the declarations as
/// unrecognized macros and skip the concern's module path entirely.
#[test]
fn a_t_struct_nested_in_a_controller_concern_runs() {
    emit_and_run::real_blog()
        .write(
            "app/controllers/concerns/window_settings.rb",
            concat!(
                "module WindowSettings\n",
                "  extend ActiveSupport::Concern\n",
                "\n",
                "  class Span < T::Struct\n",
                "    const :from_date, String\n",
                "    const :to_date, String\n",
                "    prop :label, String, default: \"window\"\n",
                "  end\n",
                "end\n",
            ),
        )
        .edit(
            "app/controllers/articles_controller.rb",
            "class ArticlesController < ApplicationController\n",
            "class ArticlesController < ApplicationController\n  include WindowSettings\n",
        )
        .run_ruby(
            concat!(
                "span = WindowSettings::Span.new(from_date: \"2026-01-01\", to_date: \"2026-01-31\")\n",
                "raise \"from\" unless span.from_date == \"2026-01-01\"\n",
                "raise \"to\" unless span.to_date == \"2026-01-31\"\n",
                "raise \"default\" unless span.label == \"window\"\n",
                "span.label = \"quarter\"\n",
                "raise \"prop\" unless span.label == \"quarter\"\n",
            ),
        )
        .assert_passes();
}

/// A clean factory call must construct the receiving T::Struct, not
/// the concern or whichever includer was seen first. Exercise native
/// emitted consumers as well as the objects, independently of the
/// analyzer's inferred return types (invariant 6).
#[test]
fn a_shared_struct_factory_runs_for_both_includers_in_both_orders() {
    let reading = "class Reading < T::Struct\n  include Factory\n  PREFIX = \"local:\"\n  const :label, String\nend\n";
    let packet = "class Packet < T::Struct\n  include Factory\n  const :size, Integer\nend\n";
    for declarations in [format!("{reading}{packet}"), format!("{packet}{reading}")] {
        emit_and_run::real_blog()
            .write("app/services/factory.rb", r#"module Factory
  def self.included(base)
    base.extend(ClassMethods)
  end
  module ClassMethods
    def build(**fields)
      new(**fields).freeze
    end
    def prefix
      "old:"
    end
  end
  def self.prefix
    "initial module:"
  end
end
"#)
            // Reopening a carrier must retain build and take the newer
            // prefix, whose default still belongs to Factory's scope.
            .write("app/services/factory_extension.rb", r#"module Factory
  PREFIX = "reading:"
  module ClassMethods
    def fixed
      Reading.new(label: "fixed").freeze
    end
    def prefix(value = PREFIX)
      value
    end
  end
  def self.prefix
    "module:"
  end
end
"#)
            .write("app/services/values.rb", &declarations)
            .write("app/services/factory_consumer.rb", r#"class FactoryConsumer
  def self.label
    Reading.prefix + Reading.build(label: "sensor").label.upcase
  end
  def self.size
    Packet.build(size: 7).size * 3
  end
end
"#)
            .write("app/controllers/factory_probes_controller.rb", r#"class FactoryProbesController < ApplicationController
  def index
    @label = Reading.build(label: "probe").label
    @size = Packet.build(size: 7).size
    render plain: FactoryConsumer.label
  end
end
"#)
            .run_ruby(r#"
reading = Reading.build(label: "probe")
packet = Packet.build(size: 7)
raise "reading identity" unless reading.class == Reading
raise "reading field" unless reading.label == "probe"
raise "reading freeze" unless reading.frozen?
raise "packet identity" unless packet.class == Packet
raise "packet field" unless packet.size == 7
raise "packet freeze" unless packet.frozen?
raise "label consumer" unless FactoryConsumer.label == "reading:SENSOR"
raise "size consumer" unless FactoryConsumer.size == 21
raise "module singleton" unless Factory.prefix == "module:"
fixed = Packet.fixed
raise "fixed-other identity" unless fixed.class == Reading
raise "fixed-other field" unless fixed.label == "fixed"
raise "fixed-other freeze" unless fixed.frozen?
"#)
            .assert_passes();
    }
}

/// A literal table override must reach the emitted row readers and SQL,
/// not merely quiet the analyzer. Two differently named models share the
/// real articles table via string/symbol declarations; writes through either
/// must be visible to Article, without touching convention-derived decoys.
#[test]
fn explicit_model_table_names_run_against_the_declared_table() {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "  add_foreign_key \"comments\", \"articles\"",
            r#"  create_table "archived_articles" do |t|
    t.string "title"
    t.text "body"
  end
  create_table "ledger_entries" do |t|
    t.string "title"
    t.text "body"
  end
  add_foreign_key "comments", "articles""#,
        )
        .write(
            "app/models/archived_article.rb",
            "class ArchivedArticle < ApplicationRecord\n  self.table_name = \"articles\"\nend\n",
        )
        .write(
            "app/models/ledger/entry.rb",
            r#"module Ledger
  def self.table_name_prefix
    "ledger_"
  end
  class Entry < ApplicationRecord
    self.table_name = :"art\x69cles"
  end
end
"#,
        )
        .write(
            "app/models/decoy/archived_article.rb",
            "class Decoy::ArchivedArticle < ApplicationRecord\nend\n",
        )
        .write(
            "app/models/ledger_entry.rb",
            "class LedgerEntry < ApplicationRecord\nend\n",
        )
        .run_ruby(
            r#"raise ArchivedArticle.table_name.inspect unless ArchivedArticle.table_name == "articles"
raise Ledger::Entry.table_name.inspect unless Ledger::Entry.table_name == "articles"
decoy = Decoy::ArchivedArticle.create!(title: "Conventional decoy", body: "Leave untouched")
prefixed_decoy = LedgerEntry.create!(title: "Prefixed decoy", body: "Leave untouched too")
record = ArchivedArticle.create!(title: "Original", body: "A long enough body")
raise unless Article.find(record.id).title == "Original"
entry = Ledger::Entry.find(record.id)
raise unless entry.title == "Original"
entry.update!(title: "Changed")
raise unless Article.find(record.id).title == "Changed"
entry.destroy!
raise unless Article.find_by(id: record.id).nil?
symbol_record = Ledger::Entry.create!(title: "Symbol-created", body: "A long enough body")
raise unless Article.find(symbol_record.id).title == "Symbol-created"
ArchivedArticle.find(symbol_record.id).update!(title: "String-updated")
raise unless Ledger::Entry.find(symbol_record.id).title == "String-updated"
Ledger::Entry.find(symbol_record.id).destroy!
raise unless Article.find_by(id: symbol_record.id).nil?
raise unless Decoy::ArchivedArticle.count == 1
raise unless Decoy::ArchivedArticle.find(decoy.id).title == "Conventional decoy"
raise unless LedgerEntry.count == 1
raise unless LedgerEntry.find(prefixed_decoy.id).title == "Prefixed decoy"
"#,
        )
        .assert_passes();
}

/// Not only `Model.scope`: a scope the target model inherits from an abstract base answers on an association reaching it too.
#[test]
fn an_inherited_scope_answers_on_an_association() {
    emit_and_run::real_blog()
        .write(
            "app/models/remark_base.rb",
            "class RemarkBase < ApplicationRecord\n  self.abstract_class = true\n  scope :by_alice, -> { where(commenter: \"Alice\") }\nend\n",
        )
        .edit("app/models/comment.rb", "class Comment < ApplicationRecord", "class Comment < RemarkBase")
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n\n  def alice_count\n    comments.by_alice.count\n  end",
        )
        .write(
            "test/models/article_inherited_scope_test.rb",
            r#"require "test_helper"

class ArticleInheritedScopeTest < ActiveSupport::TestCase
  test "an association answers a scope its model inherits" do
    article = articles(:one)
    before = article.alice_count
    article.comments.create!(commenter: "Alice", body: "A comment from Alice.")
    article.comments.create!(commenter: "Bob", body: "A comment from Bob.")
    assert_equal before + 1, article.alice_count
  end
end
"#,
        )
        .run_test("test/models/article_inherited_scope_test.rb")
        .assert_passes();
}

/// Not only a string default: schema.rb's unquoted `default: true`, `default: 1.5` and `default: -3` reach a new record, and a value the caller passes still wins.
#[test]
fn a_schema_default_that_is_not_a_string_seeds_a_new_record() {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "create_table \"articles\", force: :cascade do |t|",
            "create_table \"articles\", force: :cascade do |t|\n    t.boolean \"visible\", default: true\n    t.boolean \"listed\", default: true, null: false\n    t.float \"score\", default: 1.5\n    t.integer \"rank\", default: 7\n    t.integer \"offset\", default: -3, null: false\n    t.integer \"state\", default: 1, null: false",
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n  enum :state, { draft: 0, published: 1 }",
        )
        .write(
            "test/models/article_default_test.rb",
            r#"require "test_helper"

class ArticleDefaultTest < ActiveSupport::TestCase
  test "an unset column takes its schema default" do
    article = Article.new
    assert_equal true, article.visible
    assert_equal true, article.listed
    assert_equal 1.5, article.score
    assert_equal 7, article.rank
    assert_equal(-3, article.offset)
    assert article.published?
  end

  test "a created record keeps the default" do
    article = Article.create!(title: "Defaults", body: "A body long enough to validate.")
    reloaded = Article.find(article.id)
    assert_equal true, reloaded.visible
    assert_equal 7, reloaded.rank
    assert reloaded.published?
  end

  test "a value the caller passes wins over the default" do
    article = Article.new(visible: false, listed: false, rank: nil, score: nil, state: "draft")
    assert_equal false, article.visible
    assert_equal false, article.listed
    assert_nil article.rank
    assert_nil article.score
    assert article.draft?
  end
end
"#,
        )
        .run_test("test/models/article_default_test.rb")
        .assert_passes();
}

/// Not a NoMethodError: an enum's `not_<label>` scope and `<column>_before_type_cast` exist, as Rails generates them.
#[test]
fn enum_negative_scopes_and_stored_values_run() {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "create_table \"articles\", force: :cascade do |t|",
            "create_table \"articles\", force: :cascade do |t|\n    t.integer \"state\", default: 0, null: false\n    t.string \"tone\"",
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n  enum :state, { draft: 0, published: 1 }\n  enum :tone, { quiet: \"q\", loud: \"l\" }, prefix: true",
        )
        .write(
            "test/models/article_enum_scope_test.rb",
            r#"require "test_helper"

class ArticleEnumScopeTest < ActiveSupport::TestCase
  test "negative scopes" do
    article = Article.create!(title: "Scopes", body: "A body long enough to validate.", state: :published, tone: :loud)
    assert_equal 1, Article.not_draft.where(id: article.id).count
    assert_equal 0, Article.not_published.where(id: article.id).count
    assert_equal 0, Article.not_tone_loud.where(id: article.id).count
  end

  test "the stored value before the label" do
    article = Article.create!(title: "Raw", body: "A body long enough to validate.", state: :published, tone: :loud)
    reloaded = Article.find(article.id)
    assert_equal 1, ActiveRecord.adapter.find("articles", reloaded.id)["state"]
    assert_equal "l", ActiveRecord.adapter.find("articles", reloaded.id)["tone"]
  end
end
"#,
        )
        .run_test("test/models/article_enum_scope_test.rb")
        .assert_passes();
}

/// Not a NoMethodError: `read_attribute`/`write_attribute` are a model's `[]`/`[]=`, inside the model and on a record alike.
#[test]
fn read_and_write_attribute_reach_the_column() {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "create_table \"articles\", force: :cascade do |t|",
            "create_table \"articles\", force: :cascade do |t|\n    t.integer \"state\", default: 0, null: false",
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n  enum :state, { draft: 0, published: 1 }\n\n  def shout_title\n    write_attribute(:title, read_attribute(:title).upcase)\n  end",
        )
        .write(
            "test/models/article_attribute_test.rb",
            r#"require "test_helper"

class ArticleAttributeTest < ActiveSupport::TestCase
  test "read_attribute and write_attribute on a record" do
    article = articles(:one)
    article.write_attribute(:state, "published")
    assert_equal "published", article.read_attribute(:state)
    assert_equal "published", article.read_attribute("state")
    article.save!
    assert Article.find(article.id).published?
  end

  test "the bare forms inside the model" do
    article = Article.new(title: "quiet")
    article.shout_title
    assert_equal "QUIET", article.title
  end
end
"#,
        )
        .run_test("test/models/article_attribute_test.rb")
        .assert_passes();
}

/// A schema-less json/jsonb column is decoded at the public attribute
/// boundary and encoded again on assignment. In particular, the three
/// Rails spellings (`record.data`, `record[:data]`, and
/// `read_attribute(:data)`) must not expose SQLite's serialized text.
#[test]
fn json_columns_round_trip_decoded_values() {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "create_table \"articles\", force: :cascade do |t|",
            "create_table \"articles\", force: :cascade do |t|\n    t.json \"metadata\"",
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n\n  def metadata_names\n    read_attribute(:metadata).map { |entry| entry[\"name\"] }\n  end",
        )
        .write(
            "test/models/article_json_column_test.rb",
            r#"require "test_helper"

class ArticleJsonColumnTest < ActiveSupport::TestCase
  test "json values are decoded on every public read and encoded on write" do
    value = [{ "name" => "Ada", "enabled" => true }]
    article = Article.create!(title: "JSON", body: "A body long enough to validate.", metadata: value)
    article = Article.find(article.id)

    assert_equal value, article.metadata
    assert_equal value, article[:metadata]
    assert_equal value, article.attributes["metadata"]
    assert_equal ["Ada"], article.metadata_names

    article.write_attribute(:metadata, { "name" => "Grace", "enabled" => false })
    article.save!
    assert_equal({ "name" => "Grace", "enabled" => false }, Article.find(article.id).metadata)
  end
end
"#,
        )
        .run_test("test/models/article_json_column_test.rb")
        .assert_passes();
}

/// Temporal and JSON columns share one storage slot: text in the
/// database, a native or decoded value at the public reader, and a
/// normalizing writer for `create!`, `[]=`, and reload.
#[test]
fn column_slot_temporal_and_json_round_trip() {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "create_table \"articles\", force: :cascade do |t|",
            "create_table \"articles\", force: :cascade do |t|\n    t.datetime \"reviewed_at\"\n    t.date \"published_on\"\n    t.json \"metadata\"\n    t.jsonb \"payload\"",
        )
        .write(
            "test/models/article_column_slot_test.rb",
            r#"require "test_helper"

class ArticleColumnSlotTest < ActiveSupport::TestCase
  test "temporal text and json text share the slot boundary" do
    reviewed = Time.utc(2020, 1, 2, 3, 4, 5)
    published = Date.new(2020, 1, 2)
    metadata = [{ "name" => "Ada", "enabled" => true }]
    payload = { "on" => true }
    article = Article.create!(
      title: "Slot",
      body: "A body long enough to validate.",
      reviewed_at: reviewed,
      published_on: published,
      metadata: metadata,
      payload: payload
    )
    article = Article.find(article.id)

    assert_equal reviewed, article.reviewed_at
    assert_equal "2020-01-02 03:04:05.000000", article.reviewed_at_raw
    assert_equal published, article.published_on
    assert_equal published, article[:published_on]
    assert_equal metadata, article.metadata
    assert_equal metadata, article[:metadata]
    assert_equal metadata, article.attributes["metadata"]
    assert_equal payload, article.payload
    assert_equal payload, article[:payload]

    article[:reviewed_at] = Time.utc(2021, 2, 3, 4, 5, 6)
    article[:metadata] = { "name" => "Grace" }
    article.save!
    reloaded = Article.find(article.id)
    assert_equal Time.utc(2021, 2, 3, 4, 5, 6), reloaded.reviewed_at
    assert_equal "2021-02-03 04:05:06.000000", reloaded.reviewed_at_raw
    assert_equal({ "name" => "Grace" }, reloaded.metadata)
    assert_equal published, reloaded.published_on
  end
end
"#,
        )
        .run_test("test/models/article_column_slot_test.rb")
        .assert_passes();
}

/// Not the column default: `enum …, default:` is the value Rails gives an unset attribute, and a value the caller passes still wins.
#[test]
fn an_enum_default_option_seeds_a_new_record() {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "create_table \"articles\", force: :cascade do |t|",
            "create_table \"articles\", force: :cascade do |t|\n    t.integer \"priority\"\n    t.string \"tone\", default: \"quiet\"",
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n  enum :priority, { low: 0, high: 1 }, default: :high\n  enum :tone, { quiet: \"quiet\", loud: \"loud\" }, default: :loud",
        )
        .write(
            "test/models/article_enum_default_test.rb",
            r#"require "test_helper"

class ArticleEnumDefaultTest < ActiveSupport::TestCase
  test "an unset enum takes the declared default" do
    article = Article.new
    assert article.high?
    assert article.loud?
  end

  test "a created record keeps it" do
    article = Article.create!(title: "Defaults", body: "A body long enough to validate.")
    reloaded = Article.find(article.id)
    assert_equal "high", reloaded.priority
    assert_equal "loud", reloaded.tone
  end

  test "a value the caller passes wins" do
    article = Article.new(priority: :low, tone: :quiet)
    assert article.low?
    assert article.quiet?
  end
end
"#,
        )
        .run_test("test/models/article_enum_default_test.rb")
        .assert_passes();
}

/// Not stored as 0: a label no mapping names raises ArgumentError as Rails' enum type does, and a string-backed enum reads back its label.
#[test]
fn an_enum_rejects_a_label_it_does_not_name() {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "create_table \"articles\", force: :cascade do |t|",
            "create_table \"articles\", force: :cascade do |t|\n    t.integer \"state\", default: 0, null: false\n    t.integer \"priority\"\n    t.string \"tone\"",
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n  enum :state, { draft: 0, published: 1 }\n  enum :priority, { low: 0, high: 1 }\n  enum :tone, { quiet: \"q\", loud: \"l\" }",
        )
        .write(
            "test/models/article_enum_reject_test.rb",
            r#"require "test_helper"

class ArticleEnumRejectTest < ActiveSupport::TestCase
  test "a label the mapping does not name raises" do
    labels = ["bogus", "Draft", "loud!"]
    article = articles(:one)
    assert_raises(ArgumentError) { article.state = labels[0] }
    assert_raises(ArgumentError) { article.state = labels[1] }
    assert_raises(ArgumentError) { article.update(state: labels[0]) }
    assert_raises(ArgumentError) { article[:state] = labels[0] }
    assert_raises(ArgumentError) { Article.new(state: labels[0]) }
    assert_raises(ArgumentError) { article.tone = labels[2] }
    assert_equal "draft", Article.find(article.id).state
  end

  test "a blank value clears a nullable enum" do
    blank = ""
    article = Article.new(priority: "high")
    article.priority = blank
    assert_nil article.priority
  end

  test "a string-backed enum stores its value and reads its label" do
    article = Article.create!(title: "Tone", body: "A body long enough to validate.", tone: :loud)
    reloaded = Article.find(article.id)
    assert_equal "loud", reloaded.tone
    assert reloaded.loud?
    assert_equal 1, Article.loud.where(id: article.id).count
    assert_equal({ "quiet" => "q", "loud" => "l" }, Article.tones)
  end
end
"#,
        )
        .run_test("test/models/article_enum_reject_test.rb")
        .assert_passes();
}

/// Not `"published".to_i`: an enum column assigned a label at run time stores the label's value, as Rails does.
#[test]
fn an_enum_label_assigned_at_run_time_stores_its_value() {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "create_table \"articles\", force: :cascade do |t|",
            "create_table \"articles\", force: :cascade do |t|\n    t.integer \"state\", default: 0",
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n  enum :state, { draft: 0, published: 1 }",
        )
        .write(
            "test/models/article_enum_label_test.rb",
            r#"require "test_helper"

class ArticleEnumLabelTest < ActiveSupport::TestCase
  test "a label reaching the setter, update or []= stores its value" do
    labels = ["published", "draft"]
    article = articles(:one)

    article.state = labels[0]
    article.save!
    assert Article.find(article.id).published?

    article.update(state: labels[1])
    assert Article.find(article.id).draft?

    article[:state] = labels[0]
    article.save!
    assert Article.find(article.id).published?
    assert_equal 0, Article.where(state: :draft).where(id: article.id).count
  end
end
"#,
        )
        .run_test("test/models/article_enum_label_test.rb")
        .assert_passes();
}

/// Not the stored integer: an integer-mapped enum reads back its label, as Rails' reader, `[]` and `attributes` do.
#[test]
fn an_integer_enum_reads_back_its_label() {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "create_table \"articles\", force: :cascade do |t|",
            "create_table \"articles\", force: :cascade do |t|\n    t.integer \"state\", default: 0",
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n  enum :state, { draft: 0, published: 1 }\n\n  def state_label\n    state.humanize\n  end",
        )
        .write(
            "test/models/article_enum_reader_test.rb",
            r#"require "test_helper"

class ArticleEnumReaderTest < ActiveSupport::TestCase
  test "an integer enum reads back its label" do
    article = articles(:one)
    article.update(state: :published)
    reloaded = Article.find(article.id)
    assert_equal "published", reloaded.state
    assert_equal "Published", reloaded.state_label
    assert_equal "published", reloaded[:state]
    assert_equal "published", reloaded.attributes["state"]
    assert reloaded.published?
    assert !reloaded.draft?
    assert reloaded.state == "published"
    assert_equal 1, Article.where(state: :published).where(id: article.id).count
  end
end
"#,
        )
        .run_test("test/models/article_enum_reader_test.rb")
        .assert_passes();
}

/// A Concern's enum must reach a concrete child's readers and query mapping,
/// without replacing that child's own enum or leaking to an unrelated model.
#[test]
fn an_abstract_bases_concern_enum_runs_on_its_child() {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "create_table \"articles\", force: :cascade do |t|",
            "create_table \"articles\", force: :cascade do |t|\n    t.integer \"state\", default: 0",
        )
        .edit(
            "db/schema.rb",
            "create_table \"comments\", force: :cascade do |t|",
            "create_table \"comments\", force: :cascade do |t|\n    t.integer \"state\", default: 0",
        )
        .write(
            "app/models/concerns/publication_state.rb",
            "module PublicationState\n  extend ActiveSupport::Concern\n  included { enum :state, { draft: 0, live: 3 } }\nend\n",
        )
        .write(
            "app/models/concerns/local_state.rb",
            "module LocalState\n  extend ActiveSupport::Concern\n  included { enum :state, { queued: 2, shipped: 7 } }\nend\n",
        )
        .write(
            "app/models/content_base.rb",
            "class ContentBase < ApplicationRecord\n  self.abstract_class = true\n  self.table_name = \"articles\"\n  include PublicationState\nend\n",
        )
        .write(
            "app/models/direct_base.rb",
            "class DirectBase < ApplicationRecord\n  self.abstract_class = true\n  self.table_name = \"articles\"\n  enum :state, { draft: 0, live: 3 }\nend\n",
        )
        .write(
            "app/models/special_article.rb",
            r#"class SpecialArticle < DirectBase
  self.table_name = "articles"
  include LocalState

  def self.shipped_count(id)
    where(state: :shipped).where(id: id).count
  end

  def self.queued_count(id)
    where(state: :queued).where(id: id).count
  end
end
"#,
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord",
            r#"class Article < ContentBase
  def self.live_count(id)
    where(state: :live).where(id: id).count
  end

  def self.draft_count(id)
    where(state: :draft).where(id: id).count
  end"#,
        )
        // Query the app-owned methods: the global lowering deliberately
        // cannot guess a test-side `state` label across conflicting maps.
        .write(
            "test/models/concern_enum_inheritance_test.rb",
            r#"require "test_helper"

class ConcernEnumInheritanceTest < ActiveSupport::TestCase
  test "a base Concern supplies labels and stored query values to its child" do
    article = articles(:one)
    article.update(state: :live)
    reloaded = Article.find(article.id)
    assert_equal "live", reloaded.state
    assert_equal "live", reloaded[:state]
    assert_equal "live", reloaded.attributes["state"]
    assert reloaded.live?
    assert !reloaded.draft?
    assert_equal 1, Article.live_count(article.id)
    assert_equal 1, Article.where(state: 3).where(id: article.id).count
    assert_equal 0, Article.draft_count(article.id)
  end

  test "a child Concern keeps its own asymmetric enum mapping" do
    article = SpecialArticle.create!(title: "Override", body: "Long enough body", state: :shipped)
    reloaded = SpecialArticle.find(article.id)
    assert_equal "shipped", reloaded.state
    assert_equal "shipped", reloaded[:state]
    assert_equal "shipped", reloaded.attributes["state"]
    assert reloaded.shipped?
    assert !reloaded.queued?
    assert_equal 1, SpecialArticle.shipped_count(article.id)
    assert_equal 1, SpecialArticle.where(state: 7).where(id: article.id).count
    assert_equal 0, SpecialArticle.queued_count(article.id)
  end

  test "an unrelated model keeps an ordinary integer reader" do
    comment = Comment.new(state: 3)
    assert_equal 3, comment.state
    assert_equal 3, comment[:state]
    assert_equal 3, comment.attributes["state"]
    assert !comment.respond_to?(:live?)
    assert !comment.respond_to?(:shipped?)
  end
end
"#,
        )
        .run_test("test/models/concern_enum_inheritance_test.rb")
        .assert_passes();
}

/// Not left on the String: `humanize` and `titleize` are ActiveSupport reopens, answered here as ActiveSupport does.
#[test]
fn string_humanize_and_titleize_run() {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            r#"class Article < ApplicationRecord
  has_many :comments, dependent: :destroy

  def self.inflections_probe
    ["employee_salary".humanize, "author_id".humanize, "hello-world".titleize, "SSLError".titleize, "raiders_of_the_lost_ark".titleize].join("|")
  end"#,
        )
        .write(
            "test/models/article_inflections_test.rb",
            r#"require "test_helper"

class ArticleInflectionsTest < ActiveSupport::TestCase
  test "humanize and titleize answer as ActiveSupport does" do
    assert_equal "Employee salary|Author|Hello World|Ssl Error|Raiders Of The Lost Ark", Article.inflections_probe
  end
end
"#,
        )
        .run_test("test/models/article_inflections_test.rb")
        .assert_passes();
}

/// #293: `classify`, `camelize` and `dasherize` were typed as Strings and
/// emitted onto a String no runtime reopens, so the action answered 500.
/// The request is the issue's own; `issue_inflections` repeats it outside
/// a controller, `namespaced_inflections` adds the aliases, namespaces,
/// a schema prefix and the remaining Inflector bodies, and
/// `nullable_inflections` reads them on a `String | nil` receiver.
fn string_inflections_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  get \"/widgets\", to: \"widgets#index\"\n",
        )
        .write(
            "app/controllers/widgets_controller.rb",
            r#"class WidgetsController < ApplicationController
  def index
    kind = "gear_boxes"
    render plain: [kind.classify, kind.camelize, kind.singularize, kind.humanize, kind.dasherize].join(" ")
  end
end
"#,
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            r#"class Article < ApplicationRecord
  has_many :comments, dependent: :destroy

  def self.issue_inflections
    kind = "gear_boxes"
    [kind.classify, kind.camelize, kind.singularize, kind.humanize, kind.dasherize].join(" ")
  end

  def self.namespaced_inflections
    path = "admin/gear_boxes"
    name = "Admin::GearBox"
    [path.camelcase, path.classify, "shop.gear_boxes".classify, "gear_boxes".titlecase, path.dasherize,
     name.deconstantize, name.foreign_key, "Admin::APIKey".foreign_key].join("|")
  end

  def self.nullable_path(flag)
    flag ? "admin/gear_boxes" : nil
  end

  def self.nullable_name(flag)
    flag ? "Admin::GearBox" : nil
  end

  def self.nullable_inflections
    path = nullable_path(true)
    name = nullable_name(true)
    [path.camelize, path.classify, path.dasherize, name.deconstantize, name.foreign_key].join("|")
  end

  def self.nil_camelize
    nullable_path(false).camelize
  end

  def self.nil_classify
    nullable_path(false).classify
  end

  def self.nil_dasherize
    nullable_path(false).dasherize
  end

  def self.nil_deconstantize
    nullable_name(false).deconstantize
  end

  def self.nil_foreign_key
    nullable_name(false).foreign_key
  end

  def self.nil_camelcase
    nullable_path(false).camelcase
  end

  def self.nil_titlecase
    nullable_path(false).titlecase
  end"#,
        )
}

/// Rails 8.1's answers (ActiveSupport 8.1.4).
const ISSUE_INFLECTIONS: &str = "GearBox GearBoxes gear_box Gear boxes gear-boxes";
const NAMESPACED_INFLECTIONS: &str =
    "Admin::GearBoxes|Admin::GearBox|GearBox|Gear Boxes|admin/gear-boxes|Admin|gear_box_id|api_key_id";
/// A `String | nil` receiver: the String arm answers, nil raises as in Rails.
const NULLABLE_INFLECTIONS: &str = "Admin::GearBoxes|Admin::GearBox|admin/gear-boxes|Admin|gear_box_id";

#[test]
fn string_inflections_with_an_inflector_body_run() {
    string_inflections_app()
        .write(
            "test/controllers/widgets_controller_test.rb",
            &format!(
                r#"require "test_helper"

class WidgetsControllerTest < ActionDispatch::IntegrationTest
  test "the inflections answer as ActiveSupport does" do
    get "/widgets"
    assert_equal "{ISSUE_INFLECTIONS}", response.body
    assert_equal "{ISSUE_INFLECTIONS}", Article.issue_inflections
    assert_equal "{NAMESPACED_INFLECTIONS}", Article.namespaced_inflections
    assert_equal "{NULLABLE_INFLECTIONS}", Article.nullable_inflections
    assert_equal "undefined method 'camelize' for nil", assert_raises(NoMethodError) {{ Article.nil_camelize }}.message
    assert_equal "undefined method 'classify' for nil", assert_raises(NoMethodError) {{ Article.nil_classify }}.message
    assert_equal "undefined method 'dasherize' for nil", assert_raises(NoMethodError) {{ Article.nil_dasherize }}.message
    assert_equal "undefined method 'deconstantize' for nil", assert_raises(NoMethodError) {{ Article.nil_deconstantize }}.message
    assert_equal "undefined method 'foreign_key' for nil", assert_raises(NoMethodError) {{ Article.nil_foreign_key }}.message
    assert_equal "undefined method 'camelcase' for nil", assert_raises(NoMethodError) {{ Article.nil_camelcase }}.message
    assert_equal "undefined method 'titlecase' for nil", assert_raises(NoMethodError) {{ Article.nil_titlecase }}.message
  end
end
"#
            ),
        )
        .run_test("test/controllers/widgets_controller_test.rb")
        .assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn string_inflections_with_an_inflector_body_run_on_spinel() {
    let run = string_inflections_app().run_spinel(&format!(
        r#"raise "issue: " + Article.issue_inflections unless Article.issue_inflections == "{ISSUE_INFLECTIONS}"
raise "namespaced: " + Article.namespaced_inflections unless Article.namespaced_inflections == "{NAMESPACED_INFLECTIONS}"
raise "nullable: " + Article.nullable_inflections unless Article.nullable_inflections == "{NULLABLE_INFLECTIONS}"
def nil_message
  yield
  "nothing raised"
rescue NoMethodError => e
  e.message
end
raise "nil camelize" unless nil_message {{ Article.nil_camelize }} == "undefined method 'camelize' for nil"
raise "nil classify" unless nil_message {{ Article.nil_classify }} == "undefined method 'classify' for nil"
raise "nil dasherize" unless nil_message {{ Article.nil_dasherize }} == "undefined method 'dasherize' for nil"
raise "nil deconstantize" unless nil_message {{ Article.nil_deconstantize }} == "undefined method 'deconstantize' for nil"
raise "nil foreign_key" unless nil_message {{ Article.nil_foreign_key }} == "undefined method 'foreign_key' for nil"
raise "nil camelcase" unless nil_message {{ Article.nil_camelcase }} == "undefined method 'camelcase' for nil"
raise "nil titlecase" unless nil_message {{ Article.nil_titlecase }} == "undefined method 'titlecase' for nil"
puts "string inflections passed"
"#
    ));
    run.assert_passes();
    assert!(run.stdout.contains("string inflections passed"), "{}", run.stdout);
}

/// #293: `tableize` needs Rails' plural rules, which the runtime does not
/// carry (its `pluralize_word` keeps a word that ends in "s", where Rails
/// answers "statuses"), and `strip_heredoc` has no runtime body. Both are
/// refused at check time, not emitted onto a String no runtime reopens.
#[test]
fn string_extensions_without_a_runtime_body_are_refused() {
    for (call, method) in [
        (r#""Status".tableize"#, "tableize"),
        (r#""  gear\n  boxes\n".strip_heredoc"#, "strip_heredoc"),
    ] {
        let errors = emit_and_run::real_blog()
            .edit(
                "app/models/article.rb",
                "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
                &format!(
                    "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy\n\n  def self.probe\n    {call}\n  end"
                ),
            )
            .check_errors();
        let gap = format!("{method} lacks a shared runtime/lowering implementation");
        assert!(errors.iter().any(|e| e.contains(&gap)), "{method}: {errors:?}");
    }
}

/// The exact inclusion bridge and its carrier can be in different
/// reopenings. Consuming the bridge must follow the carrier's identity,
/// not whether both declarations happened to share a source file.
#[test]
fn a_reopened_factory_carrier_runs_with_its_bridge_in_either_file_order() {
    for (bridge, carrier) in [
        ("app/services/factory.rb", "app/services/factory_extension.rb"),
        ("app/services/factory_bridge.rb", "app/services/factory.rb"),
    ] {
        emit_and_run::real_blog()
            .write(bridge, r#"module Factory
  def self.included(base)
    base.extend(ClassMethods)
  end
end
"#)
            .write(carrier, r#"module Factory
  module ClassMethods
    def build(**fields)
      new(**fields).freeze
    end
  end
end
"#)
            .write("app/services/values.rb", r#"class Reading < T::Struct
  include Factory
  const :label, String
end
class Packet < T::Struct
  include Factory
  const :size, Integer
end
"#)
            .write("app/services/split_factory_consumer.rb", r#"class SplitFactoryConsumer
  def self.label
    Reading.build(label: "split").label.upcase
  end
  def self.size
    Packet.build(size: 11).size * 3
  end
end
"#)
            .run_ruby(r#"
reading = Reading.build(label: "split")
packet = Packet.build(size: 11)
raise "split reading identity" unless reading.class == Reading
raise "split reading field" unless reading.label == "split"
raise "split reading freeze" unless reading.frozen?
raise "split packet identity" unless packet.class == Packet
raise "split packet field" unless packet.size == 11
raise "split packet freeze" unless packet.frozen?
raise "split label consumer" unless SplitFactoryConsumer.label == "SPLIT"
raise "split size consumer" unless SplitFactoryConsumer.size == 33
"#)
            .assert_passes();
    }
}

/// A hook that extends its carrier and then does more, as AASM's and
/// flag_shih_tzu's do. The extend is the carrier splice's job; the rest of
/// the hook must still run for each includer.
#[test]
fn an_included_hook_that_extends_its_carrier_and_more_runs() {
    emit_and_run::real_blog()
        .write("app/services/tracking.rb", r#"module Tracking
  INCLUDERS = []

  def self.included(base)
    base.extend Tracking::ClassMethods
    INCLUDERS << base.name
  end

  module ClassMethods
    def tracked_label
      "tracked " + name
    end
  end
end
"#)
        .write("app/services/labelled.rb", r#"module Labelled
  def self.included(base)
    base.extend Labelled::ClassMethods
  end

  module ClassMethods
    def label_text
      "label " + name
    end
  end
end
"#)
        .write("app/services/reading.rb", r#"class Reading
  include Tracking
  include Labelled
end
"#)
        .run_ruby(r#"
raise "carrier method: #{Reading.tracked_label.inspect}" unless Reading.tracked_label == "tracked Reading"
raise "qualified bridge: #{Reading.label_text.inspect}" unless Reading.label_text == "label Reading"
raise "hook body: #{Tracking::INCLUDERS.inspect}" unless Tracking::INCLUDERS == ["Reading"]
"#)
        .assert_passes();
}

/// The same `included` hooks written inside the module's own
/// `class << self`, as action_policy's policy modules do. Ruby runs
/// `def included(base)` there exactly like `def self.included(base)`, so
/// each includer gets the carrier's methods and the rest of the hook
/// still runs.
#[test]
fn an_included_hook_in_class_self_that_extends_its_carrier_runs() {
    emit_and_run::real_blog()
        .write("app/services/factory.rb", r#"module Factory
  class << self
    def included(base)
      base.extend ClassMethods
    end
  end

  module ClassMethods
    def build_label
      "built " + name
    end
  end
end
"#)
        .write("app/services/tracking.rb", r#"module Tracking
  INCLUDERS = []

  class << self
    def included(base)
      base.extend ClassMethods
      INCLUDERS << base.name
    end
  end

  module ClassMethods
    def tracked_label
      "tracked " + name
    end
  end
end
"#)
        .write("app/services/reading.rb", r#"class Reading
  include Factory
  include Tracking
end
"#)
        .run_ruby(r#"
raise "bridge: #{Reading.build_label.inspect}" unless Reading.build_label == "built Reading"
raise "carrier method: #{Reading.tracked_label.inspect}" unless Reading.tracked_label == "tracked Reading"
raise "hook body: #{Tracking::INCLUDERS.inspect}" unless Tracking::INCLUDERS == ["Reading"]
"#)
        .assert_passes();
}

/// A `class << self` hook that opens `class << base` instead of extending
/// a carrier, the same shape `included_hook_class_methods_run` covers for
/// `def self.included`.
#[test]
fn an_included_hook_in_class_self_that_opens_the_includer_singleton_runs() {
    emit_and_run::real_blog()
        .write("app/services/factory.rb", r#"module Factory
  class << self
    def included(base)
      class << base
        def build_label
          "built " + name
        end
      end
    end
  end
end
"#)
        .write("app/services/reading.rb", "class Reading\n  include Factory\nend\n")
        .write("app/services/reading_label.rb", "class ReadingLabel\n  def self.text = Reading.build_label.upcase\nend\n")
        .run_ruby(r#"
raise "includer method: #{Reading.build_label.inspect}" unless Reading.build_label == "built Reading"
raise "typed caller: #{ReadingLabel.text.inspect}" unless ReadingLabel.text == "BUILT READING"
"#)
        .assert_passes();
}

/// The `class << self` bridge and its `ClassMethods` carrier in different
/// reopenings, in either file order.
#[test]
fn a_class_self_included_bridge_runs_with_its_carrier_in_another_file() {
    for (bridge, carrier) in [
        ("app/services/factory.rb", "app/services/factory_extension.rb"),
        ("app/services/factory_bridge.rb", "app/services/factory.rb"),
    ] {
        emit_and_run::real_blog()
            .write(bridge, r#"module Factory
  class << self
    def included(base)
      base.extend ClassMethods
    end
  end
end
"#)
            .write(carrier, r#"module Factory
  module ClassMethods
    def build_label
      "built " + name
    end
  end
end
"#)
            .write("app/services/reading.rb", "class Reading\n  include Factory\nend\n")
            .run_ruby(r#"
raise "split bridge: #{Reading.build_label.inspect}" unless Reading.build_label == "built Reading"
"#)
            .assert_passes();
    }
}

/// `resources :x, only: [] do … end` nests routes under a parent with no
/// routes of its own. Ingest rejected the empty list and dropped the
/// parent with every route nested in it.
#[test]
fn nested_routes_under_an_only_empty_parent_run() {
    emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "  resources :articles do\n    resources :comments, only: [:create, :destroy]\n  end\n",
            "  resources :articles\n  resources :articles, only: [] do\n    resources :comments, only: [:create, :destroy]\n  end\n",
        )
        .run_test("test/controllers/comments_controller_test.rb")
        .assert_passes();
}

/// A collection render with `as: :for` emitted a
/// positional `for` param and a `|for|` block param.
#[test]
fn a_collection_render_with_a_reserved_word_as_local_runs() {
    let run = on_the_index(
        emit_and_run::real_blog().write(
            "app/views/articles/_row.html.erb",
            "<li class=\"for-row\"><%= binding.local_variable_get(:for).title %></li>\n",
        )
        .write(
            "app/views/articles/_assigns_row.html.erb",
            "<li class=\"for-assigns-row\"><%= local_assigns[:for].title %></li>\n",
        ),
        "<%= render partial: \"row\", collection: @articles, as: :for %>\n\
         <%= render partial: \"assigns_row\", collection: @articles, as: :for %>\n",
        "    assert_select \"li.for-row\", Article.count\n    \
             assert_select \"li.for-assigns-row\", Article.count\n",
    );
    run.assert_passes();
}

/// Not `module ApplicationController`, which cannot load beside the controller's own `class ApplicationController`: a class nested in a controller reopens the controller as a class.
#[test]
fn a_class_nested_in_a_controller_loads() {
    emit_and_run::real_blog()
        .edit(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\n",
            "class ApplicationController < ActionController::Base\n  class Failure < StandardError\n    attr_reader :status\n\n    def initialize(status)\n      @status = status\n      super(\"failed with #{status}\")\n    end\n  end\n\n",
        )
        .edit(
            "app/controllers/articles_controller.rb",
            "class ArticlesController < ApplicationController\n",
            "class ArticlesController < ApplicationController\n  class Missing < StandardError\n  end\n\n",
        )
        .write(
            "test/models/article_nested_class_test.rb",
            r#"require "test_helper"

class ArticleNestedClassTest < ActiveSupport::TestCase
  test "a class nested in a controller is the one the source declared" do
    failure = ApplicationController::Failure.new(404)
    assert_equal 404, failure.status
    assert_equal "failed with 404", failure.message
    assert_kind_of StandardError, ArticlesController::Missing.new
    assert ArticlesController < ApplicationController
  end
end
"#,
        )
        .run_test("test/models/article_nested_class_test.rb")
        .assert_passes();
}

#[test]
fn safe_navigation_comparisons_execute_for_nil_and_string_values() {
    emit_and_run::real_blog()
        .edit("app/models/article.rb", "class Article < ApplicationRecord\n", r#"class Article < ApplicationRecord
  def title_long?
    ((title && title.length) || 0) > 1
  end
  def title_short?
    (title&.length || 0) < 1
  end
"#)
        .run_ruby(r#"
article = Article.new(title: "long")
raise "truthy chain" unless article.title_long? && !article.title_short?
article.title = nil
raise "nil chain" unless !article.title_long? && article.title_short?
"#)
        .assert_passes();
}

#[test]
fn forwarded_proc_expressions_execute_once_in_an_emitted_app() {
    emit_and_run::real_blog()
        .write("app/services/block_forward_probe.rb", r#"class BlockForwardProbe
  def initialize
    @calls = 0
    @callback = ->(x) { x * 2 }
  end
  def compute(n)
    @calls += 1
    ->(x) { x + n }
  end
  def run
    doubled = [1, 2].map(&@callback)
    added = [1, 2].map(&compute(3))
    [doubled, added, @calls]
  end
end
"#)
        .run_ruby("raise 'forwarded expression' unless BlockForwardProbe.new.run == [[2, 4], [4, 5], 1]")
        .assert_passes();
}

#[test]
fn typed_instance_keywords_bind_values_and_keep_positional_hashes() {
    emit_and_run::real_blog()
        .write("app/services/keyword_fetcher.rb", r##"
class KeywordFetcher
  def fetch(url, ip: url.upcase)
    "#{url}@#{ip}"
  end

  def merge(url, opts = {})
    "#{url}#{opts}"
  end
end
"##)
        .write("app/services/keyword_locator.rb", r#"
class KeywordLocator
  def locate(url)
    KeywordFetcher.new.fetch(url, ip: "192.0.2.1")
  end

  def merged(url)
    KeywordFetcher.new.merge(url, opts: 1)
  end
end
"#)
        .run_ruby(r#"
locator = KeywordLocator.new
raise "keyword bound to hash" unless locator.locate("host") == "host@192.0.2.1"
raise "positional hash rewritten" unless locator.merged("host") == 'host{opts: 1}'
"#)
        .assert_passes();
}

#[test]
fn multiple_erb_openers_execute_inside_an_output_block() {
    on_the_index(emit_and_run::real_blog(), r#"<span class="multi-opener"><%= capture do %>
<% [1, 2].each do |number|
       unless number.nil? %><%= number %><% end %><% end %><% end %></span>"#,
        "    assert_select \"span.multi-opener\", \"12\"\n")
        .assert_passes();
}

#[test]
fn trailing_erb_comments_execute_without_swallowing_output_terminators() {
    on_the_index(emit_and_run::real_blog(), r#"<span class="commented-title"><%= capture do %>
<% [1, 2].each do |number| %><%= "n: #{number}" #@label %><% end #$numbers %><% end #{capture} %></span>"#,
        "    assert_select \"span.commented-title\", \"n: 1n: 2\"\n")
        .assert_passes();
}

/// `cookies.permanent` in each spelling Rails accepts, over one plain
/// write as the control. The permanent jar was the identity until
/// 2026-10-06, so these went out with no Expires and ended with the
/// browser session: campfire's sign-in did not survive a restart.
fn permanent_cookie_app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write(
            "app/controllers/visits_controller.rb",
            r#"class VisitsController < ApplicationController
  def index
    cookies.permanent[:last_room] = 7
    cookies.signed.permanent[:session_token] = { value: "tok", httponly: true, same_site: :lax }
    cookies.permanent.signed[:remember] = "me"
    cookies[:plain] = "p"
    head :no_content
  end
end
"#,
        )
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :visits, only: :index\nend\n",
        )
        .write("app/models/visit.rb", "class Visit < ApplicationRecord\nend\n")
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema[8.1].define(version: 2026_01_01_000000) do\n  create_table \"visits\", force: :cascade do |t|\n    t.string \"room\"\n  end\nend\n",
        )
}

#[test]
fn permanent_cookies_go_out_with_an_expiry() {
    permanent_cookie_app()
        .run_ruby(
            r##"status, headers, = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => "/visits", "QUERY_STRING" => "", "rack.input" => StringIO.new(""))
raise "GET /visits answered #{status}" unless status == 204
lines = headers["set-cookie"] || []
year = (Time.now.utc.year + 20).to_s
%w[last_room session_token remember].each do |name|
  line = lines.find { |l| l.start_with?("#{name}=") } or raise "no Set-Cookie for #{name}: #{lines.inspect}"
  raise "#{name} has no twenty-year Expires: #{line}" unless line =~ /; Expires=\w{3}, \d{2} \w{3} #{year} \d{2}:\d{2}:\d{2} GMT/
end
plain = lines.find { |l| l.start_with?("plain=") } or raise "no Set-Cookie for plain: #{lines.inspect}"
raise "a plain cookie must stay a session cookie: #{plain}" if plain.include?("Expires")
session = lines.find { |l| l.start_with?("session_token=") }
raise "options still apply under permanent: #{session}" unless session.include?("SameSite=Lax") && session.include?("HttpOnly")
puts "permanent cookies passed"
"##,
        )
        .assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn permanent_cookies_record_an_expiry_on_spinel() {
    permanent_cookie_app()
        .run_spinel(
            r##"controller = VisitsController.new
controller.process_action(:index)
jar = controller.cookies
year = (Time.now.utc.year + 20).to_s
["last_room", "session_token", "remember"].each do |name|
  exp = jar.flag_expires(name)
  raise "#{name} has no twenty-year expiry: #{exp.inspect}" unless exp.split(" ")[3] == year && exp.end_with?(" GMT")
end
raise "a plain cookie must stay a session cookie" unless jar.flag_expires("plain") == ""
raise "the signed permanent value must round-trip" unless jar.signed[:session_token] == "tok"
puts "permanent cookies passed"
"##,
        )
        .assert_passes();
}

/// Not the scaffold blog's `app/views.rb`, whose requires name views this tree does not have: an app with no views boots and answers a request (#164).
#[test]
fn an_app_with_no_views_boots() {
    emit_and_run::empty_app()
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write(
            "app/controllers/widgets_controller.rb",
            "class WidgetsController < ApplicationController\n  def index\n    head :no_content\n  end\nend\n",
        )
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n")
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\n  root \"widgets#index\"\n  resources :widgets, only: :index\nend\n",
        )
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema[8.1].define(version: 2026_01_01_000000) do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n",
        )
        .run_ruby(
            r#"status, = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => "/widgets", "QUERY_STRING" => "", "rack.input" => StringIO.new(""))
raise "GET /widgets answered #{status}" unless status == 204
"#,
        )
        .assert_passes();
}

#[test]
fn bundled_uri_and_http_exception_constants_run() {
    let run = emit_and_run::real_blog()
        .write(
            "app/services/http_constant_probe.rb",
            r#"class HttpConstantProbe
  def self.http?(url)
    URI.parse(url).is_a?(URI::HTTP)
  end

  def self.https?(url)
    URI.parse(url).is_a?(URI::HTTPS)
  end

  def self.invalid_uri
    begin
      URI.parse("https://bad host/")
    rescue URI::InvalidURIError
      "invalid"
    end
  end

  def self.construct
    URI::HTTP.new("http", nil, "example.test", 80, nil, "/", nil, nil, nil).to_s
  end

  def self.construct_https
    URI::HTTPS.new("https", nil, "example.test", 443, nil, "/", nil, nil, nil).to_s
  end

  def self.invalid_constructor
    begin
      URI::HTTP.new
    rescue ArgumentError
      "arity"
    end
  end

  def self.timeout(kind)
    begin
      if kind == "open"
        raise Net::OpenTimeout
      else
        raise Net::ReadTimeout
      end
    rescue Net::OpenTimeout
      "open"
    rescue Net::ReadTimeout
      "read"
    end
  end
end
"#,
        )
        .run_ruby(
            r#"raise unless HttpConstantProbe.http?("https://example.test/")
raise if HttpConstantProbe.http?("ftp://example.test/")
raise unless HttpConstantProbe.https?("https://example.test/")
raise if HttpConstantProbe.https?("http://example.test/")
raise unless HttpConstantProbe.invalid_uri == "invalid"
raise unless HttpConstantProbe.construct == "http://example.test/"
raise unless HttpConstantProbe.construct_https == "https://example.test/"
raise unless HttpConstantProbe.invalid_constructor == "arity"
raise unless HttpConstantProbe.timeout("open") == "open"
raise unless HttpConstantProbe.timeout("read") == "read"
"#,
        );
    let probe = std::fs::read_to_string(run.emitted.join("app/models/http_constant_probe.rb")).unwrap();
    assert!(probe.lines().any(|line| line == "require \"uri\""), "{probe}");
    run.assert_passes();
}

#[test]
fn bundled_response_io_json_and_runtime_value_constants_run() {
    emit_and_run::real_blog()
        .write("app/services/bundled_value_probe.rb", r#"class BundledValueProbe
  def self.responses
    [Net::HTTPOK.new("1.1", "200", "OK").is_a?(Net::HTTPOK),
     Net::HTTPRedirection.new("1.1", "302", "Found").is_a?(Net::HTTPRedirection)]
  end
  def self.buffer
    io = StringIO.new
    io << "abc"
    io.string
  end
  def self.encode
    JSON.generate([17, "hello"])
  end
  def self.ssl_error
    begin
      raise OpenSSL::OpenSSLError
    rescue OpenSSL::OpenSSLError
      "ssl"
    end
  end
  def self.attributes
    ActionText::Attachment::ATTRIBUTES.include?("sgid")
  end
  def self.allowed_tags
    Rails::HTML5::SafeListSanitizer.allowed_tags.include?("a")
  end
end
"#)
        .run_ruby(r#"raise unless BundledValueProbe.responses == [true, true]
raise unless BundledValueProbe.buffer == "abc"
raise unless BundledValueProbe.encode == '[17,"hello"]'
raise unless BundledValueProbe.ssl_error == "ssl"
raise unless BundledValueProbe.attributes
raise unless BundledValueProbe.allowed_tags
"#)
        .assert_passes();
}

/// Not `user || raise NotFound` (a syntax error) or `a && self.x = v && b` (assigns `v && b`): a command or a method assignment as an `&&`/`||` operand keeps its parentheses.
#[test]
fn a_command_operand_of_a_boolean_operator_keeps_its_parentheses() {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            r#"class Article < ApplicationRecord
  has_many :comments, dependent: :destroy

  def self.find_or_fail(id)
    find_by(id: id) || (raise ActiveRecord::RecordNotFound, "no article #{id}")
  end

  def retitle(text, persist)
    text.present? && (self.title = text) && persist && save
  end"#,
        )
        .write(
            "test/models/article_guard_test.rb",
            r#"require "test_helper"

class ArticleGuardTest < ActiveSupport::TestCase
  test "a raise operand runs only when the left operand is nil" do
    article = articles(:one)
    assert_equal article.id, Article.find_or_fail(article.id).id
    assert_raises(ActiveRecord::RecordNotFound) { Article.find_or_fail(-1) }
  end

  test "a setter operand assigns its own argument" do
    article = articles(:one)
    assert_equal false, article.retitle("Retitled", false)
    assert_equal "Retitled", article.title
  end
end
"#,
        )
        .run_test("test/models/article_guard_test.rb")
        .assert_passes();
}

/// Not `out = ""`, a literal spinel freezes: `+""` stays an unfrozen copy that the method can append to.
#[test]
fn a_mutable_string_literal_stays_mutable() {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            r##"class Article < ApplicationRecord
  has_many :comments, dependent: :destroy

  def initials
    out = +""
    title.split.each { |word| out << word[0] }
    out
  end

  def hashtag
    tag = +"#"
    tag << title.downcase.delete(" ")
  end"##,
        )
        .write(
            "test/models/article_mutable_literal_test.rb",
            r##"require "test_helper"

class ArticleMutableLiteralTest < ActiveSupport::TestCase
  test "an unfrozen copy of a literal can be appended to" do
    article = Article.new(title: "Hello World")
    assert_equal "HW", article.initials
    assert_equal "#helloworld", article.hashtag
  end
end
"##,
        )
        .run_test_frozen("test/models/article_mutable_literal_test.rb")
        .assert_passes();
}

/// The key forms Rails' PostgreSQL schema dumper writes run. `id:
/// :serial` stopped ingest ("unsupported type `serial`"), and `id: {
/// type: :string, limit: 32 }` was read as the default key, so the
/// emitted table had an integer autoincrement key where the app keeps
/// string ones.
#[test]
fn postgres_dumped_key_forms_run() {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "create_table \"articles\", force: :cascade do |t|",
            "create_table \"articles\", id: :serial, force: :cascade do |t|",
        )
        .edit(
            "db/schema.rb",
            "  add_foreign_key \"comments\", \"articles\"",
            "  create_table \"codes\", id: { type: :string, limit: 32 }, force: :cascade do |t|\n    \
             t.string \"label\"\n  end\n\n  add_foreign_key \"comments\", \"articles\"",
        )
        .write("app/models/code.rb", "class Code < ApplicationRecord\nend\n")
        .write(
            "test/models/key_forms_test.rb",
            r#"require "test_helper"

class KeyFormsTest < ActiveSupport::TestCase
  test "a serial key is generated" do
    article = Article.create!(title: "Serial", body: "A long enough body")
    assert_kind_of Integer, article.id
    assert_equal "Serial", Article.find(article.id).title
  end

  test "a string key is the one the app supplies" do
    Code.create!(id: "launch-2026", label: "Launch")
    assert_equal "Launch", Code.find("launch-2026").label
  end
end
"#,
        )
        .run_test("test/models/key_forms_test.rb")
        .assert_passes();
}

/// A unique index with `where:` constrains only the rows its predicate
/// selects. Ingest dropped the predicate, so the emitted DDL made the
/// index unique over every row: an archived article's title stayed
/// taken, `create!` raised `RecordNotUnique`, and `check` said nothing.
/// `insert_all`'s guard follows the predicate too: it skips a row only
/// when a row the index covers shares its key (`title` is `null:
/// false`, so the guard reads the index).
#[test]
fn a_partial_unique_index_constrains_only_the_rows_it_selects() {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "create_table \"articles\", force: :cascade do |t|\n    t.string \"title\"\n",
            "create_table \"articles\", force: :cascade do |t|\n    t.string \"title\", null: false\n    \
             t.datetime \"archived_at\"\n    \
             t.index [\"title\"], name: \"index_articles_on_live_title\", unique: true, where: \"(archived_at IS NULL)\"\n",
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            r#"class Article < ApplicationRecord
  has_many :comments, dependent: :destroy

  def self.import_titles(first, second, body)
    Article.insert_all([{ title: first, body: body }, { title: second, body: body }])
  end"#,
        )
        .write(
            "test/models/article_live_title_test.rb",
            r#"require "test_helper"

class ArticleLiveTitleTest < ActiveSupport::TestCase
  test "an archived article's title can be reused" do
    old = Article.create!(title: "Reused", body: "A long enough body")
    old.update!(archived_at: Time.now)
    assert Article.create!(title: "Reused", body: "A long enough body").persisted?
  end

  test "two live articles still cannot share a title" do
    Article.create!(title: "Shared", body: "A long enough body")
    assert_raises(ActiveRecord::RecordNotUnique) do
      Article.create!(title: "Shared", body: "A long enough body")
    end
  end

  test "insert_all skips a live duplicate and adds a row only archived ones share" do
    Article.create!(title: "Taken", body: "A long enough body")
    Article.create!(title: "Imported", body: "A long enough body", archived_at: Time.now)
    Article.import_titles("Taken", "Imported", "A long enough body")
    assert_equal 1, Article.where(title: "Taken").count
    assert_equal 1, Article.where(title: "Imported", archived_at: nil).count
  end
end
"#,
        )
        .run_test("test/models/article_live_title_test.rb")
        .assert_passes();
}

/// `upsert_all(unique_by:)` names the index Rails' `InsertAll` picks:
/// the first unique index by name with those columns, in any order. A
/// partial one goes into the conflict target with its `WHERE`, without
/// which SQLite matches no partial index; a full one goes in bare.
#[test]
fn upsert_all_targets_the_unique_index_rails_picks() {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "  add_foreign_key \"comments\", \"articles\"",
            r#"  create_table "slugs", force: :cascade do |t|
    t.string "name", null: false
    t.integer "hits", default: 0, null: false
    t.datetime "retired_at"
    t.index ["name"], name: "index_slugs_on_live_name", unique: true, where: "(retired_at IS NULL)"
  end

  create_table "seats", force: :cascade do |t|
    t.bigint "room_id", null: false
    t.bigint "user_id", null: false
    t.integer "visits", default: 0, null: false
    t.datetime "left_at"
    t.index ["room_id", "user_id"], name: "index_seats_on_live_room_and_user", unique: true, where: "(left_at IS NULL)"
  end

  create_table "tags", force: :cascade do |t|
    t.string "name", null: false
    t.integer "uses", default: 0, null: false
    t.datetime "retired_at"
    t.index ["name"], name: "index_tags_on_name", unique: true
    t.index ["name"], name: "index_tags_on_name_live", unique: true, where: "(retired_at IS NULL)"
  end

  add_foreign_key "comments", "articles""#,
        )
        .write("app/models/slug.rb", "class Slug < ApplicationRecord\nend\n")
        .write("app/models/seat.rb", "class Seat < ApplicationRecord\nend\n")
        .write("app/models/tag.rb", "class Tag < ApplicationRecord\nend\n")
        .write(
            "test/models/upsert_target_test.rb",
            r#"require "test_helper"

class UpsertTargetTest < ActiveSupport::TestCase
  test "a partial index: updates the live row, then inserts past a retired one" do
    Slug.upsert_all([{ name: "home", hits: 1 }], unique_by: :name)
    Slug.upsert_all([{ name: "home", hits: 2 }], unique_by: :name)
    assert_equal [2], Slug.where(name: "home").pluck(:hits)
    Slug.where(name: "home").update_all(retired_at: Time.now)
    Slug.upsert_all([{ name: "home", hits: 3 }], unique_by: :name)
    assert_equal [3], Slug.where(name: "home", retired_at: nil).pluck(:hits)
    assert_equal 2, Slug.where(name: "home").where.not(retired_at: nil).pluck(:hits).first
  end

  test "a composite partial index: unique_by in either order" do
    Seat.upsert_all([{ room_id: 1, user_id: 7, visits: 1 }], unique_by: [:room_id, :user_id])
    Seat.upsert_all([{ room_id: 1, user_id: 7, visits: 2 }], unique_by: [:user_id, :room_id])
    assert_equal [2], Seat.where(room_id: 1, user_id: 7).pluck(:visits)
  end

  test "a full index first by name: the bare conflict target" do
    Tag.upsert_all([{ name: "rails", uses: 1 }], unique_by: :name)
    Tag.where(name: "rails").update_all(retired_at: Time.now)
    Tag.upsert_all([{ name: "rails", uses: 2 }], unique_by: :name)
    assert_equal [2], Tag.where(name: "rails").pluck(:uses)
  end
end
"#,
        )
        .run_test("test/models/upsert_target_test.rb")
        .assert_passes();
}

/// The blog with its schema as a `db/structure.sql` from pg_dump 18 runs.
/// The dump opens with `\restrict <key>` and closes with `\unrestrict
/// <key>`, psql meta-commands pg_dump writes since 18 and the August 2025
/// minor releases; Rails before 7.2.3 and 8.0.3 keeps them. Ingest read
/// each as the head of an unmodeled statement and stopped. The dump is
/// the blog's `db/schema.rb` loaded into PostgreSQL 18.3 and dumped with
/// Rails' flags (`--schema-only --no-privileges --no-owner`), with the
/// `SET search_path` and `schema_migrations` lines Rails appends.
#[test]
fn a_structure_sql_from_pg_dump_18_runs() {
    emit_and_run::real_blog()
        .remove("db/schema.rb")
        .write("db/structure.sql", include_str!("support/real_blog_structure.sql"))
        .run_test("test/controllers/articles_controller_test.rb")
        .assert_passes();
}

/// An app that declares no `root` boots and dispatches (#165). `main.rb`
/// composed its route table as `[RouteTable.root] + RouteTable.table`,
/// and the routes emit defines `RouteTable.root` only for a route at
/// `/`, so the first request raised NoMethodError. `/` itself answers
/// 404, as it does in Rails when no route matches.
#[test]
fn an_app_with_no_root_route_dispatches() {
    emit_and_run::empty_app()
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write(
            "app/controllers/widgets_controller.rb",
            r#"class WidgetsController < ApplicationController
  before_action :set_widget, only: :show

  def index
    head :no_content
  end

  def show
    head :not_found unless @widget
  end

  private

  def set_widget
    @widget = Widget.find_by(id: params[:id])
  end
end
"#,
        )
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n")
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :widgets, only: %i[index show]\nend\n",
        )
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema[8.1].define(version: 2026_01_01_000000) do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n",
        )
        .run_ruby(
            r#"def get(path)
  status, = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => path, "QUERY_STRING" => "", "rack.input" => StringIO.new(""))
  status
end
{ "/widgets" => 204, "/widgets/1" => 404, "/" => 404 }.each do |path, want|
  got = get(path)
  raise "GET #{path} answered #{got}, want #{want}" unless got == want
end
"#,
        )
        .assert_passes();
}

/// `raise ActionController::RoutingError` in an action answers 404, as
/// Rails' `ActionDispatch::ExceptionWrapper` maps it to `:not_found`. A
/// Rails app raises it for a page number out of bounds. Rails' second
/// constructor argument, `failures`, is optional. The raise is in
/// `index`, which has no `before_action`, so a `RecordNotFound` from
/// `set_article` cannot answer the 404 in its place.
#[test]
fn an_action_that_raises_routing_error_answers_404() {
    emit_and_run::real_blog()
        .edit(
            "app/controllers/articles_controller.rb",
            "  def index\n",
            "  def index\n    raise ActionController::RoutingError.new(\"page out of bounds\") if params[:page] == \"0\"\n    raise ActionController::RoutingError.new(\"No route matches\", []) if params[:page] == \"-1\"\n",
        )
        .run_ruby(
            r#"def get(path, query)
  status, = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => path, "QUERY_STRING" => query, "rack.input" => StringIO.new(""))
  status
end
{ "page=0" => 404, "page=-1" => 404, "page=1" => 200, "" => 200 }.each do |query, want|
  got = get("/articles", query)
  raise "GET /articles?#{query} answered #{got}, want #{want}" unless got == want
end
"#,
        )
        .assert_passes();
}

#[path = "emit_and_run/concern_accessors.rs"]
mod concern_accessors;

#[path = "emit_and_run/action_not_found.rs"]
mod action_not_found;

#[path = "emit_and_run/finder_miss_readers.rs"]
mod finder_miss_readers;

#[path = "emit_and_run/finder_miss_messages.rs"]
mod finder_miss_messages;

/// A concern split in two, mixed into more than one controller: the
/// inner module calls a method only its includers have (through the
/// outer one). With several includers `self` in the inner module is the
/// module, the bare call fell to `untyped`, and so did everything read
/// from it — the Rails authentication generator's `Current.user` shape.
#[test]
fn a_concern_calls_a_method_every_includer_gets_from_a_sibling() {
    emit_and_run::real_blog()
        .write(
            "app/controllers/concerns/featuring.rb",
            r#"module Featuring
  extend ActiveSupport::Concern

  def featured_title
    featured_article&.title
  end
end
"#,
        )
        .write(
            "app/controllers/concerns/browsing.rb",
            r#"module Browsing
  extend ActiveSupport::Concern

  include Featuring

  private

  def featured_article
    Article.order(:id).first
  end
end
"#,
        )
        .edit(
            "app/controllers/articles_controller.rb",
            "class ArticlesController < ApplicationController\n",
            "class ArticlesController < ApplicationController\n  include Browsing\n",
        )
        .edit(
            "app/controllers/comments_controller.rb",
            "class CommentsController < ApplicationController\n",
            "class CommentsController < ApplicationController\n  include Browsing\n",
        )
        .edit(
            "app/controllers/articles_controller.rb",
            "    @articles = Article.includes(:comments).order(created_at: :desc)\n",
            "    @articles = Article.includes(:comments).order(created_at: :desc)\n    @featured = featured_title.to_s.upcase\n",
        )
        .run_test("test/controllers/articles_controller_test.rb")
        .assert_passes();
}

/// Gap #18.1: Tim Tischler's trailing keyword-hash enum runtime pin.
#[test]
fn enum_keyword_hash_mapping_predicate_runs() {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "create_table \"articles\", force: :cascade do |t|",
            "create_table \"articles\", force: :cascade do |t|\n    t.string \"kind\", default: \"kind\", null: false\n    t.integer \"priority\", default: 17, null: false",
        )
        .edit(
            "app/models/article.rb",
            "has_many :comments, dependent: :destroy\n",
            "has_many :comments, dependent: :destroy\n\n  enum :kind, kind: 'kind', other: 'other'\n  enum :priority, pending: 17, priority: 41\n",
        )
        .edit(
            "app/views/articles/show.html.erb",
            "<h1 class=\"font-bold text-4xl\"><%= @article.title %></h1>",
            "<h1 class=\"font-bold text-4xl\"><%= @article.title %></h1>\n  <p id=\"kind-predicate\"><%= @article.kind? %></p>\n  <p id=\"priority-predicate\"><%= @article.priority? %></p>",
        )
        .run_ruby(r#"
article = Article.create!(title: "Enum control", body: "A sufficiently long body")
raise "enum default predicate is false" unless article.kind?
article.kind = "other"
raise "enum predicate ignored its value" if article.kind?
raise "bare integer mapping lost stored values" unless Article.priorities == {"pending" => 17, "priority" => 41}
raise "integer enum default label is wrong" unless article.pending?
raise "column predicate shadowed enum comparison" if article.priority?
article.priority = 41
raise "integer label predicate is false" unless article.priority?
raise "integer label predicate ignored its value" if article.pending?
"#)
        .assert_passes();
}

#[test]
fn anonymous_keywords_run_without_capturing_user_bindings() {
    const METHODS: &str = r#"
  def self.pr197_forward(label, __fwd_kwargs, **)
    [label - __fwd_kwargs, pr197_sink(**)]
  end
  def self.pr197_sink(factor:, offset: 2)
    factor * 3 + offset
  end
  def self.pr197_local_collision(**)
    __fwd_kwargs = {factor: 99}
    pr197_sink(**)
  end
  def pr197_instance(label, __fwd_kwargs, **)
    [label - __fwd_kwargs, pr197_instance_sink(**)]
  end
  def pr197_instance_sink(factor:, offset: 2)
    factor * 3 + offset
  end
"#;
    let library = format!("class KeywordProbe\n{METHODS}end\n");
    emit_and_run::real_blog()
        .write("app/services/keyword_probe.rb", &library)
        .edit("app/models/article.rb", "class Article < ApplicationRecord\n",
            &format!("class Article < ApplicationRecord\n{METHODS}"))
        .run_ruby(r#"
[Article, KeywordProbe].each do |owner|
  raise "keyword packet captured a local" unless owner.pr197_local_collision(factor: 7) == 23
  raise "class keyword packet captured a positional" unless owner.pr197_forward(11, 4, factor: 7, offset: 5) == [7, 26]
  raise "class keyword default lost" unless owner.pr197_forward(11, 4, factor: 7) == [7, 23]
  instance = owner.new
  raise "instance keyword packet captured a positional" unless instance.pr197_instance(11, 4, factor: 7, offset: 5) == [7, 26]
  raise "instance keyword default lost" unless instance.pr197_instance(11, 4, factor: 7) == [7, 23]
  begin
    owner.pr197_forward(11, 4)
    raise "missing required keyword accepted"
  rescue ArgumentError
  end
end
"#).assert_passes();
}

#[test]
fn anonymous_keywords_forward_empty_and_false_values_through_super() {
    emit_and_run::real_blog()
        .write("app/services/keyword_parent.rb", r#"class KeywordParent
  def call(factor: false, offset: nil, **)
    [factor, offset]
  end
end
class KeywordChild < KeywordParent
  def call(**)
    super(**)
  end
end
"#)
        .run_ruby(r#"
probe = KeywordChild.new
raise "empty keyword packet changed defaults" unless probe.call == [false, nil]
raise "false or nil keyword was dropped" unless probe.call(factor: nil, offset: false) == [nil, false]
"#).assert_passes();
}

#[test]
fn destructuring_preserves_user_bindings_and_expression_values() {
    const TEMPLATE: &str = r#"class HygieneProbe
  def self.targets
    a, *TARGET, c = [11, 22, 33]
    [a, TARGET, c]
  end
  def self.scope(PARAM)
    a, *middle, c = [11, 22, 33]
    [a, middle, c, PARAM]
  end
  def self.expression
    (a, *middle, c = [11, 22, 33, 44])
  end
  def self.instance_targets
    @a, *@middle, @c = [*[11, 22], 33, 44]
    [@a, @middle, @c]
  end
end
"#;
    // Deliberately collide with both span-derived stems. Parameter names
    // change later offsets, so settle the source before ingesting it.
    let mut target = "__target".to_string();
    let mut param = "__param".to_string();
    let source = loop {
        let source = TEMPLATE.replace("TARGET", &target).replace("PARAM", &param);
        let next_target = format!("__mw_{}", source.find("a, *").unwrap());
        let next_param = format!("__mw_{}", source.find("a, *middle").unwrap());
        if target == next_target && param == next_param { break source }
        target = next_target;
        param = next_param;
    };
    const ASSERTIONS: &str = r#"
raise "temporary captured rest target" unless HygieneProbe.targets == [11, [22], 33]
raise "temporary captured a parameter" unless HygieneProbe.scope(41) == [11, [22], 33, 41]
raise "assignment expression lost RHS" unless HygieneProbe.expression == [11, 22, 33, 44]
raise "ivar targets or array splat changed" unless HygieneProbe.instance_targets == [11, [22, 33], 44]
"#;
    let native = std::process::Command::new("ruby").arg("-e")
        .arg(format!("{source}\n{ASSERTIONS}"))
        .output().expect("CRuby control");
    assert!(native.status.success(), "{}", String::from_utf8_lossy(&native.stderr));
    emit_and_run::real_blog().write("app/services/hygiene_probe.rb", &source)
        .run_ruby(ASSERTIONS).assert_passes();
}

#[test]
fn post_rest_destructuring_handles_short_arrays_and_evaluates_once() {
    const SOURCE: &str = r#"class DestructureProbe
  def self.first_value
    @calls ||= 0
    @calls = @calls + 1
    11
  end
  def self.short
    a, *b, c, d = [first_value, 22]
    [a, b, c, d, @calls]
  end
  def self.empty
    a, *b, c, d = []
    [a, b, c, d]
  end
  def self.one
    a, *b, c, d = [11]
    [a, b, c, d]
  end
  def self.exact
    a, *b, c, d = [11, 22, 33]
    [a, b, c, d]
  end
  def self.long
    a, *b, c, d = [11, 22, 33, 44, 55]
    [a, b, c, d]
  end
  def self.discard
    a, *, c, d = [11, 22]
    [a, c, d]
  end
end
"#;
    const ASSERTIONS: &str = r##"
expected = {short: [11, [], 22, nil, 1], empty: [nil, [], nil, nil], one: [11, [], nil, nil], exact: [11, [], 22, 33], long: [11, [22, 33], 44, 55], discard: [11, 22, nil]}
expected.each do |method, want|
  got = DestructureProbe.public_send(method)
  raise "#{method}: #{got.inspect}, expected #{want.inspect}" unless got == want
end
"##;
    let native = std::process::Command::new("ruby").arg("-e")
        .arg(format!("{SOURCE}\n{ASSERTIONS}"))
        .output().expect("CRuby control");
    assert!(native.status.success(), "{}", String::from_utf8_lossy(&native.stderr));
    emit_and_run::real_blog()
        .write("app/services/destructure_probe.rb", SOURCE)
        .run_ruby(ASSERTIONS).assert_passes();
}

#[test]
fn class_variable_compound_writes_share_the_read_storage() {
    emit_and_run::real_blog()
        .write("app/services/counter_probe.rb", r#"class CounterProbe
  @@count = nil
  def next_value
    @@count ||= 11
    @@count = @@count + 3
    @@count
  end
  def operators
    @@count += 7
    @@count -= 3
    @@count &&= @@count + 2
    @@count
  end
  def skip
    @@count = false
    @@count &&= explode
    @@count
  end
  def explode
    raise "short circuit evaluated RHS"
  end
  def self.current
    @@count
  end
end
class CounterChild < CounterProbe
end
"#)
        .run_ruby(r#"
raise "native nil initializer was dropped" unless CounterProbe.current.nil? && CounterChild.current.nil?
raise "compound write and read used different storage" unless CounterChild.new.next_value == 14
raise "class reader used per-class storage" unless CounterProbe.current == 14 && CounterChild.current == 14
raise "class variable storage split across inheritance" unless CounterProbe.new.next_value == 17
raise "class reader lost the shared update" unless CounterProbe.current == 17 && CounterChild.current == 17
raise "compound operators changed" unless CounterChild.new.operators == 23
raise "operator storage split across inheritance" unless CounterProbe.current == 23
raise "false RHS was evaluated" unless CounterProbe.new.skip == false
raise "shared false storage lost" unless CounterChild.current == false
"#).assert_passes();
}

#[test]
fn defined_guards_and_source_literals_run_after_emission() {
    emit_and_run::real_blog()
        .write("app/services/guard_probe.rb", r#"class GuardProbe
  VALUE = 11
  def self.constants
    [defined?(GuardProbe), defined?(GuardProbe::VALUE), defined?(MissingPr197), defined?(GuardProbe::MissingPr197)]
  end
  def self.uninvoked
    raise "defined? invoked its terminal method"
  end
  def self.calls
    defined?(self.uninvoked)
  end
  def self.location
    [__FILE__, __LINE__]
  end
  def self.value
    11
  end
  def self.predicates
    [defined?(self.uninvoked.nil?), defined?(self.value.nil?), defined?(self.value.present?)]
  end
  def self.simple
    [defined?(self), defined?(nil), defined?(true), defined?(false), defined?(17), defined?(MissingOuterPr197::Inner)]
  end
  def classvars
    before = defined?(@@value)
    @@value = nil
    [before, defined?(@@value)]
  end
  def visible
    11
  end
  private
  def hidden
    raise "private query invoked method"
  end
end
class GuardChild < GuardProbe
  def self.calls
    defined?(super)
  end
  def queries
    [defined?(self.visible), defined?(self.hidden)]
  end
end
"#)
        .run_ruby(r#"
raise "constant guard changed" unless GuardProbe.constants == ["constant", "constant", nil, nil]
raise "method guard changed" unless GuardProbe.calls == "method"
raise "super guard changed" unless GuardChild.calls == "super"
raise "source identity changed" unless GuardProbe.location == [File.expand_path("app/services/guard_probe.rb", Dir.pwd), 13]
raise "predicate query was lowered or evaluated as a normal call" unless GuardProbe.predicates == [nil, "method", "method"]
raise "static descriptors became booleans" unless GuardProbe.simple == ["self", "nil", "true", "false", "expression", nil]
raise "nil class variable was confused with absence" unless GuardProbe.new.classvars == [nil, "class variable"]
raise "inherited or private method query changed" unless GuardChild.new.queries == ["method", nil]
"#).assert_passes();
}

/// A routed action with a template and no method behind it: Rails runs
/// `show` whether or not `def show` exists, so `before_action
/// :set_article, only: %i[show …]` still feeds `articles/show`. `check`
/// reported every `@article` in that template as having no known type.
#[test]
fn a_template_only_action_is_fed_by_its_before_action() {
    emit_and_run::real_blog()
        .edit(
            "app/controllers/articles_controller.rb",
            "  # GET /articles/1 or /articles/1.json\n  def show\n  end\n\n",
            "",
        )
        .run_test("test/controllers/articles_controller_test.rb")
        .assert_passes();
}

#[test]
fn array_and_hash_checks_preserve_members() {
    emit_and_run::real_blog()
        .write("app/lib/container_narrowing_probe.rb", r##"class ContainerNarrowingProbe
  def self.array_members
    value = ["alpha", "beta"]
    if value.is_a?(Array)
      value.map { |item| item.upcase }
    else
      raise("not an Array")
    end
  end
  def self.hash_members
    value = {"answer" => 41}
    if value.is_a?(Hash)
      value.map { |key, item| "#{key.upcase}=#{item + 1}" }
    else
      raise("not a Hash")
    end
  end
  def self.nested_members
    value = [{"name" => "alpha"}, {"name" => "beta"}]
    if value.is_a?(Array)
      value.map do |item|
        if item.is_a?(Hash)
          item.fetch("name").upcase
        else
          raise("not a Hash")
        end
      end
    else
      raise("not an Array")
    end
  end
end
"##)
        .run_ruby(r#"
raise "Array members changed" unless ContainerNarrowingProbe.array_members == ["ALPHA", "BETA"]
raise "Hash members changed" unless ContainerNarrowingProbe.hash_members == ["ANSWER=42"]
raise "nested members changed" unless ContainerNarrowingProbe.nested_members == ["ALPHA", "BETA"]
"#)
        .assert_passes();
}

const ARTICLES_CONTROLLER: &str = "app/controllers/articles_controller.rb";
const TRACK_FILTER: &str = "  def track\n    \
                              @tracked = %w[index show].include?(action_name)\n    \
                              @action = action_name\n  \
                            end\n";
const TRACKED_CALLS: &str = "<p id=\"tracked\"><%= @tracked %> <%= @action %></p>\n";
const TRACKED_ASSERTION: &str = "    assert_match(/<p id=\"tracked\">true index<\\/p>/, response.body)\n";

/// A filter reads `action_name`. Rails gives the name of the action as a
/// String. The emitted filter body raised NameError, because only the
/// dispatcher had `action_name`, and the dispatcher gave a Symbol.
#[test]
fn a_before_action_reads_action_name() {
    on_the_index(
        emit_and_run::real_blog()
            .edit(
                ARTICLES_CONTROLLER,
                "  before_action :set_article,",
                "  before_action :track\n  before_action :set_article,",
            )
            .edit(ARTICLES_CONTROLLER, "  private\n", &format!("  private\n{TRACK_FILTER}\n")),
        TRACKED_CALLS,
        TRACKED_ASSERTION,
    )
    .assert_passes();
}

/// The same filter from a concern that the controller includes.
#[test]
fn a_before_action_from_a_concern_reads_action_name() {
    on_the_index(
        emit_and_run::real_blog()
            .write(
                "app/controllers/concerns/tracks_action.rb",
                &format!(
                    "module TracksAction\n  \
                       extend ActiveSupport::Concern\n\n  \
                       included do\n    \
                         before_action :track\n  \
                       end\n\n  \
                       private\n\n{TRACK_FILTER}end\n"
                ),
            )
            .edit(
                ARTICLES_CONTROLLER,
                "class ArticlesController < ApplicationController\n",
                "class ArticlesController < ApplicationController\n  include TracksAction\n",
            ),
        TRACKED_CALLS,
        TRACKED_ASSERTION,
    )
    .assert_passes();
}

/// A private method reads `self.action_name`, and the action calls it.
#[test]
fn a_private_method_reads_self_action_name() {
    on_the_index(
        emit_and_run::real_blog()
            .edit(
                ARTICLES_CONTROLLER,
                "    @articles = Article.includes(:comments).order(created_at: :desc)\n",
                "    @articles = Article.includes(:comments).order(created_at: :desc)\n    \
                   @label = label\n",
            )
            .edit(
                ARTICLES_CONTROLLER,
                "  private\n",
                "  private\n  def label\n    self.action_name\n  end\n\n",
            ),
        "<p id=\"label\"><%= @label %></p>\n",
        "    assert_match(/<p id=\"label\">index<\\/p>/, response.body)\n",
    )
    .assert_passes();
}

/// A block filter and a lambda filter read `action_name`. Their bodies run
/// inside the dispatcher, so a bare `action_name` read the dispatcher's
/// Symbol, and `self.action_name` read `""`.
#[test]
fn block_and_lambda_filters_read_action_name() {
    on_the_index(
        emit_and_run::real_blog().edit(
            ARTICLES_CONTROLLER,
            "  before_action :set_article,",
            "  before_action { @bare = action_name }\n  \
               before_action -> { @own = self.action_name }\n  \
               before_action :set_article,",
        ),
        "<p id=\"names\"><%= @bare.inspect %> <%= @own.inspect %></p>\n",
        "    assert_match(/<p id=\"names\">&quot;index&quot; &quot;index&quot;<\\/p>/, response.body)\n",
    )
    .assert_passes();
}

/// A block `after_action` reads `action_name`. It runs inside the
/// dispatcher too, so it read the dispatcher's Symbol.
#[test]
fn a_block_after_action_reads_action_name() {
    emit_and_run::real_blog()
        .edit(
            ARTICLES_CONTROLLER,
            "  before_action :set_article,",
            "  after_action only: :index do\n    \
                 raise \"after_action read #{action_name.inspect}\" unless action_name == \"index\"\n  \
               end\n  \
               before_action :set_article,",
        )
        .run_test(CONTROLLER_TEST)
        .assert_passes();
}

/// A `rescue_from` block reads `action_name`. The dispatcher runs the
/// handler, so it read the dispatcher's Symbol.
#[test]
fn a_rescue_from_block_reads_action_name() {
    emit_and_run::real_blog()
        .edit(
            ARTICLES_CONTROLLER,
            "  before_action :set_article,",
            "  rescue_from ActiveRecord::RecordNotFound do\n    \
                 redirect_to articles_path, notice: \"missing #{action_name.inspect}\"\n  \
               end\n  \
               before_action :set_article,",
        )
        .edit(
            CONTROLLER_TEST,
            INDEX_ASSERTION,
            &format!(
                "{INDEX_ASSERTION}    get \"/articles/0\"\n    \
                   follow_redirect!\n    \
                   assert_match(/id=\"notice\">missing &quot;show&quot;</, response.body)\n"
            ),
        )
        .run_test(CONTROLLER_TEST)
        .assert_passes();
}

/// An inherited filter has an `if:` lambda that reads `action_name`. The
/// dispatcher runs the guard, so it compared the dispatcher's Symbol with
/// a String, and the filter never ran.
#[test]
fn an_inherited_filter_guard_reads_action_name() {
    on_the_index(
        emit_and_run::real_blog().edit(
            "app/controllers/application_controller.rb",
            "  allow_browser versions: :modern\n",
            "  allow_browser versions: :modern\n  \
               before_action :mark, if: -> { action_name == \"index\" }\n\n  \
               private\n\n  \
               def mark\n    \
                 @marked = true\n  \
               end\n",
        ),
        "<p id=\"marked\"><%= @marked.inspect %></p>\n",
        "    assert_match(/<p id=\"marked\">true<\\/p>/, response.body)\n",
    )
    .assert_passes();
}

/// An inherited filter and an inlined filter read `action_name`, for the
/// target that runs the dispatcher natively.
fn action_name_app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n",
        )
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\n  \
               before_action :track\n\n  \
               private\n\n  \
               def track\n    \
                 @tracked = %w[index show].include?(action_name)\n  \
               end\n\
             end\n",
        )
        .write("config/routes.rb", "Rails.application.routes.draw do\n  get \"/widgets\", to: \"widgets#index\"\nend\n")
        .write(
            "app/controllers/widgets_controller.rb",
            "class WidgetsController < ApplicationController\n  \
               before_action :name_it\n\n  \
               def index\n    \
                 render plain: \"#{@tracked} #{@name}\"\n  \
               end\n\n  \
               private\n\n  \
               def name_it\n    \
                 @name = action_name\n  \
               end\n\
             end\n",
        )
}

const ACTION_NAME_ASSERTIONS: &str = r#"
require_relative "app/controllers/widgets_controller"
controller = WidgetsController.new
controller.process_action(:index)
raise "action_name: #{controller.body}" unless controller.body == "true index"
puts "action_name passed"
"#;

#[test]
fn filters_read_action_name_as_a_string() {
    action_name_app().run_ruby(ACTION_NAME_ASSERTIONS).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn filters_read_action_name_as_a_string_on_spinel() {
    action_name_app().run_spinel(ACTION_NAME_ASSERTIONS).assert_passes();
}

/// `case/in` structural pattern matching (#f9): taking `CaseMatchNode`
/// from an ingest error to a typed `CaseMatch` node is a claim the
/// emitted program actually dispatches through it (invariant 6), not
/// just that `check` stops reporting `unsupported expression node:
/// CaseMatchNode`. A PORO under `app/lib` (same placement as the
/// `Deprecation` overlay above) exercises a `Capture` pattern
/// (`Integer => n`) and a plain-class `Value` pattern (`String`) — the
/// two shapes real-blog's own model/controller code never uses, so
/// this is the only thing that runs them through CRuby at all.
#[test]
fn case_in_pattern_matching_runs() {
    emit_and_run::real_blog()
        .write(
            "app/lib/pattern_matcher.rb",
            "class PatternMatcher\n  \
               def self.classify(x)\n    \
                 case x\n    \
                 in Integer => n\n      \
                   n * 2\n    \
                 in String\n      \
                   0\n    \
                 end\n  \
               end\nend\n",
        )
        .write(
            "test/models/pattern_matcher_test.rb",
            "require \"test_helper\"\n\n\
             class PatternMatcherTest < ActiveSupport::TestCase\n  \
               test \"case/in dispatches by pattern and captures a binding\" do\n    \
                 assert_equal 10, PatternMatcher.classify(5)\n    \
                 assert_equal 0, PatternMatcher.classify(\"hi\")\n  \
               end\n\
             end\n",
        )
        .run_test("test/models/pattern_matcher_test.rb")
        .assert_passes();
}

/// `Pathname#join` takes any number of parts, and an app writes
/// `Rails.root.join("source", "posts")` as often as the one-part form.
/// `check` is clean on the call, so the emitted `Rails::AppPath#join`
/// must accept every part, or none, and join them like Pathname does.
#[test]
fn rails_root_join_takes_any_number_of_parts() {
    let run = rails_root_join::overlay().run_ruby(rails_root_join::ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("Rails.root.join contract passed"));
}

/// An Active Job argument serializer extends a Rails base that the
/// runtime does not port. The emit drops the class with a
/// `lower_residue` warning, so the tree still loads. Before, the class
/// was kept, and `app/models.rb` raised `uninitialized constant
/// ActiveJob::Serializers` at boot.
#[test]
fn an_active_job_object_serializer_does_not_stop_the_boot() {
    emit_and_run::real_blog()
        .write(
            "app/serializers/article_serializer.rb",
            r#"class ArticleSerializer < ActiveJob::Serializers::ObjectSerializer
  def klass
    Article
  end

  def serialize(article)
    super("id" => article.id)
  end

  def deserialize(hash)
    Article.find(hash["id"])
  end
end
"#,
        )
        .run_test("test/controllers/articles_controller_test.rb")
        .assert_passes();
}

/// `include ActiveSupport::NumberHelper` in a helper gives it the same
/// number helpers as `ActionView::Helpers::NumberHelper`. No target
/// ships that namespace, so the include must not reach the emitted
/// module. Before, `application_helper.rb` raised `uninitialized
/// constant ActiveSupport::NumberHelper` at boot.
#[test]
fn an_active_support_number_helper_include_does_not_stop_the_boot() {
    emit_and_run::real_blog()
        .write(
            "app/helpers/application_helper.rb",
            r#"module ApplicationHelper
  include ActiveSupport::NumberHelper

  def article_total(count)
    number_with_delimiter(count)
  end
end
"#,
        )
        .run_ruby(
            r#"raise "delimiter" unless ApplicationHelper.article_total(1234567) == "1,234,567"
puts "ok"
"#,
        )
        .assert_passes();
}

/// Campfire's `TimeLimitedVideoPreviewer` subclasses Rails'
/// `ActiveStorage::Previewer::VideoPreviewer`. Before the nested class
/// existed in the runtime, `app/models.rb` raised
/// `uninitialized constant ActiveStorage::Previewer::VideoPreviewer`
/// at boot. The capture body (Timeout / IO.popen) is a separate ledger
/// entry; this pins the constant that inheritance needs to load.
#[test]
fn time_limited_video_previewer_subclass_boots() {
    emit_and_run::real_blog()
        .write(
            "lib/rails_ext/time_limited_video_previewer.rb",
            r#"# Campfire's inheritance shape (capture body omitted — Timeout/IO are
# not yet modeled). The NameError at boot was the missing superclass.
class TimeLimitedVideoPreviewer < ActiveStorage::Previewer::VideoPreviewer
  TIME_LIMIT = 10
end
"#,
        )
        .write(
            "config/initializers/extensions.rb",
            r##"%w[ rails_ext ].each do |extensions_dir|
  Dir["#{Rails.root}/lib/#{extensions_dir}/*"].each { |path| require "#{extensions_dir}/#{File.basename(path)}" }
end
"##,
        )
        .run_ruby(
            r#"raise "missing nested class" unless defined?(ActiveStorage::Previewer::VideoPreviewer)
raise "subclass missing" unless defined?(TimeLimitedVideoPreviewer)
raise "wrong parent" unless TimeLimitedVideoPreviewer < ActiveStorage::Previewer::VideoPreviewer
raise "wrong grandparent" unless TimeLimitedVideoPreviewer < ActiveStorage::Previewer
raise "TIME_LIMIT" unless TimeLimitedVideoPreviewer::TIME_LIMIT == 10
raise "PreviewError missing" unless defined?(ActiveStorage::PreviewError)
raise "Error base missing" unless defined?(ActiveStorage::Error)
raise "PreviewError parent" unless ActiveStorage::PreviewError < ActiveStorage::Error
raise "Error parent" unless ActiveStorage::Error < StandardError
puts "time_limited_video_previewer boot ok"
"#,
        )
        .assert_passes();
}

/// `javascript_include_tag :application` names the source with a
/// Symbol, as Rails allows. The call is hoisted to a constant, so it
/// runs at load. Before, the runtime called `include?` on the Symbol,
/// and the layout raised `NoMethodError` at boot.
#[test]
fn a_symbol_source_for_javascript_include_tag_renders_a_script_tag() {
    emit_and_run::real_blog()
        .edit(
            "app/views/layouts/application.html.erb",
            "    <%= javascript_importmap_tags %>\n",
            "    <%= javascript_importmap_tags %>\n    <%= javascript_include_tag :application %>\n",
        )
        .write(
            "app/views/articles/_scripts.html.erb",
            "<%= javascript_include_tag :admin, defer: true %>",
        )
        .run_ruby(
            r#"html = Views::Articles.scripts(nil)
raise "script tag: #{html}" unless html == %(<script src="/assets/admin.js" defer="defer"></script>)
puts "ok"
"#,
        )
        .assert_passes();
}

/// A Symbol source that a value holds, not a literal, reaches the
/// runtime as a Symbol. Before, the runtime called `include?` on it and
/// raised `NoMethodError`.
#[test]
fn a_symbol_source_in_a_value_for_javascript_include_tag_renders_a_script_tag() {
    emit_and_run::real_blog()
        .write(
            "app/views/articles/_scripts.html.erb",
            "<% source = :admin %><%= javascript_include_tag source %>",
        )
        .run_ruby(
            r#"html = Views::Articles.scripts(nil)
raise "script tag: #{html}" unless html == %(<script src="/assets/admin.js"></script>)
puts "ok"
"#,
        )
        .assert_passes();
}

/// A Stimulus `data:` hash has `controller:` and `action:` keys, as
/// `url_for` options do. Before, the lowerer read it as `url_for`
/// options and wrote a route helper named after its keys. A dashed key
/// made that name a syntax error, and the app did not load.
#[test]
fn a_stimulus_data_hash_on_link_to_renders_its_data_attributes() {
    a_stimulus_data_hash_renders(
        r#"<%= link_to "Open", articles_path, data: { controller: "menu", action: "menu#open", "menu-id-value": 1 } %>"#,
        &[r#"href="/articles""#, ">Open</a>"],
    );
}

/// `form_with` takes the `data:` hash for its `<form>` in `html:`, one
/// level deeper.
#[test]
fn a_stimulus_data_hash_in_form_with_html_options_renders_its_data_attributes() {
    a_stimulus_data_hash_renders(
        r#"<%= form_with url: articles_path, html: { data: { controller: "menu", action: "menu#open", "menu-id-value": 1 } } do |f| %><%= f.submit "Go" %><% end %>"#,
        &[r#"action="/articles""#, r#"value="Go""#],
    );
}

/// A `data:` hash that a local holds is not a literal at the call.
#[test]
fn a_stimulus_data_hash_in_a_local_renders_its_data_attributes() {
    a_stimulus_data_hash_renders(
        r#"<% d = { controller: "menu", action: "menu#open", "menu-id-value": 1 } %><%= link_to "Open", articles_path, data: d %>"#,
        &[r#"href="/articles""#, ">Open</a>"],
    );
}

/// A `data:` hash with a `.merge` on it is a call, not a Hash literal.
#[test]
fn a_merged_stimulus_data_hash_renders_its_data_attributes() {
    a_stimulus_data_hash_renders(
        r#"<%= link_to "Open", articles_path, data: { controller: "menu", action: "menu#open", "menu-id-value": 1 }.merge(turbo: false) %>"#,
        &[r#"href="/articles""#, r#"data-turbo="false""#, ">Open</a>"],
    );
}

/// Renders `erb` as a partial, and expects the Stimulus attributes and
/// each of `parts` in the HTML.
fn a_stimulus_data_hash_renders(erb: &str, parts: &[&str]) {
    let stimulus = [r#"data-controller="menu""#, r#"data-action="menu#open""#, r#"data-menu-id-value="1""#];
    let expected: Vec<String> = stimulus
        .iter()
        .chain(parts)
        .map(|p| format!("{p:?}"))
        .collect();
    emit_and_run::real_blog()
        .write("app/views/articles/_menu.html.erb", erb)
        .run_ruby(&format!(
            r#"html = Views::Articles.menu(nil)
[{}].each do |part|
  raise "missing #{{part}}: #{{html}}" unless html.include?(part)
end
puts "ok"
"#,
            expected.join(", ")
        ))
        .assert_passes();
}

/// `t.integer …, limit: 8` is a `bigint` now (the width Rails creates),
/// where it was an `integer`. On SQLite both are INTEGER and both type
/// as `Integer`, so the emitted program must keep a value past 32 bits
/// through a save and a reload, as it did before.
#[test]
fn an_eight_byte_integer_column_keeps_a_value_past_32_bits() {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "create_table \"articles\", force: :cascade do |t|",
            "create_table \"articles\", force: :cascade do |t|\n    t.integer \"views\", limit: 8, default: 0, null: false",
        )
        .write(
            "test/models/article_views_test.rb",
            r#"require "test_helper"

class ArticleViewsTest < ActiveSupport::TestCase
  test "a value past 32 bits survives a reload" do
    article = Article.create!(title: "Popular", body: "A long enough body", views: 5_000_000_000)
    assert_equal 5_000_000_000, Article.find(article.id).views
  end
end
"#,
        )
        .run_test("test/models/article_views_test.rb")
        .assert_passes();
}

#[test]
fn literal_data_factories_and_aliases_check_cleanly_and_execute() {
    use roundhouse::ident::{ClassId, Symbol};
    use roundhouse::ty::Ty;

    let run = emit_and_run::real_blog()
        .write("app/lib/factory_examples.rb", data_factory::DECLARATIONS)
        .write("app/controllers/data_probe_controller.rb", r#"class DataProbeController < ApplicationController
  def index
    @declared = FactoryExamples::First::Result.new("first", 1.0, false)
    @alias = FactoryExamples::First::ChainedAlias.new(name: nil, score: 0.0, enabled: true)
    @second = FactoryExamples::Second::Result.new(name: "second")
    @empty = FactoryExamples::Empty::Result.new
  end
end
"#)
        .run_ruby(r#"
first = FactoryExamples::First.build
aliased = FactoryExamples::First.aliased
qualified = FactoryExamples::First.qualified
second = FactoryExamples::Second.build
empty = FactoryExamples::Empty.build
raise "wrong declared class" unless first.class == FactoryExamples::First::Result
raise "alias created another class" unless aliased.class == first.class && FactoryExamples::First::Alias == first.class && FactoryExamples::First::ChainedAlias == first.class
raise "member values changed" unless first.name == "first" && first.score == 0.8 && first.enabled == false
raise "nil/zero/true changed" unless aliased.name.nil? && aliased.score == 0.0 && aliased.enabled == true
raise "positional constructor changed" unless qualified.name == "qualified" && qualified.score == 1.0 && qualified.enabled == false
raise "same-named factory crossed owners" unless second.class == FactoryExamples::Second::Result && second.name == "second" && second.class != first.class
raise "empty factory changed" unless empty.class == FactoryExamples::Empty::Result && empty.members.empty?
raise "Data lost immutability" unless first.frozen? && !first.respond_to?(:name=)
puts "Data factory identity, aliases, constructors, values and immutability passed"
"#);
    run.assert_passes();
    let emitted = std::fs::read_to_string(run.emitted.join("app/models/factory_examples/first.rb")).unwrap();
    assert!(emitted.contains("Data.define"), "{emitted}");
    for (owner, stem, readers) in [
        ("FactoryExamples::First", "first", vec!["name", "score", "enabled"]),
        ("FactoryExamples::Second", "second", vec!["name"]),
        ("FactoryExamples::Empty", "empty", vec![]),
    ] {
        let sidecar = std::fs::read_to_string(run.emitted.join(
            format!("sig/app/models/factory_examples/{stem}.rbs")
        )).unwrap();
        assert!(sidecar.contains("class Result < ::Data"), "{sidecar}");
        assert!(!sidecar.contains("class Alias") && !sidecar.contains("class ChainedAlias"), "{sidecar}");
        let signatures = roundhouse::rbs::parse_app_signatures(&sidecar).expect("emitted RBS parses");
        let id = ClassId(Symbol::from(format!("{owner}::Result")));
        let factory = signatures.get(&id).expect("factory return type is declared");
        assert!(factory.contains_key(&Symbol::from("new")), "{sidecar}");
        for reader in readers {
            let signature = &factory[&Symbol::from(reader)];
            assert!(matches!(signature, Ty::Fn { ret, .. } if **ret == Ty::Untyped), "{sidecar}");
            assert!(!factory.contains_key(&Symbol::from(format!("{reader}="))), "{sidecar}");
        }
        let owner_methods = &signatures[&ClassId(Symbol::from(owner))];
        let Ty::Fn { ret, .. } = &owner_methods[&Symbol::from("build")] else {
            panic!("build has no function signature: {sidecar}");
        };
        assert_eq!(ret.as_ref(), &Ty::Class { id, args: vec![].into() }, "{sidecar}");
    }
}

/// Execute emitted Ruby to check keyword/positional construction, `with`, method
/// visibility, aliases, and lexical constants. A preceding `defined?` also catches
/// factory declarations hoisted ahead of their original source position.
#[test]
fn custom_data_factories_preserve_methods_initializers_and_scope() {
    let run = emit_and_run::real_blog()
        .write("app/lib/factory_examples.rb", data_factory::CUSTOM_DECLARATIONS)
        .edit("app/lib/factory_examples.rb", "    PREFIX = \"order\"", "    PREFIX = \"order\"\n    BEFORE = defined?(State)")
        .run_ruby(&format!("{}\nraise 'declaration was hoisted' unless FactoryExamples::Stateful::BEFORE.nil?", data_factory::CUSTOM_ASSERTIONS));
    run.assert_passes();
    assert!(run.stdout.contains("custom Data factory contract passed"));
}

/// A file in `app/models/<model>/` often reopens the model only to
/// hold a nested class. That reopen is a namespace, so the model keeps
/// its own file. Before, the reopen became a library class whose file
/// was the model's file, so the emit wrote the nested class over the
/// model, and `Article.find` raised `NoMethodError`.
#[test]
fn a_model_reopened_to_hold_a_nested_class_keeps_its_model() {
    a_reopen_at_keeps_the_model("app/models/article/summary.rb");
}

/// The same reopen outside `app/models`. The ingest reads these
/// folders later, and the reopen must not write over the model there
/// either.
#[test]
fn a_model_reopened_in_app_services_keeps_its_model() {
    a_reopen_at_keeps_the_model("app/services/article/summary.rb");
}

#[test]
fn a_model_reopened_in_lib_keeps_its_model() {
    a_reopen_at_keeps_the_model("lib/article/summary.rb");
}

fn a_reopen_at_keeps_the_model(path: &str) {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "  validates :body, presence: true, length: { minimum: 10 }\n",
            "  validates :body, presence: true, length: { minimum: 10 }\n\n  DRAFT = \"draft\".freeze\n\n  def summary\n    Summary.new(self)\n  end\n",
        )
        .write(
            path,
            r##"class Article
  class Summary
    def initialize(article)
      @article = article
    end

    def text
      "#{@article.title} (#{Article::DRAFT})"
    end
  end
end
"##,
        )
        .write(
            "test/models/article_summary_test.rb",
            r##"require "test_helper"

class ArticleSummaryTest < ActiveSupport::TestCase
  test "the model and its nested class both load" do
    article = Article.find(articles(:one).id)
    assert_equal "#{article.title} (draft)", article.summary.text
    assert Article < ApplicationRecord
  end
end
"##,
        )
        .run_test("test/models/article_summary_test.rb")
        .assert_passes();
}

/// lobsters' `Sponge` keeps the exceptions it shrugs off in a class-body
/// Array, so a refusal stub for any one of them fails the whole app at
/// load. These are the library constants it names (plus its TLS modes,
/// `Github`'s `Net::HTTPSuccess` test and `Utils`' `OpenSSL::Random`),
/// read and rescued as Ruby has them.
#[test]
fn network_library_constants_load_and_rescue() {
    let run = emit_and_run::empty_app()
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write(
            "app/models/fetcher.rb",
            r#"class Fetcher
  PROBLEMS = [
    EOFError,
    Errno::ECONNRESET,
    Net::HTTPBadResponse,
    OpenSSL::SSL::SSLError,
    Zlib::DataError,
    Encoding::CompatibilityError
  ].freeze

  def verify_mode(strict)
    strict ? OpenSSL::SSL::VERIFY_PEER : OpenSSL::SSL::VERIFY_NONE
  end

  def shrug(error)
    raise error, "boom"
  rescue *PROBLEMS => e
    e.message
  end

  def success?(response)
    response.is_a?(Net::HTTPSuccess)
  end

  def self.random_byte
    OpenSSL::Random.random_bytes(1)
  end
end
"#,
        )
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema[8.1].define(version: 2026_01_01_000000) do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n",
        )
        .run_ruby(
            r##"f = Fetcher.new
raise "PROBLEMS: #{Fetcher::PROBLEMS.size}" unless Fetcher::PROBLEMS.size == 6
[Net::HTTPBadResponse, Zlib::DataError, Encoding::CompatibilityError].each do |error|
  got = f.shrug(error)
  raise "#{error} not rescued: #{got.inspect}" unless got == "boom"
end
raise "VERIFY_PEER" unless f.verify_mode(true) == OpenSSL::SSL::VERIFY_PEER
raise "VERIFY_NONE" unless f.verify_mode(false) == OpenSSL::SSL::VERIFY_NONE
ok = Net::HTTPOK.new("1.1", "200", "OK")
raise "HTTPSuccess" unless f.success?(ok)
raise "HTTPSuccess negative" if f.success?(Object.new)
byte = Fetcher.random_byte
raise "random_bytes: #{byte.inspect}" unless byte.is_a?(String) && byte.bytesize == 1
"##,
        );
    run.assert_passes();
}

/// `URI::RFC2396_PARSER.escape(url)` (lobsters' webmention job) and its
/// siblings are the uri library's parsers: they run on the ruby family,
/// and a strict target, with no uri runtime, reports the call.
fn uri_parser_app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write(
            "app/models/linker.rb",
            r#"class Linker
  def self.rfc2396(url)
    URI::RFC2396_PARSER.escape(url.to_s)
  end

  def self.rfc3986(url)
    URI::RFC3986_PARSER.unescape(url.to_s)
  end

  def self.default(url)
    URI::DEFAULT_PARSER.escape(url.to_s)
  end
end
"#,
        )
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema[8.1].define(version: 2026_01_01_000000) do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n",
        )
}

#[test]
fn uri_parser_constants_escape_on_the_ruby_family() {
    uri_parser_app()
        .run_ruby(
            r##"raise "rfc2396" unless Linker.rfc2396("a b/é") == "a%20b/%C3%A9"
raise "rfc3986" unless Linker.rfc3986("a%20b") == "a b"
raise "default" unless Linker.default("a b") == "a%20b"
"##,
        )
        .assert_passes();
}

#[test]
fn uri_parser_calls_are_reported_on_a_strict_target() {
    let (_emitted, errors) = uri_parser_app().emit(roundhouse::project::BuildTarget::Rust);
    for call in ["URI::RFC2396_Parser#escape", "URI::RFC3986_Parser#unescape", "URI::RFC3986_Parser#escape"] {
        assert!(errors.iter().any(|e| e.contains(&format!("{call} not supported (rust)"))), "{call}: {errors:#?}");
    }
    assert!(!errors.iter().any(|e| e.contains("constant not supported")), "{errors:#?}");
}

/// `module_function :name` promotes the def to the module, and ingest
/// retargets the sibling bare call to `Owner.name`. That receiver is
/// generated, not written, so it must resolve rather than emit a refusal
/// stub (lobsters' `EmailBlocklistValidation`, run on every User save).
#[test]
fn module_function_retargeted_call_runs() {
    let run = emit_and_run::empty_app()
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write(
            "app/models/blocklist.rb",
            r#"module Blocklist
  def blocked
    email && on_blocklist?(email)
  end

  def on_blocklist?(email)
    email.end_with?("@spam.test")
  end

  module_function :on_blocklist?
end
"#,
        )
        .write(
            "app/models/signup.rb",
            "class Signup\n  include Blocklist\n\n  attr_reader :email\n\n  def initialize(email)\n    @email = email\n  end\nend\n",
        )
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema[8.1].define(version: 2026_01_01_000000) do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n",
        )
        .run_ruby(
            r#"raise "module call" unless Blocklist.on_blocklist?("a@spam.test")
raise "blocked" unless Signup.new("a@spam.test").blocked
raise "not blocked" if Signup.new("a@ok.test").blocked
"#,
        );
    run.assert_passes();
}

/// lobsters keeps its site settings in a top-level
/// `class << Rails.application` block. Rubydex records no constant
/// references inside a singleton class opened on an expression, so the
/// `ENV` and `Rails` written there have no source answer; they must still
/// resolve as the modeled constants, not emit as refusal stubs.
#[test]
fn rails_application_singleton_settings_read_env_and_rails() {
    let run = emit_and_run::empty_app()
        .write(
            "config/application.rb",
            r#"module TestApp
  class Application < Rails::Application
  end
end

class << Rails.application
  def open_signups?
    ENV["OPEN_SIGNUPS"] == "true"
  end

  def domain
    "example.test"
  end

  def root_url
    "https://#{Rails.application.domain}/"
  end
end
"#,
        )
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema[8.1].define(version: 2026_01_01_000000) do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n",
        )
        .run_ruby(
            r#"ENV["OPEN_SIGNUPS"] = "true"
raise "ENV read: open_signups? must be true" unless Rails.application.open_signups?
ENV["OPEN_SIGNUPS"] = "false"
raise "ENV read: open_signups? must be false" if Rails.application.open_signups?
url = Rails.application.root_url
raise "Rails.application chain: #{url.inspect}" unless url == "https://example.test/"
"#,
        );
    run.assert_passes();
}

/// `if:` / `unless:` guards a callback. Ingest used to reject the
/// declaration outright, so the callback was silently dropped and ran in
/// no circumstance. A zero-arity lambda body (`if: -> { color.blank? }`)
/// is spliced as the guard; a Symbol (`unless: :loud?`) is the predicate
/// call, negated. This runs the emitted program to prove the callback
/// fires exactly when Rails would.
#[test]
fn a_conditional_callback_runs_only_when_its_condition_holds() {
    let run = emit_and_run::empty_app()
        .write(
            "config/application.rb",
            "module TestApp\n  class Application < Rails::Application\n  end\nend\n",
        )
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write(
            "app/models/widget.rb",
            r#"class Widget < ApplicationRecord
  before_validation :assign_color, on: :create, if: -> { color.blank? }
  before_save :shout, unless: :loud?

  private
    def assign_color
      self.color = "default"
    end

    def loud?
      self.name == "LOUD"
    end

    def shout
      self.name = self.name.to_s.upcase
    end
end
"#,
        )
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema[8.1].define(version: 2026_01_01_000000) do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"color\"\n    t.string \"name\"\n  end\nend\n",
        )
        .run_ruby(
            r#"quiet = Widget.create!(name: "quiet")
raise "if: true must run the callback: #{quiet.color.inspect}" unless quiet.color == "default"
raise "unless: true (not loud?) must run the callback: #{quiet.name.inspect}" unless quiet.name == "QUIET"
loud = Widget.create!(name: "LOUD", color: "red")
raise "if: false must skip the callback: #{loud.color.inspect}" unless loud.color == "red"
raise "unless: false (loud?) must skip the callback: #{loud.name.inspect}" unless loud.name == "LOUD"
"#,
        );
    run.assert_passes();
}

/// Both `if:` and `unless:` on one callback: Rails runs it only when the
/// `if:` holds AND the `unless:` does not, so the two guards compose as
/// `if` and `!unless`. The emitted program must honour all four
/// combinations.
#[test]
fn a_callback_with_both_if_and_unless_requires_both() {
    let run = emit_and_run::empty_app()
        .write(
            "config/application.rb",
            "module TestApp\n  class Application < Rails::Application\n  end\nend\n",
        )
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write(
            "app/models/widget.rb",
            r#"class Widget < ApplicationRecord
  before_save :stamp, if: :ready?, unless: :blocked?

  def ready?
    self.color.present?
  end

  def blocked?
    self.name == "blocked"
  end

  def stamp
    self.name = "STAMPED"
  end
end
"#,
        )
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema[8.1].define(version: 2026_01_01_000000) do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"color\"\n    t.string \"name\"\n  end\nend\n",
        )
        .run_ruby(
            r#"not_ready = Widget.create!(name: "x")
raise "if: false must skip: #{not_ready.name.inspect}" unless not_ready.name == "x"
blocked = Widget.create!(color: "red", name: "blocked")
raise "unless: true must skip: #{blocked.name.inspect}" unless blocked.name == "blocked"
runs = Widget.create!(color: "red", name: "x")
raise "if: true and unless: false must run: #{runs.name.inspect}" unless runs.name == "STAMPED"
"#,
        );
    run.assert_passes();
}
/// A predicate the app defines on `String` is that method, not an
/// inquirer comparison against its own name.
#[test]
fn a_string_predicate_the_app_defines_is_not_folded_as_an_inquiry() {
    emit_and_run::real_blog()
        .write(
            "lib/rails_ext/string.rb",
            "class String\n  def shout?\n    self == upcase\n  end\n\n  def self.special?\n    true\n  end\nend\n",
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord",
            "class Article < ApplicationRecord\n  def shouting?\n    title.to_s.shout?\n  end\n\n  def shouting_inquirer?\n    title.to_s.inquiry.shout?\n  end\n\n  def class_side_inquirer?\n    title.to_s.inquiry.special?\n  end",
        )
        .run_ruby(
            r#"
raise "folded to a comparison" unless Article.new(title: "LOUD", body: "b").shouting?
raise "answers true for everything" if Article.new(title: "quiet", body: "b").shouting?
raise "inquirer folded to a comparison" unless Article.new(title: "LOUD", body: "b").shouting_inquirer?
raise "inquirer answers true for everything" if Article.new(title: "quiet", body: "b").shouting_inquirer?
raise "class-side predicate blocked the fold" unless Article.new(title: "special", body: "b").class_side_inquirer?
raise "class-side fold answers true for everything" if Article.new(title: "quiet", body: "b").class_side_inquirer?
"#,
        )
        .assert_passes();
}

/// The same through a module the app includes into `String`.
#[test]
fn a_string_predicate_from_an_included_module_is_not_folded_as_an_inquiry() {
    emit_and_run::real_blog()
        .write(
            "lib/rails_ext/string.rb",
            "module Shouting\n  def shout?\n    self == upcase\n  end\nend\n\nclass String\n  include Shouting\nend\n",
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord",
            "class Article < ApplicationRecord\n  def shouting?\n    title.to_s.shout?\n  end",
        )
        .run_ruby(
            r#"
raise "folded to a comparison" unless Article.new(title: "LOUD", body: "b").shouting?
raise "answers true for everything" if Article.new(title: "quiet", body: "b").shouting?
"#,
        )
        .assert_passes();
}

/// `includes(:comments)` distributes each parent's children by binary
/// search over the children's foreign keys, sorted by the preload query
/// (`ActiveRecord.lower_bound`), where it used to scan every child per
/// parent — O(N * M), a million comparisons at 1,000 x 1,000, which put
/// the emitted index behind Rails' keyed preloader
/// (koduki/example-rails-aot). The comment-byte gates are blind to
/// grouping, so this renders each article's preloaded comments by body:
/// inserts interleaved across articles, a parent with no children, and
/// a run at the end of the sorted list all have to land, in insertion
/// order within each article.
#[test]
fn includes_distributes_each_parents_children_in_order() {
    emit_and_run::real_blog()
        .edit(
            "app/views/articles/_article.html.erb",
            "(<%= pluralize(article.comments.size, \"comment\") %>)",
            "(<%= pluralize(article.comments.size, \"comment\") %>)<i class=\"pc\"><%= article.title %>=<%= article.comments.map(&:body).join(\",\") %></i>",
        )
        .write(
            "test/controllers/articles_preload_controller_test.rb",
            r#"require "test_helper"

class ArticlesPreloadControllerTest < ActionDispatch::IntegrationTest
  test "includes distributes each article's comments" do
    a = Article.create!(title: "Alpha", body: "A sufficiently long body for validation.")
    b = Article.create!(title: "Beta", body: "A sufficiently long body for validation.")
    c = Article.create!(title: "Gamma", body: "A sufficiently long body for validation.")
    Article.create!(title: "Delta", body: "A sufficiently long body for validation.")
    Comment.create!(article_id: c.id, commenter: "x", body: "c1")
    Comment.create!(article_id: a.id, commenter: "x", body: "a1")
    Comment.create!(article_id: c.id, commenter: "x", body: "c2")
    Comment.create!(article_id: b.id, commenter: "x", body: "b1")
    Comment.create!(article_id: a.id, commenter: "x", body: "a2")
    Comment.create!(article_id: c.id, commenter: "x", body: "c3")
    get articles_url
    assert_response :success
    assert_match(/<i class="pc">Alpha=a1,a2<\/i>/, response.body)
    assert_match(/<i class="pc">Beta=b1<\/i>/, response.body)
    assert_match(/<i class="pc">Gamma=c1,c2,c3<\/i>/, response.body)
    assert_match(/<i class="pc">Delta=<\/i>/, response.body)
  end
end
"#,
        )
        .run_test("test/controllers/articles_preload_controller_test.rb")
        .assert_passes();
}

/// A rooted `class_name:` names the top-level class (chatwoot's
/// `has_many :portals, class_name: "::Portal"`).
#[test]
fn a_rooted_class_name_association_runs() {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n  has_many :rooted_comments, class_name: \"::Comment\"\n\n  def first_rooted_body\n    rooted_comments.first.body\n  end\n",
        )
        .run_ruby(
            r#"article = Article.create!(title: "Rooted", body: "Body text here")
Comment.create!(article_id: article.id, commenter: "Ann", body: "First remark")
raise "rooted association count" unless article.rooted_comments.count == 1
raise "rooted association read" unless article.first_rooted_body == "First remark"
"#,
        )
        .assert_passes();
}

/// Interface keys belong to `as:`, even when the Concern name matches it.
#[test]
fn a_polymorphic_inverse_from_a_concern_runs() {
    assert_polymorphic_inverse_from_a_concern_runs("Notifiable");
}

/// A different Concern name must preserve the same id/type interface.
#[test]
fn a_polymorphic_inverse_from_a_differently_named_concern_runs() {
    assert_polymorphic_inverse_from_a_concern_runs("NotificationOwner");
}

fn assert_polymorphic_inverse_from_a_concern_runs(concern: &str) {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "  create_table \"comments\", force: :cascade do |t|",
            "  create_table \"notifications\", force: :cascade do |t|\n    t.integer \"notifiable_id\"\n    t.string \"notifiable_type\"\n  end\n\n  create_table \"comments\", force: :cascade do |t|",
        )
        .write(
            "app/models/notification.rb",
            "class Notification < ApplicationRecord\n  belongs_to :notifiable, polymorphic: true\n  def owner_title\n    notifiable.title\n  end\nend\n",
        )
        .write(
            "app/models/concerns/notifiable.rb",
            &format!("module {concern}\n  extend ActiveSupport::Concern\n  included do\n    has_many :notifications, as: :notifiable\n    has_many :explicit_notifications, class_name: \"Notification\", as: :notifiable, foreign_key: :notifiable_id\n    has_one :first_notification, class_name: \"Notification\", as: :notifiable\n    has_one :last_notification, class_name: \"Notification\", as: :notifiable, foreign_key: :notifiable_id\n  end\nend\n"),
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            &format!("class Article < ApplicationRecord\n  include {concern}\n"),
        )
        .run_ruby(
            r#"article = Article.create!(title: "Owner", body: "Body text here")
note = Notification.create!(notifiable_id: article.id, notifiable_type: "Article")
reloaded = Notification.find(note.id)
raise "owner id changed" unless reloaded.notifiable_id == article.id
raise "owner type changed" unless reloaded.notifiable_type == "Article"
raise "polymorphic read" unless reloaded.owner_title == "Owner"
raise "inverse read" unless article.notifications.count == 1
raise "explicit inverse read" unless article.explicit_notifications.count == 1
raise "default singular inverse read" unless article.first_notification.owner_title == "Owner"
raise "explicit singular inverse read" unless article.last_notification.owner_title == "Owner"
"#,
        )
        .assert_passes();
}

/// An explicit `foreign_key:` is the key, even when its name matches
/// the Concern-derived default. Only a defaulted key is rehomed.
#[test]
fn an_explicit_key_named_like_its_concern_is_kept() {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "  create_table \"comments\", force: :cascade do |t|",
            "  create_table \"remarks\", force: :cascade do |t|\n    t.integer \"remarkable_id\"\n    t.string \"body\"\n  end\n\n  create_table \"comments\", force: :cascade do |t|",
        )
        .write(
            "app/models/remark.rb",
            "class Remark < ApplicationRecord\n  belongs_to :article, foreign_key: :remarkable_id\nend\n",
        )
        .write(
            "app/models/concerns/remarkable.rb",
            "module Remarkable\n  extend ActiveSupport::Concern\n  included do\n    has_many :remarks, foreign_key: :remarkable_id\n    has_one :first_remark, class_name: \"Remark\", foreign_key: :remarkable_id\n  end\nend\n",
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n  include Remarkable\n",
        )
        .run_ruby(
            r#"article = Article.create!(title: "Owner", body: "Body text here")
Remark.create!(remarkable_id: article.id, body: "hi")
raise "inverse read" unless article.remarks.count == 1
raise "singular inverse read" unless article.first_remark.body == "hi"
"#,
        )
        .assert_passes();
}

/// Not `super: no superclass method 'password='`: a model's own password writer that calls `super` reaches `has_secure_password`'s writer, as the macro's module method does in Rails.
#[test]
fn a_password_writer_that_calls_super_runs() {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "create_table \"articles\", force: :cascade do |t|",
            "create_table \"articles\", force: :cascade do |t|\n    t.string \"password_digest\"\n    t.string \"recovery_password_digest\"",
        )
        .edit(
            "db/schema.rb",
            "create_table \"comments\", force: :cascade do |t|",
            "create_table \"comments\", force: :cascade do |t|\n    t.string \"password_digest\"",
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            r#"class Article < ApplicationRecord
  has_many :comments, dependent: :destroy
  has_secure_password
  has_secure_password :recovery_password

  def password=(value)
    @password_supplied = true
    super(value == "" ? nil : value)
  end

  def password_supplied?
    @password_supplied == true
  end

  def seed_plaintext(value)
    @password = value
    @recovery_password = value
  end

  def recovery_password=(value)
    @recovery_password_supplied = true
    super
  end

  def recovery_password_supplied?
    @recovery_password_supplied == true
  end

  def recovery_plaintext
    @recovery_password
  end"#,
        )
        .edit(
            "app/models/comment.rb",
            "class Comment < ApplicationRecord",
            r#"class Comment < ApplicationRecord
  has_secure_password

  def password=(value)
    @password_supplied = true
    super
  end

  def password_supplied?
    @password_supplied == true
  end

  def seed_plaintext(value)
    @password = value
  end"#,
        )
        .write(
            "test/models/article_password_writer_test.rb",
            r#"require "test_helper"

class ArticlePasswordWriterTest < ActiveSupport::TestCase
  # Each assignment ends in nil, which the macro's writer stores
  # without reaching bcrypt (CI's unit job does not install it).
  test "super(x) passes x to the macro's writer" do
    article = articles(:one)
    article.seed_plaintext("seeded")
    assert !article.password_supplied?
    article.password = ""
    assert article.password_supplied?
    assert_nil article.password
  end

  test "a bare super passes the writer's own argument" do
    comment = comments(:one)
    comment.seed_plaintext("seeded")
    assert !comment.password_supplied?
    comment.password = nil
    assert comment.password_supplied?
    assert_nil comment.password
  end

  test "each secure-password attribute reaches its own macro writer" do
    article = articles(:one)
    article.seed_plaintext("seeded")
    article.password = ""
    assert article.password_supplied?
    assert_nil article.password
    assert_equal "seeded", article.recovery_plaintext
    assert !article.recovery_password_supplied?
    article.recovery_password = nil
    assert article.recovery_password_supplied?
    assert_nil article.recovery_plaintext
    assert_nil article.password
  end
end
"#,
        )
        .run_test("test/models/article_password_writer_test.rb")
        .assert_passes();
}

/// `validate :m` with Rails 7.1's password-change guard shape: a Symbol
/// dirty predicate as `if:` and a lambda as `unless:` (campfire's
/// `password_challenge_matches`), `on: :create` (campfire's Session),
/// `password_challenge` assigned through `update`, and an empty password
/// leaving the digest as it was. Each assignment stays off bcrypt (CI's
/// unit job does not install it).
#[test]
fn validate_guards_and_the_password_challenge_run() {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "create_table \"articles\", force: :cascade do |t|",
            "create_table \"articles\", force: :cascade do |t|\n    t.string \"password_digest\"",
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            r#"class Article < ApplicationRecord
  has_many :comments, dependent: :destroy
  has_secure_password validations: false
  validate :challenge_matches, if: :will_save_change_to_title?, unless: -> { password_challenge.nil? }
  validate :body_free, on: :create

  def challenge_matches
    errors.add(:password_challenge, :invalid) unless password_challenge == "ok"
  end

  def body_free
    errors.add(:body, "is taken") if body == "only on create"
  end"#,
        )
        .write(
            "test/models/article_validate_guards_test.rb",
            r#"require "test_helper"

class ArticleValidateGuardsTest < ActiveSupport::TestCase
  test "the challenge is checked only when the guarded column changes and a challenge is given" do
    id = articles(:one).id
    assert_not Article.find(id).update(title: "A new title", password_challenge: "wrong")
    assert Article.find(id).update(title: "A new title", password_challenge: "ok")
    assert Article.find(id).update(title: "Another title")
    assert Article.find(id).update(body: "A different long body", password_challenge: "wrong")
  end

  test "on: :create runs before the first save only" do
    assert_not Article.new(title: "Fresh", body: "only on create").save
    assert articles(:one).update(body: "only on create")
  end

  test "an empty password leaves the digest as it was" do
    article = articles(:one)
    article.update!(password_digest: "stored digest")
    article.update!(password: "")
    assert_equal "stored digest", Article.find(article.id).password_digest
  end
end
"#,
        )
        .run_test("test/models/article_validate_guards_test.rb")
        .assert_passes();
}

/// `flash` in an integration test is the last request's flash, read
/// after a redirect the test does not follow (campfire's profile tests:
/// `assert_equal "✓", flash[:notice]`).
#[test]
fn an_integration_test_reads_the_last_requests_flash() {
    emit_and_run::real_blog()
        .write(
            "test/controllers/articles_flash_controller_test.rb",
            r#"require "test_helper"

class ArticlesFlashControllerTest < ActionDispatch::IntegrationTest
  test "the redirect's notice is the flash" do
    post articles_url, params: { article: { title: "Flashed", body: "A sufficiently long article body." } }
    assert_redirected_to article_url(Article.find_by(title: "Flashed"))
    assert_equal "Article was successfully created.", flash[:notice]
    assert_nil flash[:alert]
  end
end
"#,
        )
        .run_test("test/controllers/articles_flash_controller_test.rb")
        .assert_passes();
}

/// A model class method a scope reaches (`titled.find_by_slug(s)`)
/// takes the relation as a trailing `__rel`, so the call carries one
/// argument more than the test's `stubs(...).with(s)` names; the ruby
/// lanes' mocha replay still matches it (campfire's
/// `User.stubs(:find_by_transfer_id).with(transfer_id)` against
/// `User.active.find_by_transfer_id(params[:id])`), and the named
/// argument still tells two stubs apart.
#[test]
#[ignore = "requires the mocha gem"]
fn a_stub_with_args_matches_a_scope_reached_class_method() {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            r#"class Article < ApplicationRecord
  has_many :comments, dependent: :destroy
  scope :titled, -> { where.not(title: nil) }

  def self.find_by_slug(slug)
    where(title: slug).first
  end

  def self.titled_slug(slug)
    titled.find_by_slug(slug)
  end"#,
        )
        .write(
            "test/models/article_scoped_stub_test.rb",
            r#"require "test_helper"

class ArticleScopedStubTest < ActiveSupport::TestCase
  test "the stub answers the scoped call it names" do
    other = articles(:two)
    Article.stubs(:find_by_slug).with("a-slug").returns(other)
    Article.stubs(:find_by_slug).with("another").returns(articles(:one))
    assert_equal other.id, Article.titled_slug("a-slug").id
    assert_equal other.id, Article.find_by_slug("a-slug").id
    assert_equal articles(:one).id, Article.titled_slug("another").id
  end
end
"#,
        )
        .run_test("test/models/article_scoped_stub_test.rb")
        .assert_passes();
}

#[path = "emit_and_run/relation_finders.rs"]
mod relation_finders;
#[path = "emit_and_run/attach_hash.rs"]
mod attach_hash;
#[path = "emit_and_run/many_attached.rs"]
mod many_attached;

/// A controller under `ActionController::API`, the base `rails new
/// --api` writes, dispatches (#163). The runtime defined only `Base`,
/// so the ruby tree raised NameError loading ApplicationController, and
/// every request to the spinel binary answered 500 (`undefined method
/// 'params='`). The `before_action` reads `params[:id]`, which is the
/// writer the spinel dispatcher failed on.
#[test]
fn an_action_controller_api_controller_dispatches() {
    emit_and_run::empty_app()
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::API\nend\n",
        )
        .write(
            "app/controllers/widgets_controller.rb",
            r#"class WidgetsController < ApplicationController
  before_action :set_widget, only: :show

  def index
    head :no_content
  end

  def show
    head :not_found unless @widget
  end

  private

  def set_widget
    @widget = Widget.find_by(id: params[:id])
  end
end
"#,
        )
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n")
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\n  root \"widgets#index\"\n  resources :widgets, only: %i[index show]\nend\n",
        )
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema[8.1].define(version: 2026_01_01_000000) do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n",
        )
        .run_ruby(
            r#"widget = Widget.create!(name: "a")
def get(path)
  status, = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => path, "QUERY_STRING" => "", "rack.input" => StringIO.new(""))
  status
end
{ "/widgets" => 204, "/widgets/#{widget.id}" => 204, "/widgets/#{widget.id + 1}" => 404 }.each do |path, want|
  got = get(path)
  raise "GET #{path} answered #{got}, want #{want}" unless got == want
end
"#,
        )
        .assert_passes();
}

#[path = "emit_and_run/pagy.rs"]
mod pagy;
#[path = "emit_and_run/string_bytes.rs"]
mod string_bytes;

/// A Slim view is ingested rather than skipped, so `check` going quiet on
/// it is a claim the emitted page renders. Swap the blog's index for a
/// Slim twin that exercises the grammar (shortcuts merging with a
/// `class=`, Ruby and boolean attributes, `tag: child` nesting, output
/// and code lines with a block, `|` text, comments) and assert on the
/// rendered markup, not just the status.
#[test]
fn a_slim_view_renders() {
    let slim = r#"= turbo_stream_from "articles"
- content_for :title, "Articles"
- turbo_exempts_page_from_cache

/ never rendered
.w-full
  - if notice.present?
    p.py-2#notice = notice
  .flex.justify-between
    h1.font-bold.text-4xl Articles
    = link_to "New article", new_article_path, class: "rounded-md"
  #articles.min-w-full class="space-y-5" data-count=@articles.size
    - if @articles.any?
      = render @articles
    - else
      p.text-center No articles found.
  ul.slim-list(data-kind="list" hidden)
    - @articles.each do |article|
      li: a href=article_path(article) = article.title
  p.slim-text
    | plain text
"#;
    emit_and_run::real_blog()
        .remove("app/views/articles/index.html.erb")
        .write("app/views/articles/index.html.slim", slim)
        .edit(
            "test/controllers/articles_controller_test.rb",
            "    assert_select \"h1\", \"Articles\"\n",
            "    assert_select \"h1.font-bold\", \"Articles\"\n    \
             assert_select \"#articles.min-w-full.space-y-5[data-count]\"\n    \
             assert_select \"ul.slim-list[data-kind=list][hidden] li a\", minimum: 1\n    \
             assert_select \"p.slim-text\", \"plain text\"\n",
        )
        .run_test("test/controllers/articles_controller_test.rb")
        .assert_passes();
}

/// A view directory with a hyphen (`product-item/`) is legal in Rails and
/// common in real apps, but `Views::Product-item` and a `product-item`
/// parameter are not Ruby. The emitted partial must load and render.
#[test]
fn a_hyphenated_view_directory_renders() {
    emit_and_run::real_blog()
        .write(
            "app/views/note-card/_note.html.erb",
            "<aside class=\"note-card\"><%= note %></aside>\n",
        )
        .edit(
            "app/views/articles/index.html.erb",
            "<div class=\"w-full\">\n",
            "<div class=\"w-full\">\n  <%= render \"note-card/note\", note: \"hyphen ok\" %>\n",
        )
        .edit(
            "test/controllers/articles_controller_test.rb",
            "    assert_select \"h1\", \"Articles\"\n",
            "    assert_select \"h1\", \"Articles\"\n    assert_select \"aside.note-card\", \"hyphen ok\"\n",
        )
        .run_test("test/controllers/articles_controller_test.rb")
        .assert_passes();
}

/// Rails' own guards against a request-steered header, ahead of the
/// server's (which drops any header holding a control character):
/// `redirect_to` deletes CR and LF from the location
/// (`_compute_redirect_to_location`, actionpack 8.1), and Active
/// Storage serves only `inline` or `attachment`, whatever disposition a
/// URL asks for (`content_disposition_with`, activestorage 8.1) — the
/// blob redirect route takes it from a query param and signs it into
/// the disk URL whose Content-Disposition it becomes.
fn header_values_app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("db/schema.rb", r#"ActiveRecord::Schema.define do
  create_table "docs", force: :cascade do |t|
    t.string "name"
  end
  create_table "active_storage_blobs", force: :cascade do |t|
    t.string "key", null: false
    t.string "filename", null: false
    t.string "content_type"
    t.text "metadata"
    t.string "service_name", null: false
    t.bigint "byte_size", null: false
    t.string "checksum"
    t.datetime "created_at", null: false
  end
  create_table "active_storage_attachments", force: :cascade do |t|
    t.string "name", null: false
    t.string "record_type", null: false
    t.bigint "record_id", null: false
    t.bigint "blob_id", null: false
    t.datetime "created_at", null: false
  end
end
"#)
        .write("app/models/doc.rb", "class Doc < ApplicationRecord\n  has_one_attached :file\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\n  get \"/bounce\", to: \"docs#bounce\"\nend\n")
        .write("app/controllers/docs_controller.rb", r#"class DocsController < ApplicationController
  def bounce
    redirect_to params[:back]
  end
end
"#)
}

#[test]
fn request_steered_header_values_stay_one_line() {
    header_values_app()
        .run_ruby(r#"
require_relative "app/controllers/docs_controller"
controller = DocsController.new
controller.params = { "back" => "/next\r\nSet-Cookie: pwned=1" }
controller.process_action(:bounce)
location = controller.location.to_s
raise "CR/LF reached the Location: #{location.inspect}" if location.include?("\r") || location.include?("\n")
raise "the rest of the location is kept, as Rails keeps it: #{location.inspect}" unless location == "/nextSet-Cookie: pwned=1"

asked = ActiveStorage::DiskKey.decode(ActiveStorage::DiskKey.encode("k", "attachment\r\nSet-Cookie: pwned=1"))
raise "an unknown disposition was signed as asked: #{asked.inspect}" unless asked == ["k", "inline"]
kept = ActiveStorage::DiskKey.decode(ActiveStorage::DiskKey.encode("k", "attachment"))
raise "attachment is a disposition: #{kept.inspect}" unless kept == ["k", "attachment"]
puts "header values passed"
"#)
        .assert_passes();
}

fn query_value_app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\n  protect_from_forgery with: :exception\nend\n")
        .write("db/schema.rb", r#"ActiveRecord::Schema.define do
  create_table "widgets", force: :cascade do |t|
    t.string "name"
  end
end
"#)
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n")
        .write("config/routes.rb", r#"Rails.application.routes.draw do
  get "/limited", to: "widgets#limited"
  get "/paged", to: "widgets#paged"
  get "/sorted", to: "widgets#sorted"
  get "/sorted_by", to: "widgets#sorted_by"
  get "/sorted_str", to: "widgets#sorted_str"
  get "/bounce", to: "widgets#bounce"
  post "/touch", to: "widgets#touch"
  get "/headed", to: "widgets#headed"
  get "/tails", to: "widgets#tails"
end
"#)
        .write("app/controllers/widgets_controller.rb", r#"class WidgetsController < ApplicationController
  def limited
    render plain: Widget.order(:name).limit(params[:n]).map { |w| w.name }.join(",")
  end

  def paged
    render plain: Widget.order(:name).offset(params[:skip]).map { |w| w.name }.join(",")
  end

  def sorted
    render plain: Widget.order(name: params[:dir]).map { |w| w.name }.join(",")
  end

  def sorted_by
    render plain: Widget.order(params[:sort] => :asc).map { |w| w.name }.join(",")
  end

  def sorted_str
    render plain: Widget.order(params[:sort]).map { |w| w.name }.join(",")
  end

  def bounce
    redirect_to params[:back]
  end

  def touch
    render plain: "ok"
  end

  def headed
    head :created, location: params[:back]
  end

  def tails
    render plain: Widget.order(:name).first(1).map { |w| w.name }.join(",") + Widget.order(:name).last(1).map { |w| w.name }.join(",")
  end
end
"#)
}

fn query_value_assertions() -> &'static str {
    r#"
require_relative "app/controllers/widgets_controller"
ActionController::Base.allow_forgery_protection = false
Widget.create!(name: "beta")
Widget.create!(name: "alpha")
Widget.create!(name: "gamma")

def run(action, params)
  controller = WidgetsController.new
  controller.request_method = "GET"
  controller.params = params
  controller.process_action(action)
  controller.body
end

def rejected(action, params)
  "ran: " + run(action, params)
rescue ArgumentError
  "rejected"
end

got = run(:limited, { "n" => "2" })
raise "a numeric String limit is Rails' Integer(): #{got}" unless got == "alpha,beta"
got = rejected(:limited, { "n" => "(SELECT COUNT(*) FROM widgets)" })
raise "LIMIT took SQL: #{got}" unless got == "rejected"

got = run(:paged, { "skip" => "1" })
raise "a numeric String offset is Rails' to_i: #{got}" unless got == "beta,gamma"
got = run(:paged, { "skip" => "(SELECT 2)" })
raise "OFFSET took SQL: #{got}" unless got == "alpha,beta,gamma"
got = run(:paged, { "skip" => "1; SELECT 1" })
raise "OFFSET to_i prefix: #{got}" unless got == "beta,gamma"

got = run(:sorted, { "dir" => "desc" })
raise "a String direction: #{got}" unless got == "gamma,beta,alpha"
got = rejected(:sorted, { "dir" => "asc, (SELECT 1)" })
raise "ORDER direction took SQL: #{got}" unless got == "rejected"

got = run(:sorted_by, { "sort" => "name" })
raise "a String column key: #{got}" unless got == "alpha,beta,gamma"
got = rejected(:sorted_by, { "sort" => "(SELECT 1)" })
raise "ORDER column took SQL: #{got}" unless got == "rejected"
got = run(:sorted_by, { "sort" => "widgets.name" })
raise "table.col hash key: #{got}" unless got == "alpha,beta,gamma"

got = run(:sorted_str, { "sort" => "name desc" })
raise "string order: #{got}" unless got == "gamma,beta,alpha"
got = rejected(:sorted_str, { "sort" => "id DESC, (SELECT 1)" })
raise "string ORDER took SQL: #{got}" unless got == "rejected"
got = Widget.all.order("LOWER(name)").map { |w| w.name }.join(",")
raise "LOWER(name) order: #{got}" unless got == "alpha,beta,gamma"
got = Widget.all.order("RANDOM()").map { |w| w.name }.length
raise "RANDOM() order rejected" unless got == 3
got = rejected(:sorted_str, { "sort" => "SLEEP()" })
raise "SLEEP() order: #{got}" unless got == "rejected"
got = rejected(:sorted_str, { "sort" => "LOWER(name); SELECT 1" })
raise "LOWER plus splice: #{got}" unless got == "rejected"

rel = Widget.all.order(:name)
begin
  rel.last_n("(SELECT 1)")
  raise "last_n accepted SQL"
rescue ArgumentError
  got = rel.order(:name).map { |w| w.name }.join(",")
  raise "last_n mutated orders: #{got}" unless got == "alpha,beta,gamma"
end
got = Widget.all.order(:name).first_n("2").map { |w| w.name }.join(",")
raise "first_n string: #{got}" unless got == "alpha,beta"
got = Widget.all.order(:name).limit(2.9).map { |w| w.name }.join(",")
raise "float limit truncate: #{got}" unless got == "alpha,beta"

puts "query values passed"
"#
}

#[test]
fn query_params_are_values_not_sql() {
    query_value_app()
        .run_ruby(query_value_assertions())
        .assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn query_params_are_values_not_sql_on_spinel() {
    let script = format!(
        "Db.configure(\":memory:\")\nSchema.statements.each {{ |sql| Db.exec(sql) }}\nActiveRecord.adapter = SqliteAdapter\n{}",
        query_value_assertions()
    );
    query_value_app().run_spinel(&script).assert_passes();
}

#[test]
fn request_steered_head_and_headers_stay_one_line() {
    query_value_app()
        .run_ruby(r#"
require_relative "app/controllers/widgets_controller"
ActionController::Base.allow_forgery_protection = false
controller = WidgetsController.new
controller.params = { "back" => "/next\r\nSet-Cookie: pwned=1" }
controller.process_action(:headed)
location = controller.location.to_s
raise "head location kept CR/LF: #{location.inspect}" if location.include?("\r") || location.include?("\n")

controller = WidgetsController.new
controller.headers["X-Link"] = "a\r\nSet-Cookie: pwned=1"
raise "CR/LF header was stored" unless controller.headers["X-Link"].nil?

controller.headers["X-Ok"] = "one-line"
raise "legal header dropped" unless controller.headers["X-Ok"] == "one-line"

controller.headers["X-Rev"] = nil
raise "nil header write stored a value" unless controller.headers["X-Rev"].nil?
raise "nil header wiped a sibling" unless controller.headers["X-Ok"] == "one-line"
puts "head and headers passed"
"#)
        .assert_passes();
}

#[test]
fn redirect_to_rejects_an_unvalidated_host() {
    query_value_app()
        .run_ruby(r#"
require_relative "app/controllers/widgets_controller"
ActionController::Base.allow_forgery_protection = false
controller = WidgetsController.new
controller.request = ActionDispatch::TestRequest.create("HTTP_HOST" => "app.example")
controller.request_method = "GET"
controller.params = { "back" => "http://evil.example/" }
begin
  controller.process_action(:bounce)
  raise "open redirect ran: #{controller.location.inspect}"
rescue ArgumentError
end

controller = WidgetsController.new
controller.request = ActionDispatch::TestRequest.create("HTTP_HOST" => "app.example")
controller.request_method = "GET"
controller.params = { "back" => "/home" }
controller.process_action(:bounce)
raise "relative redirect stays a path: #{controller.location.inspect}" unless controller.location == "http://app.example/home"

controller = WidgetsController.new
controller.request = ActionDispatch::TestRequest.create("HTTP_HOST" => "app.example")
controller.request_method = "GET"
controller.params = { "back" => "http://app.example/ok" }
controller.process_action(:bounce)
raise "same-host absolute refused: #{controller.location.inspect}" unless controller.location == "http://app.example/ok"

controller = WidgetsController.new
controller.request = ActionDispatch::TestRequest.create("HTTP_HOST" => "evil.example")
controller.request_method = "GET"
controller.session[:return_to_after_authenticating] = controller.request.url
controller.params = { "back" => controller.session[:return_to_after_authenticating] }
controller.request = ActionDispatch::TestRequest.create("HTTP_HOST" => "app.example")
controller.request_method = "GET"
begin
  controller.process_action(:bounce)
  raise "spoofed request.url honored: #{controller.location.inspect}"
rescue ArgumentError
end

controller = WidgetsController.new
controller.request = ActionDispatch::TestRequest.create("HTTP_HOST" => "app.example")
controller.request_method = "GET"
controller.params = { "back" => "/\\evil.example" }
begin
  controller.process_action(:bounce)
  raise "backslash host honored: #{controller.location.inspect}"
rescue ArgumentError
end

controller = WidgetsController.new
controller.request = ActionDispatch::TestRequest.create("HTTP_HOST" => "app.example")
controller.request_method = "GET"
controller.params = { "back" => "///evil.example" }
begin
  controller.process_action(:bounce)
  raise "triple-slash honored: #{controller.location.inspect}"
rescue ArgumentError
end

controller = WidgetsController.new
controller.request = ActionDispatch::TestRequest.create("HTTP_HOST" => "app.example")
controller.request_method = "GET"
controller.params = { "back" => "/\t/evil.example" }
begin
  controller.process_action(:bounce)
  raise "tab host honored: #{controller.location.inspect}"
rescue ArgumentError
end
puts "open redirect passed"
"#)
        .assert_passes();
}

#[test]
fn csrf_rejects_a_post_without_the_session_token() {
    query_value_app()
        .run_ruby(r#"
require_relative "app/controllers/widgets_controller"
ActionController::Base.allow_forgery_protection = true
controller = WidgetsController.new
ActionController::Current.controller = controller
req = ActionDispatch::TestRequest.create("HTTP_HOST" => "app.example", "REQUEST_METHOD" => "POST")
controller.request = req
ActionController::Current.request = req
controller.request_method = "POST"
controller.params = {}
controller.process_action(:touch)
raise "empty CSRF ran: #{controller.status}" unless controller.status == 422

controller = WidgetsController.new
ActionController::Current.controller = controller
req = ActionDispatch::TestRequest.create("HTTP_HOST" => "app.example", "REQUEST_METHOD" => "GET")
controller.request = req
ActionController::Current.request = req
controller.request_method = "GET"
masked = ActionView::ViewHelpers.form_authenticity_token
secret = controller.session[:_csrf_token].to_s
raise "session secret not minted: #{secret.inspect}" if secret.empty?
raise "token was the raw secret: #{masked.inspect}" if masked == secret
raise "masked token did not verify" unless ActionController::AuthenticityToken.valid?(masked, secret)

controller = WidgetsController.new
ActionController::Current.controller = controller
req = ActionDispatch::TestRequest.create("HTTP_HOST" => "app.example", "REQUEST_METHOD" => "POST")
controller.request = req
ActionController::Current.request = req
controller.request_method = "POST"
controller.session[:_csrf_token] = secret
controller.params = { "authenticity_token" => masked }
controller.process_action(:touch)
raise "matching masked CSRF failed: #{controller.status} #{controller.body}" unless controller.status == 200 && controller.body == "ok"

controller = WidgetsController.new
ActionController::Current.controller = controller
req = ActionDispatch::TestRequest.create("HTTP_HOST" => "app.example", "REQUEST_METHOD" => "POST")
controller.request = req
ActionController::Current.request = req
controller.request_method = "POST"
controller.session[:_csrf_token] = secret
controller.params = { "authenticity_token" => secret }
controller.process_action(:touch)
raise "unmasked session token failed: #{controller.status}" unless controller.status == 200

controller = WidgetsController.new
ActionController::Current.controller = controller
req = ActionDispatch::TestRequest.create("HTTP_HOST" => "app.example", "REQUEST_METHOD" => "POST", "HTTP_X_CSRF_TOKEN" => masked)
controller.request = req
ActionController::Current.request = req
controller.request_method = "POST"
controller.session[:_csrf_token] = secret
controller.params = {}
controller.process_action(:touch)
raise "X-CSRF-Token header failed: #{controller.status}" unless controller.status == 200 && controller.body == "ok"
puts "csrf passed"
"#)
        .assert_passes();
}

#[test]
fn csrf_implicit_default_and_skip_before_action_run() {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n")
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\n  post \"/touch\", to: \"widgets#touch\"\n  post \"/open\", to: \"open#touch\"\nend\n")
        .write("app/controllers/widgets_controller.rb", "class WidgetsController < ApplicationController\n  def touch\n    render plain: \"ok\"\n  end\nend\n")
        .write("app/controllers/open_controller.rb", "class OpenController < ApplicationController\n  skip_before_action :verify_authenticity_token\n  def touch\n    render plain: \"open\"\n  end\nend\n")
        .run_ruby(r#"
require_relative "app/controllers/widgets_controller"
require_relative "app/controllers/open_controller"
ActionController::Base.allow_forgery_protection = true
controller = WidgetsController.new
ActionController::Current.controller = controller
req = ActionDispatch::TestRequest.create("HTTP_HOST" => "app.example", "REQUEST_METHOD" => "POST")
controller.request = req
ActionController::Current.request = req
controller.request_method = "POST"
controller.params = {}
controller.process_action(:touch)
raise "implicit CSRF ran: #{controller.status}" unless controller.status == 422

controller = OpenController.new
ActionController::Current.controller = controller
req = ActionDispatch::TestRequest.create("HTTP_HOST" => "app.example", "REQUEST_METHOD" => "POST")
controller.request = req
ActionController::Current.request = req
controller.request_method = "POST"
controller.params = {}
controller.process_action(:touch)
raise "skip_before_action did not skip: #{controller.status} #{controller.body}" unless controller.status == 200 && controller.body == "open"
puts "csrf skip passed"
"#)
        .assert_passes();
}

/// A prior explicit include is a no-op when an included block repeats it.
/// The Concern itself never gains the nested module as an ancestor.
#[test]
fn a_repeated_included_block_include_preserves_host_and_concern_ancestry() {
    emit_and_run::real_blog()
        .write(
            "app/models/concerns/signing.rb",
            "module Signing\n  extend ActiveSupport::Concern\n  included do\n    include Signing::Codes\n  end\n  def shout\n    \"outer\"\n  end\nend\n",
        )
        .write(
            "app/models/concerns/signing/codes.rb",
            "module Signing::Codes\n  extend ActiveSupport::Concern\n  def shout\n    \"inner\"\n  end\nend\n",
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n  include Signing::Codes\n  include Signing\n",
        )
        .run_ruby(
            r#"raise "concern ancestry changed" if Signing.ancestors.include?(Signing::Codes)
a = Article.new(title: "Hi", body: "Body text here")
raise "repeated include changed precedence" unless a.shout == "outer"
raise "host ancestry changed" unless Article.ancestors.index(Signing) < Article.ancestors.index(Signing::Codes)
"#,
        )
        .assert_passes();
}

#[test]
fn a_shared_factory_respects_an_overridden_constructor() {
    emit_and_run::real_blog()
        .write("app/services/custom_factory.rb", r#"module CustomFactory
  class_methods do
    def build
      new
    end
  end
end
class FactoryReading < T::Struct
  const :label, String
end
class FactoryPacket
  include CustomFactory
  def self.new
    FactoryReading.new(label: "custom")
  end
end
class FactoryConsumer
  def self.label
    FactoryPacket.build.label.upcase
  end
end
"#)
        .run_ruby(r#"
raise "constructor identity" unless FactoryPacket.build.class == FactoryReading
raise "constructor consumer" unless FactoryConsumer.label == "CUSTOM"
"#)
        .assert_passes();
}

/// Unlike a hash pattern with keys, `{}` requires the hash to be empty.
/// A bare `is_a?(Hash)` check silently chose the wrong case arm for
/// every nonempty hash. `**` explicitly permits the remaining keys.
#[test]
fn an_empty_hash_pattern_rejects_extra_keys() {
    emit_and_run::real_blog()
        .write(
            "app/helpers/hash_pattern_probe.rb",
            r#"class HashPatternProbe
  #: (Hash[Symbol, Integer]) -> bool
  def self.empty_match(value)
    case value
    in {}
      true
    else
      false
    end
  end

  #: (Hash[Symbol, Integer]) -> bool
  def self.open_match(value)
    case value
    in { ** }
      true
    else
      false
    end
  end

  #: (Hash[Symbol, Integer]) -> bool
  def self.key_match(value)
    case value
    in { x: 1 }
      true
    else
      false
    end
  end
end
"#,
        )
        .run_ruby(
            r#"raise "empty hash did not match" unless HashPatternProbe.empty_match({})
raise "nonempty hash incorrectly matched {}" if HashPatternProbe.empty_match({ x: 1 })
raise "open hash pattern rejected extra keys" unless HashPatternProbe.open_match({ x: 1 })
raise "keyed pattern rejected extra keys" unless HashPatternProbe.key_match({ x: 1, y: 2 })
raise "keyed pattern accepted a missing key" if HashPatternProbe.key_match({ y: 2 })
"#,
        )
        .assert_passes();
}

#[test]
fn model_rest_and_block_parameters_run_with_their_source_arity() {
    emit_and_run::real_blog()
        .edit("app/models/article.rb", "class Article < ApplicationRecord\n", r#"class Article < ApplicationRecord
  def tagged(*labels)
    labels.join(",")
  end
  def pair(first, *rest, last)
    [first, rest.join(","), last].join("|")
  end
  def each_title(&blk)
    [title, "tail"].each(&blk)
  end
  def forward_titles(...)
    each_title(...)
  end
  def both(*args, **opts)
    [args.join(","), opts[:tag]].join("|")
  end
"#)
        .write("app/services/rest_control.rb", r#"class RestControl
  def tagged(*labels)
    labels.join(",")
  end
end
"#)
        .run_ruby(r#"
article = Article.new(title: "source")
control = RestControl.new
raise "model rest" unless article.tagged("x", "y") == "x,y"
raise "empty rest" unless article.tagged == ""
raise "library control" unless control.tagged("x", "y") == article.tagged("x", "y")
raise "post parameter" unless article.pair("head", "a", "b", "last") == "head|a,b|last"
raise "empty post rest" unless article.pair("head", "last") == "head||last"
seen = []
result = article.each_title { |value| seen << value.upcase }
raise "block values" unless seen == ["SOURCE", "TAIL"]
raise "block return" unless result == ["source", "tail"]
forwarded = []
article.forward_titles { |value| forwarded << value.upcase }
raise "forwarded block preservation" unless forwarded == seen
raise "rest with keywords" unless article.both("x", "y", tag: "z") == "x,y|z"
raise "empty positional rest" unless article.both(tag: "z") == "|z"
"#).assert_passes();
}

#[test]
fn duplicate_route_only_options_use_the_last_value_at_runtime() {
    emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "resources :articles do",
            "resources :articles, only: [], only: [:index, :show, :new, :create, :edit, :update, :destroy] do",
        )
        .run_test("test/controllers/articles_controller_test.rb")
        .assert_passes();
}

#[test]
fn duplicate_route_except_options_use_the_last_value_at_runtime() {
    emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "resources :articles do",
            "resources :articles, except: [:show], except: [] do",
        )
        .run_test("test/controllers/articles_controller_test.rb")
        .assert_passes();
}

#[test]
fn rubydex_qualified_value_constants_survive_shared_lowerings() {
    emit_and_run::real_blog()
        .write("app/services/collection_constants.rb", r#"
class CollectionConstants
  WORDS = ["a", "bb"]
  LENGTHS = WORDS.index_by(&:length)
  def self.values
    [LENGTHS[2], "a".in?(WORDS)]
  end
end
"#)
        .run_ruby("raise 'qualified lowered constants' unless CollectionConstants.values == ['bb', true]")
        .assert_passes();
}

/// `invisible_captcha only: :create` → before_action that heads :ok when
/// a honeypot field is filled (invariant 6: the survey gap closing is a
/// claim the emitted gate runs).
#[test]
fn invisible_captcha_blocks_spam_posts() {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\n  protect_from_forgery with: :exception\nend\n")
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"users\", force: :cascade do |t|\n    t.string \"email\"\n  end\nend\n")
        .write("app/models/user.rb", "class User < ApplicationRecord\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\n  post \"/users\", to: \"users#create\"\nend\n")
        .write(
            "app/controllers/users_controller.rb",
            "class UsersController < ApplicationController\n  invisible_captcha only: :create\n\n  def create\n    render plain: \"created\"\n  end\nend\n",
        )
        .run_ruby(r#"
require_relative "app/controllers/users_controller"
ActionController::Base.allow_forgery_protection = false
controller = UsersController.new
ActionController::Current.controller = controller
req = ActionDispatch::TestRequest.create("HTTP_HOST" => "app.example", "REQUEST_METHOD" => "POST")
controller.request = req
ActionController::Current.request = req
controller.request_method = "POST"
controller.params = { "subtitle" => "http://spam.example" }
controller.process_action(:create)
raise "honeypot did not block: #{controller.status} #{controller.body}" unless controller.status == 200 && controller.body.to_s.empty?
controller = UsersController.new
ActionController::Current.controller = controller
req = ActionDispatch::TestRequest.create("HTTP_HOST" => "app.example", "REQUEST_METHOD" => "POST")
controller.request = req
ActionController::Current.request = req
controller.request_method = "POST"
controller.params = { "email" => "ok@example.com" }
controller.process_action(:create)
raise "clean post failed: #{controller.status} #{controller.body}" unless controller.status == 200 && controller.body == "created"
puts "invisible_captcha passed"
"#)
        .assert_passes();
}

/// `impersonates :user` wraps `current_user` and exposes pretender's
/// impersonate / stop helpers (invariant 6).
#[test]
fn impersonates_switches_current_user() {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\n  def current_user\n    User.find_by(id: session[:signed_in_user_id])\n  end\n\n  impersonates :user\nend\n",
        )
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"users\", force: :cascade do |t|\n    t.string \"email\"\n  end\nend\n")
        .write("app/models/user.rb", "class User < ApplicationRecord\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\n  get \"/who\", to: \"who#show\"\nend\n")
        .write(
            "app/controllers/who_controller.rb",
            "class WhoController < ApplicationController\n  def show\n    render plain: [true_user&.email, current_user&.email].join(\",\")\n  end\nend\n",
        )
        .run_ruby(r#"
require_relative "app/controllers/who_controller"
admin = User.create!(email: "admin@example.com")
other = User.create!(email: "other@example.com")
controller = WhoController.new
ActionController::Current.controller = controller
req = ActionDispatch::TestRequest.create("HTTP_HOST" => "app.example")
controller.request = req
ActionController::Current.request = req
controller.session[:signed_in_user_id] = admin.id
raise "baseline true=#{controller.true_user&.email} current=#{controller.current_user&.email}" unless controller.true_user&.email == "admin@example.com" && controller.current_user&.email == "admin@example.com"
controller.impersonate_user(other)
raise "impersonating true=#{controller.true_user&.email} current=#{controller.current_user&.email}" unless controller.true_user&.email == "admin@example.com" && controller.current_user&.email == "other@example.com"
controller.stop_impersonating_user
raise "stopped true=#{controller.true_user&.email} current=#{controller.current_user&.email}" unless controller.true_user&.email == "admin@example.com" && controller.current_user&.email == "admin@example.com"
puts "impersonates passed"
"#)
        .assert_passes();
}

/// Controller ivars written through `instance_variable_set` with a
/// statically resolvable name (literal, `controller_name`, or a helper
/// that inflects `self.class`) reach the template, and a template that
/// assigns an ivar reaches the layout — the same view context Rails
/// uses. Overlay is abstract; the forcing fixture is Writebook.
fn record_ivar_set_overlay() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit(
            "app/controllers/articles_controller.rb",
            "    def set_article\n      @article = Article.find(params.expect(:id))\n    end\n",
            r#"    def set_article
      instance_variable_set "@#{instance_name}", Article.find(params.expect(:id))
    end

    def instance_name
      controller_record_name.underscore
    end

    def controller_record_name
      self.class.to_s.remove("Controller").demodulize.singularize
    end
"#,
        )
        .edit(
            "app/views/articles/show.html.erb",
            "<% content_for :title, \"Showing article\" %>\n",
            "<% @section_class = \"reading\" %>\n<% content_for :title, \"Showing article\" %>\n",
        )
        .edit(
            "app/views/layouts/application.html.erb",
            "<main class=\"container mx-auto mt-28 px-5 flex flex-col\">",
            "<main class=\"container mx-auto mt-28 px-5 flex flex-col <%= @section_class %>\">",
        )
}

/// Rich text is SANITIZED when it renders, as Rails' `ActionText::Content
/// #to_s` does: the content partial is `render_action_text_content`,
/// which runs `sanitize_action_text_content` over the rendered
/// fragment. The stored HTML is whatever the editor (or an attacker
/// posting the form directly) sent, so serving it as stored is a
/// stored XSS on every page that renders `<%= record.rich_text %>`.
fn rich_text_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "  has_many :comments, dependent: :destroy\n",
            "  has_many :comments, dependent: :destroy\n  has_rich_text :content\n",
        )
        .edit(
            "db/schema.rb",
            "  add_foreign_key \"comments\", \"articles\"\n",
            "  create_table \"action_text_rich_texts\", force: :cascade do |t|\n    \
             t.string \"name\", null: false\n    \
             t.text \"body\"\n    \
             t.string \"record_type\", null: false\n    \
             t.bigint \"record_id\", null: false\n    \
             t.datetime \"created_at\", null: false\n    \
             t.datetime \"updated_at\", null: false\n  \
             end\n\n  \
             add_foreign_key \"comments\", \"articles\"\n",
        )
        .edit(
            "app/views/articles/show.html.erb",
            "  <h1 class=\"font-bold text-4xl\"><%= @article.title %></h1>\n",
            "  <h1 class=\"font-bold text-4xl\"><%= @article.title %></h1>\n  \
             <section id=\"rich\"><%= @article.content %></section>\n",
        )
}


#[test]
fn instance_variable_set_and_view_assigned_layout_ivar_run() {
    record_ivar_set_overlay()
        .run_test("test/controllers/articles_controller_test.rb")
        .assert_passes();
}

#[test]
fn controller_name_instance_variable_set_runs_on_show() {
    emit_and_run::real_blog()
        .edit(
            "app/controllers/articles_controller.rb",
            "    def set_article\n      @article = Article.find(params.expect(:id))\n    end\n",
            "    def set_article\n      instance_variable_set(\"@#{controller_name.singularize}\", Article.find(params.expect(:id)))\n    end\n",
        )
        .run_test("test/controllers/articles_controller_test.rb")
        .assert_passes();
}

#[test]
fn campfire_capture_stdlib_consts_run() {
    // Campfire tip's TimeLimitedVideoPreviewer#capture and web-push pool
    // name IO / Timeout / Process. Registering them clears check errors;
    // this pin proves the emitted Ruby actually runs those Consts
    // (invariant 6) — without claiming the full ActiveStorage capture
    // path (#557 lands VideoPreviewer separately).
    emit_and_run::real_blog()
        .write(
            "app/models/capture_stdlib_probe.rb",
            r#"class CaptureStdlibProbe
  def self.exercise
    timed_out = false
    begin
      Timeout.timeout(0.05) { sleep 1 }
    rescue Timeout::Error
      timed_out = true
    end
    raise "Timeout.timeout did not fire" unless timed_out

    pid = Process.pid
    raise "Process.pid" unless pid.is_a?(Integer) && pid > 0

    clock = Process.clock_gettime(Process::CLOCK_MONOTONIC)
    raise "CLOCK_MONOTONIC" unless clock.is_a?(Float)

    out = IO.popen(["echo", "hi"], in: IO::NULL, err: IO::NULL) { |io| io.read }
    raise "IO.popen" unless out.to_s.include?("hi")

    killed = false
    IO.popen(["sleep", "30"]) do |io|
      Process.kill(:KILL, io.pid)
      killed = true
    end
    raise "Process.kill" unless killed

    src = StringIO.new("xy")
    dst = StringIO.new
    n = IO.copy_stream(src, dst)
    raise "IO.copy_stream" unless n == 2 && dst.string == "xy"

    rescued = false
    begin
      raise SystemCallError, "x"
    rescue SystemCallError
      rescued = true
    end
    raise "SystemCallError" unless rescued

    rescued = false
    begin
      raise OpenSSL::SSL::SSLError, "x"
    rescue OpenSSL::SSL::SSLError
      rescued = true
    end
    raise "OpenSSL::SSL::SSLError" unless rescued

    # Vips::Error is registered for Campfire's attachment rescue; the
    # constant is supplied by ruby-vips at runtime, not by this probe.

    "ok"
  end
end
"#,
        )
        .run_ruby(
            r#"
raise "probe" unless CaptureStdlibProbe.exercise == "ok"
"#,
        )
        .assert_passes();
}

#[test]
fn campfire_video_preview_config_runs() {
    // Campfire tip initializer sets video_preview_arguments (gte(t,5))
    // and swaps previewers VideoPreviewer → TimeLimitedVideoPreviewer.
    // Suite asserts ActiveStorage.previewers / video_preview_arguments;
    // poster reads the vf filter from the same config.
    emit_and_run::real_blog()
        .write(
            "lib/rails_ext/time_limited_video_previewer.rb",
            r#"class TimeLimitedVideoPreviewer < ActiveStorage::Previewer::VideoPreviewer
  TIME_LIMIT = 10
end
"#,
        )
        .write(
            "config/initializers/extensions.rb",
            r#"Dir[Rails.root.join("lib/rails_ext/*.rb")].sort.each { |f| require f }
"#,
        )
        .write(
            "config/initializers/active_storage.rb",
            r#"require "rails_ext/time_limited_video_previewer"

Rails.application.configure do
  config.active_storage.video_preview_arguments =
    "-vf 'select=eq(n\\,0)+eq(key\\,1)+gt(scene\\,0.015)+gte(t\\,5),loop=loop=-1:size=2,trim=start_frame=1'" \
    " -frames:v 1 -f image2"

  config.active_storage.previewers = config.active_storage.previewers.map do |previewer|
    previewer == ActiveStorage::Previewer::VideoPreviewer ? TimeLimitedVideoPreviewer : previewer
  end
end
"#,
        )
        .run_ruby(
            r#"
raise "args" unless ActiveStorage.video_preview_arguments.include?("gte(t\\,5)")
raise "filter" unless ActiveStorage.video_preview_vf_filter.include?("gte(t\\,5)")
raise "previewers include" unless ActiveStorage.previewers.include?(TimeLimitedVideoPreviewer)
raise "previewers exclude" if ActiveStorage.previewers.include?(ActiveStorage::Previewer::VideoPreviewer)

# Rails' ActiveStorage.paths — mutable executable locations.
raise "paths default" unless ActiveStorage.paths.is_a?(Hash)
previous = ActiveStorage.paths[:ffmpeg]
ActiveStorage.paths[:ffmpeg] = "/tmp/fake-ffmpeg"
raise "paths assign" unless ActiveStorage.paths[:ffmpeg] == "/tmp/fake-ffmpeg"
ActiveStorage.paths[:ffmpeg] = previous

# TIME_LIMIT on the video subclass → class-side poster (preview_timeout
# override is the typed API; TIME_LIMIT is the campfire/stub_const path).
raise "TIME_LIMIT" unless TimeLimitedVideoPreviewer::TIME_LIMIT == 10
raise "preview_timeout default" unless TimeLimitedVideoPreviewer.preview_timeout.nil?
raise "time limit" unless ActiveStorage::Previewer.poster_time_limit == 10

# `-vf` must match as a whole option, not a prefix of `-vframes`.
def ActiveStorage.video_preview_arguments
  "-vframes 1 -vf 'scale=320:240' -f image2"
end
raise "vf vs vframes" unless ActiveStorage.video_preview_vf_filter == "scale=320:240"
"#,
        )
        .assert_passes();
}

/// Conditional GET: `fresh_when etag:` sets ETag and answers 304 when
/// `If-None-Match` matches (campfire CachedResponses native validators).
#[test]
fn fresh_when_etag_answers_not_modified() {
    emit_and_run::real_blog()
        .run_ruby(
            r##"
c = ActionController::Base.new
c.request = ActionDispatch::Request.new({ "HTTP_IF_NONE_MATCH" => "W/\"abc123\"" })
c.fresh_when(etag: "abc123", template: false)
raise "status #{c.status}" unless c.status == 304
raise "performed" unless c.performed?
raise "etag #{c.headers["ETag"].inspect}" unless c.headers["ETag"] == "W/\"abc123\""

fresh = ActionController::Base.new
fresh.request = ActionDispatch::Request.new({ "REQUEST_METHOD" => "GET" })
fresh.fresh_when(etag: "abc123", template: false)
raise "fresh status #{fresh.status}" unless fresh.status == 200
raise "fresh performed" if fresh.performed?
raise "fresh etag" unless fresh.headers["ETag"] == "W/\"abc123\""

other = ActionController::Base.new
other.request = ActionDispatch::Request.new({ "HTTP_IF_NONE_MATCH" => "W/\"old\"" })
other.fresh_when({ etag: "new" })
raise "mismatch status" unless other.status == 200
raise "mismatch etag" unless other.headers["ETag"] == "W/\"new\""

# Quoted opaque-tags may contain commas; blind split must not 304 on `"b"`.
comma = ActionController::Base.new
comma.request = ActionDispatch::Request.new({ "HTTP_IF_NONE_MATCH" => "\"a,b\"" })
comma.fresh_when(etag: "b", template: false)
raise "comma status #{comma.status}" unless comma.status == 200

# Wildcard list member matches any validator.
star = ActionController::Base.new
star.request = ActionDispatch::Request.new({ "HTTP_IF_NONE_MATCH" => "\"other\", *" })
star.fresh_when(etag: "abc123", template: false)
raise "star status #{star.status}" unless star.status == 304

# Unquoted tokens are not entity-tags.
bare = ActionController::Base.new
bare.request = ActionDispatch::Request.new({ "HTTP_IF_NONE_MATCH" => "abc123" })
bare.fresh_when(etag: "abc123", template: false)
raise "bare status #{bare.status}" unless bare.status == 200

# RFC 9110: 304 only for GET/HEAD — POST still sets ETag but stays 200.
post = ActionController::Base.new
post.request = ActionDispatch::Request.new({
  "REQUEST_METHOD" => "POST",
  "HTTP_IF_NONE_MATCH" => "W/\"abc123\"",
})
post.fresh_when(etag: "abc123", template: false)
raise "post status #{post.status}" unless post.status == 200
raise "post etag" unless post.headers["ETag"] == "W/\"abc123\""
raise "post performed" if post.performed?

# stale? on POST stays stale (true) so `if stale?` still runs the body.
stale_post = ActionController::Base.new
stale_post.request = ActionDispatch::Request.new({
  "REQUEST_METHOD" => "POST",
  "HTTP_IF_NONE_MATCH" => "W/\"abc123\"",
})
raise "stale? post" unless stale_post.stale?(etag: "abc123") == true
raise "stale? post performed" if stale_post.performed?

# Matching INM on GET: stale? is false AND the response is 304.
stale_fresh = ActionController::Base.new
stale_fresh.request = ActionDispatch::Request.new({
  "REQUEST_METHOD" => "GET",
  "HTTP_IF_NONE_MATCH" => "W/\"abc123\"",
})
raise "stale? match" unless stale_fresh.stale?(etag: "abc123") == false
raise "stale? match status #{stale_fresh.status}" unless stale_fresh.status == 304
raise "stale? match performed" unless stale_fresh.performed?

# Mismatch stays stale and unperformed so the action can render.
stale_miss = ActionController::Base.new
stale_miss.request = ActionDispatch::Request.new({
  "REQUEST_METHOD" => "GET",
  "HTTP_IF_NONE_MATCH" => "W/\"old\"",
})
raise "stale? miss" unless stale_miss.stale?(etag: "abc123") == true
raise "stale? miss performed" if stale_miss.performed?
raise "stale? miss etag" unless stale_miss.headers["ETag"] == "W/\"abc123\""
"##
        )
        .assert_passes();
}

/// `Model.none` and `where(id: [])` are Rails NullRelations: empty
/// terminals with zero adapter selects. Negation / `or` must not stick
/// or invert nullness (thermos H1).
#[test]
fn relation_none_and_empty_in_do_not_query() {
    emit_and_run::real_blog()
        .run_ruby(
            r##"
QUERIES = []
adapter = ActiveRecord.adapter
orig = adapter.method(:select_rows)
adapter.define_singleton_method(:select_rows) do |sql|
  QUERIES << sql
  orig.call(sql)
end

def assert_no_query(label)
  before = QUERIES.length
  yield
  n = QUERIES.length - before
  raise "#{label}: #{n} queries" if n > 0
end

row = Article.create!(title: "NullRel", body: "long enough body")

none = Article.none
raise "klass" unless none.klass == Article
assert_no_query("none.to_a") { raise "to_a" unless none.to_a.empty? }
assert_no_query("none.ids") { raise "ids" unless none.ids.empty? }
assert_no_query("none.empty?") { raise "empty?" unless none.empty? }
assert_no_query("none.exists?") { raise "exists?" if none.exists? }
assert_no_query("none.any?") { raise "any?" if none.any? }
assert_no_query("none.count") { raise "count" unless none.count == 0 }
assert_no_query("none.pluck") { raise "pluck" unless none.pluck(:id).empty? }

empty_in = Article.where(id: [])
assert_no_query("empty_in.to_a") { raise "empty_in to_a" unless empty_in.to_a.empty? }
assert_no_query("empty_in.ids") { raise "empty_in ids" unless empty_in.ids.empty? }
assert_no_query("empty_in.empty?") { raise "empty_in empty?" unless empty_in.empty? }
assert_no_query("empty_in.count") { raise "empty_in count" unless empty_in.count == 0 }

# Negated empty IN is a tautology — must not be NullRelation.
# (`where.not` lowers to Relation#not; call it directly in this MRI script.)
not_empty = Article.all.not(id: [])
raise "not null?" if not_empty.null_relation?
raise "not to_a" unless not_empty.to_a.map(&:id).include?(row.id)

excl = Article.all.excluding([])
raise "excluding null?" if excl.null_relation?
raise "excluding to_a" unless excl.to_a.map(&:id).include?(row.id)

# Left-null OR productive side yields the other (Rails NullRelation#or).
or_side = Article.none.or(Article.where(id: row.id))
raise "or null?" if or_side.null_relation?
raise "or to_a" unless or_side.to_a.map(&:id) == [row.id]

# Temporary find_by / find_sole_by(id: []) must not stick null on the receiver.
rel = Article.all.where("1=1")
raise "find_by empty" unless rel.find_by(id: []).nil?
raise "find_by sticky" if rel.null_relation?
raise "find_by still loads" unless rel.to_a.map(&:id).include?(row.id)
begin
  rel.find_sole_by(id: [])
  raise "find_sole_by should raise"
rescue ActiveRecord::RecordNotFound
end
raise "find_sole_by sticky" if rel.null_relation?

# merge(none) adopts null.
merged = Article.all.where("1=1").merge(Article.none)
raise "merge null?" unless merged.null_relation?
assert_no_query("merge.none.ids") { raise "merge ids" unless merged.ids.empty? }

# Empty-IN NullRelation must clear when unscope drops the contradiction
# (Thermos M1). `none.unscope(where: :id)` stays null — none's (1 = 0)
# is unattributed.
restored = Article.where(id: []).unscope(where: :id)
raise "unscope sticky null?" if restored.null_relation?
raise "unscope restores rows" unless restored.to_a.map(&:id).include?(row.id)
still_none = Article.none.unscope(where: :id)
raise "none unscope null?" unless still_none.null_relation?
assert_no_query("none.unscope.ids") { raise "none unscope ids" unless still_none.ids.empty? }
assert_no_query("none.limit.unscope.size") {
  n = Article.none.limit(1).unscope(where: :id).size
  raise "none limit size #{n}" unless n == 0
}

# Compound empty-IN keeps null when a sibling where is unscoped away.
compound = Article.where(id: []).where(title: "NullRel")
raise "compound null?" unless compound.null_relation?
kept = compound.unscope(where: :title)
raise "compound unscope cleared null?" unless kept.null_relation?
assert_no_query("compound.unscope.ids") { raise "compound ids" unless kept.ids.empty? }

# A value that literally contains "1=0" must not stick @null after the
# empty-IN clause is unscoped (structural tracking, not SQL text search).
titled = Article.create!(title: "1=0", body: "long enough body for pin")
sticky = Article.where(id: []).where(title: "1=0").unscope(where: :id)
raise "title 1=0 sticky null?" if sticky.null_relation?
raise "title 1=0 row" unless sticky.to_a.map(&:id).include?(titled.id)

# preloaded integer ids must not call .id on Fixnum.
pre = Article.all.preloaded([row.id], true)
raise "preloaded ids" unless pre.ids == [row.id]
"##
        )
        .assert_passes();
}

#[test]
fn an_rbs_array_block_runs_after_app_emission() {
    emit_and_run::real_blog()
        .write("app/lib/batch.rb", runtime_block_signature::RUBY)
        .write("sig/batch.rbs", runtime_block_signature::RBS)
        .run_ruby("raise 'wrong sum' unless Batch.new.consume == 3")
        .assert_passes();
}

/// `pairs.to_h` with no block reads each element as a [key, value] pair; the
/// ivar rewritten through it keeps its String keys and Integer values.
#[test]
fn an_ivar_rewritten_through_sort_by_to_h_runs() {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n  def word_counts\n    @word_counts = {}\n    body.split.each do |w|\n      @word_counts[w] = 0 if @word_counts[w].nil?\n      @word_counts[w] += 1\n    end\n    @word_counts = @word_counts.sort_by { |k, v| [-v, k] }.to_h\n  end\n\n  def top_word\n    word_counts.keys.first\n  end\n",
        )
        .run_ruby(
            r#"article = Article.create!(title: "Counts", body: "b a b c b a")
counts = article.word_counts
raise counts.inspect unless counts == { "b" => 3, "a" => 2, "c" => 1 }
raise article.top_word.inspect unless article.top_word == "b"
"#,
        )
        .assert_passes();
}

/// A class an initializer defines and the app reads only through `[]`
/// (forem's `ApplicationConfig["KEY"]`) is the app's, as `X.` and `X::` are.
#[test]
fn an_initializer_class_read_through_brackets_runs() {
    emit_and_run::real_blog()
        .write(
            "config/initializers/app_settings.rb",
            "class AppSettings\n  DEFAULTS = { \"BANNER\" => \"Welcome\" }.freeze\n\n  def self.[](key)\n    DEFAULTS.fetch(key, \"\")\n  end\nend\n",
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n  def banner\n    \"#{AppSettings[\"BANNER\"]}: #{title}\"\n  end\n",
        )
        .run_ruby(
            r#"article = Article.create!(title: "Brackets", body: "Body text here")
raise article.banner.inspect unless article.banner == "Welcome: Brackets"
"#,
        )
        .assert_passes();
}

/// Array `&` and `|` are set intersection and union in Ruby. The
/// typed targets used to print their native operators: bitwise on
/// TypeScript (two arrays coerce to `0`), a `TypeError` on Python
/// lists, and a `.&(…)` method call that Rust cannot parse.
fn array_set_operators_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n  def self.common_ids\n    [1, 2, 2, 3] & [2, 3, 4]\n  end\n\n  def self.either_ids\n    [3, 1, 1] | [2, 1]\n  end\n",
        )
        .write(
            "test/models/set_ops_test.rb",
            "require \"test_helper\"\n\nclass SetOpsTest < ActiveSupport::TestCase\n  test \"array set operators\" do\n    assert_equal [2, 3], Article.common_ids\n    assert_equal [3, 1, 2], Article.either_ids\n  end\nend\n",
        )
}

#[test]
fn array_set_operators_run() {
    array_set_operators_app()
        .run_test("test/models/set_ops_test.rb")
        .assert_passes();
}

#[test]
fn array_set_operators_are_not_native_infix_on_typed_targets() {
    use roundhouse::project::BuildTarget;
    for (target, file, intersect, union) in [
        (
            BuildTarget::Typescript,
            "app/models/article.ts",
            "((__l, __r) => [...new Set([...__l])].filter(x => __r.includes(x)))([1, 2, 2, 3], [2, 3, 4])",
            "[...new Set([...[3, 1, 1], ...[2, 1]])]",
        ),
        (
            BuildTarget::Python,
            "app/v2/models.py",
            "(lambda __l, __r, __eq: [x for i, x in enumerate(__l) if any(__eq(x, y) for y in __r) and not any(__eq(x, y) for y in __l[:i])])([1, 2, 2, 3], [2, 3, 4], lambda a, b: type(a) is type(b) and a == b)",
            "(lambda __a, __eq: [x for i, x in enumerate(__a) if not any(__eq(x, y) for y in __a[:i])])([*[3, 1, 1], *[2, 1]], lambda a, b: type(a) is type(b) and a == b)",
        ),
        (
            BuildTarget::Rust,
            "src/models/article.rs",
            "if __rhs.contains(x) && !__out.contains(x)",
            "for x in __lhs.iter().chain(__rhs.iter())",
        ),
    ] {
        let (tree, errors) = array_set_operators_app().emit(target);
        assert!(errors.is_empty(), "{target:?}: {errors:?}");
        let src = std::fs::read_to_string(tree.join(file))
            .unwrap_or_else(|e| panic!("{target:?}: read {file}: {e}"));
        assert!(src.contains(intersect), "{target:?} `&`:\n{src}");
        assert!(src.contains(union), "{target:?} `|`:\n{src}");
    }
}

/// `Model.delete_all` returns the affected-row count, as Rails does —
/// the class form and the scoped `Relation` form alike. The class form
/// used to type as `Int` while running to `nil`.
#[test]
fn delete_all_returns_affected_row_count() {
    emit_and_run::real_blog()
        .write(
            "test/models/article_delete_all_test.rb",
            r#"require "test_helper"

class ArticleDeleteAllTest < ActiveSupport::TestCase
  test "delete_all returns the number of rows deleted" do
    Comment.delete_all
    Article.delete_all
    3.times { |i| Article.create!(title: "gone-#{i}", body: "Body text here") }
    Article.create!(title: "kept", body: "Body text here")
    assert_equal 3, Article.where("title LIKE 'gone-%'").delete_all
    assert_equal 1, Article.delete_all
    assert_equal 0, Article.delete_all
  end
end
"#,
        )
        .run_test("test/models/article_delete_all_test.rb")
        .assert_passes();
}

/// Destructured block parameters and nested multi-write targets bind
/// every name. campfire's `|(host, secure, origin), index|` was emitted
/// as `|index|`, and its body read three names nothing bound.
#[test]
fn nested_destructuring_binds_every_name() {
    const SOURCE: &str = r#"class NestedDestructureProbe
  def self.block_params
    out = []
    [[1, 2, 3], [4, 5, 6]].each_with_index do |(a, b, c), index|
      out << [a, b, c, index]
    end
    out
  end
  def self.deep_block
    [[1, [2, 3]]].map { |(a, (b, c))| [a, b, c] }
  end
  def self.lambda_param
    adder = ->((a, b)) { a + b }
    adder.call([20, 22])
  end
  def self.multi_write
    _, (_, removed) = [1, [2, 3]]
    removed
  end
  def self.deep_multi_write
    a, (b, (c, d)) = [1, [2, [3, 4]]]
    [a, b, c, d]
  end
  def self.expression
    (a, (b, c) = [1, [2, 3]])
  end
end
"#;
    const ASSERTIONS: &str = r##"
expected = {block_params: [[1, 2, 3, 0], [4, 5, 6, 1]], deep_block: [[1, 2, 3]], lambda_param: 42, multi_write: 3, deep_multi_write: [1, 2, 3, 4], expression: [1, [2, 3]]}
expected.each do |method, want|
  got = NestedDestructureProbe.public_send(method)
  raise "#{method}: #{got.inspect}, expected #{want.inspect}" unless got == want
end
"##;
    let native = std::process::Command::new("ruby").arg("-e")
        .arg(format!("{SOURCE}\n{ASSERTIONS}"))
        .output().expect("CRuby control");
    assert!(native.status.success(), "{}", String::from_utf8_lossy(&native.stderr));
    emit_and_run::real_blog()
        .write("app/services/nested_destructure_probe.rb", SOURCE)
        .run_ruby(ASSERTIONS).assert_passes();
}

/// A splat inside a nested destructure: `(month, *row)` from
/// `each_with_object`, plus the nearby shapes (anonymous splat, a
/// trailing name, a scalar element, a deeper group). The emitted Ruby
/// has to bind what MRI binds; a quiet ingest is not that claim.
#[test]
fn nested_destructure_splat_binds_the_tail() {
    const SOURCE: &str = r#"class AryBox
  def to_ary
    [7, 8, 9]
  end
end

class NestedSplatProbe
  def self.each_with_object
    [[1, 2, 3, 4], [5]].each_with_object({}) { |(month, *row), map| map[month] = row }
  end
  def self.anonymous
    out = []
    [[1, 2, 3]].each { |(month, *)| out << month }
    out
  end
  def self.only_rest
    out = []
    [[1, 2]].map { |(*row)| out << row }
    out
  end
  def self.post
    a, (b, *c, d) = [1, [2, 3, 4, 5]]
    [a, b, c, d]
  end
  def self.short_post
    a, (b, *c, d) = [1, [2]]
    [a, b, c, d]
  end
  def self.scalar
    a, (b, *c) = [1, 5]
    [a, b, c]
  end
  def self.coerced
    a, (b, *c) = [1, AryBox.new]
    [a, b, c]
  end
  def self.deep
    a, (b, (c, *d)) = [1, [2, [3, 4, 5]]]
    [a, b, c, d]
  end
  def self.beside
    (a, *b), c = [[1, 2, 3], 9]
    [a, b, c]
  end
  def self.ivars
    _head, (@month, *@row) = [0, [4, 5, 6]]
    [@month, @row]
  end
  def self.expression
    (a, (b, *c) = [1, [2, 3, 4]])
  end
  def self.lambda_param
    unpack = ->((month, *row)) { [month, row] }
    unpack.call([1, 2, 3])
  end
  def self.empty_block
    [[1, 2, 3]].map { |(month, *row)| }
  end
  def self.private_to_ary
    a, (b, *c) = [0, PrivateBox.new([4, 5])]
    [a, b, c]
  end
  def self.basic_object
    bare = BareElement.new
    a, (b, *c) = [0, bare]
    [a, b.equal?(bare), c]
  end
end

class PrivateBox
  def initialize(values)
    @values = values
  end

  def to_ary
    @values
  end
  private :to_ary
end

class BareElement < BasicObject
end
"#;
    const ASSERTIONS: &str = r##"
expected = {
  each_with_object: {1 => [2, 3, 4], 5 => []},
  anonymous: [1],
  only_rest: [[1, 2]],
  post: [1, 2, [3, 4], 5],
  short_post: [1, 2, [], nil],
  scalar: [1, 5, []],
  coerced: [1, 7, [8, 9]],
  deep: [1, 2, 3, [4, 5]],
  beside: [1, [2, 3], 9],
  ivars: [4, [5, 6]],
  expression: [1, [2, 3, 4]],
  lambda_param: [1, [2, 3]],
  empty_block: [nil],
  private_to_ary: [0, 4, [5]],
  basic_object: [0, true, []]
}
expected.each do |method, want|
  got = NestedSplatProbe.public_send(method)
  raise "#{method}: #{got.inspect}, expected #{want.inspect}" unless got == want
end
"##;
    let native = std::process::Command::new("ruby").arg("-e")
        .arg(format!("{SOURCE}\n{ASSERTIONS}"))
        .output().expect("CRuby control");
    assert!(native.status.success(), "{}", String::from_utf8_lossy(&native.stderr));
    emit_and_run::empty_app()
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("db/schema.rb", "ActiveRecord::Schema.define(version: 1) do\nend\n")
        .write("app/helpers/nested_splat_probe.rb", SOURCE)
        .run_ruby(ASSERTIONS).assert_passes();
}

/// `recv.m(**payload, badge: b)` where nothing types `recv`: the app's
/// one `m` takes `**rest`, so the `**` the ingest desugar erased is put
/// back (campfire's `WebPush::Pool#deliver_later`). Passed positionally
/// it is Ruby 3's `wrong number of arguments (given 1, expected 0)`.
#[test]
fn a_keyword_splat_to_an_untyped_receiver_keeps_its_double_splat() {
    emit_and_run::real_blog()
        // A MODEL method, as campfire's `Push::Subscription#notification`
        // is: models keep `badge:` a keyword beside `**params`.
        .edit("app/models/comment.rb", "  validates :commenter", r#"  def kwsplat_probe_note(badge: 0, **params)
    KwSplatNote.new(**params, badge: badge)
  end

  validates :commenter"#)
        .write("app/services/kwsplat_probe.rb", r#"class KwSplatNote
  attr_reader :title, :badge

  def initialize(title:, badge:)
    @title, @badge = title, badge
  end
end

class KwSplatCaller
  def self.call(items, payload)
    items.map { |item| item.kwsplat_probe_note(**payload, badge: 3) }
  end
end
"#)
        .run_ruby(r#"
article = Article.create!(title: "Splat title", body: "A sufficiently long article body.")
comment = Comment.create!(article: article, commenter: "Reader", body: "Comment body")
got = KwSplatCaller.call([comment], { title: "t" }).map { |n| [n.title, n.badge] }
raise "keyword splat lost: #{got.inspect}" unless got == [["t", 3]]
"#)
        .assert_passes();
}

/// Rails' `association(:name).loaded?` on a belongs_to: false until the
/// reader runs, true after — the question campfire's presentation tests
/// ask of a page of messages.
#[test]
fn association_loaded_answers_for_a_belongs_to() {
    emit_and_run::real_blog()
        .write("app/services/loaded_probe.rb", r#"class LoadedProbe
  def self.flags(comment)
    before = comment.association(:article).loaded?
    comment.article
    [before, comment.association(:article).loaded?]
  end
end
"#)
        .run_ruby(r#"
article = Article.create!(title: "Loaded title", body: "A sufficiently long article body.")
comment = Comment.create!(article: article, commenter: "Reader", body: "Comment body")
got = LoadedProbe.flags(Comment.find(comment.id))
raise "association(:article).loaded? answered #{got.inspect}" unless got == [false, true]
"#)
        .assert_passes();
}

/// Campfire's messages_count test: `Message.reflect_on_association(:room)`
/// and `assert_not reflection.counter_cache_column` (trigger-backed count,
/// not an AR `counter_cache:`).
#[test]
fn reflect_on_association_answers_without_counter_cache() {
    emit_and_run::real_blog()
        .run_ruby(r#"
reflection = Comment.reflect_on_association(:article)
raise "missing reflection" if reflection.nil?
raise "expected Symbol name, got #{reflection.name.inspect}" unless reflection.name == :article
raise "counter_cache_column should be nil, got #{reflection.counter_cache_column.inspect}" unless reflection.counter_cache_column.nil?
raise "unknown assoc should be nil" unless Comment.reflect_on_association(:missing).nil?
"#)
        .assert_passes();
}

/// `owner.<has_many>.reload` reads the rows again (campfire's rooms
/// test: `assert_empty room.memberships.reload`).
#[test]
fn has_many_reload_reads_the_rows_again() {
    emit_and_run::real_blog()
        .write("app/services/reload_probe.rb", r#"class ReloadProbe
  def self.counts(article)
    before = article.comments.size
    Comment.create!(article_id: article.id, commenter: "Late", body: "Arrived later")
    [before, article.comments.reload.size]
  end
end
"#)
        .run_ruby(r#"
article = Article.create!(title: "Reload title", body: "A sufficiently long article body.")
Comment.create!(article: article, commenter: "Reader", body: "Comment body")
got = ReloadProbe.counts(Article.find(article.id))
raise "reload answered #{got.inspect}" unless got == [1, 2]
"#)
        .assert_passes();
}

/// `reorder(Arel.sql("+articles.id"))` keeps its fragment: Rails takes an
/// `Arel.sql` literal past the column-name check (campfire's
/// `reorder(Arel.sql("+messages.created_at"))`, SQLite's index-skipping
/// unary plus), while a bare String with the same text is refused.
#[test]
fn an_arel_sql_order_fragment_passes_the_column_check() {
    emit_and_run::real_blog()
        .edit("app/models/article.rb", "class Article < ApplicationRecord\n", r#"class Article < ApplicationRecord
  scope :plus_ordered, -> { order(:title).reorder(Arel.sql("+articles.id")) }
"#)
        .run_ruby(r#"
Article.create!(title: "Second", body: "A sufficiently long article body.")
Article.create!(title: "First", body: "A sufficiently long article body.")
got = Article.plus_ordered.map(&:title)
raise "Arel.sql order answered #{got.inspect}" unless got == ["Second", "First"]
begin
  Article.plus_ordered.reorder("+articles.id").to_a
  raise "a bare String fragment passed the column check"
rescue ArgumentError
end
"#)
        .assert_passes();
}

/// A concern's `before_update -> { … }` lambda callback, Rails'
/// `attachment_changes`, and commit callbacks that run after `after_save`
/// — campfire's `Message::Searchable` reindexes a message whose file was
/// replaced: the lambda notes `attachment_changes.key?("attachment")`,
/// and the `after_update_commit` reads the NEW file's name, which only
/// exists once the save's own `after_save` has attached it. The lambda
/// was dropped from the concern, `attachment_changes` didn't exist, and
/// the commit hooks fired before `after_save`.
#[test]
fn a_replaced_attachment_is_seen_by_the_commit_callback() {
    header_values_app()
        .edit("db/schema.rb", "\nend\n", r#"
  create_table "active_storage_variant_records", force: :cascade do |t|
    t.bigint "blob_id", null: false
    t.string "variation_digest", null: false
  end
end
"#)
        .write("app/models/doc.rb", "class Doc < ApplicationRecord\n  include Tracked\n  has_one_attached :file\nend\n")
        .write("app/models/doc/tracked.rb", r#"module Doc::Tracked
  extend ActiveSupport::Concern

  included do
    before_update -> { @file_replaced = attachment_changes.key?("file") }
    after_update_commit :note_file, if: :file_replaced?
  end

  def seen
    @seen
  end

  private
    def file_replaced?
      @file_replaced
    end

    def note_file
      @seen = file.filename.to_s
    end
end
"#)
        .run_ruby(r#"
upload = ->(name) { { io: StringIO.new("bytes"), filename: name, content_type: "text/plain" } }
doc = Doc.create!(name: "first", file: upload.("a.txt"))
doc.update!(name: "renamed")
raise "a rename noted a file: #{doc.seen.inspect}" unless doc.seen.nil?
doc.update!(file: upload.("b.txt"))
raise "the commit callback saw #{doc.seen.inspect}" unless doc.seen == "b.txt"
raise "the change outlived the save: #{doc.attachment_changes.inspect}" unless doc.attachment_changes.empty?
"#)
        .assert_passes();
}

/// Three things Rails' test environment does that campfire's tests lean
/// on, given on the CRuby tree's test support:
/// - `allow_forgery_protection = false` (Rails' generated test.rb) means
///   forms carry no authenticity token, so a cached fragment and a fresh
///   render of it are the same bytes (campfire's messages caching test);
/// - `ActiveSupport::Notifications.subscribe(regexp)` hears the fragment
///   cache's reads and writes, keys in Rails' shape, until unsubscribed;
/// - `freeze_time` stops `Time.current` on the instant records are
///   stamped with (campfire's user test compares the two).
#[test]
fn rails_test_environment_tokens_cache_events_and_clock() {
    emit_and_run::real_blog()
        .edit("app/views/articles/_article.html.erb", "<div id=\"<%= dom_id(article) %>\"", "<% cache article do %>\n<div id=\"<%= dom_id(article) %>\"")
        .edit("app/views/articles/_article.html.erb", "  </div>\n</div>\n", "  </div>\n</div>\n<% end %>\n")
        .write(
            "test/controllers/test_environments_controller_test.rb",
            r#"require "test_helper"

class TestEnvironmentsControllerTest < ActionDispatch::IntegrationTest
  test "forms carry no token while forgery protection is off" do
    get new_article_url
    assert_select "form"
    assert_select "input[name='authenticity_token']", count: 0
    get articles_url
    assert_select "form.button_to"
    assert_select "input[name='authenticity_token']", count: 0
  end

  test "a subscriber hears fragment cache reads and writes until it unsubscribes" do
    keys = []
    subscriber = ActiveSupport::Notifications.subscribe(/\Acache_(read|write)\.active_support\z/) do |*, payload|
      keys << payload[:key]
    end
    previous_caching = ActionController::Base.perform_caching
    ActionController::Base.perform_caching = true
    begin
      get articles_url
      assert keys.any? { |key| key.include?("articles/_article/articles/") }, keys.inspect
      heard = keys.size
      ActiveSupport::Notifications.unsubscribe(subscriber)
      get articles_url
      assert_equal heard, keys.size
    ensure
      ActionController::Base.perform_caching = previous_caching
      ActiveSupport::Notifications.unsubscribe(subscriber)
    end
  end

  test "freeze_time stops Time.current on the instant records are stamped with" do
    freeze_time
    article = Article.create!(title: "Frozen", body: "A sufficiently long article body.")
    assert_equal Time.current, article.created_at
    assert_equal Time.current, Article.find(article.id).updated_at
  end
end
"#,
        )
        .run_test("test/controllers/test_environments_controller_test.rb")
        .assert_passes();
}

/// A Duration handed to the stdlib as a timeout, and `Timeout.timeout`
/// answering its block's value — campfire's unfurl (`DEADLINE =
/// 10.seconds` around the fetch, `open_timeout: 7.seconds` into
/// `Net::HTTP.start`). The Duration did no arithmetic, so
/// `TCPSocket.new(connect_timeout:)` could not convert it and every fetch failed;
/// and `Timeout.timeout { record }` typed Untyped, so `render json:`
/// wrote the record's `inspect` instead of its JSON.
#[test]
fn a_duration_times_out_the_stdlib_and_a_timed_block_keeps_its_type() {
    emit_and_run::real_blog()
        .edit("config/routes.rb", "  resources :articles do\n", "  get \"/timed/:id\", to: \"articles#timed\"\n  get \"/twice\", to: \"articles#twice\"\n  resources :articles do\n")
        .edit("app/controllers/articles_controller.rb", "class ArticlesController < ApplicationController\n", r#"class ArticlesController < ApplicationController
  LIMIT = 2.seconds

  def timed
    card = Timeout.timeout(LIMIT) { Card.new(title: Article.find(params[:id]).title) }
    render json: card
  end

  def twice
    first = Article.first
    again = Article.first
    render plain: (first.id == again.id).to_s
  end

"#)
        .write("app/models/card.rb", "class Card\n  include ActiveModel::Model\n\n  attr_accessor :title\nend\n")
        .write(
            "test/controllers/timed_articles_controller_test.rb",
            r#"require "test_helper"

require "active_record/testing/query_assertions"

class TimedArticlesControllerTest < ActionDispatch::IntegrationTest
  include ActiveRecord::Assertions::QueryAssertions

  test "a request replays an identical query, as Rails' query cache does" do
    assert_queries_count(1) { get "/twice" }
    assert_equal "true", response.body
    assert_queries_count(2) { Article.first; Article.first }
  end

  test "the timed lookup renders the model's JSON" do
    article = articles(:one)
    get "/timed/#{article.id}"
    assert_response :success
    assert_equal article.title, JSON.parse(response.body)["title"]
  end

  test "duration works with socket and stdlib timeout APIs" do
    server = TCPServer.new("127.0.0.1", 0)
    socket = TCPSocket.new("127.0.0.1", server.addr[1], connect_timeout: 1.second)
    socket.close
    server.close
    assert_nil IO.select(nil, nil, nil, 0.01.seconds)
    assert_equal :ok, Timeout.timeout(1.second) { :ok }
    http = Net::HTTP.new("127.0.0.1", 80)
    http.open_timeout = 1.second
    assert_equal 1.0, http.open_timeout
    assert_equal 1.5, (2.seconds - 0.5).to_f
    assert_equal 1.5, (2.seconds - 0.5.seconds).to_f
    assert_operator 1.second, :<, 2
  end
end
"#,
        )
        .run_test("test/controllers/timed_articles_controller_test.rb")
        .assert_passes();
}

#[test]
fn rich_text_renders_sanitized() {
    rich_text_app()
        .write(
            "test/controllers/rich_text_sanitize_controller_test.rb",
            r#"require "test_helper"

class RichTextSanitizeControllerTest < ActionDispatch::IntegrationTest
  test "stored rich text is sanitized on render" do
    article = Article.create!(title: "Rich", body: "Body text here",
      content: %(<p>kept <b>bold</b></p><script>alert(1)</script><img src="x" onerror="alert(2)"><a href="javascript:alert(3)">link</a><iframe src="https://evil.example"></iframe><svg onload="alert(4)"></svg>))
    get article_url(article)
    assert_response :success
    rich = response.body[/<section id="rich">.*?<\/section>/m].to_s
    assert_includes rich, "<p>kept <b>bold</b></p>"
    assert_not_includes rich, "<script"
    assert_not_includes rich, "onerror"
    assert_not_includes rich, "javascript:"
    assert_not_includes rich, "<iframe"
    assert_not_includes rich, "onload="
  end
end
"#,
        )
        .run_test("test/controllers/rich_text_sanitize_controller_test.rb")
        .assert_passes();
}

/// Destroying a record takes its attachments with it, as `dependent:`
/// says: Rails' default `:purge_later` deletes the join row and purges
/// the blob in an `ActiveStorage::PurgeJob` (run inline here, with no
/// queue adapter), and `dependent: false` leaves both. Without it
/// campfire's `Room#destroy_one_message_at_a_time` left every message's
/// attachment row and file behind.
#[test]
fn destroying_an_owner_purges_its_attachments_as_dependent_says() {
    header_values_app()
        .edit("db/schema.rb", "\nend\n", r#"
  create_table "active_storage_variant_records", force: :cascade do |t|
    t.bigint "blob_id", null: false
    t.string "variation_digest", null: false
  end
end
"#)
        .write("app/models/doc.rb", "class Doc < ApplicationRecord\n  has_one_attached :file\n  has_one_attached :keep, dependent: false\nend\n")
        .run_ruby(r#"
upload = ->(name) { { io: StringIO.new("bytes"), filename: name, content_type: "text/plain" } }
count = ->(table) { ActiveRecord::Base.connection.select_value("SELECT count(*) FROM #{table}") }
doc = Doc.create!(name: "first", file: upload.("a.txt"), keep: upload.("b.txt"))
raise "attached #{count.("active_storage_attachments")}" unless count.("active_storage_attachments") == 2
doc.destroy
raise "rows left: #{count.("active_storage_attachments")}" unless count.("active_storage_attachments") == 1
raise "blobs left: #{count.("active_storage_blobs")}" unless count.("active_storage_blobs") == 1
raise "PurgeJob not recorded" unless ActiveJob.performed.include?("ActiveStorage::PurgeJob")
"#)
        .assert_passes();
}

/// Fixtures load the way Rails' `insert_fixtures_set` loads them: raw
/// rows, no validations and no callbacks. A callback that raises would
/// abort the load if it ran; campfire's Message `after_create_commit`
/// marked memberships unread at load time, so the fixture users began
/// with unread rooms Rails never gives them.
#[test]
fn fixture_rows_load_without_running_callbacks() {
    emit_and_run::real_blog()
        .edit("app/models/comment.rb", "class Comment < ApplicationRecord\n", "class Comment < ApplicationRecord\n  before_save { raise \"a callback ran for a fixture row\" }\n  after_create_commit { raise \"a commit callback ran for a fixture row\" }\n")
        .write(
            "test/models/fixture_load_test.rb",
            r#"require "test_helper"

class FixtureLoadTest < ActiveSupport::TestCase
  test "every comment fixture is in the table, stamped" do
    assert_operator Comment.count, :>, 0
    assert Comment.all.all? { |comment| comment.created_at && comment.updated_at }
  end
end
"#,
        )
        .run_test("test/models/fixture_load_test.rb")
        .assert_passes();
}

/// `owner.assoc.create!(attributes)` in a test, with the attributes in a
/// local: the test-side association rewrite kept only a LITERAL hash and
/// replaced anything else with the foreign key alone, so campfire's
/// `rooms(:pets).messages.create!(attributes)` saved a message with no
/// creator ("Validation failed: Creator must exist"). The value now
/// merges the key in, and the association's key wins over the caller's,
/// as in Rails.
#[test]
fn an_association_create_keeps_attributes_held_in_a_local() {
    emit_and_run::real_blog()
        .write(
            "test/models/assoc_create_test.rb",
            r#"require "test_helper"

class AssocCreateTest < ActiveSupport::TestCase
  test "attributes in a local reach the record" do
    attributes = { commenter: "Reader", body: "Comment body" }
    comment = articles(:one).comments.create!(attributes)
    assert_equal "Reader", comment.commenter
    assert_equal articles(:one).id, comment.article_id
  end

  test "the association's key wins over the caller's" do
    comment = articles(:one).comments.create!(commenter: "Lit", body: "Literal body", article_id: articles(:two).id)
    assert_equal articles(:one).id, comment.article_id
  end
end
"#,
        )
        .run_test("test/models/assoc_create_test.rb")
        .assert_passes();
}

/// Active Storage's "was it made?" questions, as campfire's
/// presentation asks them so a view never makes a preview: a variant is
/// `processed?` once its record exists, and its `image` is that record's
/// (both looked up, not made — asked of a file that cannot be decoded,
/// `image` used to raise the decode error again);
/// `preview(:poster)` names a variant the owner's `has_one_attached`
/// block declares; and `url_for` of a Preview held in a typed local is
/// the preview's representation URL, not the object.
#[test]
fn a_variant_is_processed_once_its_record_exists_and_a_preview_resolves_its_name() {
    header_values_app()
        .edit("db/schema.rb", "\nend\n", r#"
  create_table "active_storage_variant_records", force: :cascade do |t|
    t.bigint "blob_id", null: false
    t.string "variation_digest", null: false
  end
end
"#)
        .write("app/models/doc.rb", r#"class Doc < ApplicationRecord
  has_one_attached :file do |attachable|
    attachable.variant :thumb, resize_to_limit: [ 10, 10 ]
    attachable.variant :poster, format: :webp, resize_to_limit: [ 10, 10 ]
  end

  def poster_url
    poster = file.preview(:poster)
    ActionView::ViewHelpers.url_for(poster)
  end
end
"#)
        .run_ruby(r#"
doc = Doc.create!(name: "first", file: { io: StringIO.new("bytes"), filename: "a.mov", content_type: "video/quicktime" })
thumb = doc.file.representation(:thumb)
raise "processed before any record" if thumb.processed?
raise "image made a variant of bytes that are no image" unless thumb.image.nil?
raise "variant records: #{ActiveStorage::VariantRecord.count}" unless ActiveStorage::VariantRecord.count == 0
image = ActiveStorage::Blob.create_and_upload!("png", "a.png", "image/png")
connection = ActiveRecord::Base.connection
connection.execute("INSERT INTO active_storage_variant_records (blob_id, variation_digest) VALUES (#{doc.file.blob.id}, '#{thumb.variation.digest}')")
record_id = connection.select_value("SELECT max(id) FROM active_storage_variant_records")
connection.execute("INSERT INTO active_storage_attachments (name, record_type, record_id, blob_id, created_at) VALUES ('image', 'ActiveStorage::VariantRecord', #{record_id}, #{image.id}, '2026-01-01')")
raise "not processed once its record exists" unless doc.file.representation(:thumb).processed?
raise "variant records: #{ActiveStorage::VariantRecord.count}" unless ActiveStorage::VariantRecord.count == 1
raise "no image once its record exists" if doc.file.representation(:thumb).image.nil?
preview = doc.file.preview(:poster)
raise "preview named #{preview.variation&.name.inspect}" unless preview.variation.name == "poster"
url = doc.poster_url
raise "url_for(preview) answered #{url.inspect}" unless url.is_a?(String) && url.include?("/representations/")
"#)
        .assert_passes();
}

/// `insert_all!` and `insert_all` are one raw INSERT per row in Rails:
/// no validations, no callbacks, timestamps filled. `insert_all!`
/// raises on a duplicate and answers an `ActiveRecord::Result` of the
/// RETURNING columns, whose `rows` are Arrays of values (campfire's
/// search test reads new ids with `.rows.flatten`); `insert_all` skips
/// a duplicate. Both used to run the save callbacks, and `insert_all!`
/// was undefined.
#[test]
fn bulk_inserts_skip_callbacks_and_insert_all_bang_returns_its_rows() {
    emit_and_run::real_blog()
        .edit("app/models/comment.rb", "class Comment < ApplicationRecord\n", r#"class Comment < ApplicationRecord
  after_create_commit { raise "a callback ran for a bulk insert" }

  def self.bulk_ids(article, names)
    rows = names.map { |name| { article_id: article.id, commenter: name, body: "Bulk body" } }
    Comment.insert_all!(rows, returning: %w[ id commenter ]).rows
  end

  def self.bulk_skipping(article, names)
    rows = names.map { |name| { article_id: article.id, commenter: name, body: "Bulk body" } }
    Comment.insert_all(rows)
  end
"#)
        .run_ruby(r#"
article = Article.create!(title: "Bulk", body: "A sufficiently long article body.")
rows = Comment.bulk_ids(article, ["a", "b"])
raise "rows #{rows.inspect}" unless rows.map(&:last) == ["a", "b"] && rows.all? { |r| r.first.is_a?(Integer) }
raise "timestamps" unless Comment.where(article_id: article.id).all? { |c| c.created_at && c.updated_at }
Comment.bulk_skipping(article, ["c"])
raise "count #{Comment.where(article_id: article.id).count}" unless Comment.where(article_id: article.id).count == 3
"#)
        .assert_passes();
}

/// campfire's `MessagesController#create` from 2c53c46 on: the
/// controller renders the message once, hands the markup to a concern
/// method by keyword (`broadcast_create(html:)`), which broadcasts it
/// with `**(html ? { html: html } : {})`, and the turbo-stream view
/// appends the same String (`turbo_stream.append target, @html`). Its
/// cache layer folds `I18n.locale` into keys and copies response headers
/// by downcased name (`response.headers.slice(*CACHE_HEADERS)`).
///
/// Before: the keyword reached the flattened `broadcast_create(html =
/// nil)` as a Hash, `broadcast_append_to` with that option was left
/// unlowered (undefined on the record), the view looked for a
/// `message_htmls/_message_html` partial, `I18n` was undefined, and
/// `headers["ETag"]` / `headers["etag"]` were two headers.
#[test]
fn rendered_markup_reaches_the_broadcast_and_the_stream_as_given() {
    emit_and_run::real_blog()
        .edit("config/routes.rb", "  resources :articles do\n", "  post \"/articles/:article_id/comments/:id/announce\", to: \"comments#announce\", as: :announce_article_comment\n  resources :articles do\n")
        .edit("app/models/comment.rb", "  belongs_to :article\n", "  include Announces\n  belongs_to :article\n")
        .write("app/models/comment/announces.rb", r##"module Comment::Announces
  def announce(html: nil)
    broadcast_append_to article, :announcements, target: [ article, :announcements ], **(html ? { html: html } : {})
  end
end
"##)
        .edit("app/controllers/comments_controller.rb", "  def destroy\n", r##"  def announce
    @comment = @article.comments.find(params[:id])
    @comment_html = render_to_string partial: "comments/comment", formats: :html, locals: { comment: @comment }
    @comment.announce(html: @comment_html)
    response.headers["X-Probe"] = "first"
    response.headers["x-probe"] = "second"
    response.headers["X-Gone"] = "soon"
    response.headers.delete("x-gone")
    response.headers["X-Copied"] = response.headers.slice("X-PROBE", "x-absent").map { |name, value| "#{name}=#{value}" }.join(",")
    response.headers["X-Locale"] = I18n.locale.to_s
  end

  def destroy
"##)
        .write("app/views/comments/announce.turbo_stream.erb", "<%= turbo_stream.append dom_id(@comment.article, :announcements), @comment_html %>\n")
        .write("test/controllers/announcements_controller_test.rb", r##"require "test_helper"

class AnnouncementsControllerTest < ActionDispatch::IntegrationTest
  setup do
    @article = Article.create!(title: "Announced", body: "A sufficiently long article body.")
    @comment = @article.comments.create!(commenter: "Ann", body: "Said <b>once</b>")
    @stream = "#{@article.to_gid_param}:announcements"
  end

  test "the rendered comment is broadcast and streamed as given" do
    post announce_article_comment_url(@article, @comment, format: :turbo_stream)
    assert_response :success
    assert_includes response.body, %(<turbo-stream action="append" target="announcements_article_#{@article.id}">)
    assert_includes response.body, %(<div id="comment_#{@comment.id}")
    assert_includes response.body, "Said &lt;b&gt;once&lt;/b&gt;"
    assert_not_includes response.body, "&lt;div"

    sent = ActionCable.server.pubsub.broadcasts(@stream).map { |broadcast| JSON.parse(broadcast) }
    assert_equal 1, sent.size
    assert_includes sent.first, %(<div id="comment_#{@comment.id}")
    assert_not_includes sent.first, "&lt;div"
  end

  test "header names match without regard to case" do
    post announce_article_comment_url(@article, @comment, format: :turbo_stream)
    assert_equal "second", response.headers["X-Probe"]
    assert_nil response.headers["X-Gone"]
    assert_equal "x-probe=second", response.headers["X-Copied"]
    assert_equal "en", response.headers["X-Locale"]
  end

  test "markup handed in replaces the partial, and without it the partial renders" do
    @comment.announce(html: "<p>handed in</p>")
    @comment.announce
    sent = ActionCable.server.pubsub.broadcasts(@stream).map { |broadcast| JSON.parse(broadcast) }
    assert_equal 2, sent.size
    assert_includes sent.first, "<p>handed in</p>"
    assert_includes sent.last, %(<div id="comment_#{@comment.id}")
  end
end
"##)
        .run_test("test/controllers/announcements_controller_test.rb")
        .assert_passes();
}

/// Framework surface campfire main's tests and cache keys reach, each
/// with Rails' behaviour:
/// - `request.format.to_s` is the negotiated Mime string;
/// - `Rails.application.env_config` keeps what a test sets in it;
/// - a helper test's `controller` is a test controller, whose
///   `perform_caching` is false in the test environment;
/// - `update_columns` and `delete` skip callbacks;
/// - `raise ActiveRecord::Rollback` undoes the transaction quietly;
/// - `clear_enqueued_jobs`, `assert_nothing_raised`, `assert_dom_equal`.
fn rails_surface_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit("config/routes.rb", "  resources :articles do\n", "  get \"/format\", to: \"articles#format_probe\"\n  resources :articles do\n")
        .edit("app/controllers/articles_controller.rb", "class ArticlesController < ApplicationController\n", r##"class ArticlesController < ApplicationController
  def format_probe
    render plain: request.format.to_s
  end

"##)
        .write("app/helpers/articles_helper.rb", r##"module ArticlesHelper
  def caching_label
    controller.perform_caching ? "cached" : "fresh"
  end
end
"##)
        .write("app/jobs/touch_job.rb", "class TouchJob < ApplicationJob\n  def perform(article)\n  end\nend\n")
        .edit("app/models/article.rb", "class Article < ApplicationRecord\n", "class Article < ApplicationRecord\n  SAVED = []\n  after_save { SAVED << title }\n\n")
        .write("test/helpers/articles_helper_test.rb", r##"require "test_helper"

class ArticlesHelperTest < ActionView::TestCase
  test "a helper reads the test controller" do
    assert_equal "fresh", caching_label
  end

  test "markup compares up to attribute order and the whitespace between tags" do
    assert_dom_equal %(<p class="a" id="b">x</p>), %(<p id="b"  class="a">x</p>\n)
    assert_nothing_raised { Article.count }
  end
end
"##)
        .write("test/controllers/format_probes_controller_test.rb", r##"require "test_helper"

class FormatProbesControllerTest < ActionDispatch::IntegrationTest
  include ActiveJob::TestHelper

  test "the request format is its Mime string" do
    get "/format"
    assert_equal "text/html", response.body
  end

  test "env_config keeps what a test sets" do
    previous = Rails.application.env_config["action_dispatch.show_exceptions"]
    Rails.application.env_config["action_dispatch.show_exceptions"] = :rescuable
    assert_equal :rescuable, Rails.application.env_config["action_dispatch.show_exceptions"]
  ensure
    Rails.application.env_config["action_dispatch.show_exceptions"] = previous
  end

  test "update_columns and delete skip callbacks" do
    article = Article.create!(title: "Columns", body: "A sufficiently long article body.")
    saved = Article::SAVED.size
    article.update_columns(title: "Renamed")
    assert_equal saved, Article::SAVED.size
    assert_equal "Renamed", Article.find(article.id).title
    article.delete
    assert_nil Article.find_by(id: article.id)
  end

  test "a rollback undoes the transaction without raising" do
    count = Article.count
    result = Article.transaction do
      Article.create!(title: "Gone", body: "A sufficiently long article body.")
      raise ActiveRecord::Rollback
    end
    assert_nil result
    assert_equal count, Article.count
  end

  test "clear_enqueued_jobs forgets what was enqueued" do
    article = Article.create!(title: "Queued", body: "A sufficiently long article body.")
    TouchJob.perform_later(article)
    assert_enqueued_jobs 1, only: TouchJob
    clear_enqueued_jobs
    assert_no_enqueued_jobs only: TouchJob
  end
end
"##)
}

#[test]
fn rails_surface_for_cache_keys_rollbacks_and_jobs() {
    rails_surface_app()
        .run_test("test/controllers/format_probes_controller_test.rb")
        .assert_passes();
}

#[test]
fn a_helper_test_has_a_test_controller_and_dom_assertions() {
    rails_surface_app()
        .run_test("test/helpers/articles_helper_test.rb")
        .assert_passes();
}

/// Three ingest/emit shapes campfire main's web push pool reaches:
/// - `@idle[address].pop&.first` evaluates the receiver ONCE (the `&.`
///   desugar read it twice, popping two entries);
/// - classes nested in a class under a runtime namespace
///   (`class WebPush::Connections` with `class HTTP < Net::HTTP; include
///   Stages; end`) are emitted, a module a sibling includes first;
/// - each nested class keeps its own name inside the parent.
#[test]
fn a_safe_navigated_call_runs_once_and_nested_classes_survive_a_compact_parent() {
    emit_and_run::real_blog()
        .write("lib/web_push/probe_pool.rb", r##"class WebPush::ProbePool
  class Lost < StandardError; end

  module Stages
    def stage
      :checking
    end
  end

  class HTTP < Net::HTTP
    include Stages
  end

  def initialize
    @idle = { "a" => [ [ :first, 1 ], [ :second, 2 ] ] }
  end

  def checkout(address)
    @idle[address].pop&.first
  end

  def left(address)
    @idle[address].size
  end

  def connection
    HTTP.new("example.com", 443)
  end
end
"##)
        .run_ruby(r##"
pool = WebPush::ProbePool.new
taken = pool.checkout("a")
raise "took #{taken.inspect}" unless taken == :second
raise "popped #{2 - pool.left("a")}" unless pool.left("a") == 1
raise "no stage" unless pool.connection.stage == :checking
raise "HTTP is #{WebPush::ProbePool::HTTP.superclass}" unless WebPush::ProbePool::HTTP.superclass == Net::HTTP
raise "Lost" unless WebPush::ProbePool::Lost.ancestors.include?(StandardError)
"##)
        .assert_passes();
}

/// `def m(...)` that only forwards to `super`, where `super` is Net::HTTP's
/// private `begin_transport(req)` / `connect` (campfire's
/// `WebPush::Connections::Stages`): the overrides take those signatures
/// (`lower::known_super_forwarding`), the spinel emit has no refusal left,
/// and on both lanes the hooks run in CRuby's order around two requests
/// over a connection the server closes after each.
fn super_forwarding_transport_hooks_app() -> emit_and_run::Overlay {
    // Top level, not nested in a compact parent: a class nested in
    // `class A::B` is undefined on spinel (matz/spinel#8369), which is
    // campfire's own spelling and not what this pins.
    emit_and_run::real_blog().write("lib/probe_stages.rb", r##"module ProbeStages
  attr_reader :stage

  def trace
    @trace ||= []
  end

  private
    def begin_transport(...)
      trace << "begin"
      @stage = :checking
      super.tap { @stage = :sent }
    end

    def connect(...)
      trace << "connect"
      @stage = :connecting if @stage == :checking
      super
    end
end

class ProbeHTTP < Net::HTTP
  include ProbeStages
end
"##)
}

const SUPER_FORWARDING_SCRIPT: &str = r##"
server = TCPServer.new("127.0.0.1", 0)
port = server.addr[1]
t = Thread.new do
  2.times do |i|
    c = server.accept
    while (line = c.gets)
      break if line.strip.empty?
    end
    c.write("HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nr#{i}")
    c.close
  end
end
http = ProbeHTTP.new("127.0.0.1", port)
http.start
bodies = [http.get("/").body, http.get("/").body]
http.finish
t.join
raise "bodies #{bodies.inspect}" unless bodies == ["r0", "r1"]
raise "stage #{http.stage.inspect}" unless http.stage == :sent
raise "trace #{http.trace.inspect}" unless http.trace == ["connect", "begin", "begin", "connect"]
"##;

#[test]
fn forwarding_into_net_http_transport_hooks_runs_on_ruby() {
    super_forwarding_transport_hooks_app().run_ruby(SUPER_FORWARDING_SCRIPT).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn forwarding_into_net_http_transport_hooks_runs_on_spinel() {
    super_forwarding_transport_hooks_app().run_spinel(SUPER_FORWARDING_SCRIPT).assert_passes();
}

/// A `test/test_helpers/` module that one test class includes itself
/// (campfire's `include PushServiceTestHelper`), carried whole into the
/// test's file: its nested class, its module methods, a constructor
/// whose keywords stay keywords (`Server.new(**options)`), a duration in
/// a module method, and `assert_not_kind_of`.
#[test]
fn a_test_helper_module_one_test_includes_is_carried_whole() {
    emit_and_run::real_blog()
        .write("test/test_helpers/probe_server_helper.rb", r##"module ProbeServerHelper
  NAME = "probe"

  class Server
    attr_reader :status, :label

    def initialize(status: "201 Created", label: -> { NAME })
      @status, @label = status, label.call
    end
  end

  class << self
    def started_at
      1.minute.ago
    end
  end

  private
    def with_server(**options)
      yield Server.new(**options)
    end
end
"##)
        .write("test/models/probe_server_test.rb", r##"require "test_helper"

class ProbeServerTest < ActiveSupport::TestCase
  include ProbeServerHelper

  test "the helper's class takes its keywords" do
    with_server(status: "500 Oops") do |server|
      assert_equal "500 Oops", server.status
      assert_equal "probe", server.label
      assert_not_kind_of String, server
    end
    with_server do |server|
      assert_equal "201 Created", server.status
    end
  end

  test "a module method reads a duration" do
    assert ProbeServerHelper.started_at < Time.now
  end
end
"##)
        .run_test("test/models/probe_server_test.rb")
        .assert_passes();
}

/// campfire main keeps `rooms.messages_count` with SQLite triggers it
/// installs itself, and repairs the count once fixtures are loaded:
/// - a `load_fixtures` override on the app's `ActiveSupport::TestCase`
///   (`fixtures = super; …; fixtures`) runs its statements after the
///   fixtures, ahead of the tests' own setup;
/// - `connection.data_source_exists?` / `column_exists?` read SQLite's
///   catalog, `raw_connection.transaction(:immediate)` is a write
///   transaction below ActiveRecord, `connection_pool.release_connection`
///   answers;
/// - `clear_query_caches_for_current_thread` leaves the cache on.
#[test]
fn a_fixture_hook_and_trigger_maintenance_run_as_rails_runs_them() {
    emit_and_run::real_blog()
        .edit("test/test_helper.rb", "    # Add more helper methods to be used by all tests here...\n", r##"    def load_fixtures(config)
      fixtures = super
      Article::Counting.ensure!
      fixtures
    end
"##)
        .edit("db/schema.rb", "    t.string \"title\"\n", "    t.string \"title\"\n    t.integer \"comments_total\", default: 0, null: false\n")
        .write("app/models/article/counting.rb", r##"class Article::Counting
  TRIGGER = "comments_ai_articles_total"

  def self.ensure!(connection = ActiveRecord::Base.connection)
    return unless connection.adapter_name.match?(/sqlite/i)
    return unless connection.data_source_exists?(:articles)
    return unless connection.column_exists?(:articles, :comments_total)
    return if installed?(connection)

    connection.raw_connection.transaction(:immediate) do
      connection.execute("UPDATE articles SET comments_total = (SELECT COUNT(*) FROM comments WHERE comments.article_id = articles.id)")
      connection.execute("CREATE TRIGGER #{TRIGGER} AFTER INSERT ON comments BEGIN UPDATE articles SET comments_total = comments_total + 1 WHERE id = NEW.article_id; END")
    end
  end

  def self.installed?(connection = ActiveRecord::Base.connection)
    connection.select_value("SELECT 1 FROM sqlite_master WHERE type = 'trigger' AND name = #{connection.quote(TRIGGER)}").present?
  end
end
"##)
        .write("test/models/article_counting_test.rb", r##"require "test_helper"

class ArticleCountingTest < ActiveSupport::TestCase
  test "the fixture hook installed the trigger and backfilled the counts" do
    assert Article::Counting.installed?
    article = articles(:one)
    assert_equal article.comments.count, article.comments_total
    article.comments.create!(commenter: "Ann", body: "Counted by the trigger")
    assert_equal article.comments.count, article.reload.comments_total
  end

  test "schema questions read the catalog" do
    connection = ActiveRecord::Base.connection
    assert connection.data_source_exists?(:comments)
    assert_not connection.data_source_exists?(:nothing_here)
    assert connection.column_exists?(:articles, :comments_total)
    assert_not connection.column_exists?(:articles, :missing)
    assert_nil ActiveRecord::Base.connection_pool.release_connection
    assert_nil ActiveRecord::Base.clear_query_caches_for_current_thread
  end

  test "a raw write transaction rolls back when its block raises" do
    connection = ActiveRecord::Base.connection
    count = Comment.count
    assert_raises(RuntimeError) do
      connection.raw_connection.transaction(:immediate) do
        connection.execute("DELETE FROM comments")
        raise "abandon"
      end
    end
    assert_equal count, Comment.count
  end
end
"##)
        .run_test("test/models/article_counting_test.rb")
        .assert_passes();
}

/// campfire main's header-only forgery protection, from the test side:
/// - an app helper overriding `token_tag` to answer "" leaves every form
///   without a token field (Rails' form helpers build it through that);
/// - a POST carrying `_method: "delete"` is routed as the DELETE it says
///   (Rack's MethodOverride, as both production dispatchers apply it);
/// - the integration session's `reset!` starts a fresh one.
#[test]
fn a_blank_token_tag_override_method_override_and_session_reset() {
    emit_and_run::real_blog()
        .write("app/helpers/application_helper.rb", r##"module ApplicationHelper
  private
    def token_tag(*)
      ""
    end
end
"##)
        .write("test/controllers/token_free_forms_controller_test.rb", r##"require "test_helper"

class TokenFreeFormsControllerTest < ActionDispatch::IntegrationTest
  setup do
    @previous = ActionController::Base.allow_forgery_protection
    ActionController::Base.allow_forgery_protection = true
  end

  teardown do
    ActionController::Base.allow_forgery_protection = @previous
  end

  test "forms carry no token field" do
    get new_article_url
    assert_response :success
    assert_select "form"
    assert_select "input[name='authenticity_token']", count: 0
  end

  test "a POST with _method is the verb it names" do
    ActionController::Base.allow_forgery_protection = false
    article = Article.create!(title: "Overridden", body: "A sufficiently long article body.")
    assert_difference "Article.count", -1 do
      post article_url(article), params: { _method: "delete" }
    end
  end

  test "reset! starts a fresh session" do
    https!
    host! "blog.example.test"
    get articles_url
    reset!
    assert_not https?
    assert_equal "www.example.com", host
    assert_nil response
  end
end
"##)
        .run_test("test/controllers/token_free_forms_controller_test.rb")
        .assert_passes();
}

/// A bounded `ActiveSupport::Cache::MemoryStore` keeps an html-safe
/// String (a SafeBuffer) as such, as Rails' store does. campfire main
/// caches each text message's rendered body this way (`auto_link`
/// answers a SafeBuffer); the store refused anything but an exact
/// String, so with caching on every message body rendered empty.
#[test]
fn a_memory_store_keeps_an_html_safe_string() {
    emit_and_run::real_blog()
        .run_ruby(r##"
store = ActiveSupport::Cache::MemoryStore.new(size: 4096)
body = store.fetch("presentation") { "<b>hi</b>".html_safe }
raise "fetch answered #{body.inspect}" unless body == "<b>hi</b>" && body.html_safe?
again = store.read("presentation")
raise "read answered #{again.inspect}" unless again == "<b>hi</b>" && again.html_safe?
raise "the cached copy is the caller's object" if again.equal?(body)
"##)
        .assert_passes();
}

/// A model-only app (no controllers) must boot: `config/routes.rb`
/// requires `application_controller` only when the app defines one, the
/// same defined-only rule the per-controller requires follow.
#[test]
fn a_model_only_app_boots_without_an_application_controller() {
    emit_and_run::empty_app()
        .write("db/schema.rb", r#"ActiveRecord::Schema.define do
  create_table "widgets" do |t|
    t.integer "owner_id", null: false
    t.boolean "enabled", default: false, null: false
    t.string "name", null: false
  end
end
"#)
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\n  validates :name, presence: true\nend\n")
        .write("test/fixtures/widgets.yml", "on:\n  owner_id: 7\n  name: a\n  enabled: true\n\noff:\n  owner_id: 8\n  name: b\n  enabled: false\n")
        .write("test/models/widget_test.rb", r#"require "test_helper"

class WidgetTest < ActiveSupport::TestCase
  test "enabled defaults to false" do
    refute Widget.new(owner_id: 1).enabled
  end

  test "requires name" do
    record = Widget.new(owner_id: 1, name: nil)
    refute_predicate record, :valid?
    assert_includes record.errors[:name], "can't be blank"
  end

  test "can query by owner_id and enabled status" do
    record = widgets(:on)
    assert Widget.exists?(owner_id: record.owner_id, enabled: true)
    refute Widget.exists?(owner_id: record.owner_id, enabled: false)
  end
end
"#)
        .run_test("test/models/widget_test.rb")
        .assert_passes();
}

/// An app with nothing in it but what the emitted test boot needs: an
/// application controller, an empty route table and a schema.
fn bare_test_app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table :things do |t|\n    t.string :name\n  end\nend\n")
}

/// A gem keeps its tests at `test/` root over a base its helper
/// defines (`class GemCompat::TestCase < ActiveSupport::TestCase`
/// in test/test_helper.rb). With `test_paths: [test]` the helper was
/// read a second time as a test file, and the emitted subclass never
/// loaded its base: NameError on `GemCompat` before any test ran.
#[test]
fn a_test_case_base_defined_in_the_helper_is_loaded_by_its_subclasses() {
    bare_test_app()
        .write("app/.keep", "")
        .write("roundhouse.yml", "test_paths:\n  - test\n")
        .write("lib/fragments.rb", "module Fragments\n  def self.random\n    \"RANDOM()\"\n  end\nend\n")
        .write("test/test_helper.rb", "require \"active_support\"\n\nmodule GemCompat\n  class TestCase < ActiveSupport::TestCase\n  end\nend\n")
        .write("test/fragments_test.rb", "require \"test_helper\"\n\nclass FragmentsTest < GemCompat::TestCase\n  test \"random\" do\n    assert_equal \"RANDOM()\", Fragments.random\n  end\nend\n")
        .run_test("test/models/fragments_test.rb")
        .assert_passes();
}

/// A helper that defines only a plain class with a superclass (a logger
/// subclass the tests share) is not a test-case base: no test file, no
/// autorun driver for zero tests.
#[test]
fn a_plain_class_in_the_helper_is_not_read_as_a_test_case() {
    let (emitted, errors) = bare_test_app()
        .write("app/.keep", "")
        .write("roundhouse.yml", "test_paths:\n  - test\n")
        .write("lib/fragments.rb", "module Fragments\n  def self.random\n    \"RANDOM()\"\n  end\nend\n")
        .write("test/test_helper.rb", "require \"active_support\"\nrequire \"logger\"\n\nclass RecordingLogger < Logger\nend\n")
        .write("test/fragments_test.rb", "require \"test_helper\"\n\nclass FragmentsTest < ActiveSupport::TestCase\n  test \"random\" do\n    assert_equal \"RANDOM()\", Fragments.random\n  end\nend\n")
        .emit(roundhouse::project::BuildTarget::Ruby);
    assert!(errors.is_empty(), "got {errors:?}");
    let emitted_tests: Vec<String> = ["test", "test/models"]
        .iter()
        .filter_map(|d| std::fs::read_dir(emitted.join(d)).ok())
        .flat_map(|d| d.flatten().map(|e| e.file_name().to_string_lossy().into_owned()))
        .collect();
    assert!(emitted_tests.iter().any(|f| f == "fragments_test.rb"), "got {emitted_tests:?}");
    assert!(emitted_tests.iter().all(|f| !f.contains("recording_logger")), "got {emitted_tests:?}");
}

/// `teardown do … end` in a test class fell to the drop for unrecognized
/// class-body statements: check stayed clean and the cleanup never ran
/// (a common Rails idiom). ActiveSupport runs the class's
/// `def teardown` first, then the blocks newest first, after every test;
/// the order asserted here is what ActiveSupport 8.1.4 produces natively.
#[test]
fn teardown_blocks_run_after_each_test_in_rails_order() {
    bare_test_app()
        .write("app/.keep", "")
        .write("test/models/teardown_order_test.rb", r#"require "test_helper"

class TeardownOrderTest < ActiveSupport::TestCase
  LOG = []

  teardown do
    LOG << "one"
  end

  def teardown
    LOG << "def"
  end

  teardown do
    LOG << "two"
  end

  test "a first" do
    LOG << "a"
  end

  test "b second" do
    assert_equal ["a", "def", "two", "one"], LOG
  end
end
"#)
        .run_test("test/models/teardown_order_test.rb")
        .assert_passes();
}

/// A subclass `teardown do` must still reach TestBase's mocha_verify.
/// Without `super`, an `expects(...).never` from test A stays installed
/// and test B fails with "unexpected invocation" (campfire CachedResponses).
#[test]
fn teardown_do_emits_super_so_mocha_verify_still_runs() {
    let (emitted, errors) = bare_test_app()
        .write("app/.keep", "")
        .write(
            "test/models/teardown_super_test.rb",
            r#"require "test_helper"

class TeardownSuperTest < ActiveSupport::TestCase
  teardown do
    cleanup
  end

  test "a" do
    assert true
  end
end
"#,
        )
        .emit(roundhouse::project::BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:?}");
    let src = std::fs::read_to_string(emitted.join("test/models/teardown_super_test.rb")).unwrap();
    assert!(
        src.contains("def teardown") && src.contains("super()"),
        "teardown do must call super for mocha_verify:\n{src}"
    );
}

/// One test file whose teardown prints how many times the block's
/// effect (`Widget.hit`) ran, so a failing run shows where it stopped.
fn destructured_block_app(name: &str, members: &str) -> emit_and_run::Overlay {
    bare_test_app()
        .write(
            "app/models/widget.rb",
            "class Widget\n  HITS = []\n\n  def self.flag\n    false\n  end\n\n  def self.hit\n    HITS << 1\n  end\n\n  def self.hits\n    HITS.size\n  end\n\n  def self.reset\n    HITS.clear\n  end\nend\n",
        )
        .write("test/test_helper.rb", "require \"active_support\"\n")
        .write(
            "test/models/widget_test.rb",
            &format!(
                "require \"test_helper\"\n\nclass WidgetTest < ActiveSupport::TestCase\n  setup do\n    Widget.reset\n  end\n\n  teardown do\n    puts \"CASE {name} HITS=#{{Widget.hits}}\"\n  end\n\n{members}\nend\n"
            ),
        )
}

/// Assertions inside the block on the right of `a, b = call do … end`
/// are lowered where they are written, and a test class's own
/// `assert_includes` is the one called, not Minitest's. Each case's
/// outcome is the native Minitest run's.
#[test]
fn assertions_inside_a_destructured_block_run_where_they_are_written() {
    let cases = [
        ("plain", "  test \"t\" do\n    first, second = [1, 2].map do |x|\n      assert_equal x, x\n      Widget.hit\n      x * 10\n    end\n    assert_equal [10, 20], [first, second]\n    assert_equal 2, Widget.hits\n  end\n"),
        ("own", "  def assert_includes(collection, item)\n    Widget.hit\n    assert collection.downcase.include?(item.downcase)\n  end\n\n  def assert_shouted(text)\n    assert_equal text.upcase, text\n  end\n\n  test \"t\" do\n    first, second = [\"HELLO\", \"LOUD\"].map do |text|\n      assert_includes text, \"hello\" if text == \"HELLO\"\n      assert_shouted text\n      text\n    end\n    assert_includes first, \"hello\"\n    assert_equal 2, Widget.hits\n  end\n"),
        ("splat", "  test \"t\" do\n    first, *rest = [1, 2, 3].map do |x|\n      assert_equal x, x\n      Widget.hit\n      x\n    end\n    assert_equal 1, first\n    assert_equal [2, 3], rest\n    assert_equal 3, Widget.hits\n  end\n"),
    ];
    for (name, members) in cases {
        let run = destructured_block_app(name, members).run_test("test/models/widget_test.rb");
        run.assert_passes();
        assert!(run.stdout.contains(&format!("CASE {name} HITS=")), "{name}: {}", run.stdout);
    }
}

/// A failing assertion inside that block fails the test and stops what
/// follows it, inside the block and after it; the effect count is the
/// native run's.
#[test]
fn a_failing_assertion_inside_a_destructured_block_stops_what_follows() {
    let cases = [
        ("plain_fail", 1, "  test \"t\" do\n    first, second = [1, 2].map do |x|\n      assert_equal 1, x\n      Widget.hit\n      x\n    end\n    Widget.hit\n  end\n"),
        ("own_fail", 0, "  def assert_shouted(text)\n    assert_equal text.upcase, text\n  end\n\n  test \"t\" do\n    first, second = [\"quiet\"].map do |text|\n      assert_shouted text\n      Widget.hit\n      text\n    end\n    Widget.hit\n  end\n"),
    ];
    for (name, hits, members) in cases {
        let run = destructured_block_app(name, members).run_test("test/models/widget_test.rb");
        assert!(run.errors.is_empty(), "{name}: {:?}", run.errors);
        assert!(!run.success, "{name}: the failing assertion passed\n{}", run.stdout);
        assert!(run.stdout.contains(&format!("CASE {name} HITS={hits}\n")), "{name}: {}", run.stdout);
    }
}

/// An app with no tables at all, the shape of a gem's unit test:
/// a library class and a test of it. The harness loads
/// `Schema.statements` unconditionally, and an empty schema emitted no
/// `Schema` module, so the test died with NameError before running.
#[test]
fn an_app_without_a_schema_runs_its_library_tests() {
    emit_and_run::empty_app()
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("lib/sql_fragments.rb", "module SqlFragments\n  class Sqlite3\n    class << self\n      def random\n        \"RANDOM()\"\n      end\n\n      def excluded(column)\n        \"excluded.#{column}\"\n      end\n    end\n  end\nend\n")
        .write("test/lib/sql_fragments_test.rb", r#"require "test_helper"

class SqlFragmentsTest < ActiveSupport::TestCase
  test "random" do
    assert_equal "RANDOM()", SqlFragments::Sqlite3.random
  end

  test "excluded" do
    assert_equal "excluded.name", SqlFragments::Sqlite3.excluded("name")
  end
end
"#)
        .run_test("test/models/sql_fragments_test.rb")
        .assert_passes();
}


/// Rails' `exists?(conditions = :none)` asks for any row when called bare
/// and answers false for a nil/false argument (`return false if
/// !conditions`), on the model class and on a relation alike.
#[test]
fn exists_without_an_argument_asks_for_any_row_and_nil_is_false() {
    emit_and_run::real_blog()
        .write(
            "test/models/article_exists_test.rb",
            r#"require "test_helper"

class ArticleExistsTest < ActiveSupport::TestCase
  test "bare exists? is any row; exists?(nil) is false" do
    Comment.delete_all
    Article.delete_all
    assert_not Article.exists?
    assert_not Article.all.exists?
    article = Article.create!(title: "Exists", body: "Body text here")
    assert Article.exists?
    assert Article.all.exists?
    assert Article.exists?(article.id)
    assert_not Article.exists?(nil)
    assert_not Article.all.exists?(nil)
  end
end
"#,
        )
        .run_test("test/models/article_exists_test.rb")
        .assert_passes();
}

/// Rails' `association_primary_key` is the `belongs_to`'s `primary_key:`,
/// else the target's primary key: the reader looks the target up by that
/// column, the writer stores that column's value in the foreign key, and
/// `joins` compares the foreign key with it.
#[test]
fn belongs_to_primary_key_option_names_the_key_the_foreign_key_holds() {
    emit_and_run::empty_app()
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"products\", force: :cascade do |t|\n    t.string \"uuid\"\n    t.string \"name\"\n  end\n  create_table \"prices\", force: :cascade do |t|\n    t.string \"product_uuid\"\n    t.integer \"cents\"\n  end\nend\n",
        )
        .write("app/models/product.rb", "class Product < ApplicationRecord\nend\n")
        .write(
            "app/models/price.rb",
            "class Price < ApplicationRecord\n  belongs_to :product, foreign_key: \"product_uuid\", primary_key: \"uuid\"\n\n  def self.for_product_named(name)\n    joins(:product).where(products: { name: name }).count\n  end\nend\n",
        )
        .run_ruby(
            "a = Product.create!(uuid: \"u-a\", name: \"A\")\n\
             b = Product.create!(uuid: \"u-b\", name: \"B\")\n\
             price = Price.create!(product_uuid: \"u-a\", cents: 5)\n\
             raise \"reader: #{price.product&.name.inspect}\" unless price.product.name == \"A\"\n\
             price.product = b\n\
             raise \"writer: #{price.product_uuid.inspect}\" unless price.product_uuid == \"u-b\"\n\
             price.save!\n\
             joined = Price.for_product_named(\"B\")\n\
             raise \"join: #{joined}\" unless joined == 1\n\
             ok = Price.new(product_uuid: \"u-a\", cents: 1)\n\
             raise \"valid? must find by uuid: #{ok.errors.full_messages}\" unless ok.valid?\n\
             missing = Price.new(product_uuid: \"u-missing\", cents: 1)\n\
             raise \"valid? must reject missing uuid\" if missing.valid?\n\
             puts \"belongs_to primary_key passed\"\n",
        )
        .assert_passes();
}

#[test]
fn generated_keyless_model_saves_without_assigning_an_id() {
    emit_and_run::empty_app()
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write("app/models/tagging.rb", "class Tagging < ApplicationRecord\nend\n")
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table :taggings, id: false do |t|\n    t.integer :tag_id\n  end\nend\n",
        )
        .run_ruby(
            r#"tagging = Tagging.new(tag_id: 7)
def tagging.id=(_value)
  raise "id= called for a keyless model"
end
raise "save failed" unless tagging.save
raise "primary key" unless Tagging.primary_key.nil?
raise "not persisted" unless tagging.persisted? && !tagging.new_record?
rows = Tagging.connection.select_rows("SELECT tag_id FROM taggings")
raise "inserted rows: #{rows.inspect}" unless rows == [[7]]
"#,
        )
        .assert_passes();
}

#[test]
fn a_primary_key_added_by_a_later_migration_persists_updates() {
    emit_and_run::empty_app()
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n")
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write(
            "db/migrate/20260101000000_create_widgets.rb",
            "class CreateWidgets < ActiveRecord::Migration[8.1]\n  def change\n    create_table :widgets, id: false do |t|\n      t.integer :value\n    end\n  end\nend\n",
        )
        .write(
            "db/migrate/20260102000000_add_widget_id.rb",
            "class AddWidgetId < ActiveRecord::Migration[8.1]\n  def change\n    add_column :widgets, :id, :integer, primary_key: true\n  end\nend\n",
        )
        .run_ruby(
            r#"# Attach to the existing table produced by Rails 8.1.4's migrations.
Db.exec("DROP TABLE widgets")
Db.exec('CREATE TABLE "widgets" ("value" integer, "id" integer PRIMARY KEY AUTOINCREMENT NOT NULL)')
widget = Widget.new(value: 7)
raise "insert failed" unless widget.save && widget.persisted?
widget.value = 8
raise "update failed" unless widget.save
rows = Widget.connection.select_rows("SELECT value, id FROM widgets")
raise "update was lost: #{rows.inspect}" unless rows == [[8, 1]]
raise "primary key" unless Widget.primary_key == "id" && widget.id == 1
"#,
        )
        .assert_passes();
}

/// A runtime façade's scalar constants (`Markly::SMART = 0` in
/// runtime/ruby/gem_facades.rb) used to carry no type: the constant table
/// typed Hash/Array/`.new` values only, so after the Rubydex integration an
/// app's `Markly::SMART` reference became a refusal stub and lobsters'
/// `Markdowner` raised at render. On the CRuby tree Markly is the real gem.
#[test]
fn markly_flag_constants_resolve_and_render_through_the_real_gem() {
    let run = emit_and_run::empty_app()
        .write("Gemfile", "source \"https://rubygems.org\"\ngem \"rails\"\ngem \"markly\"\n")
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write(
            "lib/markdown_renderer.rb",
            "class MarkdownRenderer\n  def self.render(text)\n    root = Markly.parse(text, flags: Markly::SMART, extensions: [:strikethrough])\n    root.to_html(flags: Markly::DEFAULT, extensions: [:strikethrough])\n  end\nend\n",
        )
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema[8.1].define(version: 2026_01_01_000000) do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n",
        )
        .run_ruby(
            r#"html = MarkdownRenderer.render("**bold** ~~gone~~")
raise "Markly render: #{html.inspect}" unless html.include?("<strong>bold</strong>") && html.include?("<del>gone</del>")
"#,
        );
    run.assert_passes();
}

/// A class body that initializes `@@` variables beside constants was
/// refused whole at ingest ("requires source ordering"), because the
/// emitter does not keep their relative order. When every constant and
/// every initializer is a literal neither can observe the other, so the
/// order is free. Lobsters' `Twitter` (`@@CONSUMER_KEY = nil`,
/// `MAX_TWEET_LEN = 280`) disappeared from the tree, and /settings raised.
#[test]
fn literal_class_variables_beside_literal_constants_ingest() {
    let run = emit_and_run::empty_app()
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write(
            "lib/poster.rb",
            "class Poster\n  cattr_accessor :CONSUMER_KEY\n\n  @@CONSUMER_KEY = nil\n\n  MAX_LEN = 280\n  SHORT_LEN = 23\n\n  def self.enabled?\n    self.CONSUMER_KEY.present?\n  end\n\n  def self.room\n    MAX_LEN - SHORT_LEN\n  end\nend\n",
        )
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema[8.1].define(version: 2026_01_01_000000) do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n",
        )
        .run_ruby(
            r#"raise "enabled? must start false" if Poster.enabled?
raise "constants: #{Poster.room}" unless Poster.room == 257
Poster.CONSUMER_KEY = "key"
raise "enabled? must follow the class variable" unless Poster.enabled?
"#,
        );
    run.assert_passes();
}

/// `recv.try(:m)` narrows to the classes that define `m`, and a model
/// defines every column reader without spelling it as a `def`. Counting
/// only `def`s made lobsters' `@user.try(:username)` narrow to `ModNote`
/// (whose `delegate :username` is a method) and answer nil for every
/// `User`: a logged-in page rendered `data-username=''`.
#[test]
fn try_answers_a_column_reader_beside_a_class_that_defines_the_method() {
    let run = emit_and_run::empty_app()
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n")
        .write("lib/note.rb", "class Note\n  def name\n    \"note\"\n  end\nend\n")
        .write(
            "lib/presenter.rb",
            "class Presenter\n  def self.label(obj)\n    obj.try(:name)\n  end\nend\n",
        )
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema[8.1].define(version: 2026_01_01_000000) do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n",
        )
        .run_ruby(
            r#"w = Widget.new(name: "gizmo")
raise "column reader: #{Presenter.label(w).inspect}" unless Presenter.label(w) == "gizmo"
raise "defined method: #{Presenter.label(Note.new).inspect}" unless Presenter.label(Note.new) == "note"
raise "nil receiver" unless Presenter.label(nil).nil?
"#,
        );
    run.assert_passes();
}

#[test]
fn render_inertia_always_prop_survives_a_partial_reload() {
    emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  get \"/pages/show\", to: \"pages#show\"\n",
        )
        .write(
            "app/controllers/pages_controller.rb",
            r#"class PagesController < ApplicationController
  def show
    render inertia: "pages/show", props: {
      n: 1,
      kept: InertiaRails.always { "yes" }
    }
  end
end
"#,
        )
        .write(
            "test/controllers/pages_controller_test.rb",
            r#"require "test_helper"

class PagesControllerTest < ActionDispatch::IntegrationTest
  test "always is present when the partial reload did not ask for it" do
    get "/pages/show", headers: {
      "X-Inertia" => "true",
      "X-Inertia-Partial-Component" => "pages/show",
      "X-Inertia-Partial-Data" => "n"
    }
    assert_response :success
    assert_equal({ "n" => 1, "kept" => "yes" }, response.parsed_body["props"])
  end
end
"#,
        )
        .run_test("test/controllers/pages_controller_test.rb")
        .assert_passes();
}

#[test]
fn render_inertia_computed_props_answer_json_and_html() {
    emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  get \"/pages/computed\", to: \"pages#computed\"\n  get \"/pages/local\", to: \"pages#local\"\n  get \"/pages/bad\", to: \"pages#bad\"\n",
        )
        .write(
            "config/initializers/inertia_rails.rb",
            "InertiaRails.configure do |config|\n  config.use_script_element_for_initial_page = true\nend\n",
        )
        .write(
            "app/controllers/pages_controller.rb",
            r#"class PagesController < ApplicationController
  def computed
    @stamp = "from-ivar"
    @article = Article.find_by!(title: "Getting Started with Rails")
    render inertia: "pages/computed", props: computed_props
  end

  def local
    props = { title: "from-local", n: 2 }
    render inertia: "pages/local", props: props
  end

  def bad
    render inertia: "pages/bad", props: "nope"
  end

  private
    def computed_props
      {
        title: "from-method",
        n: 1,
        label: -> { @stamp },
        secret: InertiaRails.optional { "opt" },
        stats: InertiaRails.defer { { total: 3 } },
        kept: InertiaRails.always { "yes" },
        when: Time.utc(2026, 10, 19, 11, 22, 0),
        row: { article: @article },
        rows: [@article]
      }
    end
end
"#,
        )
        .write(
            "test/controllers/pages_controller_test.rb",
            r#"require "test_helper"

class PagesControllerTest < ActionDispatch::IntegrationTest
  test "a method hash evaluates lambdas and defers the rest" do
    get "/pages/computed", headers: { "X-Inertia" => "true" }
    assert_response :success
    page = response.parsed_body
    assert_equal "from-method", page["props"]["title"]
    assert_equal 1, page["props"]["n"]
    assert_equal "from-ivar", page["props"]["label"]
    assert_equal "yes", page["props"]["kept"]
    assert_equal "2026-10-19T11:22:00.000Z", page["props"]["when"]
    assert_equal "Getting Started with Rails", page["props"]["row"]["article"]["title"]
    assert_equal "Getting Started with Rails", page["props"]["rows"][0]["title"]
    assert_nil page["props"]["secret"]
    assert_nil page["props"]["stats"]
    assert_equal({ "default" => ["stats"] }, page["deferredProps"])
  end

  test "a partial reload evaluates the requested optional and deferred props" do
    get "/pages/computed", headers: {
      "X-Inertia" => "true",
      "X-Inertia-Partial-Component" => "pages/computed",
      "X-Inertia-Partial-Data" => "secret,stats"
    }
    assert_response :success
    assert_equal({ "secret" => "opt", "stats" => { "total" => 3 }, "kept" => "yes" }, response.parsed_body["props"])
    assert_nil response.parsed_body["deferredProps"]
  end

  test "a local hash is the page props" do
    get "/pages/local"
    assert_response :success
    assert_includes response.body, %("title":"from-local")
    assert_includes response.body, %("n":2)
  end

  test "a partial reload keeps the requested prop and an always prop" do
    get "/pages/computed", headers: {
      "X-Inertia" => "true",
      "X-Inertia-Partial-Component" => "pages/computed",
      "X-Inertia-Partial-Data" => "n"
    }
    assert_response :success
    assert_equal({ "n" => 1, "kept" => "yes" }, response.parsed_body["props"])
  end

  test "a non-hash is not an empty page" do
    assert_raises(TypeError) do
      get "/pages/bad", headers: { "X-Inertia" => "true" }
    end
  end
end
"#,
        )
        .run_test("test/controllers/pages_controller_test.rb")
        .assert_passes();
}

#[test]
fn render_inertia_computed_component_answers_json_and_partial_reload() {
    emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  get \"/pages/constant\", to: \"pages#constant\"\n  get \"/pages/named\", to: \"pages#named\"\n  get \"/pages/joined\", to: \"pages#joined\"\n",
        )
        .write(
            "config/initializers/inertia_rails.rb",
            "InertiaRails.configure do |config|\n  config.use_script_element_for_initial_page = true\nend\n",
        )
        .write(
            "app/controllers/pages_controller.rb",
            r#"class PagesController < ApplicationController
  PAGE = "pages/from-constant"

  def constant
    render inertia: PAGE, props: { title: "c", n: 1 }
  end

  def named
    render inertia: page_name, props: { title: "m", n: 2 }
  end

  def joined
    render inertia: "pages/" + action_name, props: { title: "j", n: 3 }
  end

  private
    def page_name
      "pages/from-method"
    end
end
"#,
        )
        .write(
            "test/controllers/pages_controller_test.rb",
            r#"require "test_helper"

class PagesControllerTest < ActionDispatch::IntegrationTest
  test "a constant is the component" do
    get "/pages/constant", headers: { "X-Inertia" => "true" }
    assert_response :success
    assert_equal "pages/from-constant", response.parsed_body["component"]
    assert_equal({ "title" => "c", "n" => 1 }, response.parsed_body["props"])
  end

  test "a method is the component and a partial reload matches that name" do
    get "/pages/named", headers: {
      "X-Inertia" => "true",
      "X-Inertia-Partial-Component" => "pages/from-method",
      "X-Inertia-Partial-Data" => "n"
    }
    assert_response :success
    assert_equal "pages/from-method", response.parsed_body["component"]
    assert_equal({ "n" => 2 }, response.parsed_body["props"])
  end

  test "a joined name is the component" do
    get "/pages/joined"
    assert_response :success
    assert_includes response.body, %("component":"pages/joined")
    assert_includes response.body, %("title":"j")
  end
end
"#,
        )
        .run_test("test/controllers/pages_controller_test.rb")
        .assert_passes();
}

#[test]
fn inertia_clear_history_reaches_the_next_visit_once() {
    emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  get \"/pages/show\", to: \"pages#show\"\n  delete \"/pages\", to: \"pages#destroy\"\n  delete \"/pages/keep\", to: \"pages#keep\"\n",
        )
        .write(
            "config/initializers/inertia_rails.rb",
            "InertiaRails.configure do |config|\n  config.version = \"v1\"\n  config.always_include_errors_hash = true\n  config.use_script_element_for_initial_page = true\nend\n",
        )
        .write(
            "app/controllers/pages_controller.rb",
            r#"class PagesController < ApplicationController
  def show
    render inertia: "pages/show", props: { title: "kept" }
  end

  def destroy
    redirect_to "/pages/show", inertia: { errors: { base: ["gone"] }, clear_history: true }
  end

  def keep
    redirect_to "/pages/show", inertia: { clear_history: false }
  end
end
"#,
        )
        .write(
            "test/controllers/pages_controller_test.rb",
            r#"require "test_helper"

class PagesControllerTest < ActionDispatch::IntegrationTest
  INERTIA = { "X-Inertia" => "true", "X-Inertia-Version" => "v1" }

  test "clear_history is on the next visit and then gone" do
    delete "/pages", headers: INERTIA
    assert_response 303
    assert_redirected_to "/pages/show"
    get "/pages/show", headers: INERTIA
    assert_equal true, response.parsed_body["clearHistory"]
    assert_equal({ "base" => ["gone"] }, response.parsed_body["props"]["errors"])
    get "/pages/show", headers: INERTIA
    assert_equal false, response.parsed_body["clearHistory"]
    assert_equal({}, response.parsed_body["props"]["errors"])
  end

  test "a stale version keeps clear_history for the full-page reload" do
    delete "/pages", headers: INERTIA
    get "/pages/show", headers: { "X-Inertia" => "true", "X-Inertia-Version" => "v0" }
    assert_response 409
    get "/pages/show"
    assert_includes response.body, %("clearHistory":true)
    get "/pages/show"
    assert_includes response.body, %("clearHistory":false)
  end

  test "a false flag does not clear history" do
    delete "/pages/keep", headers: INERTIA
    assert_response 303
    get "/pages/show", headers: INERTIA
    assert_equal false, response.parsed_body["clearHistory"]
  end
end
"#,
        )
        .run_test("test/controllers/pages_controller_test.rb")
        .assert_passes();
}

#[test]
fn inertia_preserve_fragment_reaches_the_next_visit_once() {
    emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  get \"/pages/show\", to: \"pages#show\"\n  patch \"/pages\", to: \"pages#update\"\n  patch \"/pages/drop\", to: \"pages#drop\"\n",
        )
        .write(
            "config/initializers/inertia_rails.rb",
            "InertiaRails.configure do |config|\n  config.version = \"v1\"\n  config.use_script_element_for_initial_page = true\nend\n",
        )
        .write(
            "app/controllers/pages_controller.rb",
            r#"class PagesController < ApplicationController
  def show
    render inertia: "pages/show", props: { title: "kept" }
  end

  def update
    redirect_to "/pages/show", inertia: { preserve_fragment: true }
  end

  def drop
    redirect_to "/pages/show", inertia: { preserve_fragment: false }
  end
end
"#,
        )
        .write(
            "test/controllers/pages_controller_test.rb",
            r#"require "test_helper"

class PagesControllerTest < ActionDispatch::IntegrationTest
  INERTIA = { "X-Inertia" => "true", "X-Inertia-Version" => "v1" }

  test "preserve_fragment is on the next visit and then gone" do
    patch "/pages", headers: INERTIA
    assert_response 303
    get "/pages/show", headers: INERTIA
    assert_equal true, response.parsed_body["preserveFragment"]
    get "/pages/show", headers: INERTIA
    assert_nil response.parsed_body["preserveFragment"]
  end

  test "a stale version keeps preserve_fragment for the full-page reload" do
    patch "/pages", headers: INERTIA
    get "/pages/show", headers: { "X-Inertia" => "true", "X-Inertia-Version" => "v0" }
    assert_response 409
    get "/pages/show"
    assert_includes response.body, %("preserveFragment":true)
    get "/pages/show"
    refute_includes response.body, "preserveFragment"
  end

  test "a false flag does not preserve the fragment" do
    patch "/pages/drop", headers: INERTIA
    assert_response 303
    get "/pages/show", headers: INERTIA
    assert_nil response.parsed_body["preserveFragment"]
  end
end
"#,
        )
        .run_test("test/controllers/pages_controller_test.rb")
        .assert_passes();
}

#[test]
fn inertia_location_is_a_conflict_for_an_inertia_visit() {
    emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  get \"/pages/away\", to: \"pages#away\"\n  get \"/pages/away_break\", to: \"pages#away_break\"\n",
        )
        .write(
            "app/controllers/pages_controller.rb",
            r#"class PagesController < ApplicationController
  def away
    inertia_location "https://example.com/away"
  end

  def away_break
    inertia_location "https://example.com/away\r\nSet-Cookie: x=1"
  end
end
"#,
        )
        .write(
            "test/controllers/pages_controller_test.rb",
            r#"require "test_helper"

class PagesControllerTest < ActionDispatch::IntegrationTest
  test "an Inertia visit is a 409 with the external url" do
    get "/pages/away", headers: { "X-Inertia" => "true" }
    assert_response 409
    assert_equal "https://example.com/away", response.headers["X-Inertia-Location"]
  end

  test "a normal visit redirects to the external url" do
    get "/pages/away"
    assert_response 302
    assert_equal "https://example.com/away", response.headers["Location"]
  end

  test "a line break is removed before either location is written" do
    get "/pages/away_break", headers: { "X-Inertia" => "true" }
    assert_response 409
    assert_equal "https://example.com/awaySet-Cookie: x=1", response.headers["X-Inertia-Location"]
    get "/pages/away_break"
    assert_response 302
    assert_equal "https://example.com/awaySet-Cookie: x=1", response.headers["Location"]
  end
end
"#,
        )
        .run_test("test/controllers/pages_controller_test.rb")
        .assert_passes();
}

#[test]
fn render_inertia_answers_html_and_json_visits() {
    emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  get \"/pages/show\", to: \"pages#show\"\n",
        )
        .write(
            "config/initializers/inertia_rails.rb",
            "InertiaRails.configure do |config|\n  config.use_script_element_for_initial_page = true\nend\n",
        )
        .write(
            "app/controllers/pages_controller.rb",
            r#"class PagesController < ApplicationController
  def show
    render inertia: "pages/show", props: {
      title: "Hello </script>",
      count: -> { Article.count },
      label: -> { "eager-lambda" }
    }
  end
end
"#,
        )
        .write(
            "test/controllers/pages_controller_test.rb",
            r#"require "test_helper"

class PagesControllerTest < ActionDispatch::IntegrationTest
  test "a first visit renders the page object inside the layout" do
    get "/pages/show"
    assert_response :success
    page = %({"component":"pages/show","props":{"title":"Hello \\u003c/script\\u003e","count":#{Article.count},"label":"eager-lambda"},) +
      %("url":"/pages/show","version":null,"encryptHistory":false,"clearHistory":false})
    assert_includes response.body, %(<script data-page="app" type="application/json">#{page}</script>\n<div id="app"></div>)
    assert_includes response.body, "<title>Real Blog</title>"
    assert_equal "X-Inertia", response.headers["vary"]
  end

  test "an Inertia visit gets the page object as JSON" do
    get "/pages/show", headers: { "X-Inertia" => "true" }
    assert_response :success
    assert_equal "true", response.headers["x-inertia"]
    assert_equal "X-Inertia", response.headers["vary"]
    page = response.parsed_body
    assert_equal "pages/show", page["component"]
    assert_equal(
      { "title" => "Hello </script>", "count" => Article.count, "label" => "eager-lambda" },
      page["props"]
    )
    assert_equal "/pages/show", page["url"]
  end
end
"#,
        )
        .run_test("test/controllers/pages_controller_test.rb")
        .assert_passes();
}

#[test]
fn inertia_protocol_shared_props_reloads_deferral_and_errors() {
    emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  get \"/inertia_articles\", to: \"inertia_articles#index\"\n  patch \"/inertia_articles\", to: \"inertia_articles#update\"\n",
        )
        .write(
            "config/initializers/inertia_rails.rb",
            "InertiaRails.configure do |config|\n  config.version = \"v1\"\n  config.encrypt_history = true\n  config.always_include_errors_hash = true\n  config.use_script_element_for_initial_page = true\nend\n",
        )
        .edit(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\n",
            "class ApplicationController < ActionController::Base\n  inertia_share articleCount: -> { Article.count },\n                appName: \"Real Blog\",\n                secret: InertiaRails.optional { \"opt\" }\n",
        )
        .write(
            "app/controllers/inertia_articles_controller.rb",
            r#"class InertiaArticlesController < ApplicationController
  def index
    render inertia: "inertia_articles/index", props: {
      titles: Article.order(:id).pluck(:title),
      inertiaVisit: request.inertia?,
      stats: InertiaRails.defer { { total: Article.count } }
    }
  end

  def update
    if params[:title].to_s.empty?
      redirect_to "/inertia_articles", inertia: { errors: { title: ["can't be blank"] } }
    else
      redirect_to "/inertia_articles"
    end
  end
end
"#,
        )
        .write(
            "test/controllers/inertia_articles_controller_test.rb",
            r#"require "test_helper"

class InertiaArticlesControllerTest < ActionDispatch::IntegrationTest
  INERTIA = { "X-Inertia" => "true", "X-Inertia-Version" => "v1" }

  test "a first visit includes shared props and lists the deferred one" do
    get "/inertia_articles"
    assert_response :success
    props = %("props":{"errors":{},"articleCount":#{Article.count},"appName":"Real Blog",) +
      %("titles":#{Article.order(:id).pluck(:title).to_json},"inertiaVisit":false},)
    assert_includes response.body, props
    assert_includes response.body, %("version":"v1","encryptHistory":true,"clearHistory":false,)
    assert_includes response.body, %("sharedProps":["errors","articleCount","appName","secret"],"deferredProps":{"default":["stats"]}})
  end

  test "an Inertia visit with the current version gets JSON" do
    get "/inertia_articles", headers: INERTIA
    assert_response :success
    page = response.parsed_body
    assert_equal %w[errors articleCount appName titles inertiaVisit], page["props"].keys
    assert_equal true, page["props"]["inertiaVisit"]
    assert_equal({ "default" => ["stats"] }, page["deferredProps"])
  end

  test "a stale version gets 409 and the URL to reload" do
    get "/inertia_articles", headers: { "X-Inertia" => "true", "X-Inertia-Version" => "v0" }
    assert_response 409
    assert_equal "http://www.example.com/inertia_articles", response.headers["x-inertia-location"]
  end

  test "a partial reload answers only the props it names" do
    get "/inertia_articles", headers: INERTIA.merge("X-Inertia-Partial-Component" => "inertia_articles/index",
                                                    "X-Inertia-Partial-Data" => "secret,stats")
    page = response.parsed_body
    assert_equal({ "errors" => {}, "secret" => "opt", "stats" => { "total" => Article.count } }, page["props"])
    assert_nil page["deferredProps"]
  end

  test "a partial reload can leave props out" do
    get "/inertia_articles", headers: INERTIA.merge("X-Inertia-Partial-Component" => "inertia_articles/index",
                                                    "X-Inertia-Partial-Except" => "titles,articleCount")
    assert_equal %w[errors appName secret inertiaVisit stats], response.parsed_body["props"].keys
  end

  test "partial headers for another component are ignored" do
    get "/inertia_articles", headers: INERTIA.merge("X-Inertia-Partial-Component" => "other/page",
                                                    "X-Inertia-Partial-Data" => "secret")
    assert_equal %w[errors articleCount appName titles inertiaVisit], response.parsed_body["props"].keys
  end

  test "a failed Inertia PATCH redirects with 303 and the errors reach the next visit once" do
    patch "/inertia_articles", params: { title: "" }, headers: INERTIA
    assert_response 303
    assert_redirected_to "/inertia_articles"
    get "/inertia_articles", headers: INERTIA
    assert_equal({ "title" => ["can't be blank"] }, response.parsed_body["props"]["errors"])
    get "/inertia_articles", headers: INERTIA
    assert_equal({}, response.parsed_body["props"]["errors"])
  end

  test "a stale version keeps the stored errors for the full-page reload" do
    patch "/inertia_articles", params: { title: "" }, headers: INERTIA
    get "/inertia_articles", headers: { "X-Inertia" => "true", "X-Inertia-Version" => "v0" }
    assert_response 409
    get "/inertia_articles"
    assert_includes response.body, %("props":{"errors":{"title":["can't be blank"]},)
  end

  test "a PATCH outside Inertia keeps its 302" do
    patch "/inertia_articles", params: { title: "ok" }
    assert_response 302
  end
end
"#,
        )
        .run_test("test/controllers/inertia_articles_controller_test.rb")
        .assert_passes();
}

/// The same boundary on the native lane, which sanitizes with the shared
/// runtime's safe-list scanner rather than the rails-html-sanitizer gem.
#[test]
#[ignore = "requires the Spinel toolchain"]
fn rich_text_renders_sanitized_on_spinel() {
    rich_text_app()
        .run_spinel(
            r#"html = ActionText::Content.new(%(<p>kept <b>bold</b></p><script>alert(1)</script><img src="x" onerror="alert(2)"><a href="javascript:alert(3)">link</a>)).to_s
raise "lost the allowed markup: #{html}" unless html.include?("<p>kept <b>bold</b></p>")
raise "script survived: #{html}" if html.include?("<script")
raise "event handler survived: #{html}" if html.include?("onerror")
raise "javascript: URL survived: #{html}" if html.include?("javascript:")
puts "rich text sanitized"
"#,
        )
        .assert_passes();
}


#[test]
fn emitted_like_escaping_matches_rails_default_and_custom_markers() {
    emit_and_run::real_blog()
        .write("app/services/like_probe.rb", r#"class LikeProbe
  def self.default(value)
    ActiveRecord::Base.sanitize_sql_like(value)
  end

  def self.custom(value, escape)
    ActiveRecord::Base.sanitize_sql_like(value, escape)
  end

  def self.inherited(value)
    Article.sanitize_sql_like(value)
  end
end
"#)
        .run_ruby(r#"
cases = [
  ["", "\\", ""],
  ["plain text", "\\", "plain text"],
  ["100%_done", "\\", "100\\%\\_done"],
  ["back\\slash%_", "\\", "back\\\\slash\\%\\_"],
  ["100%_!", "!", "100!%!_!!"],
  ["100%_", "%", "100%%%_"],
  ["100%_", "_", "100_%__"],
  ["ab%_ab", "ab", "ababab%ab_abab"],
  ["100%_", "", "100%_"],
  ["café_50%", "!", "café!_50!%"],
  ["é%_é", "é", "ééé%é_éé"],
  ["x%_", "\\\\", "x\\%\\_"],
  ["x%_", '\0', "x%_"],
  ["x%_", '\&', "x%_"],
  ["x%_", '\1', "x%_"],
  ["x%_", '\9', "x%_"],
  ["x%_", '\+', "x%_"],
  ["x%_", '\10', "x0%0_"],
  ["x%_", '\`', "xx%x%_"],
  ["x%_", "\\'", "x%_%__"],
  ["x%_", '\q', 'x\q%\q_']
]
cases.each do |value, escape, expected|
  actual = LikeProbe.custom(value, escape)
  raise "custom #{value.inspect}/#{escape.inspect}: #{actual.inspect}, expected #{expected.inspect}" unless actual == expected
  if escape == "\\"
    raise "default marker differs" unless LikeProbe.default(value) == expected
    raise "model inheritance differs" unless LikeProbe.inherited(value) == expected
  end
end
[
  ['\k<name>', IndexError, "undefined group name reference: name"],
  ['\k<>', IndexError, "undefined group name reference: "],
  ['\k<name', RuntimeError, "invalid group name reference format"]
].each do |escape, error_class, message|
  begin
    LikeProbe.custom("x%_", escape)
    raise "named capture accepted"
  rescue error_class => error
    raise "wrong named capture error: #{error.message.inspect}" unless error.message == message
  end
  raise "unused replacement raised" unless LikeProbe.custom("plain", escape) == "plain"
end
database = SQLite3::Database.new(":memory:")
database.execute("CREATE TABLE like_values (title TEXT)")
["100%_done", "100Xdone", "100XYdone", 'back\\slash%_'].each do |title|
  database.execute("INSERT INTO like_values VALUES (?)", title)
end
["100%_done", 'back\\slash%_'].each do |value|
  rows = database.execute("SELECT title FROM like_values WHERE title LIKE ? ESCAPE '\\'", LikeProbe.default(value))
  raise "SQLite literal LIKE differs: #{rows.inspect}" unless rows == [[value]]
end
database.close
puts "sanitize_sql_like: default, inherited, custom, replacement escapes, and SQLite ESCAPE passed"
"#)
        .assert_passes();
}


#[test]
fn vite_tags_resolve_from_the_manifest() {
    const MANIFEST: &str = r#"{
  "entrypoints/inertia.jsx": {"file": "assets/inertia-abc.js", "src": "entrypoints/inertia.jsx", "isEntry": true,
                              "imports": ["_ui-1.js", "_react-2.js"], "css": ["assets/inertia-abc.css"]},
  "_ui-1.js": {"file": "assets/ui-1.js", "imports": ["_react-2.js"], "css": ["assets/ui-1.css"]},
  "_react-2.js": {"file": "assets/react-2.js"}
}
"#;
    emit_and_run::real_blog()
        .write("public/vite/.vite/manifest.json", MANIFEST)
        .write("public/vite/assets/inertia-abc.js", "console.log('inertia');\n")
        .write(
            "config/initializers/inertia_rails.rb",
            "InertiaRails.configure do |config|\n  config.version = ViteRuby.digest\nend\n",
        )
        .edit(
            "app/views/layouts/application.html.erb",
            "    <%= javascript_importmap_tags %>\n",
            "    <%= javascript_importmap_tags %>\n    <%= vite_react_refresh_tag %>\n    <%= vite_client_tag %>\n    <%= vite_javascript_tag \"inertia.jsx\" %>\n    <%= inertia_ssr_head %>\n",
        )
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  get \"/pages/show\", to: \"pages#show\"\n",
        )
        .write(
            "app/controllers/pages_controller.rb",
            "class PagesController < ApplicationController\n  def show\n    render inertia: \"pages/show\"\n  end\nend\n",
        )
        .write(
            "test/controllers/pages_controller_test.rb",
            &(r##"require "test_helper"

class PagesControllerTest < ActionDispatch::IntegrationTest
  MANIFEST = <<-'JSON'
"##.to_string()
                + MANIFEST
                + r##"JSON

  test "the layout includes the build's tags and the page includes its digest" do
    get "/pages/show"
    assert_response :success
    tags = %(<script src="/vite/assets/inertia-abc.js" crossorigin="" type="module"></script>) +
      %(<link rel="modulepreload" href="/vite/assets/react-2.js" as="script" crossorigin="">\n) +
      %(<link rel="modulepreload" href="/vite/assets/ui-1.js" as="script" crossorigin="">) +
      %(<link rel="stylesheet" crossorigin="" href="/vite/assets/inertia-abc.css" />\n) +
      %(<link rel="stylesheet" crossorigin="" href="/vite/assets/ui-1.css" />)
    assert_includes response.body, tags
    assert_no_match(/vite_|@vite|RefreshRuntime/, response.body)
    digest = "cbf29ce484222325".to_i(16)
    mask = 2**64 - 1
    MANIFEST.each_byte do |b|
      digest = ((digest ^ b) * 0x100000001b3) & mask
    end
    assert_includes response.body, %(<div id="app" data-page="{&quot;component&quot;:&quot;pages/show&quot;,&quot;props&quot;:{},) +
      %(&quot;url&quot;:&quot;/pages/show&quot;,&quot;version&quot;:&quot;#{format("%016x", digest)}&quot;,)
    assert File.exist?("public/vite/assets/inertia-abc.js")
  end
end
"##),
        )
        .run_test("test/controllers/pages_controller_test.rb")
        .assert_passes();
}


// ---------------------------------------------------------------------
// vite_typescript_tag / vite_javascript_tag: extra HTML options are
// passed through to the script tag, the way vite_rails does. Without
// this, a call like `vite_typescript_tag "gadget.ts", crossorigin:
// "anonymous"` was an ingest gap for the whole call — on a large real
// app that gap blocked every route that reached the view.
// ---------------------------------------------------------------------

#[test]
fn vite_tag_html_options_pass_through_to_the_script_tag() {
    const MANIFEST: &str = r#"{
  "entrypoints/gadget.ts": {"file": "assets/gadget-abc.js", "src": "entrypoints/gadget.ts", "isEntry": true},
  "entrypoints/widget.ts": {"file": "assets/widget-xyz.js", "src": "entrypoints/widget.ts", "isEntry": true}
}
"#;
    emit_and_run::real_blog()
        .write("public/vite/.vite/manifest.json", MANIFEST)
        .write("public/vite/assets/gadget-abc.js", "console.log('gadget');\n")
        .write("public/vite/assets/widget-xyz.js", "console.log('widget');\n")
        .edit(
            "app/views/layouts/application.html.erb",
            "    <%= javascript_importmap_tags %>\n",
            "    <%= javascript_importmap_tags %>\n    <%= vite_typescript_tag \"gadget.ts\", crossorigin: \"anonymous\", defer: true %>\n    <%= vite_typescript_tag \"widget.ts\", data: { turbo_track: \"reload\" } %>\n",
        )
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  get \"/vite_options\", to: \"vite_options#show\"\n",
        )
        .write(
            "app/controllers/vite_options_controller.rb",
            "class ViteOptionsController < ApplicationController\n  def show\n  end\nend\n",
        )
        .write("app/views/vite_options/show.html.erb", "<p>vite options probe</p>\n")
        .write(
            "test/controllers/vite_options_controller_test.rb",
            r##"require "test_helper"

class ViteOptionsControllerTest < ActionDispatch::IntegrationTest
  test "crossorigin, defer and a data hash pass through like vite_rails" do
    get "/vite_options"
    assert_response :success
    assert_includes response.body,
      %(<script src="/vite/assets/gadget-abc.js" crossorigin="anonymous" type="module" defer="defer"></script>)
    assert_includes response.body,
      %(<script src="/vite/assets/widget-xyz.js" crossorigin="" type="module" data-turbo-track="reload"></script>)
    assert_no_match(/vite_typescript_tag/, response.body)
  end
end
"##,
        )
        .run_test("test/controllers/vite_options_controller_test.rb")
        .assert_passes();
}

/// `crossorigin:`/`type:` overrides are a plain (non-boolean) attribute in
/// Rails' `TagBuilder#tag_options`: a `"` or `&` in the value must be
/// html-escaped like any other attribute text, `nil` omits the attribute
/// entirely rather than rendering `=""`, and `false` (not `nil`, not
/// `true`) renders the literal string `"false"`.
#[test]
fn vite_tag_crossorigin_and_type_overrides_escape_nil_and_false_like_rails() {
    const MANIFEST: &str = r#"{
  "entrypoints/sprocket.ts": {"file": "assets/sprocket-abc.js", "src": "entrypoints/sprocket.ts", "isEntry": true},
  "entrypoints/doohickey.ts": {"file": "assets/doohickey-abc.js", "src": "entrypoints/doohickey.ts", "isEntry": true},
  "entrypoints/gizmo.ts": {"file": "assets/gizmo-abc.js", "src": "entrypoints/gizmo.ts", "isEntry": true}
}
"#;
    emit_and_run::real_blog()
        .write("public/vite/.vite/manifest.json", MANIFEST)
        .write("public/vite/assets/sprocket-abc.js", "console.log('sprocket');\n")
        .write("public/vite/assets/doohickey-abc.js", "console.log('doohickey');\n")
        .write("public/vite/assets/gizmo-abc.js", "console.log('gizmo');\n")
        .edit(
            "app/views/layouts/application.html.erb",
            "    <%= javascript_importmap_tags %>\n",
            "    <%= javascript_importmap_tags %>\n    <%= vite_typescript_tag 'sprocket.ts', type: 'a\"b&c' %>\n    <%= vite_typescript_tag 'doohickey.ts', crossorigin: nil, type: nil %>\n    <%= vite_typescript_tag 'gizmo.ts', crossorigin: false %>\n",
        )
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  get \"/vite_escaping\", to: \"vite_escaping#show\"\n",
        )
        .write(
            "app/controllers/vite_escaping_controller.rb",
            "class ViteEscapingController < ApplicationController\n  def show\n  end\nend\n",
        )
        .write("app/views/vite_escaping/show.html.erb", "<p>vite escaping probe</p>\n")
        .write(
            "test/controllers/vite_escaping_controller_test.rb",
            r##"require "test_helper"

class ViteEscapingControllerTest < ActionDispatch::IntegrationTest
  test "crossorigin/type overrides escape, omit nil, and render false like Rails" do
    get "/vite_escaping"
    assert_response :success
    assert_includes response.body,
      %(<script src="/vite/assets/sprocket-abc.js" crossorigin="" type="a&quot;b&amp;c"></script>)
    assert_includes response.body,
      %(<script src="/vite/assets/doohickey-abc.js"></script>)
    assert_includes response.body,
      %(<script src="/vite/assets/gizmo-abc.js" crossorigin="false" type="module"></script>)
    assert_no_match(/vite_typescript_tag/, response.body)
  end
end
"##,
        )
        .run_test("test/controllers/vite_escaping_controller_test.rb")
        .assert_passes();
}



// ---------------------------------------------------------------------
// friendly_id: `Article.friendly.find` (slugged + history)
// ---------------------------------------------------------------------

/// real-blog with friendly_id on Article, as the EngineeredAt models
/// write it (`extend FriendlyId` + `friendly_id :title, use: %i[slugged
/// history]`), a `slug` column, the gem's `friendly_id_slugs` table, and
/// the controller's `set_article` reading through `Article.friendly`.
fn friendly_article_overlay(use_modules: &str) -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit(
            "db/schema.rb",
            "  create_table \"articles\", force: :cascade do |t|\n    t.string \"title\"\n",
            "  create_table \"friendly_id_slugs\", force: :cascade do |t|\n    t.string \"slug\", null: false\n    t.integer \"sluggable_id\", null: false\n    t.string \"sluggable_type\", limit: 50\n    t.string \"scope\"\n    t.datetime \"created_at\"\n    t.index [\"slug\", \"sluggable_type\"], name: \"index_friendly_id_slugs_on_slug_and_sluggable_type\"\n  end\n\n  create_table \"articles\", force: :cascade do |t|\n    t.string \"title\"\n    t.string \"slug\"\n",
        )
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            &format!("class Article < ApplicationRecord\n  extend FriendlyId\n  friendly_id :title, use: {use_modules}\n"),
        )
        .edit(
            "app/controllers/articles_controller.rb",
            "@article = Article.find(params.expect(:id))",
            "@article = Article.friendly.find(params[:id])",
        )
        .write(
            "test/controllers/articles_controller_test.rb",
            r#"require "test_helper"

class ArticlesControllerTest < ActionDispatch::IntegrationTest
  setup do
    Comment.delete_all
    Article.delete_all
    @gamma = Article.create!(title: "Gamma Post", body: "gamma body is long enough", slug: "gamma-post")
    # A slug that is another row's id: friendly_id looks the slug up
    # FIRST, so /articles/<gamma.id> is this article, not gamma.
    @beta = Article.create!(title: "Beta Post", body: "beta body is long enough here", slug: @gamma.id.to_s)
    @alpha = Article.create!(title: "Alpha Post", body: "alpha body is long enough", slug: "alpha-post")
  end

  test "a slug finds its article" do
    get "/articles/alpha-post"
    assert_response :success
    assert_select "h1", "Alpha Post"
  end

  test "an id still finds its article when no slug matches" do
    get "/articles/#{@alpha.id}"
    assert_response :success
    assert_select "h1", "Alpha Post"
    get "/articles/#{@beta.id}"
    assert_response :success
    assert_select "h1", "Beta Post"
  end

  test "a slug that is another record's id wins over the id" do
    get "/articles/#{@gamma.id}"
    assert_response :success
    assert_select "h1", "Beta Post"
  end
end
"#,
        )
}

/// friendly_id's finder over `Article.friendly.find`, in the emitted
/// Ruby: slug first, then the id, 404 otherwise (the pairs
/// `tools/compare` reproduces against the gem itself).
#[test]
fn friendly_find_resolves_a_slug_then_an_id() {
    friendly_article_overlay("%i[slugged]")
        .run_test("test/controllers/articles_controller_test.rb")
        .assert_passes();
}

/// `:history` also finds an article by a slug it no longer has, newest
/// history row first; a current slug beats a history row.
#[test]
fn friendly_find_with_history_finds_a_former_slug() {
    let overlay = friendly_article_overlay("%i[slugged history]").write(
        "test/controllers/articles_controller_test.rb",
        r#"require "test_helper"

class ArticlesControllerTest < ActionDispatch::IntegrationTest
  setup do
    Comment.delete_all
    Article.delete_all
    @old = Article.create!(title: "Renamed Post", body: "renamed body is long enough", slug: "renamed-post")
    @other = Article.create!(title: "Other Post", body: "other body is long enough", slug: "other-post")
    conn = ActiveRecord::Base.connection
    # history rows: the former slug "first-name" belonged to @old; the
    # slug "contested" is in history for BOTH articles (the newest row,
    # the higher id, is @other's); a history row for a different model
    # type must not match.
    conn.execute("INSERT INTO friendly_id_slugs (slug, sluggable_type, sluggable_id) VALUES ('first-name', 'Article', #{@old.id})")
    conn.execute("INSERT INTO friendly_id_slugs (slug, sluggable_type, sluggable_id) VALUES ('contested', 'Article', #{@old.id})")
    conn.execute("INSERT INTO friendly_id_slugs (slug, sluggable_type, sluggable_id) VALUES ('contested', 'Article', #{@other.id})")
    conn.execute("INSERT INTO friendly_id_slugs (slug, sluggable_type, sluggable_id) VALUES ('wrong-type', 'Topic', #{@old.id})")
    conn.execute("INSERT INTO friendly_id_slugs (slug, sluggable_type, sluggable_id) VALUES ('other-post', 'Article', #{@old.id})")
  end

  test "a former slug finds its article" do
    get "/articles/first-name"
    assert_response :success
    assert_select "h1", "Renamed Post"
  end

  test "the newest history row wins" do
    get "/articles/contested"
    assert_response :success
    assert_select "h1", "Other Post"
  end

  test "a current slug beats a history row for another article" do
    get "/articles/other-post"
    assert_response :success
    assert_select "h1", "Other Post"
  end

end
"#,
    );
    overlay.run_test("test/controllers/articles_controller_test.rb").assert_passes();
}

/// What matches nothing is `RecordNotFound` (404 through the dispatcher,
/// as in Rails), with friendly_id's message for a String that could not
/// be a key and Rails' own for one that could. Run through the
/// dispatcher: the emitted test harness reports a raise as a failure.
#[test]
fn friendly_find_answers_404_for_what_matches_nothing() {
    friendly_article_overlay("%i[slugged history]")
        .run_ruby(
            r##"
alpha = Article.create!(title: "Alpha Post", body: "alpha body is long enough", slug: "alpha-post")
ActiveRecord::Base.connection.execute("INSERT INTO friendly_id_slugs (slug, sluggable_type, sluggable_id) VALUES ('wrong-type', 'Topic', #{alpha.id})")
ActiveRecord::Base.connection.execute("INSERT INTO friendly_id_slugs (slug, sluggable_type, sluggable_id) VALUES ('old-alpha', 'Article', #{alpha.id})")
def status(path)
  status, = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => path, "QUERY_STRING" => "", "rack.input" => StringIO.new(""))
  status
end
want = {
  "/articles/alpha-post" => 200,
  "/articles/old-alpha" => 200,
  "/articles/#{alpha.id}" => 200,
  "/articles/no-such-post" => 404,
  "/articles/wrong-type" => 404,
  "/articles/#{alpha.id + 1000}" => 404,
  "/articles/#{alpha.id}-alpha-post" => 404,
}
want.each do |path, code|
  got = status(path)
  raise "GET #{path} answered #{got}, want #{code}" unless got == code
end

def not_found(arg)
  Article.friendly.find(arg)
  raise "no raise for #{arg.inspect}"
rescue ActiveRecord::RecordNotFound => e
  e
end
e = not_found("no-such-post")
raise "message #{e.message}" unless e.message == "can't find record with friendly id: \"no-such-post\""
raise "model/key/id #{[e.model, e.primary_key, e.id].inspect}" unless [e.model, e.primary_key, e.id] == ["Article", "slug", "no-such-post"]
e = not_found("12abc")
raise "prefix message #{e.message}" unless e.message == "can't find record with friendly id: \"12abc\""
e = not_found("9999")
raise "id message #{e.message}" unless e.message == "Couldn't find Article with 'id'=\"9999\""
e = not_found(9999)
raise "integer message #{e.message}" unless e.message == "Couldn't find Article with 'id'=9999"
e = not_found(nil)
raise "nil message #{e.message}" unless e.message == "Couldn't find Article without an ID"
# `friendly` mid-chain keeps the relation's conditions and finds the same record.
raise "chained" unless Article.where(title: "Alpha Post").friendly.find("old-alpha").id == alpha.id
begin
  Article.where(title: "Nope").friendly.find("alpha-post")
  raise "scope ignored"
rescue ActiveRecord::RecordNotFound
end
# friendly_id's to_param: the slug, else the id as a String.
raise "to_param #{alpha.to_param.inspect}" unless alpha.to_param == "alpha-post"
blank = Article.create!(title: "Blank Slug", body: "blank body is long enough")
raise "blank to_param #{blank.to_param.inspect}" unless blank.to_param == blank.id.to_s
puts "PASS friendly 404s"
"##,
        )
        .assert_passes();
}

/// The finder against friendly_id 5.7.0 itself: the same seed and the
/// same inputs ran through the real gem (ActiveRecord 8.1 + sqlite,
/// `Article.friendly.find`, `:slugged, :history`), and each line below is
/// what the gem printed — the record found, or the RecordNotFound message,
/// model, key and id. Rows get ids 1..3: gamma (slug "gamma-post"), beta
/// (slug "1", gamma's id), alpha (slug "alpha-post").
#[test]
fn friendly_find_matches_the_gem_on_its_oracle_inputs() {
    friendly_article_overlay("%i[slugged history]")
        .run_ruby(
            r##"
gamma = Article.create!(title: "Gamma", body: "gamma body is long enough", slug: "gamma-post")
beta = Article.create!(title: "Beta", body: "beta body is long enough here", slug: gamma.id.to_s)
alpha = Article.create!(title: "Alpha", body: "alpha body is long enough", slug: "alpha-post")
raise "ids #{[gamma.id, beta.id, alpha.id]}" unless [gamma.id, beta.id, alpha.id] == [1, 2, 3]
conn = ActiveRecord::Base.connection
[["first-name", "Article", alpha.id], ["contested", "Article", alpha.id], ["contested", "Article", beta.id],
 ["wrong-type", "Topic", alpha.id], ["beta-old", "Article", beta.id]].each do |s, t, i|
  conn.execute("INSERT INTO friendly_id_slugs (slug, sluggable_type, sluggable_id) VALUES ('#{s}', '#{t}', #{i})")
end
oracle = [
  ["alpha-post", "found Alpha"],
  ["first-name", "found Alpha"],
  ["contested", "found Beta"],
  ["wrong-type", "NOTFOUND can't find record with friendly id: \"wrong-type\" [\"Article\",\"slug\",\"wrong-type\"]"],
  ["1", "found Beta"],
  ["2", "found Beta"],
  ["3", "found Alpha"],
  ["no-such", "NOTFOUND can't find record with friendly id: \"no-such\" [\"Article\",\"slug\",\"no-such\"]"],
  ["3-alpha-post", "NOTFOUND can't find record with friendly id: \"3-alpha-post\" [\"Article\",\"slug\",\"3-alpha-post\"]"],
  ["1003", "NOTFOUND Couldn't find Article with 'id'=\"1003\" [\"Article\",\"id\",\"1003\"]"],
  ["12abc", "NOTFOUND can't find record with friendly id: \"12abc\" [\"Article\",\"slug\",\"12abc\"]"],
  [" 3 ", "found Alpha"],
  [3, "found Alpha"],
  [9999, "NOTFOUND Couldn't find Article with 'id'=9999 [\"Article\",\"id\",9999]"],
  [nil, "NOTFOUND Couldn't find Article without an ID [\"Article\",\"id\",nil]"],
  ["beta-old", "found Beta"],
]
oracle.each do |arg, want|
  got = begin
    "found #{Article.friendly.find(arg).title}"
  rescue ActiveRecord::RecordNotFound => e
    "NOTFOUND #{e.message} [#{e.model.inspect},#{e.primary_key.inspect},#{e.id.inspect}]"
  end
  raise "#{arg.inspect}: got #{got}, gem says #{want}" unless got == want
end
puts "PASS friendly oracle"
"##,
        )
        .assert_passes();
}
#[path = "emit_and_run/acts_as_votable.rs"]
mod acts_as_votable;

/// A file whose only statement is `Breadcrumb = Data.define(…)`, a
/// class nested under it with a compact name, and a constant that
/// another class builds from it. No file reopens `class Breadcrumb`,
/// so only the Data factory declares it.
fn top_level_data_factory_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write("app/models/breadcrumb.rb", "Breadcrumb = Data.define(:name, :path)\n")
        .write("app/models/breadcrumb/trail.rb", "class Breadcrumb::Trail\n  def self.label = \"trail\"\nend\n")
        .write("app/models/app_nav.rb", "class AppNav\n  HOME = Breadcrumb.new(name: \"Home\", path: \"/\")\n\n  def self.home = HOME\nend\n")
        .write("app/controllers/data_probe_controller.rb", r#"class DataProbeController < ApplicationController
  def index
    @crumb = Breadcrumb.new(name: "Posts", path: "/posts")
    @path = @crumb.path
    @trail = Breadcrumb::Trail.label
    @home = AppNav.home.name
  end
end
"#)
}

const TOP_LEVEL_DATA_FACTORY: &str = r#"
keyword = Breadcrumb.new(name: "Posts", path: "/posts")
positional = Breadcrumb.new("Posts", "/posts")
raise "readers changed" unless keyword.name == "Posts" && keyword.path == "/posts"
raise "Data lost value equality" unless keyword == positional
raise "nested class lost" unless Breadcrumb::Trail.label == "trail"
raise "class constant lost" unless AppNav.home.name == "Home" && AppNav.home.path == "/"
puts "top-level Data factory passed"
"#;

#[test]
fn a_top_level_data_factory_file_checks_cleanly_and_executes() {
    let run = top_level_data_factory_app().run_ruby(&format!(r#"{TOP_LEVEL_DATA_FACTORY}
raise "not a Data class" unless Breadcrumb.superclass == Data && Breadcrumb.name == "Breadcrumb"
raise "members changed" unless Breadcrumb.members == [:name, :path]
raise "Data lost immutability" unless Breadcrumb.new("a", "b").frozen? && !Breadcrumb.new("a", "b").respond_to?(:name=)
raise "a nested class leaked to the top level" if Object.const_defined?(:Trail, false)
begin
  Breadcrumb.new(name: "Posts")
  raise "a missing member was accepted"
rescue ArgumentError
end
"#));
    run.assert_passes();
    let emitted = std::fs::read_to_string(run.emitted.join("app/models/breadcrumb.rb")).unwrap();
    assert_eq!(emitted.matches("Data.define").count(), 1, "{emitted}");
    assert!(emitted.contains("Breadcrumb = Data.define(:name, :path)"), "{emitted}");
    let sidecar = std::fs::read_to_string(run.emitted.join("sig/app/models/breadcrumb.rbs")).unwrap();
    assert!(sidecar.contains("class Breadcrumb < ::Data"), "{sidecar}");
    let signatures = roundhouse::rbs::parse_app_signatures(&sidecar).expect("emitted RBS parses");
    let breadcrumb = &signatures[&roundhouse::ident::ClassId(roundhouse::ident::Symbol::from("Breadcrumb"))];
    for method in ["new", "name", "path"] {
        assert!(breadcrumb.contains_key(&roundhouse::ident::Symbol::from(method)), "{sidecar}");
    }
}

#[test]
fn a_class_nested_by_reopening_a_top_level_data_class_executes() {
    emit_and_run::real_blog()
        .write("app/models/breadcrumb.rb", "Breadcrumb = Data.define(:name, :path)\n")
        .write("app/models/breadcrumb/separator.rb", "class Breadcrumb\n  class Separator\n    def self.text = \" > \"\n  end\nend\n")
        .write("app/controllers/data_probe_controller.rb", r#"class DataProbeController < ApplicationController
  def index
    @crumb = Breadcrumb.new("Posts", "/posts").name
    @separator = Breadcrumb::Separator.text
  end
end
"#)
        .run_ruby(r#"
raise "Data class lost" unless Breadcrumb.new("Posts", "/posts").path == "/posts"
raise "nested class lost" unless Breadcrumb::Separator.text == " > "
raise "a nested class leaked to the top level" if Object.const_defined?(:Separator, false)
puts "reopened Data class passed"
"#)
        .assert_passes();
}

#[test]
#[ignore = "requires the native Spinel compiler"]
fn a_top_level_data_factory_file_runs_natively() {
    top_level_data_factory_app()
        .run_spinel(TOP_LEVEL_DATA_FACTORY)
        .assert_passes();
}

fn data_define_block_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write("app/models/timeframe_limits.rb", "module TimeframeLimits\n  TIMEFRAME_LIMIT = 5\nend\n")
        .write("app/models/timeframe.rb", r##"Timeframe = Data.define(:id, :label_text) do
  include TimeframeLimits

  TIMEFRAME_OPTIONS = { "this_year" => "This year", "all_time" => "All time" }.freeze
  TIMEFRAME_LIMIT = 10
  DEFAULT_TIMEFRAME = new("all_time", nil)
  @current = DEFAULT_TIMEFRAME

  def self.find(id) = new(id, TIMEFRAME_OPTIONS.fetch(id))
  def self.current = @current
  def limit = TIMEFRAME_LIMIT

  def label = "#{label_text}!"
  def label_text = TIMEFRAME_OPTIONS.fetch(id)

  private

  def secret = "hidden"
end
"##)
        .write("app/models/timeframe_report.rb", "class TimeframeReport\n  def self.option_count = TIMEFRAME_OPTIONS.size\nend\n")
        .write("app/controllers/data_probe_controller.rb", r#"class DataProbeController < ApplicationController
  def index
    @label = Timeframe.find("this_year").label
    @default = DEFAULT_TIMEFRAME.id
    @current = Timeframe.current.id
    @limit = Timeframe.find("this_year").limit
    @count = TimeframeReport.option_count
  end
end
"#)
}

const DATA_DEFINE_BLOCK: &str = r#"
raise "class method lost" unless Timeframe.find("this_year").label == "This year!"
raise "reader override lost" unless Timeframe.new("all_time", "ignored").label_text == "All time"
raise "constant built with new lost" unless DEFAULT_TIMEFRAME.is_a?(Timeframe) && DEFAULT_TIMEFRAME.id == "all_time"
raise "constant read from another file lost" unless TimeframeReport.option_count == 2
raise "class state lost its constant" unless Timeframe.current.equal?(DEFAULT_TIMEFRAME)
raise "a mixin constant shadowed the block's constant" unless Timeframe.find("this_year").limit == 10
raise "private method exposed" if Timeframe.new("all_time", nil).respond_to?(:secret)
puts "Data.define block passed"
"#;

#[test]
fn a_data_define_block_is_the_class_body_and_executes() {
    let run = data_define_block_app().run_ruby(&format!(r#"{DATA_DEFINE_BLOCK}
raise "not a Data class" unless Timeframe.superclass == Data && Timeframe.members == [:id, :label_text]
raise "block constant not on Object" unless Object.const_defined?(:TIMEFRAME_OPTIONS, false)
raise "block constant on the Data class" if Timeframe.const_defined?(:TIMEFRAME_OPTIONS, false)
"#));
    run.assert_passes();
    let sidecar = std::fs::read_to_string(run.emitted.join("sig/app/models/timeframe.rbs")).unwrap();
    let signatures = roundhouse::rbs::parse_app_signatures(&sidecar).expect("emitted RBS parses");
    let timeframe = &signatures[&roundhouse::ident::ClassId(roundhouse::ident::Symbol::from("Timeframe"))];
    for method in ["new", "id", "label_text", "find", "label"] {
        assert!(timeframe.contains_key(&roundhouse::ident::Symbol::from(method)), "{sidecar}");
    }
    assert_eq!(sidecar.matches("def label_text").count(), 1, "{sidecar}");
}

#[test]
#[ignore = "requires the native Spinel compiler"]
fn a_data_define_block_runs_natively() {
    data_define_block_app()
        .run_spinel(DATA_DEFINE_BLOCK)
        .assert_passes();
}

fn qualified_data_define_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write("app/models/article/summary.rb", r##"Article::Summary = Data.define(:article) do
  ARTICLE_SUMMARY_PREFIX = "Summary of "

  def self.for(article) = new(article)

  def text = "#{ARTICLE_SUMMARY_PREFIX}#{article.title}"
end
"##)
        .write("app/models/reports/line.rb", "Reports::Line = Data.define(:label, :amount)\n")
        .write("app/models/formatting.rb", "module Formatting\n  def format_count = \"top\"\nend\n")
        .write("app/models/reports/formatting.rb", "module Reports\n  module Formatting\n    def format_count = \"nested\"\n  end\nend\n")
        .write("app/models/reports/total.rb", "Reports::Total = Data.define(:amount) do\n  include Formatting\nend\n")
        .write("app/models/catalog.rb", "class Catalog\n  def self.size = 1\nend\n")
        .write("app/models/catalog/entry.rb", "Catalog::Entry = Data.define(:name)\n")
        .write("app/controllers/data_probe_controller.rb", r#"class DataProbeController < ApplicationController
  def index
    @summary = Article::Summary.for(Article.new(title: "Hello")).text
    @line = Reports::Line.new("articles", 2).amount
    @total = Reports::Total.new(2).format_count
    @entry = Catalog::Entry.new("first").name
  end
end
"#)
}

const QUALIFIED_DATA_DEFINE: &str = r#"
raise "a class namespace lost its Data class" unless Article::Summary.for(Article.new(title: "Hello")).text == "Summary of Hello"
raise "a module namespace lost its Data class" unless Reports::Line.new("articles", 2).amount == 2
raise "the include did not resolve from the top level" unless Reports::Total.new(2).format_count == "top"
raise "a library class namespace lost its Data class" unless Catalog::Entry.new("first").name == "first" && Catalog.size == 1
puts "qualified Data.define passed"
"#;

#[test]
fn a_qualified_data_define_file_executes_in_its_namespace() {
    qualified_data_define_app()
        .run_ruby(&format!(r#"{QUALIFIED_DATA_DEFINE}
raise "not a Data class" unless Article::Summary.superclass == Data && Reports::Line.superclass == Data
raise "the class left its namespace" if Object.const_defined?(:Summary, false) || Object.const_defined?(:Line, false)
raise "the namespace class lost its superclass" unless Article.superclass == ApplicationRecord
raise "block constant not on Object" unless Object.const_defined?(:ARTICLE_SUMMARY_PREFIX, false)
"#))
        .assert_passes();
}

#[test]
#[ignore = "requires the native Spinel compiler"]
fn a_qualified_data_define_file_runs_natively() {
    qualified_data_define_app()
        .run_spinel(QUALIFIED_DATA_DEFINE)
        .assert_passes();
}

/// A model's methods that call each other, a column and an association on implicit `self`.
fn implicit_self_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "  validates :title, presence: true\n",
            "  validates :title, presence: true\n\n  def headline\n    title.to_s.upcase\n  end\n\n  def shout\n    headline + \"!\"\n  end\n\n  def comment_count\n    comments.size\n  end\n",
        )
        .edit(
            "test/models/article_test.rb",
            "  test \"destroys comments when article is destroyed\" do\n",
            "  test \"methods read the record on implicit self\" do\n    article = articles(:one)\n    assert_equal \"GETTING STARTED WITH RAILS!\", article.shout\n    assert_equal 1, article.comment_count\n    assert_equal \"!\", Article.new.shout\n  end\n\n  test \"destroys comments when article is destroyed\" do\n",
        )
}

#[test]
fn model_methods_read_the_record_on_implicit_self() {
    implicit_self_app().run_test("test/models/article_test.rb").assert_passes();
}

/// Runs a translated test suite in an emitted tree: each command must succeed.
fn run_emitted(emitted: &std::path::Path, commands: &[(&str, &[&str])]) {
    for (program, args) in commands {
        let output = std::process::Command::new(program)
            .args(*args)
            .env_remove("CARGO_TARGET_DIR")
            .current_dir(emitted)
            .output()
            .unwrap_or_else(|e| panic!("run {program}: {e}"));
        assert!(output.status.success(), "{program} {args:?} in {}\n{}\n{}", emitted.display(),
            String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    }
}

/// Rust rendered each bare call as a free function (`title()`), and the methods without `&self`.
#[test]
#[ignore = "requires the Rust toolchain; builds and runs an emitted app"]
fn model_methods_read_the_record_on_implicit_self_on_rust() {
    let (emitted, errors) = implicit_self_app().emit(roundhouse::project::BuildTarget::Rust);
    assert!(errors.is_empty(), "{errors:?}");
    run_emitted(&emitted, &[("cargo", &["test", "--quiet"])]);
}

/// TypeScript read `headline` as a property (`this.headline + "!"`).
#[test]
#[ignore = "requires Node; installs and runs an emitted app"]
fn model_methods_read_the_record_on_implicit_self_on_typescript() {
    let (emitted, errors) = implicit_self_app().emit(roundhouse::project::BuildTarget::Typescript);
    assert!(errors.is_empty(), "{errors:?}");
    run_emitted(&emitted, &[("npm", &["install", "--silent", "--no-audit", "--no-fund"]), ("npm", &["test"])]);
}

/// Go formatted the `*string` column with `%v`, the pointer's address.
#[test]
#[ignore = "requires Go; builds and runs an emitted app"]
fn model_methods_read_the_record_on_implicit_self_on_go() {
    let (emitted, errors) = implicit_self_app().emit(roundhouse::project::BuildTarget::Go);
    assert!(errors.is_empty(), "{errors:?}");
    run_emitted(&emitted, &[("go", &["mod", "tidy"]), ("go", &["test", "./..."])]);
}

/// A controller of its own, outside real-blog's resources, reading `params` into a local.
fn probe_controller_app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit("config/routes.rb", "  root \"articles#index\"\n", "  root \"articles#index\"\n  get \"/probe/:id\", to: \"probe#show\", as: :probe\n  get \"/probe/:id/joined\", to: \"probe#joined\", as: :probe_joined\n  get \"/search\", to: \"probe#search\", as: :search\n  post \"/search\", to: \"probe#search\"\n  delete \"/search\", to: \"probe#search\"\n")
        .write(
            "app/controllers/probe_controller.rb",
            "class ProbeController < ApplicationController\n  def show\n    article = Article.find(params[:id])\n    render plain: article.title.to_s\n  end\n\n  def search\n    render plain: \"q=\" + params[:q].to_s + \" page=\" + params[:page].to_s\n  end\n\n  def joined\n    article = Article.find(params[:id])\n    render plain: article.comments.map(&:commenter).join(\"|\") + \" \" + [article.comments.size, 0].join(\",\") + \" \" + [true, false].join(\",\")\n  end\nend\n",
        )
        .write(
            "test/controllers/probe_controller_test.rb",
            "require \"test_helper\"\n\nclass ProbeControllerTest < ActionDispatch::IntegrationTest\n  setup do\n    @article = articles(:one)\n  end\n\n  test \"reads params into a local\" do\n    get probe_url(@article)\n    assert_response :success\n    assert_equal \"Getting Started with Rails\", response.body\n  end\n\n  test \"joins what is not a String\" do\n    get probe_joined_url(@article)\n    assert_equal \"Alice 1,0 true,false\", response.body\n  end\n\n  test \"reads the query string\" do\n    get search_url(q: \"rails\", page: 2)\n    assert_equal \"q=rails page=2\", response.body\n  end\n\n  test \"leaves an omitted key out of the query string\" do\n    get search_url(q: \"rails\")\n    assert_equal \"q=rails page=\", response.body\n  end\n\n  test \"lets the query string win over the form\" do\n    post search_url(q: \"query\"), params: { q: \"form\", page: \"3\" }\n    assert_equal \"q=query page=3\", response.body\n  end\n\n  test \"reads a bodyless delete\" do\n    delete search_url(q: \"gone\")\n    assert_equal \"q=gone page=\", response.body\n  end\nend\n",
        )
}

#[test]
fn a_controller_reads_params_into_a_local() {
    probe_controller_app().run_test("test/controllers/probe_controller_test.rb").assert_passes();
}

/// Rust gave the controller no `params` field (only ivar writes and a few node shapes were walked), and its `process_action` on `&self` called a `&mut self` stub.
#[test]
#[ignore = "requires the Rust toolchain; builds and runs an emitted app"]
fn a_controller_reads_params_into_a_local_on_rust() {
    let (emitted, errors) = probe_controller_app().emit(roundhouse::project::BuildTarget::Rust);
    assert!(errors.is_empty(), "{errors:?}");
    run_emitted(&emitted, &[("cargo", &["test", "--quiet"])]);
}

/// A controller test whose setup names a fixture other than `one`.
fn setup_fixture_label_app() -> emit_and_run::Overlay {
    // In `ArticlesControllerTest`: a test class named for no controller is not emitted.
    emit_and_run::real_blog().write(
        "test/controllers/articles_controller_test.rb",
        "require \"test_helper\"\n\nclass ArticlesControllerTest < ActionDispatch::IntegrationTest\n  setup do\n    @article = articles(:two)\n  end\n\n  test \"shows the fixture the setup names\" do\n    get article_url(@article)\n    assert_response :success\n    assert_select \"h1\", \"Understanding MVC Architecture\"\n  end\nend\n",
    )
}

#[test]
fn a_controller_test_reads_the_fixture_its_setup_names() {
    setup_fixture_label_app().run_test("test/controllers/articles_controller_test.rb").assert_passes();
}

/// Rust primed every such ivar with the `one` fixture, whatever the setup named.
#[test]
#[ignore = "requires the Rust toolchain; builds and runs an emitted app"]
fn a_controller_test_reads_the_fixture_its_setup_names_on_rust() {
    let (emitted, errors) = setup_fixture_label_app().emit(roundhouse::project::BuildTarget::Rust);
    assert!(errors.is_empty(), "{errors:?}");
    run_emitted(&emitted, &[("cargo", &["test", "--quiet"])]);
}

/// The CRuby tree's `rake test` load path, as its Rakefile spells it, answers a gem's `require "logger"` with Ruby's
/// own Logger (ruby-vips calls `Logger.new($stdout)`), and only an app with the blog's Article gets the scaffold's Article tests.
#[test]
fn cruby_rake_load_path_keeps_rubys_own_libraries() {
    use roundhouse::project::BuildTarget;
    let (emitted, errors) = emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n")
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n")
        .emit(BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:?}");
    for scaffold in [
        "test/models/article_broadcasts_test.rb",
        "test/views/articles_test.rb",
        "test/views/articles_json_test.rb",
        "test/query_count_test.rb",
        "test/relation_find_test.rb",
        "test/route_helpers_test.rb",
    ] {
        assert!(!emitted.join(scaffold).exists(), "{scaffold}");
    }
    let rakefile = std::fs::read_to_string(emitted.join("Rakefile")).unwrap();
    let libs: Vec<String> = rakefile
        .lines()
        .filter_map(|line| Some(format!("-I{}", line.trim().strip_prefix("t.libs << \"")?.strip_suffix('"')?)))
        .collect();
    assert!(!libs.is_empty(), "{rakefile}");
    let output = emit_and_run::ruby()
        .args(&libs)
        .args(["-e", "require \"logger\"; Logger.new($stdout).level = Logger::WARN"])
        .current_dir(&*emitted)
        .output()
        .unwrap();
    assert!(output.status.success(), "{libs:?}\n{}", String::from_utf8_lossy(&output.stderr));

    let (emitted, _) = emit_and_run::real_blog().emit(BuildTarget::Ruby);
    assert!(emitted.join("test/models/article_broadcasts_test.rb").exists());
}

/// campfire's `MessagesHelper#auto_link_text`: `sanitize` with the
/// caller's lists, `Nokogiri::HTML5.fragment`, its text nodes by XPath,
/// `replace` with a parsed fragment, and `to_html(preserve_newline:)`.
/// The Ruby tree loads the real nokogiri gem behind the façade's types.
#[test]
fn nokogiri_html5_fragment_text_nodes_are_rewritten_and_serialized() {
    let run = emit_and_run::real_blog()
        .write(
            "app/helpers/articles_helper.rb",
            r#"module ArticlesHelper
  LINK_TAGS = %w[a div pre]
  LINK_ATTRIBUTES = %w[href]

  def link_words(html)
    fragment = Nokogiri::HTML5.fragment(sanitize(html, tags: LINK_TAGS, attributes: LINK_ATTRIBUTES))
    fragment.xpath(".//text()[not(ancestor::a)]").each do |node|
      if node.text.match?(/go/)
        text = node.to_html
        linked = text.sub("go", "<a href=\"/go\">go</a>")
        node.replace Nokogiri::HTML5.fragment(linked) unless linked == text
      end
    end
    fragment.to_html(preserve_newline: true)
  end
end
"#,
        )
        .run_ruby(
            r#"out = ArticlesHelper.link_words("<div onclick=\"x()\">go <em>on</em> <a href=\"/k\">go</a></div><pre>\n\n\ngo</pre>")
want = "<div><a href=\"/go\">go</a> on <a href=\"/k\">go</a></div><pre>\n\n<a href=\"/go\">go</a></pre>"
raise "link_words: #{out.inspect}" unless out == want
puts "nokogiri html5 helper passed"
"#,
        );
    run.assert_passes();
    assert!(run.stdout.contains("nokogiri html5 helper passed"));
}
#[path = "emit_and_run/proc_new_and_std_streams.rs"]
mod proc_new_and_std_streams;
#[path = "emit_and_run/core_constants.rs"]
mod core_constants;
#[path = "emit_and_run/module_protocol_receivers.rs"]
mod module_protocol_receivers;

#[path = "emit_and_run/module_protocol_block_self.rs"]
mod module_protocol_block_self;
#[path = "emit_and_run/ipaddr_include.rs"]
mod ipaddr_include;
#[path = "emit_and_run/hash_subclass_include.rs"]
mod hash_subclass_include;
#[path = "emit_and_run/module_query_class_unions.rs"]
mod module_query_class_unions;
#[path = "emit_and_run/guarded_declarations.rs"]
mod guarded_declarations;
#[path = "emit_and_run/lock_versions.rs"]
mod lock_versions;
#[path = "emit_and_run/helper_diagnostics.rs"]
mod helper_diagnostics;
#[path = "emit_and_run/value_seq_order.rs"]
mod value_seq_order;
#[path = "emit_and_run/typescript_error_class.rs"]
mod typescript_error_class;
#[path = "emit_and_run/class_macros.rs"]
mod class_macros;
#[path = "emit_and_run/accessor_override.rs"]
mod accessor_override;
#[path = "emit_and_run/reset_counters.rs"]
mod reset_counters;
#[path = "emit_and_run/console_only_definitions.rs"]
mod console_only_definitions;
#[path = "emit_and_run/open_classes.rs"]
mod open_classes;
#[path = "emit_and_run/parameterized_mailer.rs"]
mod parameterized_mailer;
#[path = "emit_and_run/model_delegate_to_reader.rs"]
mod model_delegate_to_reader;
#[path = "emit_and_run/private_calls.rs"]
mod private_calls;
#[path = "emit_and_run/inherited_visibility.rs"]
mod inherited_visibility;
#[path = "emit_and_run/class_side_relations.rs"]
mod class_side_relations;
#[path = "emit_and_run/dynamic_where_forms.rs"]
mod dynamic_where_forms;
#[path = "emit_and_run/enum_cross_class_mapping.rs"]
mod enum_cross_class_mapping;
#[path = "emit_and_run/jbuilder_record_values.rs"]
mod jbuilder_record_values;
#[path = "emit_and_run/nil_operand.rs"]
mod nil_operand;
#[path = "emit_and_run/descendant_ivar.rs"]
mod descendant_ivar;
#[path = "emit_and_run/initializer_top_level_def.rs"]
mod initializer_top_level_def;
#[path = "emit_and_run/unknown_gem_namespace.rs"]
mod unknown_gem_namespace;
#[path = "emit_and_run/record_rebinding.rs"]
mod record_rebinding;
#[path = "emit_and_run/eager_load_filter.rs"]
mod eager_load_filter;
#[path = "emit_and_run/url_helpers_include.rs"]
mod url_helpers_include;
#[path = "emit_and_run/length_is_in_within.rs"]
mod length_is_in_within;
#[path = "emit_and_run/bigdecimal_clamp.rs"]
mod bigdecimal_clamp;
#[path = "emit_and_run/partial_binop.rs"]
mod partial_binop;
#[path = "emit_and_run/union_block_params.rs"]
mod union_block_params;
#[path = "emit_and_run/format_once.rs"]
mod format_once;
#[path = "emit_and_run/condition_lambda_record.rs"]
mod condition_lambda_record;
#[path = "emit_and_run/where_time_range.rs"]
mod where_time_range;
#[path = "emit_and_run/route_record_to_param.rs"]
mod route_record_to_param;
#[path = "emit_and_run/module_factory.rs"]
mod module_factory;
#[path = "emit_and_run/spinel_unknown_gem.rs"]
mod spinel_unknown_gem;
#[path = "emit_and_run/json_column_name.rs"]
mod json_column_name;
#[path = "emit_and_run/block_locals.rs"]
mod block_locals;

/// `ActiveSupport::JSON.encode(value, escape: false)` — Rails' document
/// without the HTML-entity escaping, which campfire's fragment keys can
/// digest instead of the escaped one — lowers to the runtime's
/// `encode_without_escape`, keyword or braced, and plain `encode` keeps
/// its escaping.
#[test]
fn json_encode_with_escape_false_runs() {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n  has_many :comments, dependent: :destroy",
            r#"class Article < ApplicationRecord
  has_many :comments, dependent: :destroy

  def plain_doc
    ActiveSupport::JSON.encode([ "<b>&</b>", { title: title, n: 2 } ], escape: false)
  end

  def braced_doc
    ActiveSupport::JSON.encode([ "<i>" ], { escape: false })
  end

  def escaped_doc
    ActiveSupport::JSON.encode([ "<b>&</b>" ])
  end"#,
        )
        .write(
            "test/models/article_json_escape_test.rb",
            r#"require "test_helper"

class ArticleJsonEscapeTest < ActiveSupport::TestCase
  test "escape: false leaves the entities alone" do
    article = Article.new(title: "A <title> & more", body: "A sufficiently long article body.")
    assert_equal %q(["<b>&</b>",{"title":"A <title> & more","n":2}]), article.plain_doc
    assert_equal %q(["<i>"]), article.braced_doc
    assert_equal %(["\\u003cb\\u003e\\u0026\\u003c/b\\u003e"]), article.escaped_doc
  end
end
"#,
        )
        .run_test("test/models/article_json_escape_test.rb")
        .assert_passes();
}

#[path = "emit_and_run/fresh_builtin_extend.rs"]
mod fresh_builtin_extend;

#[path = "emit_and_run/module_function_includer.rs"]
mod module_function_includer;

#[path = "emit_and_run/unknown_return.rs"]
mod unknown_return;

#[path = "emit_and_run/hash_ivar_symbol_keys.rs"]
mod hash_ivar_symbol_keys;

#[path = "emit_and_run/date_time_package.rs"]
mod date_time_package;

#[path = "emit_and_run/date_error.rs"]
mod date_error;

#[path = "emit_and_run/data_point_shapes.rs"]
mod data_point_shapes;

#[path = "emit_and_run/bang_relation_builders.rs"]
mod bang_relation_builders;

#[path = "emit_and_run/helper_param_after_views.rs"]
mod helper_param_after_views;

#[path = "emit_and_run/layout_ivar_unset.rs"]
mod layout_ivar_unset;

#[path = "emit_and_run/stub_facade_source_gem.rs"]
mod stub_facade_source_gem;

#[path = "emit_and_run/helper_render_partial.rs"]
mod helper_render_partial;

#[path = "emit_and_run/where_record_seed_redirect_back.rs"]
mod where_record_seed_redirect_back;

#[path = "emit_and_run/joins_several_associations.rs"]
mod joins_several_associations;

#[path = "emit_and_run/chained_find_by_association_key.rs"]
mod chained_find_by_association_key;

#[path = "emit_and_run/bare_save_without_validation.rs"]
mod bare_save_without_validation;

#[path = "emit_and_run/route_param_recall.rs"]
mod route_param_recall;

#[path = "emit_and_run/reopened_class_constructor.rs"]
mod reopened_class_constructor;

#[path = "emit_and_run/flash_any_key.rs"]
mod flash_any_key;

#[path = "emit_and_run/select_alias_attributes.rs"]
mod select_alias_attributes;

#[path = "emit_and_run/route_arg_column_named_like_association.rs"]
mod route_arg_column_named_like_association;

#[path = "emit_and_run/params_merged_key_type.rs"]
mod params_merged_key_type;

#[path = "emit_and_run/view_nil_seeded_loop_local.rs"]
mod view_nil_seeded_loop_local;

#[path = "emit_and_run/polymorphic_source_through.rs"]
mod polymorphic_source_through;

#[path = "emit_and_run/namespaced_controller_symbol.rs"]
mod namespaced_controller_symbol;

#[path = "emit_and_run/route_format_lookahead.rs"]
mod route_format_lookahead;

#[path = "emit_and_run/route_dot_span.rs"]
mod route_dot_span;

#[path = "emit_and_run/params_write_assign.rs"]
mod params_write_assign;

#[path = "emit_and_run/find_each_mailer.rs"]
mod find_each_mailer;

#[path = "emit_and_run/model_class_attribute.rs"]
mod model_class_attribute;
#[path = "support/record_slice_app.rs"]
mod record_slice_app;
#[path = "emit_and_run/record_slice.rs"]
mod record_slice;

#[path = "emit_and_run/csrf_meta_tag_alias.rs"]
mod csrf_meta_tag_alias;
