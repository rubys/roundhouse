# frozen_string_literal: true

# Real Rails generators, not replacement implementations. Load gem machinery
# before enabling the narrow hooks; the captured phase is explicitly bounded.
gem "activemodel", "8.1.4"
gem "activesupport", "8.1.4"
require "active_model"

class RailsCoreProduct
  include ActiveModel::AttributeMethods
  # A genuine, documented fixed-arity lane. Default (...) forwarding is tested
  # separately in the unchanged rails_probe.rb, not rewritten to make it pass.
  attribute_method_suffix "_describe", parameters: ""
  attr_accessor :name

  private

  def attribute_describe(attribute)
    "#{attribute}:#{@name}"
  end
end

class CallbackProduct
  include ActiveModel::Validations
  include ActiveModel::Validations::Callbacks
  before_validation :twist
  validate :check_weight

  def prime(seed, weight)
    @ledger = seed
    @probe_weight = weight
    @allowed = false
  end

  def twist
    @ledger = @ledger * 7 + 3
    @probe_weight = @probe_weight + 11
  end

  def review
    judge
  end

  def ledger
    @ledger
  end

  def probe_weight
    @probe_weight
  end

  def allowed?
    @allowed
  end

  private

  def judge
    @ledger = @ledger * 3 + 1
    @allowed = @probe_weight % 5 != 2
  end

  def check_weight
    judge
    errors.add(:base, "probe weight has residue two") unless @allowed
  end
end

if defined?(BootToCore)
  BootToCore.capture do
    RailsCoreProduct.alias_attribute(:title, :name)
    RailsCoreProduct.define_attribute_methods(:name)
  end
else
  RailsCoreProduct.alias_attribute(:title, :name)
  RailsCoreProduct.define_attribute_methods(:name)
end
