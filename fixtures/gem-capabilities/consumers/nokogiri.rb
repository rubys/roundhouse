class SurveysController < ActionController::Base
  def index
    document = Nokogiri.XML('<items><item id="7">Alpha &amp; Beta</item><item id="9">Other</item></items>')
    [document.at_xpath('/items/item[@id="7"]').text, document.xpath("/items/item").size]
  end
end
