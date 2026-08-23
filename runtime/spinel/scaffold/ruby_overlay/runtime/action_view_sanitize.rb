# `sanitize`, `strip_tags` and `auto_link` — the real ones, on CRuby.
#
# The shared runtime (`runtime/ruby/action_view/view_helpers_ext.rb`)
# carries a scanner that every target compiles: it agrees with
# `Rails::HTML5::FullSanitizer` on 24 of 25 `strip_tags` probes, and it
# REFUSES `sanitize` on anything containing markup rather than guess.
# That refusal is the honest answer for a target that cannot do better.
# This tree can do better, so here it does.
#
# WHY NOT FINISH THE SCANNER. Measured against
# `Rails::HTML5::SafeListSanitizer`, the safe-list pass is not a
# filtering problem, it is a PARSING one:
#
#   "<b>a<p>b</b>c"    -> "<b>a</b><p><b>b</b>c</p>"
#   "<b>unclosed"      -> "<b>unclosed</b>"
#   "</b>stray"        -> "stray"
#   "<B CLASS=y>x</B>"  -> "<b class=\"y\">x</b>"
#
# That is HTML5 tree construction with the adoption agency algorithm,
# plus name and attribute normalisation. A scanner reproduces the
# WELL-FORMED cases and diverges on the malformed ones — which for a
# sanitizer is precisely backwards, because malformed input is what an
# attacker sends. A security control that is correct only on inputs
# nobody attacks with is not a security control.
#
# So: the real gem. `rails-html-sanitizer` depends on `loofah` and
# `nokogiri`, loofah on `crass` and `nokogiri` — and nokogiri is already
# declared in the emitted Gemfile for any app that reaches it. No
# actionview dependency, so this is a Gemfile line and not a port, the
# same call `platform_agent` got. Output is Rails' byte for byte
# because it IS Rails': `ActionView::Base.safe_list_sanitizer` is this
# class.
#
# GUARDED, AND ON THE CONSTANT — not on the require. An app that never
# sanitizes must boot without the gem installed, and a gem that loads is
# still not the surface you assumed:
#
#   `Rails::HTML5` EXISTS ONLY IF `Loofah.html5_support?`. The gem's own
#   `sanitizer.rb` closes both its HTML5 sections with `end if
#   Rails::HTML::Sanitizer.html5_support?`, and Loofah answers false
#   wherever Nokogiri has no HTML5 parser — which is JRuby. Naming
#   `Rails::HTML5::SafeListSanitizer` directly took the JRuby blog tree
#   down at BOOT with `uninitialized constant Rails::HTML5`, after a
#   `require` that had succeeded. The CRuby lane was green, because
#   there the constant is there.
#
# `best_supported_vendor` is the gem's OWN answer to which one to use
# (`html5_support? ? Rails::HTML5::Sanitizer : Rails::HTML4::Sanitizer`),
# so this asks rather than re-deriving the condition — and asks the gem
# ONCE, at the version that is installed.
begin
  require "rails-html-sanitizer"
  RH_SANITIZER_VENDOR =
    if defined?(::Rails::HTML::Sanitizer) &&
       ::Rails::HTML::Sanitizer.respond_to?(:best_supported_vendor)
      ::Rails::HTML::Sanitizer.best_supported_vendor
    end
rescue LoadError
  RH_SANITIZER_VENDOR = nil
end

