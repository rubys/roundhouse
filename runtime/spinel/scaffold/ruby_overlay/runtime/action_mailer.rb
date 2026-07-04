# Minimal ActionMailer surface (CRuby overlay) — just enough for
# app/mailers/* files to LOAD. The same-dir require expansion pulls
# mailer classes into the model require graph (models reference them
# from method bodies: `BanNotification.notify(...)`), so their
# class-definition line `< ActionMailer::Base` must resolve even
# though no exercised benchmark route ever sends mail. Instance
# `mail(...)` raises if an unexercised path is actually reached —
# a loud signal beats silently dropping mail on the floor.
module ActionMailer
  class Base
    # Class-body DSL (`default from: ...`) — accepted and ignored.
    def self.default(opts = nil)
      nil
    end

    def mail(opts = {})
      raise NotImplementedError,
            "ActionMailer is not implemented in the transpiled runtime"
    end
  end
end
