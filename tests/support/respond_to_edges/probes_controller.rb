class ProbesController < ApplicationController
  skip_forgery_protection
  layout false

  # html block with side effects only; html + turbo templates exist
  def h_side
    @who = "h_side"
    respond_to do |format|
      format.html { @side = 1 }
    end
  end

  # html side-effect block + json arm
  def h_side_json
    @who = "h_side_json"
    respond_to do |format|
      format.html { @side = 1 }
      format.json { render json: { a: 1 } }
    end
  end

  # bare html + inline json, both html and turbo templates
  def h_multi
    @who = "h_multi"
    respond_to do |format|
      format.html
      format.json { render json: { a: 1 } }
    end
  end

  # html renders a different template
  def h_other
    @who = "h_other"
    respond_to do |format|
      format.html { render :other }
      format.json { render json: { a: 1 } }
    end
  end

  def h_redirect
    @who = "h_redirect"
    respond_to do |format|
      format.html { redirect_to "/elsewhere" }
      format.turbo_stream
    end
  end

  # bare html with ONLY a turbo_stream template
  def h_bare_noview
    @who = "h_bare_noview"
    respond_to do |format|
      format.html
      format.json { render json: { a: 1 } }
    end
  end

  # bare format.any, turbo template exists
  def any_bare
    @who = "any_bare"
    respond_to do |format|
      format.html { render html: "h" }
      format.any
    end
  end

  # an explicit render in the html arm, implicit tail in the turbo arm
  def mixed
    @who = "mixed"
    respond_to do |format|
      format.html { render :h_other_alt }
      format.turbo_stream { @side = 1 }
    end
  end

  # query-pinned format
  def pin
    respond_to do |format|
      format.html { render html: "h" }
      format.json { render json: { pinned: true } }
      format.turbo_stream { render turbo_stream: turbo_stream.remove("x") }
    end
  end

  # one-line multi arms in one call
  def semi
    respond_to do |format|
      format.html { render html: "h" }; format.json { render json: { s: 1 } }
    end
  end

  # html arm with conditional render, falls to implicit tail
  def h_cond
    @who = "h_cond"
    respond_to do |format|
      format.html { render html: "early" if params[:early] }
      format.turbo_stream
    end
  end
end
