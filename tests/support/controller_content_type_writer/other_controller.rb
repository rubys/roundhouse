class OtherMediaController < MediaController
  def content_type=(value)
    result = super(value)
    self.response_body = result.to_s
    result
  end
end
