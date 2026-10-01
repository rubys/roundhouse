# Renderer/Middleware protocol doubles are parameters of helpers, not Rails actions.
class SurveysController < ActionController::Base
  def index
    render plain: "analysis-only"
  end

  def page(controller, request, response, render_method)
    InertiaRails::Renderer.new("Articles/Show", controller, request, response, render_method,
      props: {title: "Synthetic", active: false, missing: nil}).render
  end

  def redirect_response(app, env)
    InertiaRails::Middleware.new(app).call(env)
  end
end
