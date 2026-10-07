# Spam gate behind `invisible_captcha` (the invisible_captcha gem).
#
# The lowering synthesizes:
#
#   before_action :detect_invisible_captcha_spam, only: …
#   def detect_invisible_captcha_spam
#     head :ok if ActionController::InvisibleCaptcha.spam?(params)
#   end
#
# `spam?` is true when a honeypot field is filled. The real gem rotates
# honeypot names (and often wires one name via its form helper). We only
# recognize `subtitle` — a published default that does not collide with
# common real fields (`url`, `website`, `email_confirm`). Custom /
# rotated names stay a survey gap until the form helper is modeled.
# Timestamp / spinner / custom `on_spam` callbacks are not modeled —
# unsupported kwargs leave the class-body macro as a survey gap.
#
# Reads through `Params.str` / `Params.provided` so the body stays
# concretely typed (same posture as pagination's `?page=` read).
require_relative "../params"

module ActionController
  module InvisibleCaptcha
    HONEYPOTS = %w[subtitle].freeze

    def self.spam?(params)
      HONEYPOTS.any? do |name|
        Params.provided(params, name) && !Params.str(params, name, "").empty?
      end
    end
  end
end
