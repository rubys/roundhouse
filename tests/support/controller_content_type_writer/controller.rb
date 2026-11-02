class MediaController < ActionController::Base
  include MediaHeaders
  def show
    case params[:kind]
    when "nil"
      write_media(value: "video/mp4")
      write_media(value: nil)
    when "initial_nil"
      write_media(value: nil)
    when "charset"
      write_media(value: "text/plain; charset=ISO-8859-1")
    when "retained_charset"
      write_media(value: "text/plain; charset=UTF-16")
      write_media(value: "application/json")
    when "symbol"
      write_media(value: :json)
    when "header_charset"
      response.headers["Content-Type"] = "text/plain; charset=UTF-16"
      write_media(value: "application/json")
    else
      write_media(value: "video/mp4")
    end
    self.response_body = "body"
  end
end
