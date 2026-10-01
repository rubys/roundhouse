class SurveysController < ActionController::Base
  def index
    stubs = Faraday::Adapter::Test::Stubs.new do |stub|
      stub.get("/items/7") do
        [200, {"content-type" => "application/json"}, '{"id":7,"active":false}']
      end
    end
    connection = Faraday.new do |builder|
      builder.adapter :test, stubs
    end
    response = connection.get("/items/7")
    stubs.verify_stubbed_calls
    [response.status, response.headers["content-type"], response.body]
  end
end
