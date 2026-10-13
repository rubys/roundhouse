# frozen_string_literal: true

# Real Rails component control, not a full Rails application boot.
gem "activesupport", "8.1.4"
gem "activemodel", "8.1.4"
require "active_model"
require "json"
require_relative "capture"

class RailsGeneratedProduct
  include ActiveModel::AttributeMethods
  attribute_method_suffix "_tag"
  attr_accessor :name

  private

  def attribute_tag(attribute)
    public_send(attribute).upcase
  end
end

begin
  BootToCore.active = ARGV.include?("--capture")
  RailsGeneratedProduct.define_attribute_methods(:name)
  BootToCore.active = false
  product = RailsGeneratedProduct.new
  product.name = "mixed"
  raise "original Rails contract failed" unless product.name_tag == "MIXED"
  method = product.method(:name_tag)
  result = { active_model: ActiveModel::VERSION::STRING, output: product.name_tag,
             method_owner: method.owner.name, source_location: method.source_location }
  if ARGV.include?("--capture")
    BootToCore.export([RailsGeneratedProduct])
    result[:status] = "exported_not_execution_verified"
  else
    result[:status] = "reference_passed"
  end
  puts JSON.generate(result)
rescue BootToCore::Unsupported => error
  puts JSON.generate(active_model: ActiveModel::VERSION::STRING, status: "unsupported", reason: error.message)
  exit 1
ensure
  BootToCore.active = false
end
