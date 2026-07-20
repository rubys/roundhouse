class Comment < Sequel::Model
  many_to_one :article

  def validate
    super
    validates_presence [:commenter, :body]
  end
end
