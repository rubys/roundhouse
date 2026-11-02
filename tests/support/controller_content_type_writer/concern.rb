module MediaHeaders
  def write_media(value:)
    self.content_type = value
  end
end
