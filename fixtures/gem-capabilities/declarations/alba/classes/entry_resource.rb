require_relative "base_resource"
require_relative "writer_resource"

class EntryResource < BaseResource
  attributes :title, :active
  attributes :note
  one :writer, resource: WriterResource
end
