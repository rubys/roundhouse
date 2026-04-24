require "minitest/autorun"
require_relative "../active_record"

# Load the blog demo's actual models against our framework Ruby.
# This is the real test: can the untouched blog models run under our
# runtime?
REAL_BLOG_ROOT = File.expand_path("../../../fixtures/real-blog", __dir__)
TRANSPILED_BLOG_ROOT = File.expand_path("fixtures/transpiled_blog", __dir__)
BLOG_MODELS = "#{TRANSPILED_BLOG_ROOT}/app/models"
BLOG_MIGRATIONS = "#{REAL_BLOG_ROOT}/db/migrate"

module BlogSchema
  def self.load!
    ActiveRecord.reset_adapter
    ActiveRecord::Broadcasts.reset_log

    # Run migrations via the actual migration files.
    Dir["#{BLOG_MIGRATIONS}/*.rb"].sort.each do |f|
      # Each migration file defines a class like CreateArticles.
      load f
    end

    # Instantiate each migration class and run it.
    ObjectSpace.each_object(Class).select { |c| c < ActiveRecord::Migration }.each do |mig|
      mig.new.migrate
    end

    # Load model files.
    load "#{BLOG_MODELS}/application_record.rb"
    Dir["#{BLOG_MODELS}/*.rb"].sort.each do |f|
      next if File.basename(f) == "application_record.rb"
      load f
    end

  end
end

module BlogFixtures
  def articles(name)
    @_fixtures ||= build_fixtures
    @_fixtures[:articles][name]
  end

  def comments(name)
    @_fixtures ||= build_fixtures
    @_fixtures[:comments][name]
  end

  private

  def build_fixtures
    a1 = Article.create(title: "Getting Started with Rails", body: "A comprehensive guide to Rails.")
    a2 = Article.create(title: "Advanced Ruby Patterns", body: "Deep dive into Ruby idioms and patterns.")
    c1 = Comment.create(article_id: a1.id, commenter: "Alice", body: "Great article, thanks!")
    c2 = Comment.create(article_id: a2.id, commenter: "Bob", body: "Very informative post.")
    {
      articles: { one: a1, two: a2 },
      comments: { one: c1, two: c2 }
    }
  end
end

class Minitest::Test
  include BlogFixtures

  def setup
    BlogSchema.load!
    @_fixtures = nil
  end
end
