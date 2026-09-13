//! A template an inherited action renders that only the INHERITOR's
//! views hold, and the route scope that puts such a request on the
//! json branch in the first place.
//!
//! campfire's `MessagesController#update` ends `format.json { render
//! :show }`; `messages/` holds no `show.json.jbuilder`, `messages/by_bots/`
//! does, and the bot API is `scope defaults: { format: :json }`. Rails
//! resolves the template per instance through the subclass's view
//! prefixes and reads the format off the scope. The emit answers the
//! first with a virtual hook — `self.__template_show_json`, the raise on
//! the defining controller, the `Views::…` call on the inheritor — and
//! the second by pinning `:json` on every route row under the scope.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

fn emitted() -> Vec<(String, String)> {
    let files: HashMap<PathBuf, Vec<u8>> = [
        (
            PathBuf::from("db/schema.rb"),
            b"ActiveRecord::Schema.define do\n  create_table \"messages\", force: :cascade do |t|\n    t.string \"body\", null: false\n  end\nend\n".to_vec(),
        ),
        (
            PathBuf::from("config/routes.rb"),
            b"Rails.application.routes.draw do\n  resources :messages, only: %i[ update ]\n  scope path: \":bot_key\", as: :bot, defaults: { format: :json } do\n    resources :messages, controller: \"messages/by_bots\", only: %i[ update ]\n  end\nend\n".to_vec(),
        ),
        (
            PathBuf::from("app/models/message.rb"),
            b"class Message < ApplicationRecord\nend\n".to_vec(),
        ),
        (
            PathBuf::from("app/controllers/messages_controller.rb"),
            b"class MessagesController < ApplicationController\n  before_action :set_message\n\n  def update\n    @message.update!(body: params[:body])\n\n    respond_to do |format|\n      format.html { redirect_to message_url(@message) }\n      format.json { render :show }\n    end\n  end\n\n  private\n    def set_message\n      @message = Message.find(params[:id])\n    end\nend\n".to_vec(),
        ),
        (
            PathBuf::from("app/controllers/messages/by_bots_controller.rb"),
            b"class Messages::ByBotsController < MessagesController\nend\n".to_vec(),
        ),
        (
            PathBuf::from("app/views/messages/by_bots/show.json.jbuilder"),
            b"json.id @message.id\njson.body @message.body\n".to_vec(),
        ),
    ]
    .into_iter()
    .collect();
    let mut app = ingest_app_from_tree(files).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    ruby::emit_spinel(&app)
        .iter()
        .map(|f| (f.path.to_string_lossy().to_string(), f.content.clone()))
        .collect()
}

fn file<'a>(files: &'a [(String, String)], suffix: &str) -> &'a str {
    &files
        .iter()
        .find(|(p, _)| p.ends_with(suffix))
        .unwrap_or_else(|| panic!("no {suffix} emitted"))
        .1
}

#[test]
fn the_defining_controller_renders_through_a_hook_that_raises() {
    let files = emitted();
    let parent = file(&files, "app/controllers/messages_controller.rb");
    assert!(
        parent.contains("render(self.__template_show_json, content_type: \"application/json\")"),
        "the json branch should render through the hook:\n{parent}"
    );
    assert!(
        parent.contains("def __template_show_json\n    raise ActionView::MissingTemplate.new(\"show\")"),
        "the defining controller's hook is Rails' raise:\n{parent}"
    );
}

#[test]
fn the_inheritor_overrides_the_hook_with_its_own_view() {
    let files = emitted();
    let child = file(&files, "app/controllers/messages/by_bots_controller.rb");
    assert!(
        child.contains("def __template_show_json\n      Views::Messages::ByBots.show_json(@message)"),
        "the inheritor's override is its own Views call:\n{child}"
    );
    let rbs = file(&files, "app/controllers/messages/by_bots_controller.rbs");
    assert!(rbs.contains("def __template_show_json: () -> String"), "{rbs}");
}

#[test]
fn a_scope_default_format_pins_the_format_on_its_routes() {
    let files = emitted();
    let routes = file(&files, "config/routes.rb");
    assert!(
        routes.contains("Route.new(\"PATCH\", \"/:bot_key/messages/:id\", :messages_by_bots, :update, :json)"),
        "the scoped route should carry :json:\n{routes}"
    );
    assert!(
        routes.contains("Route.new(\"PATCH\", \"/messages/:id\", :messages, :update)"),
        "the unscoped route stays format-free:\n{routes}"
    );
}
