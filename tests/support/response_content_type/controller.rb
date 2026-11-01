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
  def new_symbol_type
    render plain: "ok", content_type: :csv
  end
  def inherited_trailing_parameter
    self.content_type = "text/html; charset=iso-8859-1; profile=old"
    render plain: "ok", content_type: "text/csv"
  end
  def inherited_quoted
    self.content_type = "text/html; charset=\"iso-8859-1\""
    render plain: "ok", content_type: "text/csv"
  end
  def inherited_trailing_space
    self.content_type = "text/html; charset=iso-8859-1 "
    render plain: "ok", content_type: "text/csv"
  end
  def inherited_uppercase_value
    self.content_type = "text/html; charset=ISO-8859-1"
    render plain: "ok", content_type: "text/csv"
  end
  def inherited_uppercase_key
    self.content_type = "text/html; Charset=iso-8859-1"
    render plain: "ok", content_type: "text/csv"
  end
  def inherited_no_charset
    self.content_type = "text/html"
    render plain: "ok", content_type: "text/csv"
  end
  def inherited_empty_charset
    self.content_type = "text/html; charset="
    render plain: "ok", content_type: "text/csv"
  end
  def inherited_other_parameter
    self.content_type = "text/html; xcharset=iso-8859-1"
    render plain: "ok", content_type: "text/csv"
  end
  def inherited_duplicate_charset
    self.content_type = "text/html; charset=iso-8859-1; charset=utf-8"
    render plain: "ok", content_type: "text/csv"
  end
  def inherited_boundary_before
    self.content_type = "text/html; boundary=old; charset=iso-8859-1"
    render plain: "ok", content_type: "text/csv; boundary=new"
  end
  def new_quoted_charset
    render plain: "ok", content_type: 'text/csv; charset="iso-8859-1"'
  end
  def new_trailing_parameter
    render plain: "ok", content_type: "text/csv; charset=iso-8859-1; profile=new"
  end
  def new_uppercase_value
    render plain: "ok", content_type: "text/csv; charset=ISO-8859-1"
  end
  def new_uppercase_key
    render plain: "ok", content_type: "text/csv; Charset=iso-8859-1"
  end
  def new_empty_charset
    render plain: "ok", content_type: "text/csv; charset="
  end
  def new_other_parameter
    render plain: "ok", content_type: "text/csv; xcharset=iso-8859-1"
  end
end
