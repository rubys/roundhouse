module Labelled
  extend ActiveSupport::Concern

  def label(prefix = "", *parts, separator: " ")
    ([prefix, name] + parts).reject(&:empty?).join(separator)
  end
end
