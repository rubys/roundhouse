# Shapes the negotiating lowering REFUSES with a compile-time error (it cannot
# run them); oracle.rb still measures what Rails answers for each.
class RefusedProbesController < ApplicationController
  skip_forgery_protection
  layout false

  def any_multi
    respond_to do |format|
      format.html { render html: "h" }
      format.any(:json, :turbo_stream) { render plain: "multi" }
    end
  end

  # request.format forced before respond_to
  def force_json
    request.format = :json
    respond_to do |format|
      format.html { render html: "h" }
      format.json { render json: { forced: true } }
    end
  end

  def force_turbo
    request.format = :turbo_stream
    @who = "force_turbo"
    respond_to do |format|
      format.html
      format.turbo_stream
    end
  end

  # request.format forced, no respond_to
  def force_plain
    request.format = :turbo_stream
    @who = "force_plain"
  end

  # a conditional arm registration
  def cond_arm
    @who = "cond_arm"
    respond_to do |format|
      format.html { render html: "h" }
      format.json { render json: { a: 1 } } if params[:j]
    end
  end

  # an arm that assigns to the block parameter
  def assign_param
    @who = "assign_param"
    respond_to do |f|
      f.html { f = "hello"; render html: f }
      f.json { render json: { a: 1 } }
    end
  end

  # respond_to in a private helper, bare arms
  def helper_resp
    @who = "helper_resp"
    render_it
  end

  private

  def render_it
    respond_to do |format|
      format.html
      format.turbo_stream
    end
  end
end
