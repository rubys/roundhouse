# Ruby-family-only ViewHelpers extensions — a reopen, same pattern as
# active_record/connection.rb: NOT listed in runtime_loader's strict-
# target tables (the scaffold dir-walk ships it to spinel/CRuby/JRuby
# only). These bodies exercise emitter surface the elixir/rust lanes
# don't carry yet (String#include? renders .__struct__ on elixir;
# while-loops with post-loop reads hit the functionalize sign-threading
# gap) — they join the universal file when those lanes' emitters catch
# up and lobsters reaches them.
module ActionView
  module ViewHelpers
    # `number_with_delimiter(12345)` → "12,345" — comma grouping every
    # three digits, sign-aware. Integer-only, matching the signature
    # (every corpus arg is a count); while-loop over the digit string
    # so every target runtime types it; byte-equal to the CRuby overlay
    # variant it supersedes on the replay-locked /u page. The overlay's
    # `delimiter:` kwarg and float handling have no caller — the shared
    # version stays monomorphic.
    def self.number_with_delimiter(value)
      int = value.to_s
      sign = ""
      if int.start_with?("-")
        sign = "-"
        int = int[1, int.length - 1].to_s
      end
      out = ""
      i = int.length
      while i > 3
        out = "," + int[i - 3, 3].to_s + out
        i = i - 3
      end
      out = int[0, i].to_s + out
      sign + out
    end

    # Rails' sanitize_to_id — the default `id` a `*_tag` control
    # derives from its `name` ("tags[foo]" → "tags_foo"): drop "]",
    # replace every char outside [-a-zA-Z0-9:.] with "_". Char loop
    # over an inline membership literal (a module-const receiver reads
    # as an unresolved class in the strict typer), not gsub-with-regex:
    # portable and typed on every target runtime. The output alphabet
    # is attr-safe by construction, so call sites splice it unescaped.
    def self.sanitize_to_id(name)
      out = ""
      i = 0
      n = name.length
      while i < n
        # Two-arg slice + plain concat, the shapes every strict emitter
        # already ships (truncate's s[0, cutoff]); `out << c` renders as
        # an immutable-local .add() on Kotlin/Swift, and one-arg s[i]
        # isn't in the proven surface.
        c = name[i, 1].to_s
        if c != "]"
          out = out + ("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-:.".include?(c) ? c : "_")
        end
        i = i + 1
      end
      out
    end
  end
end
