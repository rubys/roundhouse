# frozen_string_literal: true
class ContentProbesController < ActionController::API
  def json
    render json: { ok: true }
  end
  def plain
    render plain: "ok"
  end
  def html
    render html: "ok"
  end
  def vendor_json
    render json: { ok: true }, content_type: "application/vnd.api+json"
  end
  def json_charset
    render json: { ok: true }, content_type: "application/json; charset=iso-8859-1"
  end
  def head_ok
    head :ok
  end
  def head_csv
    head :ok, content_type: "text/csv"
  end
  def send_csv
    send_data "ok", type: "text/csv"
  end
  def send_binary
    send_data "ok", type: "application/octet-stream"
  end
  def redirect
    redirect_to "/target"
  end
  def plain_csv
    render plain: "ok", content_type: "text/csv"
  end
  def plain_charset
    render plain: "ok", content_type: "text/plain; charset=iso-8859-1"
  end
  def head_json
    head :ok, content_type: "application/json"
  end
  def head_no_content
    head :no_content
  end
  def send_json
    send_data "ok", type: "application/json"
  end
  def inherited_charset
    self.content_type = "text/html; charset=iso-8859-1"
    render plain: "ok", content_type: "text/csv"
  end
end
