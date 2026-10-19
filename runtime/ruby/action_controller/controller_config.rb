# frozen_string_literal: true

# `ApplicationController.config` — Rails' class_attribute reader.
# Assignments land on the process-wide `ControllerConfig` that
# `Base#config` also returns, so `config.relative_url_root =` on the
# class is what an instance method reads back.
#
# Ruby family only, like the reopens beside it: base.rb transpiles to
# every target, and a constant whose initializer constructs an object
# has no strict-target form (TypeScript and Python run it before the
# class exists; Rust, Go and C# have no typed static for it).
module ActionController
  # One object for the process: `Rails.application` is not the
  # controller, and a test assigns the class reader then an instance
  # reads it back.
  class ControllerConfig
    def initialize
      @relative_url_root = +""
    end

    def relative_url_root
      @relative_url_root
    end

    def relative_url_root=(value)
      @relative_url_root = value
      @relative_url_root
    end
  end

  CONTROLLER_CONFIG_SLOT = [ControllerConfig.new]

  def self.controller_config
    CONTROLLER_CONFIG_SLOT[0]
  end

  class Base
    def self.config
      ActionController.controller_config
    end
  end
end
