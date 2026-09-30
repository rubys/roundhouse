# Supplementary analyzed adapter surface, NOT a native/runtime support claim.
# The 14 reference contracts instead exercise the real Renderer/Middleware.
class SurveysController < ApplicationController
  def index
    render inertia: "Articles/Show", props: {active: false, missing: nil}
  end
end
