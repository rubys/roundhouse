# Not mapped to Exception: a `rescue NoMethodError` would then catch every error
class NoMethodError < Exception
end

class NameError < Exception
end
