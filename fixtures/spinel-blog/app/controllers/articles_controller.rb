require_relative "application_controller"
require_relative "../models/article"
require_relative "../views"

# Lowered shape of fixtures/real-blog/app/controllers/articles_controller.rb.
#
# Real-blog uses `before_action :set_article, only: %i[show edit update destroy]`
# + `respond_to do |format| format.html { ... } end`. Lowered here:
# - `before_action` becomes an explicit `set_article if [...].include?(name)`
#   call inside `process_action`, which is the per-controller dispatch
#   case (replacing Ruby's symbol-to-method `send` dispatch the spinel
#   subset forbids).
# - `respond_to do |format|` is dropped — every action returns HTML in
#   this fixture; format dispatch isn't exercised.
# - `redirect_to @article` (polymorphic) lowered to
#   `redirect_to RouteHelpers.article_path(@article.id)`.
class ArticlesController < ApplicationController
  ACTIONS_NEEDING_ARTICLE = [:show, :edit, :update, :destroy].freeze

  def process_action(action_name)
    set_article if ACTIONS_NEEDING_ARTICLE.include?(action_name)
    case action_name
    when :index   then index
    when :show    then show
    when :new     then new_action
    when :edit    then edit
    when :create  then create
    when :update  then update
    when :destroy then destroy
    end
  end

  def index
    articles = Article.all
    # `Article.includes(:comments).order(created_at: :desc)` in
    # real-blog. The `includes` is an eager-loading optimization
    # (correctness-equivalent to plain `.all`); `order(created_at:
    # :desc)` is lowered here to in-memory sort.
    articles = articles.sort_by { |a| a.created_at.to_s }.reverse
    render(Views::Articles.index(articles, notice: @flash[:notice]))
  end

  def show
    render(Views::Articles.show(@article, notice: @flash[:notice]))
  end

  # Action method named `new_action` in Ruby because `new` is the
  # constructor inherited from Object; defining `def new` on a class
  # would shadow it. The router maps the :new action to this method.
  def new_action
    @article = Article.new
    render(Views::Articles.new(@article))
  end

  def edit
    render(Views::Articles.edit(@article))
  end

  def create
    @article = Article.new(article_params.to_h)
    if @article.save
      redirect_to(
        RouteHelpers.article_path(@article.id),
        notice: "Article was successfully created.",
      )
    else
      render(Views::Articles.new(@article), status: :unprocessable_entity)
    end
  end

  def update
    if @article.update(article_params.to_h)
      redirect_to(
        RouteHelpers.article_path(@article.id),
        notice: "Article was successfully updated.",
        status: :see_other,
      )
    else
      render(Views::Articles.edit(@article), status: :unprocessable_entity)
    end
  end

  def destroy
    @article.destroy
    redirect_to(
      RouteHelpers.articles_path,
      notice: "Article was successfully destroyed.",
      status: :see_other,
    )
  end

  def set_article
    @article = Article.find(@params[:id].to_i)
  end

  def article_params
    @params.require(:article).permit(:title, :body)
  end
end
