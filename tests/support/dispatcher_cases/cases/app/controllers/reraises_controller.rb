class ReraisesController < ApplicationController
  rescue_from ArgumentError do |error|
    raise error
  end
  def index
    raise ArgumentError, "original"
  end
end
