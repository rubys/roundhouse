# Campfire next-iteration contracts — original verified, generic snapshot pending

Same unmodified Campfire `66883b6fb1eda402245247592e0af5f54105c5eb`,
lock/bundle/Ruby 3.4.11 and compiler83b6 controls as ROUND3.md. No app,
dependency, generic exporter or compiler edits. These are **original-only
contracts**, not newly supported families: round-three export still refuses
all five candidates, so **0 new original/Core/emitted family comparisons**.
Prior evidence and mutable mention/AR attribute/enum controls remain intact.

## Executed original assertions

Each case ran in a fresh ordinary Rails process, clean allowlisted environment,
actual pinned receiver/target objects, no production secrets, DB writes or DNS.
Expectations are literal/independent, not obtained from the generated output.
All cases passed: **64 assertions total**.

| Case | Assertions | Public contract / actual objects |
|---|---:|---|
| namespace_accessor | 12 | Opengraph::Location url/url=; nil/non-nil constructors, asymmetric assignments, setter return/storage identity, public parsed_url writer and non-public overridden parsed_url reader. No validation/URI/DNS call. |
| platform_inherited | 25 | Actual ApplicationPlatform: iPhone/Android/Macintosh/nil/iPad/mixed Android+iPhone × four predicates; inherited match? stays private. |
| delegate_current | 12 | Actual Current instance and two ActionDispatch::Request objects: prefixed host/protocol, nil→non-nil→different target→nil, nil ignores extra args/block, non-nil preserves ArgumentError, setter identity. |
| delegate_filter | 8 | Actual ActionText::Content::Filter + ActionText::Content/Nokogiri fragment: identity/text, private content reader, public generated fragment, target arity, nil DelegationError vs non-nil NoMethodError/name. |
| delegate_presentation | 7 | Actual Messages::AttachmentPresentation + ActionView::Base.empty: positional/keyword/block link_to forwarding, asymmetric URLs/labels, block once, nil context error, private context reader. Message is nil and unused; render is not a root. |

next_contract.rb accepts `original CASE` or `CORE_OR_EMITTED_LOADER CASE`.
Standalone mode loads **only that loader**, not Rails/gems, and rejects loaded
Rails/ActiveRecord. Heavy delegation targets must actually be admitted into
the standalone program or remain refused; no fake view/request/content target
will be added. Plain accessor input Strings stay mutable and are passed by
identity; no frozen-state shortcut. Source/owner/visibility/parameter metadata
and hashes are recorded separately in original outputs.

## Actual delegation declarations and generated bodies

There are exactly **three app declaration sites** (plus the platform_agent gem's
own delegation). No `delegate_missing_to` or explicit `private: true` site was
found in app/models, app/helpers, app/controllers or lib.

- app/models/current.rb:4: `delegate :host, :protocol, to: :request, prefix: true, allow_nil: true`.
- lib/rails_ext/filter.rb:23: `delegate :fragment, to: :content`.
- app/helpers/messages/attachment_presentation.rb:18:
  `delegate :tag, :link_to, :broadcast_image_tag, :rails_blob_path, :url_for, to: :context`.

**Verified visibility:** the latter two delegates are public even though their
declarations follow lexical `private`; their literal attr_reader targets remain
private. All delegates have rest/keyrest/block forwarding parameters. Do not
infer `private: true` semantics from declaration placement or invent a missing
private-delegate app case.

The bounded manifest captured these exact real generated bodies, not templates
copied into replacement methods:

```ruby
def request; @attributes[:request]; end
def request=(value); @attributes[:request] = value; end
def request_host(...)
  _ = request
  if !_.nil? || nil.respond_to?(:host)
    _.host(...)
  end
end
def fragment(...)
  _ = content
  _.fragment(...)
rescue ::NoMethodError => e
  if _.nil? && e.name == :fragment
    ::Kernel.raise ::ActiveSupport::DelegationError.nil_target(:fragment, :'content')
  else
    ::Kernel.raise
  end
end
```

