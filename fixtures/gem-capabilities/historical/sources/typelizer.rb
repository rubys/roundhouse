require_relative "typed_resource"

class SurveyProbe
  def self.call
    interface = Typelizer::WriterContext.new.interface_for(TypedResource)
    interface.properties.map do |property|
      [property.name, property.type, property.optional, property.nullable]
    end
  end
end
