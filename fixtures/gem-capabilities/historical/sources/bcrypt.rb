class SurveyProbe
  def self.call
    digest = BCrypt::Password.create("synthetic-secret", cost: 4).to_s
    stored = BCrypt::Password.new(digest)
    [stored == "synthetic-secret", stored == "wrong-secret"]
  end
end
