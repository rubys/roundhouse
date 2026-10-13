class ProbesController < ApplicationController
  skip_forgery_protection
  layout false

  # html arm calls a helper that renders
  def html_helper
    @who = "html_helper"
    respond_to do |format|
      format.html { fail_it }
      format.turbo_stream
    end
  end

  # html arm calls a helper that redirects
  def html_helper_redirect
    @who = "html_helper_redirect"
    respond_to do |format|
      format.html { go_away }
      format.turbo_stream
    end
  end

  def html_cond_redirect
    @who = "html_cond_redirect"
    respond_to do |format|
      format.html { redirect_to "/x" if params[:r] }
      format.turbo_stream
    end
  end

  def html_cond_head
    @who = "html_cond_head"
    respond_to do |format|
      format.html { head :accepted if params[:r] }
      format.turbo_stream
    end
  end

  def html_unless_render
    @who = "html_unless_render"
    respond_to do |format|
      format.html do
        unless params[:r]
          render :other
        end
      end
      format.turbo_stream
    end
  end

  def html_early_return
    @who = "html_early_return"
    respond_to do |format|
      format.html do
        return redirect_to("/x") if params[:r]
        @side = 1
      end
      format.turbo_stream
    end
  end

  def html_rescue
    @who = "html_rescue"
    respond_to do |format|
      format.html do
        begin
          render :other
        rescue StandardError
          @side = 1
        end
      end
      format.turbo_stream
    end
  end

  def html_status
    @who = "html_status"
    respond_to do |format|
      format.html { render :html_status, status: :unprocessable_entity }
      format.turbo_stream
    end
  end

  # local assign before the arms
  def pre_assign
    @who = "pre_assign"
    respond_to do |format|
      @pre = 1
      format.html
      format.json { render json: [1, 2].map { |i| i * 2 } }
    end
  end

  # scaffold shape: arms inside if/else, plus a statement before
  def seq_if
    @who = "seq_if"
    respond_to do |format|
      @pre = 1
      if params[:ok]
        format.html { render html: "ok" }
        format.json { render json: { ok: true } }
      else
        format.html { render :other, status: :unprocessable_entity }
        format.json { render json: { ok: false }, status: :unprocessable_entity }
      end
    end
  end

  def top_if
    @who = "top_if"
    respond_to do |format|
      if params[:ok]
        format.html { render html: "ok" }
        format.json { render json: { ok: true } }
      else
        format.html { render :other, status: :unprocessable_entity }
        format.json { render json: { ok: false }, status: :unprocessable_entity }
      end
    end
  end

  def wants_param
    @who = "wants_param"
    respond_to do |wants|
      wants.html
      wants.json { render json: { w: 1 } }
    end
  end

  def multi_turbo
    respond_to do |format|
      format.html { render html: "h" }
      format.turbo_stream do
        render turbo_stream: [
          turbo_stream.remove("a"),
          turbo_stream.remove("b")
        ]
      end
    end
  end

  def three
    respond_to do |format|
      format.turbo_stream { render turbo_stream: turbo_stream.remove("x") }
      format.html { render html: "h" }
      format.json { render json: { a: 1 } }
    end
  end

  def json_only
    respond_to do |format|
      format.json { render json: { a: 1 } }
    end
  end

  def html_only
    respond_to do |format|
      format.html { render html: "h" }
    end
  end

  def plain_two
    @who = "plain_two"
  end

  def send_arm
    @who = "send_arm"
    respond_to do |format|
      format.html { send_data "DATA", type: "text/plain" }
      format.turbo_stream
    end
  end

  def rescue_render
    @who = "rescue_render"
    respond_to do |format|
      format.html do
        begin
          raise "boom"
        rescue StandardError
          render html: "rescued"
        end
      end
      format.turbo_stream
    end
  end

  def shadow_local
    @who = "shadow_local"
    format = "outer"
    respond_to do |wants|
      wants.html { render html: format }
      wants.json { render json: { f: format } }
    end
  end

  def numbered
    @who = "numbered"
    respond_to do
      _1.html { render html: "n" }
      _1.json { render json: { n: 1 } }
    end
  end

  def it_param
    @who = "it_param"
    respond_to do
      it.html { render html: "i" }
      it.json { render json: { i: 1 } }
    end
  end

  # branches with different arm sets
  def branch_diff
    @who = "branch_diff"
    respond_to do |format|
      @pre = 1
      if params[:ok]
        format.html { render html: "ok" }
        format.json { render json: { ok: true } }
      else
        format.html { render html: "bad" }
      end
    end
  end

  def top_branch_diff
    @who = "top_branch_diff"
    respond_to do |format|
      if params[:ok]
        format.html { render html: "ok" }
        format.json { render json: { ok: true } }
      else
        format.html { render html: "bad" }
      end
    end
  end

  # arms only in one branch, nothing registered in the other
  def if_noelse
    @who = "if_noelse"
    respond_to do |format|
      if params[:ok]
        format.html { render html: "ok" }
      end
    end
  end

  def seq_if_noelse
    @who = "seq_if_noelse"
    respond_to do |format|
      @pre = 1
      if params[:ok]
        format.html { render html: "ok" }
      end
    end
  end

  def else_other_stmt
    @who = "else_other_stmt"
    respond_to do |format|
      if params[:ok]
        format.html { render html: "ok" }
      else
        head :gone
      end
    end
  end

  # bare format.any with both an html and a turbo_stream template
  def any_two
    @who = "any_two"
    respond_to do |format|
      format.json { render json: { a: 1 } }
      format.any
    end
  end

  # nested blocks inside arms that read like the block parameter
  def nested_named
    @who = "nested_named"
    respond_to do |f|
      f.json { render json: [1, 2].map { |f| f * 2 } }
      f.html { render html: [1, 2].map { |f| f * 3 }.join(",").html_safe }
    end
  end

  def nested_rest
    @who = "nested_rest"
    respond_to do |f|
      f.json { render json: [1, 2].map { |x, f = 5| x + f } }
      f.html { render html: [1, 2].map { |x, f = 7| x * f }.join(",").html_safe }
    end
  end

  def nested_it
    @who = "nested_it"
    respond_to do
      it.json { render json: [1, 2].map { it * 2 } }
      it.html { render html: [1, 2].map { it * 3 }.join(",").html_safe }
    end
  end

  private

  def fail_it
    render html: "failed"
  end

  def go_away
    redirect_to "/gone"
  end
end
