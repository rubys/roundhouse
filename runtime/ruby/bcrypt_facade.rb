# BCrypt — password hashing. `has_secure_password`'s `authenticate` does
# `BCrypt::Password.new(digest) == password`, and the login controller
# reads `Engine::DEFAULT_COST`. LOGIN IS ON THE BENCHMARK PATH (the
# replay harness POSTs real credentials), so this raising façade only
# stands in where the real implementation can't: the spin-shaped spinel
# tree swaps this file for `require "bcrypt"` — the spinel-bcrypt spin
# package, real crypt_blowfish in carried C — whenever the app consumes
# BCrypt (see project.rs spin_shape). CRuby no-ops the whole façade
# chain (real gems there); JRuby keeps the raising stand-in.
#
# Own file rather than a gem_facades.rb section so the swap is
# whole-file — the same grain every other façade mechanism uses.
module BCrypt
  module Engine
    DEFAULT_COST = 12
  end

  class Password
    def self.create(_secret, cost: nil)
      GemFacade.fail!("BCrypt::Password.create")
      new("")
    end

    def initialize(_digest)
      GemFacade.fail!("BCrypt::Password.new")
      @digest = _digest
    end

    def ==(_other)
      GemFacade.fail!("BCrypt::Password#==")
      false
    end

    def to_s
      GemFacade.fail!("BCrypt::Password#to_s")
      ""
    end
  end
end
