# ActionMailer runtime — the delivery-capture slice.
#
# Rails' mailer machinery is method_missing at both ends (class-side
# `BanNotification.notify(...)` proxies to `new.notify(...)`, and
# delivery routes through delivery-method plugins). This runtime keeps
# the statically-resolvable core: `mail(opts)` builds a Message value
# object from the header options, and `deliver_now`/`deliver_later`
# append it to `ActionMailer::Base.deliveries` — Rails' :test delivery
# method, which is also what emitted tests will assert against. No
# SMTP; single-process apps that need real delivery get it as a
# per-target adapter later.
#
# The class-side call idiom is grounded at emit
# (`apply_mailer_class_side_lowering` synthesizes `def self.notify` →
# `new.notify(...)` wrappers on mailer classes), so nothing here is
# reflective.
#
# Known gaps, deliberately open: mailer view templates (*.text.erb)
# are not ingested yet, so Message#body stays empty unless the mail
# opts carry one; ApplicationMailer's `default from:` DSL is dropped
# at ingest, so a mailer that omits an explicit :from sends nil.
module ActionMailer
  # One built mail. Header values are stored verbatim from the mail()
  # opts — raw SQL-style honesty: the corpus interpolates arbitrary
  # strings here, and nothing downstream needs them re-typed.
  class Message
    def initialize(opts)
      @to = opts[:to]
      @from = opts[:from]
      @replyto = opts[:replyto]
      @subject = opts[:subject]
      @body = opts[:body]
    end

    def to
      @to
    end

    def from
      @from
    end

    def replyto
      @replyto
    end

    def subject
      @subject
    end

    def body
      @body
    end

    # Rails' :test delivery semantics — collect, don't transmit. Both
    # return the message so `X.notify(...).deliver_now` chains stay
    # value-shaped. `deliver_later` has no queue in a single-process
    # runtime; immediate collection is the honest equivalent.
    def deliver_now
      ActionMailer::Base.deliveries.push(self)
      self
    end

    def deliver_later
      ActionMailer::Base.deliveries.push(self)
      self
    end
  end

  class Base
    # Class-body DSL (`default from: ...`) — accepted and ignored when
    # an ingest ever carries it through (today it's dropped upstream).
    def self.default(opts = nil)
      nil
    end

    # The :test-style delivery log. Lazy-init on first read (top-level
    # assignment at require time is a load-order trap under AOT).
    def self.deliveries
      @deliveries ||= []
    end

    def mail(opts)
      Message.new(opts)
    end
  end
end
