class BareReraisesController < ApplicationController
  rescue_from ArgumentError do |error|
    raise
  end
  def index
    raise ArgumentError, "original"
  end
end
