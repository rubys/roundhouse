require_relative "adult_contract"

class SurveyProbe
  def self.call
    contract = AdultContract.new
    lower = contract.call(age: "17", active: false, note: nil)
    boundary = contract.call(age: "18", active: false, note: nil)
    upper = contract.call(age: "19", active: true, note: "note")
    [lower.success?, lower.to_h, lower.errors.to_h,
     boundary.success?, boundary.to_h, boundary.errors.to_h,
     upper.success?, upper.to_h, upper.errors.to_h]
  end
end
