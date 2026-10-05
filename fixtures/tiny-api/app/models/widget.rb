class Widget < ApplicationRecord
  include Labelled

  class Invalid < StandardError
  end

  enum :status, { draft: 0, live: 1 }

  has_many :parts, dependent: :destroy
  validates :name, presence: true

  def summary
    { id: id, name: name, status: status, parts: parts.count }
  end

  def each_part(*names, &block)
    parts.where(name: names).each(&block)
  end
end
