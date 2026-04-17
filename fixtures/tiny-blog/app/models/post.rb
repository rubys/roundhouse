class Post < ApplicationRecord
  has_many :comments
  validates :title, presence: true
  scope :recent, -> { limit(10) }
end
