class Post < ApplicationRecord
  has_many :comments
  validates :title, presence: true
  scope :recent, -> { limit(10) }
  scope :published, -> { where(published: true) }
end