Presentation link_to has the same rescue shape with target context. The receiver
is evaluated once and the target method must receive arguments/keywords/block;
nil handling must not turn a non-nil target's NoMethodError into DelegationError.
These are shared Ruby control-flow/exception/ownership issues, not per-DSL rules.

## Dependency chains and round-three boundaries

- **Namespaced generated accessor:** app/models/opengraph/location.rb:6 has
  `attr_accessor :url, :parsed_url`; own initialize at :10 simply assigns @url.
  url/url= owners are Opengraph::Location, not AR storage. The parsed_url reader
  is replaced by a private def at :41. Export refuses the simple-name gate.
- **Pure inherited helper:** platform_agent 1.0.1/lib/platform_agent.rb:4
  `initialize(user_agent_string)` calls `self.user_agent_string = user_agent_string`;
  :110 is private `attr_accessor :user_agent_string`; :112 is private
  `match?(pattern)` with body `user_agent_string.to_s.match?(pattern)`.
  Those exact owners are PlatformAgent; app ios?/android?/mobile?/desktop?
  owners are ApplicationPlatform. This cut needs no UserAgent parser, mutable
  snapshot, Rails runtime or AR storage. Export currently refuses inherited match?.
- **Current:** request/request= live in an anonymous generated module, host/
  protocol delegates in Current. Inherited ActiveSupport::CurrentAttributes
  initialize (:207) calls resolve_defaults (:239), which reads defaults and
  builds @attributes using each_with_object/Proc calls/dup. Export refuses
  omitted initializer with @attributes dependency. Do not snapshot the actual
  defaults Hash to bypass it. Actual request target host/protocol owners are
  ActionDispatch::Http::URL (:325/:299); host calls raw_host_with_port (:313),
  which uses headers/forwarded host, and protocol calls ssl?. Real Request own
  initializer (:61) and Rack/HTTP helpers are further reachable dependencies.
- **Filter:** own positional initializer stores actual Content, private content
  accessor, public generated fragment. Content#fragment is its literal native
  accessor (:27); Content initialize (:41) has optional arguments and wraps
  ActionText::Fragment/Nokogiri. Export currently refuses the namespaced root.
- **Presentation:** own required positional + keyword constructor (:2) stores
  context/message. Real target link_to is ActionView::Helpers::NavigationHelper
  (:271), not Base's own method; it uses optional arguments/block, converts
  options, computes url_target and calls content_tag. This is not a one-method
  view-runtime claim. Export currently refuses the namespaced root.

Current/presentation first class load is captured after ordinary initialization;
initialization is outside. Location/platform use ordinary initialize/eager-load
outside capture. Filter's first extension load initially failed before Rails
initialization at ActionText::Content::ContentHelper. Diagnosed as dependency
bootstrap, **not observer failure**. The manifest now loads the real locked
gem helper and ActionText::Content outside capture, captures only first require
of lib/rails_ext/filter, then initializes ordinarily outside capture. This
captures actual fragment delegation and reaches the root-name refusal. Both
initial failure and retry evidence are retained. No app class is reloaded.
Native collector failures are zero in all attempts.

## Rerun

Use the clean environment in ROUND3.md/RESULTS.md and the same bundle/DB. From
campfire-run, with CORE_RUBY_APP set and RUBYLIB allowlisted:

```sh
bundle exec ruby /home/user/workspace/campfire-core-probe/tools/boot-to-core/campfire/next_contract.rb original namespace_accessor
CAMPFIRE_CORE_LANE=namespace_accessor bundle exec ruby /home/user/workspace/campfire-core-probe/bin/rh materialize --trust-boot /home/user/workspace/campfire-core-probe/tools/boot-to-core/campfire/input.rb -o NEW_OUT
# Repeat with platform_inherited/delegate_current/delegate_filter/delegate_presentation.
# Only after successful materialization: strict check, Core contract, emit Ruby,
# fresh emitted contract. Do not count a refused or merely captured family.
```

Owned changes: input.rb, next_contract.rb, NEXT.md. The previous bounded runner
and contracts are unchanged and retain mention/AR controls. Next exact generic
snapshot/compiler identity must be verified before any new support claim.
