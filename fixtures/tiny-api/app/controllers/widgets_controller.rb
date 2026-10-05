class WidgetsController < ApplicationController
  before_action :set_widget, only: %i[show update]

  def index
    widgets = Widget.order(:name).limit(page_size)
    render json: widgets.map { |w| w.summary }
  end

  def show
    return render_problem(message: "not found", status: :not_found) unless @widget

    render json: @widget.summary
  end

  def create
    widget = Widget.new(name: params[:name], status: :draft)
    if widget.save
      render json: widget.summary, status: :created
    else
      render_problem(message: widget.errors.full_messages.join(", "))
    end
  end

  def update
    return render_problem(message: "not found", status: :not_found) unless @widget

    if @widget.update(name: params[:name])
      render json: @widget.summary
    else
      render_problem(message: @widget.errors.full_messages.join(", "))
    end
  end

  private

  def set_widget
    @widget = Widget.find_by(id: params[:id])
  end

  def page_size
    (params[:per] || 20).to_i
  end
end
