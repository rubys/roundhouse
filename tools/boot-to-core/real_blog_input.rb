# frozen_string_literal: true

# Deliberately try the real application, not a reduced model/callback stand-in.
# CORE_RUBY_APP must point at a disposable copy, with its bundle installed.
app = File.realpath(ENV.fetch("CORE_RUBY_APP"))
require File.join(app, "config/application")
BootToCore.capture do
  Rails.application.initialize!
  Rails.application.eager_load!
  Article.define_attribute_methods
  Comment.define_attribute_methods
end
BootToCore.input(roots: {
  Article => %i[title title= body body= comments valid? save destroy],
  Comment => %i[commenter commenter= body body= article valid? save destroy],
  ArticlesController => ArticlesController.action_methods.sort.map(&:to_sym),
  CommentsController => CommentsController.action_methods.sort.map(&:to_sym)
})
