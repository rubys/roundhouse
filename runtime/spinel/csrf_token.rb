# Session-backed CSRF issuance now lives in
# `runtime/ruby/action_controller/authenticity_token.rb` (masked XOR
# tokens). This file used to override `form_authenticity_token` with
# an unmasked session string; leaving that reopen in place would
# undo masking. Load order still requires this path from boot.rb.
