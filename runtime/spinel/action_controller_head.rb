# frozen_string_literal: true
# Rails 8.1.4 ActionController::Head#head(status, options = nil).
# This options-hash implementation is used by the Ruby family and Spinel;
# strict non-Ruby targets retain the smaller typed keyword implementation
# in runtime/ruby/action_controller/base.rb.
module ActionController
  class Base
    # Rails' `redirect_to …, flash: { error: … }`: any key, set after
    # notice/alert as `flash.update` does. The strict targets' shared
    # redirect_to keeps notice/alert only, as their Flash does.
    def redirect_to(path, notice: nil, alert: nil, status: :found, flash: nil)
      loc = ActionController.sanitize_location(path)
      loc = absolute_redirect_location(loc)
      loc = same_host_location(loc)
      @_location = loc
      @_status   = resolve_status(status)
      @_performed = true
      @flash[:notice] = notice unless notice.nil?
      @flash[:alert]  = alert  unless alert.nil?
      flash&.each { |key, value| @flash[key] = value }
      nil
    end

    def head(status, options = nil)
      if status.is_a?(Hash)
        raise ArgumentError, "#{status.inspect} is not a valid value for `status`."
      end
      raise AbstractController::DoubleRenderError if @_performed

      status = :ok if status.nil?
      status_code = head_status_code(status)
      content_type = +""
      content_type = head_option_content_type(options[:content_type]) unless options.nil?

      @_status = status_code
      unless options.nil?
        location = options.delete(:location)
        options.delete(:content_type)
        options.each do |key, value|
          @_headers[normalize_head_header_name(key.to_s)] = value.to_s
        end
        unless location.nil?
          resolved_location = ActionView::ViewHelpers.url_for(location).to_s
          @_location = ActionController.sanitize_location(resolved_location)
        end
      end

      if head_includes_content?(@_status)
        if !@_content_type_explicit || media_type.empty?
          @_content_type = content_type.empty? ? head_format_content_type : content_type
        end
        @_content_type = media_type
      else
        @_content_type = ""
      end

      @_body = +""
      @_performed = true
      @_head_response = true
      true
    end

    def head_response?
      @_head_response
    end

    # Render, redirect_to and head share Rails' Integer status pass-through.
    # Keep the shared strict-target resolver Symbol-only: those targets do
    # not all represent a Symbol | Integer runtime parameter.
    def resolve_status(status)
      return status if status.is_a?(Integer)
      resolve_symbol_status(status)
    end

    def head_status_code(status)
      return status if status.is_a?(Integer)
      unless STATUS_CODES.key?(status)
        raise ArgumentError, "Invalid HTTP status: #{status}"
      end
      resolve_status(status)
    end

    def head_includes_content?(status)
      !(status >= 100 && status < 200) && status != 204 && status != 205 && status != 304
    end

    def head_format_content_type
      mime_type = Mime[@request_format]
      return mime_type.to_s unless mime_type.nil?
      Mime[:html].to_s
    end

    def head_option_content_type(content_type)
      if content_type.is_a?(Symbol)
        mime_type = Mime[content_type]
        raise ArgumentError, "Unknown MIME type #{content_type}" if mime_type.nil?
        mime_type.to_s
      else
        content_type.to_s
      end
    end

    def normalize_head_header_name(name)
      name.split(/[-_]/).map do |part|
        part.empty? ? "" : part[0].upcase + part[1..-1].to_s
      end.join("-")
    end
  end
end