unless RH_SANITIZER_VENDOR.nil?
  module ActionView
    module ViewHelpers
      RH_SAFE_LIST_SANITIZER = RH_SANITIZER_VENDOR.safe_list_sanitizer.new
      RH_FULL_SANITIZER = RH_SANITIZER_VENDOR.full_sanitizer.new

      # Rails' `sanitize` returns an html_safe buffer — the whole point
      # is that the result may be spliced without re-escaping, and a
      # plain String here would be escaped again by the first helper
      # that touches it.
      def self.sanitize(html, options = {})
        return nil if html.nil?
        SafeString.new(RH_SAFE_LIST_SANITIZER.sanitize(html.to_s, **options).to_s)
      end

      def self.strip_tags(html)
        return nil if html.nil?
        SafeString.new(RH_FULL_SANITIZER.sanitize(html.to_s).to_s)
      end

      # `auto_link` — the `rails_autolink` gem (campfire's Gemfile),
      # PORTED rather than declared: unlike the sanitizer it depends on
      # actionview, which this tree does not have and does not want.
      #
      # The two regexes below are the gem's, verbatim. They are a rule
      # table — which schemes count, which characters end a URL, what an
      # e-mail local part may contain — and deriving one from taste is
      # how you end up linking half a URL.
      #
      # Rails' `auto_link` SANITIZES by default and campfire does not
      # turn that off, which is why this file is where it lives: it
      # needs the real sanitizer above.
      AUTO_LINK_RE = %r{
          (?: ((?:ed2k|ftp|http|https|irc|mailto|news|gopher|nntp|telnet|webcal|xmpp|callto|feed|svn|urn|aim|rsync|tag|ssh|sftp|rtsp|afs|file):)// | www\.\w )
          [^\s< "]+
        }ix

      AUTO_EMAIL_LOCAL_RE = /[\w.!#\$%&'*\/=?^`{|}~+-]/
      AUTO_EMAIL_RE = /(?<!#{AUTO_EMAIL_LOCAL_RE})[\w.!#\$%+-]\.?#{AUTO_EMAIL_LOCAL_RE}*@[\w-]+(?:\.[\w-]+)+/
      AUTO_LINK_CRE = [ /<[^>]+$/, /^[^>]*>/, /<a\b.*?>/i, /<\/a>/i ].freeze
      AUTO_LINK_BRACKETS = { "]" => "[", ")" => "(", "}" => "{" }.freeze

      def self.auto_link(text, options = {}, &block)
        return SafeString.new("") if text.nil? || text.to_s.empty?

        link = (options[:link] || :all).to_sym
        html = options[:html] || {}
        do_sanitize = options[:sanitize] != false
        body = do_sanitize ? sanitize(text.to_s, options[:sanitize_options] || {}).to_s : text.to_s

        out = case link
        when :email_addresses then auto_link_email_addresses(body, html, do_sanitize, &block)
        when :urls            then auto_link_urls(body, html, do_sanitize, &block)
        else auto_link_email_addresses(
               auto_link_urls(body, html, do_sanitize, &block), html, do_sanitize, &block)
        end
        do_sanitize ? SafeString.new(out) : out
      end

      # Already inside a tag, or already inside an `<a>`. The gem's
      # test, unchanged: without it a URL in an existing `href` gets
      # linked a second time, inside itself.
      def self.auto_linked?(left, right)
        (left =~ AUTO_LINK_CRE[0] && right =~ AUTO_LINK_CRE[1]) ||
          (left.rindex(AUTO_LINK_CRE[2]) && $' !~ AUTO_LINK_CRE[3])
      end

      def self.auto_link_urls(text, html_options, do_sanitize)
        attrs = html_options.map { |k, v| [ k.to_s, v ] }.to_h
        text.gsub(AUTO_LINK_RE) do
          scheme = $1
          href = $&
          pre = $`
          post = $'
          if auto_linked?(pre, post)
            href
          else
            punctuation = []
            trailing_gt = ""
            # Trailing punctuation is not part of the URL — but a
            # closing bracket IS when the URL opened one, which is what
            # makes a wikipedia link survive.
            while href.sub!(/[^\p{Word}\/\-=;]$/, "")
              punctuation.push($&)
              opening = AUTO_LINK_BRACKETS[punctuation.last]
              if opening && href.scan(opening).size > href.scan(punctuation.last).size
                href << punctuation.pop
                break
              end
            end
            trailing_gt = $& if href.sub!(/&gt;$/, "")

            link_text = block_given? ? yield(href) : href
            href = "http://" + href if scheme.nil?
            if do_sanitize
              link_text = sanitize(link_text).to_s
              href = sanitize(href).to_s
            end
            content_tag(:a, link_text, attrs.merge("href" => href)) +
              punctuation.reverse.join("") + trailing_gt
          end
        end
      end

      def self.auto_link_email_addresses(text, html_options, do_sanitize)
        text.gsub(AUTO_EMAIL_RE) do
          address = $&
          pre = $`
          post = $'
          if auto_linked?(pre, post)
            address
          else
            display = block_given? ? yield(address) : address
            if do_sanitize
              address = sanitize(address).to_s
              display = sanitize(display).to_s unless display == address
            end
            mail_to(address, display, html_options)
          end
        end
      end
    end
  end
end
