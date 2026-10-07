//! A Concern's `class_attribute`, written by its class-body macros at
//! class load (`ingest::class_attribute`). One contract for the
//! interpreted and native lanes: the macro runs on the includer, a
//! subclass that never writes inherits the parent's value, a subclass
//! that writes appends to it without touching the parent.
//!
//! The concern is the write side of a real application's preload
//! concern, private helper and `binding.local_variable_get(:if)` included.

pub const CONCERN: &str = r#"
module PreloadableConfigurationConcern
  extend ActiveSupport::Concern

  included do
    class_attribute :_preload_definitions, default: []
  end

  class_methods do
    def preload_feature_flags(codes, **options)
      add_preload_definition(
        kind: :feature_flags,
        keys: codes,
        **options
      )
    end

    def preload_site_configs(codes, **options)
      add_preload_definition(
        kind: :site_configs,
        keys: codes,
        **options
      )
    end

    def clear_preload_definitions
      self._preload_definitions = []
    end

    private

    def add_preload_definition(kind:, keys:, only: nil, if: nil)
      keys = Array(keys).map(&:to_s)

      only_actions =
        (Array(only).map(&:to_s) if only)

      self._preload_definitions += [
        {
          if: binding.local_variable_get(:if),
          keys:,
          kind:,
          only: only_actions
        }
      ]
    end
  end
end
"#;

pub fn overlay() -> super::emit_and_run::Overlay {
    super::emit_and_run::real_blog()
        .write("app/controllers/concerns/preloadable_configuration_concern.rb", CONCERN)
        .write(
            "app/controllers/probe_controller.rb",
            r#"
class ProbeController < ApplicationController
  include PreloadableConfigurationConcern
  preload_site_configs %w[a b], only: :show
  preload_feature_flags %w[f], only: %i[index show]

  def show
    render plain: self.class._preload_definitions.length.to_s
  end
end
"#,
        )
        .write("app/controllers/inherit_controller.rb", "class InheritController < ProbeController
end
")
        .write(
            "app/controllers/own_controller.rb",
            "class OwnController < ProbeController
  preload_site_configs %w[c]
end
",
        )
        .write(
            "app/controllers/cleared_controller.rb",
            "class ClearedController < ProbeController
  clear_preload_definitions
end
",
        )
}

// The reads go straight off the class. Spinel refuses `==` on a local
// holding `ProbeController._preload_definitions` once a subclass reader
// and an empty-literal write both exist, though the program is typed
// (matz/spinel#7602).
pub const ASSERTIONS: &str = r#"
require_relative "app/controllers/probe_controller"
require_relative "app/controllers/inherit_controller"
require_relative "app/controllers/own_controller"
require_relative "app/controllers/cleared_controller"
raise "parent count" unless ProbeController._preload_definitions.length == 2
raise "keys" unless ProbeController._preload_definitions[0][:keys] == ["a", "b"]
raise "kind" unless ProbeController._preload_definitions[0][:kind] == :site_configs
raise "only" unless ProbeController._preload_definitions[0][:only] == ["show"]
raise "only list" unless ProbeController._preload_definitions[1][:only] == ["index", "show"]
raise "if" unless ProbeController._preload_definitions[0][:if].nil?
raise "inherit" unless InheritController._preload_definitions.length == 2
raise "own" unless OwnController._preload_definitions.length == 3
raise "own appends" unless OwnController._preload_definitions[2][:keys] == ["c"]
raise "own only" unless OwnController._preload_definitions[2][:only].nil?
raise "cleared" unless ClearedController._preload_definitions.length == 0
raise "parent untouched" unless ProbeController._preload_definitions.length == 2
puts "class_attribute contract passed"
"#;

/// A subclass that sets the attribute to nil reads nil, as in Rails; one
/// that never set it reads its parent's.
pub fn nil_overlay() -> super::emit_and_run::Overlay {
    super::emit_and_run::real_blog()
        .write(
            "app/controllers/concerns/configurable_defs.rb",
            r#"
module ConfigurableDefs
  extend ActiveSupport::Concern

  included do
    class_attribute :defs, default: ["a"]
  end

  class_methods do
    def configure_defs(value)
      self.defs = value
    end
  end
end
"#,
        )
        .write(
            "app/controllers/defs_controller.rb",
            "class DefsController < ApplicationController\n  include ConfigurableDefs\nend\n",
        )
        .write("app/controllers/unset_defs_controller.rb", "class UnsetDefsController < DefsController\nend\n")
        .write(
            "app/controllers/nil_defs_controller.rb",
            "class NilDefsController < DefsController\n  configure_defs nil\nend\n",
        )
}

pub const NIL_ASSERTIONS: &str = r#"
require_relative "app/controllers/defs_controller"
require_relative "app/controllers/unset_defs_controller"
require_relative "app/controllers/nil_defs_controller"
raise "parent" unless DefsController.defs == ["a"]
raise "unset inherits" unless UnsetDefsController.defs == ["a"]
raise "explicit nil" unless NilDefsController.defs.nil?
puts "class_attribute nil contract passed"
"#;
