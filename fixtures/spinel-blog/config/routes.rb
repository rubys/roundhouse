require "controllers/application_controller"
require "controllers/articles_controller"
require "controllers/comments_controller"

# Flat route table. In real-blog this is generated from
# `config.routes.draw { resources :articles do resources :comments }`;
# in spinel-blog it's hand-written and the eventual transpiler
# reproduces it from the DSL.
#
# Order matters when patterns overlap: `/articles/new` is listed
# before `/articles/:id` so the literal-segment match wins (matching
# Rails's matching semantics).
module Routes
  TABLE = [
    { method: "GET",    pattern: "/articles",                            controller: ArticlesController, action: :index   },
    { method: "GET",    pattern: "/articles/new",                        controller: ArticlesController, action: :new     },
    { method: "GET",    pattern: "/articles/:id",                        controller: ArticlesController, action: :show    },
    { method: "GET",    pattern: "/articles/:id/edit",                   controller: ArticlesController, action: :edit    },
    { method: "POST",   pattern: "/articles",                            controller: ArticlesController, action: :create  },
    { method: "PATCH",  pattern: "/articles/:id",                        controller: ArticlesController, action: :update  },
    { method: "PUT",    pattern: "/articles/:id",                        controller: ArticlesController, action: :update  },
    { method: "DELETE", pattern: "/articles/:id",                        controller: ArticlesController, action: :destroy },
    { method: "POST",   pattern: "/articles/:article_id/comments",       controller: CommentsController, action: :create  },
    { method: "DELETE", pattern: "/articles/:article_id/comments/:id",   controller: CommentsController, action: :destroy },
  ].freeze

  ROOT = { method: "GET", pattern: "/", controller: ArticlesController, action: :index }.freeze
end
