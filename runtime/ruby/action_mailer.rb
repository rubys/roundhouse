# frozen_string_literal: true
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
# A mailer action's templates are rendered at compile time into the
# parts `mail` receives (`lower::mailer_templates`), with the class's
# `default` headers merged under the action's own.
module ActionMailer
  # One built mail. Header values are stored verbatim from the mail()
  # opts — raw SQL-style honesty: the corpus interpolates arbitrary
  # strings here, and nothing downstream needs them re-typed.
  # One part of a multipart message: its rendered template.
  class Part
    def initialize(body, mime)
      @body = body
      @mime = mime
    end

    def body
      @body
    end

    def decoded
      @body
    end

    def mime_type
      @mime
    end
  end

  # One built mail, as ActionMailer's `Mail::Message` answers it: address
  # headers as Arrays of bare addresses, and with both a text and an html
  # template, a multipart/alternative whose own body is empty.
  class Message
    def initialize(opts, text_body = nil, html_body = nil)
      @to = Message.addresses(opts[:to])
      @from = Message.addresses(opts[:from])
      @cc = Message.addresses(opts[:cc])
      @bcc = Message.addresses(opts[:bcc])
      @reply_to = Message.addresses(opts[:reply_to])
      @subject = opts[:subject]
      @text_body = text_body
      @html_body = html_body
      given = opts[:body]
      @body = if !text_body.nil? && !html_body.nil?
        ""
      elsif !text_body.nil?
        text_body
      elsif !html_body.nil?
        html_body
      else
        given.nil? ? "" : given.to_s
      end
    end

    # `"Name <a@example.com>"` → `"a@example.com"`; nil stays nil.
    def self.addresses(value)
      return nil if value.nil?
      list = value.is_a?(Array) ? value : [value]
      list.map do |item|
        text = item.to_s
        open = text.index("<")
        close = text.index(">")
        open.nil? || close.nil? || close < open ? text.strip : text[open + 1, close - open - 1].to_s
      end
    end

    def to
      @to
    end

    def from
      @from
    end

    def cc
      @cc
    end

    def bcc
      @bcc
    end

    def reply_to
      @reply_to
    end

    def subject
      @subject
    end

    def body
      @body
    end

    def multipart?
      !@text_body.nil? && !@html_body.nil?
    end

    def text_part
      multipart? ? Part.new(@text_body.to_s, "text/plain") : nil
    end

    def html_part
      multipart? ? Part.new(@html_body.to_s, "text/html") : nil
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

    def mail(opts, text_body = nil, html_body = nil)
      Message.new(opts, text_body, html_body)
    end

    def params
      @params ||= {}
    end

    def _with_params(params)
      @params = params
      self
    end
  end
end
