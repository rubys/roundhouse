class Person < ApplicationRecord
  after_create :capture_generated_create
  after_update :capture_generated_update
  after_save :capture_generated_save

  def capture_generated_create
    @generated_after_create = display_name
  end

  def capture_generated_update
    @generated_after_update = display_name
  end

  def capture_generated_save
    @generated_after_save = display_name
  end

  def generated_after_create
    @generated_after_create
  end

  def generated_after_update
    @generated_after_update
  end

  def generated_after_save
    @generated_after_save
  end
end
