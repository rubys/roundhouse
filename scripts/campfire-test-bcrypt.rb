# Functional tests use real bcrypt without paying the production work factor.
# Only preloaded by the interpreted suite; production DEFAULT_COST is unchanged.
require "bcrypt"
BCrypt::Engine.cost = BCrypt::Engine::MIN_COST
