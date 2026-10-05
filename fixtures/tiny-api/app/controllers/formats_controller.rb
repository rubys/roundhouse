# Base keeps this routing regression independent of the API controller gap.
class FormatsController < ActionController::Base
  # Exercise a route whose literal path includes the JSON suffix.
  def literal
    render plain: "literal"
  end

  # Return the same collection response with or without a format suffix.
  def index
    render plain: "collection"
  end

  # Expose the member ID so the test can detect an unstripped suffix.
  def show
    render plain: params[:id]
  end
end
