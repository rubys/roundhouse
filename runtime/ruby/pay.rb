# frozen_string_literal: true
# Pay — first-slice surface for Jumpstart Pro / pay gem apps.
#
# Roundhouse models the exception classes apps rescue (`Pay::Error`,
# `Pay::ActionRequired`) and the model class constants used in
# `is_a?(Pay::Charge)` / `is_a?(Pay::Subscription)`. It does NOT model
# ActiveRecord scopes, `payment_processor`, or Stripe/Braintree
# processors — those stay refused diagnostics until a later peel.
#
# On the CRuby/JRuby trees the real `pay` gem can replace this file when
# wired; Spinel keeps this façade so `rescue Pay::Error` type-checks and
# runs without inventing billing behaviour.

module Pay
  class Error < StandardError
  end

  class ActionRequired < Error
  end

  class Charge
  end

  class Subscription
  end

  class Customer
  end
end
