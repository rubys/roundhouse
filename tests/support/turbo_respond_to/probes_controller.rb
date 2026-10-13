# The `respond_to` shapes `tests/turbo_stream_respond_to.rs` runs, and the
# SAME file `oracle.rb` runs under actionpack 8.1 + turbo-rails: one source,
# two executions, so the expected table in `expected.json` is what Rails
# answered for exactly this controller.
class ProbesController < ApplicationController
  skip_forgery_protection
  layout false

  # html + bare turbo_stream (implicit template) + inline json
  def implicit
    @who = "implicit"
    respond_to do |format|
      format.html { render html: "html arm" }
      format.turbo_stream
      format.json { render json: { ok: true, kind: "implicit" } }
    end
  end

  # turbo_stream with a render block (inline builders), json with status
  def inline
    respond_to do |format|
      format.html { render html: "html inline" }
      format.turbo_stream do
        render turbo_stream: turbo_stream.update("flash", partial: "probes/flash", locals: { msg: "hello" }), status: :created
      end
      format.json { render json: { error: "nope" }, status: :unprocessable_entity }
    end
  end

  # array of builders, remove with no template
  def many
    respond_to do |format|
      format.html { render html: "html many" }
      format.turbo_stream do
        render turbo_stream: [
          turbo_stream.remove("gone"),
          turbo_stream.append("list", partial: "probes/flash", locals: { msg: "added" })
        ]
      end
    end
  end

  def head_arm
    respond_to do |format|
      format.turbo_stream { head :unprocessable_entity }
      format.html { render html: "html head" }
    end
  end

  def render_sym
    @who = "render_sym"
    respond_to do |format|
      format.turbo_stream { render :implicit }
      format.html { render html: "html render_sym" }
    end
  end

  # block with side effects, then the implicit turbo template
  def after_block
    respond_to do |format|
      format.html { render html: "html after_block" }
      format.turbo_stream do
        @who = "set in block"
      end
    end
  end

  # turbo_stream is the ONLY arm
  def turbo_only
    @who = "turbo_only"
    respond_to do |format|
      format.turbo_stream { render :implicit }
    end
  end

  # turbo_stream registered BEFORE html (matters for */*)
  def turbo_first
    respond_to do |format|
      format.turbo_stream { render turbo_stream: turbo_stream.remove("x") }
      format.html { render html: "html turbo_first" }
    end
  end

  # json is the only arm besides html
  def html_json
    respond_to do |format|
      format.html { render html: "html html_json" }
      format.json { render json: { a: 1 } }
    end
  end

  # format.any catches the rest
  def any_arm
    respond_to do |format|
      format.turbo_stream { render turbo_stream: turbo_stream.remove("x") }
      format.any { render html: "any arm" }
    end
  end

  # a bare partial name found in an ANCESTOR's directory (application/), and
  # an interpolated target
  def inherited
    @n = 7
    respond_to do |format|
      format.html { render html: "html inherited" }
      format.turbo_stream do
        render turbo_stream: [
          turbo_stream.replace("item_#{@n}", partial: "banner", locals: { text: "from application" }),
          turbo_stream.after("tip", partial: "probes/flash", locals: { msg: "tip" })
        ]
      end
    end
  end

  # a bare html arm, with a turbo_stream template beside the html one: the
  # arm runs html, so the template is the html one whatever Accept lists first
  def html_bare
    @who = "html_bare"
    respond_to do |format|
      format.html
    end
  end
end
