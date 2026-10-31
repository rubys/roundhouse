//! An app controller whose ivars share names with the runtime
//! `ActionController::Base`'s own state: `@status` set by the
//! controller's `before_action`, `@location` by ApplicationController's,
//! `@headers` by an included concern, all rendered by the view (#1614).
//! `@is_performed` is named like the runtime's `performed?` as the
//! TypeScript emit spells it (`is_performed()`).

use super::emit_and_run::{self, Overlay};

pub const APPLICATION_CONTROLLER: &str = "class ApplicationController < ActionController::Base
  before_action :set_location

  private

  def set_location
    @location = \"app location\"
  end
end
";

pub const HEADED: &str = "module Headed
  extend ActiveSupport::Concern

  included do
    before_action :set_headers
  end

  private

  def set_headers
    @headers = \"app headers\"
  end
end
";

pub const PROBES_CONTROLLER: &str = "class ProbesController < ApplicationController
  include Headed
  before_action :set_status

  def show
  end

  private

  def set_status
    @status = \"app status\"
    @is_performed = \"app flag\"
  end
end
";

pub const SHOW: &str = "<p><%= @status %></p>\n<p><%= @location %></p>\n<p><%= @headers %></p>\n<%= render \"label\", text: @status %>\n";
pub const LABEL: &str = "<span><%= text %></span>\n";

/// The show page's body under Rails 8.1.4 (actionpack 8.1.4): each app
/// value, the partial's too, with the response itself untouched (200,
/// no Location).
#[allow(dead_code)]
pub const BODY: &str =
    "<p>app status</p>\n<p>app location</p>\n<p>app headers</p>\n<span>app status</span>\n\n";

pub fn overlay() -> Overlay {
    emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema.define do\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\n  get \"/show\", to: \"probes#show\"\nend\n")
        .write("app/controllers/application_controller.rb", APPLICATION_CONTROLLER)
        .write("app/controllers/concerns/headed.rb", HEADED)
        .write("app/controllers/probes_controller.rb", PROBES_CONTROLLER)
        .write("app/views/probes/show.html.erb", SHOW)
        .write("app/views/probes/_label.html.erb", LABEL)
}
