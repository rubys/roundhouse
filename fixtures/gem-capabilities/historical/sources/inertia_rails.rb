class SurveyProbe
  def self.call(controller, request, response, render_method)
    InertiaRails::Renderer.new("Articles/Show", controller, request, response, render_method,
      props: {title: "Synthetic", active: false, missing: nil}).render
  end

  def self.redirect(app, env)
    InertiaRails::Middleware.new(app).call(env)
  end
end
