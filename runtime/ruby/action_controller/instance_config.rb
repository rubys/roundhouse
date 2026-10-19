# frozen_string_literal: true

# `config.relative_url_root` inside a controller instance: the object
# the class reader returns (`controller_config.rb`). Its own file
# because the typed-runtime gate keys signatures by method name, so the
# class and instance `config` cannot share one.
module ActionController
  class Base
    def config
      ActionController.controller_config
    end
  end
end
