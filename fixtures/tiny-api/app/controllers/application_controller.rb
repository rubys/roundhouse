# An API-only app on purpose: `ActionController::API`, no app/views,
# no `root`, uuid keys. tests/tiny_api.rs pins that shape; switching
# this parent to `Base` to get a green request removes the point of
# the fixture.
class ApplicationController < ActionController::API
  private

  def render_problem(message:, status: :unprocessable_entity)
    render json: { error: message }, status: status
  end
end
