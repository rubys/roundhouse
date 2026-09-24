# `redirect_back_or_to(fallback)` — back to the Referer when it names
# this site, else to `fallback`. Rails' default refuses another host
# (`allow_other_host` follows raise_on_open_redirects, on for new apps);
# a prefix match on `base_url` is that check without parsing.
#
# NOT in the shared `runtime/ruby/action_controller/base.rb`: it reads
# the parked request (`ActionController::Current.request`), which only
# the ruby family carries, and a method there is transpiled to every
# target — rust/kotlin/go/csharp/elixir/swift all failed to compile it
# (5aac5d8d). Required from BOTH ruby-family boots (spinel's and the
# CRuby overlay's), after the controller runtime it reopens; the strict
# targets never see it, and no app they build calls it.
#
# lobsters' comments/stories controllers answer a vote this way.
module ActionController
  class Base
    def redirect_back_or_to(fallback, notice: nil, alert: nil, status: :found)
      target = fallback
      req = ActionController::Current.request
      unless req.nil?
        ref = req.referer
        base = req.base_url
        if ref == base || ref.start_with?(base + "/")
          target = ref
        end
      end
      redirect_to(target, notice: notice, alert: alert, status: status)
    end
  end
end
