class SurveysController < ActionController::Base
  def index
    valid = SignupContract.new.call(age: 21)
    invalid = SignupContract.new.call(age: 17)
    [valid.success?, valid.to_h, invalid.success?, invalid.errors.to_h]
  end
end
