require_relative "../app/controllers/application_controller"
require_relative "../app/controllers/articles_controller"
require_relative "../app/controllers/comments_controller"

# Flat route table. In real-blog this is generated from
# `config.routes.draw { resources :articles do resources :comments }`;
# in spinel-blog it's hand-written and the eventual transpiler
# reproduces it from the DSL.
#
# Order matters when patterns overlap: `/articles/new` is listed
# before `/articles/:id` so the literal-segment match wins (matching
# Rails's matching semantics).
module Routes
  # The :controller field holds a symbol naming the controller, not a
  # class reference. Spinel's hash specializations only handle scalars
  # (Integer/String/Symbol) and tagged unions; class objects as
  # first-class hash values are not supported. The router returns the
  # symbol; main.rb's `instantiate_controller` case-dispatches it to
  # the literal `.new` call.
  TABLE = [
    { method: "GET",    pattern: "/articles",                            controller: :articles, action: :index   },
    { method: "GET",    pattern: "/articles/new",                        controller: :articles, action: :new     },
    { method: "GET",    pattern: "/articles/:id",                        controller: :articles, action: :show    },
    { method: "GET",    pattern: "/articles/:id/edit",                   controller: :articles, action: :edit    },
    { method: "POST",   pattern: "/articles",                            controller: :articles, action: :create  },
    { method: "PATCH",  pattern: "/articles/:id",                        controller: :articles, action: :update  },
    { method: "PUT",    pattern: "/articles/:id",                        controller: :articles, action: :update  },
    { method: "DELETE", pattern: "/articles/:id",                        controller: :articles, action: :destroy },
    { method: "POST",   pattern: "/articles/:article_id/comments",       controller: :comments, action: :create  },
    { method: "DELETE", pattern: "/articles/:article_id/comments/:id",   controller: :comments, action: :destroy },
  ].freeze

  ROOT = { method: "GET", pattern: "/", controller: :articles, action: :index }.freeze
end
