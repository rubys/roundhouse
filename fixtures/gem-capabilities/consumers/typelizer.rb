class SurveysController < ActionController::Base
  def index
    interface = Typelizer::WriterContext.new.interface_for(TypedResource)
    interface.properties.map do |property|
      [property.name, property.type, property.optional, property.nullable]
    end
  end
end
