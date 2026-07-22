# Session-backed lazy CSRF token for the spinel binary, overriding the
# shared runtime's empty-string default (runtime/action_view.rb). The
# spinel sibling of the CRuby overlay's action_controller_session.rb
# half: lazy generation keeps token creation (and therefore
# session-cookie emission) scoped to requests that actually render a
# csrf-consuming helper — pages without forms don't grow a session.
# Reaches the session through ActionController::Current because view
# helpers are module functions with no controller context.
#
# Randomness comes from the runtime's vendored crypto
# (lib/sp_crypto.c, always linked): url-safe base64 over
# /dev/urandom-quality bytes, the same alphabet SecureRandom's
# urlsafe_base64 uses on the CRuby side. Static-buffer contract — the
# :str return copies at the call boundary. Top-level FFI module
# (FFI plumbing must stay out of nested modules), distinctly named to
# coexist with other extern modules in one program.
#
# Load order: required by main.rb AFTER runtime/action_view, so this
# reopen wins over the shared empty-string default.
module CsrfRand
  ffi_func :sp_crypto_random_b64url, [:int], :str
end

module ActionView
  module ViewHelpers
    def self.form_authenticity_token
      session = ActionController::Current.session
      return "" if session.nil?
      token = session[:_csrf_token]
      if token.nil?
        token = CsrfRand.sp_crypto_random_b64url(32)
        session[:_csrf_token] = token
      end
      token.to_s
    end
  end
end
