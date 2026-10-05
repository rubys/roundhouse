//! Ruby stdlib singletons (`File`/`Dir`/`SecureRandom`/…), the
//! `Rails`/`Time`/`Date`/`DateTime` singletons, and the gem-ecosystem
//! catalog fold. Extracted verbatim from `Analyzer::with_adapter`.

use std::collections::HashMap;

use crate::analyze::ClassInfo;
use crate::ident::{ClassId, Symbol};
use crate::ty::Ty;

pub(in crate::analyze) fn register(classes: &mut HashMap<ClassId, ClassInfo>) {
    // Rails singleton — `Rails.application` / `Rails.logger` /
    // `Rails.cache` / `Rails.env` / `Rails.root` are pervasive
    // call shapes in real Rails code. Each maps to a runtime
    // object that's not modeled structurally here; return
    // `Ty::Untyped` (gradual escape) so method chains off them
    // propagate through dispatch without bottoming out at Var.
    // `Rails.env` is the one we can type concretely as Str.
    let mut rails_cls = ClassInfo::default();
    for m in ["application", "logger", "cache", "configuration", "root"] {
        rails_cls.class_methods.insert(Symbol::from(m), Ty::Untyped);
    }
    // `Rails.env` is an ActiveSupport::StringInquirer (a String
    // that also answers `development?`/`production?`/… as Bool),
    // not a plain Str — see the StringInquirer dispatch in send.rs.
    rails_cls.class_methods.insert(
        Symbol::from("env"),
        Ty::Class {
            id: ClassId(Symbol::from("ActiveSupport::StringInquirer")),
            args: vec![],
        },
    );
    classes.insert(ClassId(Symbol::from("Rails")), rails_cls);

    // `ActionController::BrowserBlocker.blocked?(user_agent, floors)` —
    // the gate `ingest::allow_browser` synthesizes into a controller
    // body for `allow_browser`, answered by
    // runtime/ruby/action_controller/browser_blocker.rb. Registered
    // here because a runtime `.rbs` does not feed app analysis, and an
    // unresolved send in a controller body is a dispatch error, not a
    // gradual escape.
    let mut blocker = ClassInfo::default();
    blocker.class_methods.insert(Symbol::from("blocked?"), Ty::Bool);
    classes.insert(ClassId(Symbol::from("ActionController::BrowserBlocker")), blocker);

    // `ActionController::RateLimiter.exceeded?(key, within, to)` — the
    // counter behind `rate_limit`, called from the filter method
    // `ingest::rate_limit` synthesizes; runtime/ruby/action_controller/
    // rate_limiter.rb answers it. Registered for the same reason.
    let mut limiter = ClassInfo::default();
    limiter.class_methods.insert(Symbol::from("exceeded?"), Ty::Bool);
    classes.insert(ClassId(Symbol::from("ActionController::RateLimiter")), limiter);

    // Time singleton — `Time.now` (Ruby core) / `Time.current`
    // (Rails) / `Time.at` all yield a Time *value*, and `Time.zone`
    // is a TimeZone whose `.now`/`.at`/`.local` likewise yield Time,
    // so modeling it as Time too lets those chains resolve. Time
    // values are already modeled structurally (`time_method` in
    // send.rs) and AR datetime columns type as Time, so these
    // constructors join that same surface — `Time.now.to_i` → Int,
    // `Time.current.utc` → Time — instead of bottoming out at the
    // `Untyped` gradual escape (and dragging every chained call into
    // it). `Time - x` arithmetic still resolves to `Untyped` inside
    // `time_method` because receiver-only dispatch can't tell a
    // Duration arg (→ Time) from a Time arg (→ Float).
    let time_ty = || Ty::Class {
        id: ClassId(Symbol::from("Time")),
        args: vec![],
    };
    let mut time_cls = ClassInfo::default();
    time_cls.class_methods.insert(Symbol::from("current"), time_ty());
    time_cls.class_methods.insert(Symbol::from("now"), time_ty());
    time_cls.class_methods.insert(Symbol::from("zone"), time_ty());
    time_cls.class_methods.insert(Symbol::from("at"), time_ty());
    classes.insert(ClassId(Symbol::from("Time")), time_cls);

    // Date instances are date-only values, including constructors and
    // parsers. The send dispatcher validates their argument contracts;
    // do not duplicate unvalidated return declarations here. DateTime
    // keeps its existing timestamp representation.
    classes.insert(ClassId(Symbol::from("Date")), ClassInfo::default());
    for name in ["DateTime"] {
        let mut cls = ClassInfo::default();
        cls.class_methods.insert(Symbol::from("current"), Ty::Untyped);
        cls.class_methods.insert(Symbol::from("today"), Ty::Untyped);
        cls.class_methods.insert(Symbol::from("now"), Ty::Untyped);
        // The parse family has a concrete answer where `current` /
        // `today` / `now` above do not yet: `Ty::Time` is roundhouse's
        // timestamp type. Upgrading the three older
        // entries would move types in every app that calls them, so it
        // is deliberately not part of this change.
        for parser in ["parse", "strptime", "iso8601", "civil"] {
            cls.class_methods.insert(Symbol::from(parser), Ty::Time);
        }
        classes.insert(ClassId(Symbol::from(name)), cls);
    }

    // Ruby stdlib singletons + Set — referenced by ~every Rails app but
    // not structurally modeled. Register the common call surface so
    // `File.read`, `SecureRandom.hex`, `CGI.escape`, `Set#<<` resolve to
    // a return type instead of "no known method". Return types follow
    // the official rbs gem core/stdlib signatures, narrowed to the
    // concrete cases; opaque/handle returns (`File.open`, `URI.parse`)
    // and unparameterized collection elements degrade to `Untyped` (the
    // gradual escape) so chained calls still flow. Hardcoded like the
    // Rails/Time/Date blocks above — `register_stdlib_class` never
    // clobbers an app-defined method/class of the same name.
    let str_arr = || Ty::Array { elem: Box::new(Ty::Str) };
    register_stdlib_class(classes, "SecureRandom", &[
        ("hex", Ty::Str), ("base64", Ty::Str), ("urlsafe_base64", Ty::Str),
        ("base58", Ty::Str), ("uuid", Ty::Str), ("alphanumeric", Ty::Str),
        ("random_bytes", Ty::Str), ("random_number", Ty::Untyped),
    ], &[]);
    // `Random` carries the same `Random::Formatter` surface as
    // SecureRandom since Ruby 3.3 (`Random.uuid` is campfire's test
    // notification body); `rand`/`random_number` answer Int or Float
    // by argument, so they stay gradual.
    register_stdlib_class(classes, "Random", &[
        ("uuid", Ty::Str), ("hex", Ty::Str), ("base64", Ty::Str),
        ("urlsafe_base64", Ty::Str), ("alphanumeric", Ty::Str),
        ("bytes", Ty::Str), ("random_bytes", Ty::Str),
        ("rand", Ty::Untyped), ("random_number", Ty::Untyped),
        ("new_seed", Ty::Int), ("seed", Ty::Int),
    ], &[
        ("rand", Ty::Untyped), ("bytes", Ty::Str), ("seed", Ty::Int),
    ]);
    // `Turbo::StreamsChannel` — turbo-rails' CLASS-side broadcast API,
    // the one an app reaches for when the payload is not a record's
    // partial: a rendered component, a counter, a status panel. The
    // record-side surface is registered on every model (it is mixed
    // into `ActiveRecord::Base`); this is the same five actions with
    // the stream, target and markup passed explicitly. They answer the
    // broadcast, which nothing consumes: `Nil`.
    register_stdlib_class(classes, "Turbo::StreamsChannel", &[
        ("broadcast_append_to", Ty::Nil), ("broadcast_prepend_to", Ty::Nil),
        ("broadcast_replace_to", Ty::Nil), ("broadcast_update_to", Ty::Nil),
        ("broadcast_remove_to", Ty::Nil),
    ], &[]);
    // `ENV` — a Rails app's configuration seam, and the one stdlib
    // object whose values have a single concrete type: every entry is
    // a String. `fetch` narrows to that, which is right for the two
    // forms an app writes (`fetch(key)` and `fetch(key, "default")`)
    // and a narrowing for the rarer `fetch(key, 3000)` / block forms —
    // the same trade the neighbours above make.
    let str_or_nil = || Ty::Union { variants: vec![Ty::Str, Ty::Nil] };
    register_stdlib_class(classes, "ENV", &[
        ("fetch", Ty::Str), ("[]", str_or_nil()), ("[]=", Ty::Str),
        ("key?", Ty::Bool), ("has_key?", Ty::Bool), ("include?", Ty::Bool),
        ("member?", Ty::Bool), ("key", str_or_nil()), ("delete", str_or_nil()),
        ("keys", str_arr()), ("values", str_arr()),
        ("to_h", Ty::Hash { key: Box::new(Ty::Str), value: Box::new(Ty::Str) }),
        ("replace", Ty::Hash { key: Box::new(Ty::Str), value: Box::new(Ty::Str) }),
    ], &[]);
    // `Pathname` — `Rails.root.join(...)` territory. The path-returning
    // methods answer a Pathname so a chain stays typed to its `to_s`.
    let pathname = || Ty::Class { id: ClassId(Symbol::from("Pathname")), args: vec![] };
    register_stdlib_class(classes, "Pathname", &[
        ("new", pathname()), ("pwd", pathname()), ("getwd", pathname()),
    ], &[
        ("relative_path_from", pathname()), ("join", pathname()),
        ("basename", pathname()), ("dirname", pathname()), ("expand_path", pathname()),
        ("parent", pathname()), ("realpath", pathname()),
        ("to_s", Ty::Str), ("to_path", Ty::Str), ("extname", Ty::Str),
        ("read", Ty::Str), ("write", Ty::Int),
        ("exist?", Ty::Bool), ("file?", Ty::Bool), ("directory?", Ty::Bool),
        ("children", Ty::Array { elem: Box::new(pathname()) }),
    ]);
    // `ActiveSupport::SecurityUtils.secure_compare` — the constant-time
    // comparison every hand-rolled token or basic-auth check calls.
    register_stdlib_class(classes, "ActiveSupport::SecurityUtils", &[
        ("secure_compare", Ty::Bool), ("fixed_length_secure_compare", Ty::Bool),
    ], &[]);
    register_stdlib_class(classes, "File", &[
        ("read", Ty::Str), ("binread", Ty::Str), ("write", Ty::Int),
        ("exist?", Ty::Bool), ("exists?", Ty::Bool), ("file?", Ty::Bool),
        ("directory?", Ty::Bool), ("open", Ty::Untyped),
        ("unlink", Ty::Int), ("delete", Ty::Int), ("rename", Ty::Int),
        ("join", Ty::Str), ("basename", Ty::Str), ("dirname", Ty::Str),
        ("extname", Ty::Str), ("expand_path", Ty::Str), ("size", Ty::Int),
    ], &[]);
    register_stdlib_class(classes, "Dir", &[
        ("entries", str_arr()), ("glob", str_arr()), ("[]", str_arr()),
        ("exist?", Ty::Bool), ("exists?", Ty::Bool), ("mkdir", Ty::Int),
        ("pwd", Ty::Str), ("home", Ty::Str),
    ], &[]);
    register_stdlib_class(classes, "Math", &[
        ("sqrt", Ty::Float), ("cbrt", Ty::Float), ("log", Ty::Float),
        ("log2", Ty::Float), ("log10", Ty::Float), ("exp", Ty::Float),
        ("sin", Ty::Float), ("cos", Ty::Float), ("tan", Ty::Float),
        ("atan", Ty::Float), ("atan2", Ty::Float), ("hypot", Ty::Float),
        ("pow", Ty::Float),
    ], &[]);
    register_stdlib_class(classes, "CGI", &[
        ("escape", Ty::Str), ("unescape", Ty::Str),
        ("escapeHTML", Ty::Str), ("unescapeHTML", Ty::Str),
        ("escape_html", Ty::Str), ("unescape_html", Ty::Str),
    ], &[]);
    register_stdlib_class(classes, "ERB::Util", &[
        ("html_escape", Ty::Str), ("h", Ty::Str),
        ("url_encode", Ty::Str), ("u", Ty::Str), ("json_escape", Ty::Str),
    ], &[]);
    for digest in ["Digest::MD5", "Digest::SHA1", "Digest::SHA256"] {
        register_stdlib_class(classes, digest, &[
            ("hexdigest", Ty::Str), ("digest", Ty::Str),
            ("base64digest", Ty::Str),
        ], &[]);
    }
    // `useragent` + `platform_agent`, now PORTED into
    // `runtime/ruby/user_agent.rb` rather than stubbed. Method lists are
    // that file's `.rbs` verbatim, so the analyzer and a strict target
    // agree; `PlatformAgent` is here because `ApplicationPlatform <
    // PlatformAgent` names it at LOAD time, and without the analyzer
    // knowing the pair campfire's `UserAgent.parse(ua).browser` in a
    // partial read out as `no known method parse on Class {
    // UserAgent }`.
    let user_agent_ty = Ty::Class { id: ClassId(Symbol::from("UserAgent")), args: vec![] };
    register_stdlib_class(classes, "UserAgent", &[
        ("parse", user_agent_ty.clone()),
    ], &[
        ("browser", Ty::Str), ("platform", Ty::Str), ("version", Ty::Str),
        ("os", Ty::Str),
    ]);
    register_stdlib_class(classes, "PlatformAgent", &[], &[
        ("browser", Ty::Str), ("version", Ty::Str), ("os", Ty::Str),
        ("match?", Ty::Bool), ("user_agent", user_agent_ty),
    ]);
    // `typeid`, PORTED into `runtime/ruby/typeid.rb` as a module whose
    // `new` answers a plain String (the gem's `TypeID < String` is a
    // builtin subclass the strict targets do not carry). Registered so
    // lobsters' Token concern — `self.token ||= TypeID.new(prefix)` —
    // types its token as the String column it is written into.
    register_stdlib_class(classes, "TypeID", &[("new", Ty::Str)], &[]);
    // `ActionText::ContentHelper.allowed_attributes`, from
    // `runtime/ruby/action_text.rbs` — the one façade reader campfire's
    // `ContentFilters::SanitizeAttributes` needs typed:
    //
    //     ActionText::ContentHelper.allowed_attributes ||
    //       (sanitizer_class.allowed_attributes +
    //        ActionText::Attachment::ATTRIBUTES).to_a
    //
    // The right arm goes through a CLASS OBJECT
    // (`ContentHelper.sanitizer.class`), and neither we nor spinel can
    // dispatch statically on one — our `Ty` has no singleton and
    // spinel's `sp_Class` is dynamic. So the right arm is
    // `Array[untyped]` whatever we do here, and the result was handed
    // to a `sanitize` this same file declares takes `Array[String]`.
    // Two descriptions of one list, contradicting, and on spinel the
    // contradiction surfaces as far from its cause as it can:
    // `incompatible pointer types passing 'sp_PolyArray *' to parameter
    // of type 'sp_StrArray *'`, from the C compiler, with the campfire
    // binary failing to LINK.
    //
    // Typing the LEFT arm is what closes it, because `||` now folds to
    // a left that cannot be falsy — see `analyze::body`. The runtime
    // answers Rails' computed list rather than Rails' unconfigured
    // `nil`; that divergence is in `docs/pipeline/runtime.md`, and it
    // is why this entry is here rather than the whole module.
    //
    // `runtime/ruby/**/*.rbs` does not feed app analysis — only the
    // runtime-sweep test builds a registry from it — so a façade the
    // app calls has to be named here to be known.
    register_stdlib_class(classes, "ActionText::ContentHelper", &[
        ("allowed_attributes", Ty::Array { elem: Box::new(Ty::Str) }),
    ], &[]);
    // `IPAddr` — the class `runtime/ruby/ipaddr.rb` ports. Only the
    // surface that file implements is registered, so a call beyond it
    // stays an honest gap rather than a method that types and then
    // raises. `IPAddr.new(x)` reaches the universal `.new`, so the
    // predicates are looked up as INSTANCE methods on the result.
    // `Regexp` and `MatchData` — Ruby's own, and the pair a PORT needs
    // most. `runtime/ruby/user_agent.rb` tokenizes with
    // `MATCHER.match(s)` and then reads `m[0]`, and with `match`
    // unregistered every read off `m` was gradual: 45 untyped
    // sub-expressions from one method, which is what the
    // `Ty::Untyped` ratchet in `tests/runtime_src_integration.rs` is
    // there to catch.
    //
    // ONLY the surface both lanes implement, the same rule IPAddr and
    // Net::HTTP follow. `MatchData#[]` answers `String?` because a
    // group that did not participate is nil, and pretending otherwise
    // would hand a strict target a non-null it has to trust.
    let match_data = Ty::Class { id: ClassId(Symbol::from("MatchData")), args: vec![] };
    let str_or_nil_m = Ty::Union { variants: vec![Ty::Str, Ty::Nil] };
    register_stdlib_class(classes, "Regexp", &[], &[
        ("match", Ty::Union { variants: vec![match_data.clone(), Ty::Nil] }),
        ("match?", Ty::Bool),
        ("source", Ty::Str),
    ]);
    register_stdlib_class(classes, "MatchData", &[], &[
        ("[]", str_or_nil_m.clone()),
        ("to_s", Ty::Str),
        ("pre_match", Ty::Str),
        ("post_match", Ty::Str),
        ("captures", Ty::Array { elem: Box::new(str_or_nil_m) }),
        ("size", Ty::Int),
        ("length", Ty::Int),
    ]);
    // `Zlib` — `crc32` is the whole ported surface (see
    // `runtime/ruby/zlib.rb`); registering only it keeps a call to
    // `Zlib.deflate` an honest gap instead of a method that types and
    // then fails to resolve.
    register_stdlib_class(classes, "Zlib", &[
        ("crc32", Ty::Int),
    ], &[]);
    register_stdlib_class(classes, "IPAddr", &[], &[
        ("ipv4?", Ty::Bool), ("ipv6?", Ty::Bool), ("ipv4_mapped?", Ty::Bool),
        ("loopback?", Ty::Bool), ("private?", Ty::Bool), ("link_local?", Ty::Bool),
        ("to_s", Ty::Str),
        ("octets", Ty::Array { elem: Box::new(Ty::Int) }),
    ]);
    // `Surfguard` — basecamp/surfguard's SSRF address policy, ported
    // into `runtime/ruby/surfguard.rb`. Class methods only: the module
    // has no instances. Registered to the same rule as IPAddr and
    // Zlib — ONLY the surface the port implements, so a call to one of
    // the three URL-taking entry points the port declines
    // (`enforce_public_ip` and friends) stays an honest gap instead of
    // typing clean and resolving to nothing.
    register_stdlib_class(classes, "Surfguard", &[
        ("resolve_public_ips", Ty::Array { elem: Box::new(Ty::Str) }),
        ("blocked_address?", Ty::Bool),
    ], &[]);
    // `Concurrent` — concurrent-ruby's thread pools and barrier, ported
    // into `runtime/spinel/concurrent.rb` (the ruby family runs the gem).
    // ONLY the surface the port implements, the IPAddr rule: campfire's
    // `WebPush::Pool` posts, shuts down, kills and waits on its two
    // pools, and its suite reads `completed_task_count`. `FixedThreadPool`
    // is the same surface — it IS a `ThreadPoolExecutor` in the gem and
    // in the port — registered under its own name because the universal
    // `.new` yields `Class { id: <name> }` and the registry has no
    // subclass walk for stdlib entries.
    for pool in ["Concurrent::ThreadPoolExecutor", "Concurrent::FixedThreadPool"] {
        register_stdlib_class(classes, pool, &[], &[
            ("post", Ty::Bool),
            ("shutdown", Ty::Nil),
            ("kill", Ty::Nil),
            ("wait_for_termination", Ty::Bool),
            ("completed_task_count", Ty::Int),
            ("length", Ty::Int),
            ("queue_length", Ty::Int),
            ("max_length", Ty::Int),
            ("running?", Ty::Bool),
            ("shutdown?", Ty::Bool),
        ]);
    }
    register_stdlib_class(classes, "Concurrent::CyclicBarrier", &[], &[
        ("wait", Ty::Bool),
        ("parties", Ty::Int),
        ("number_waiting", Ty::Int),
    ]);
    // `Net::HTTP` — a real client on BOTH lanes: CRuby's own stdlib, and
    // spinel's `packages/net` (HTTPS included, since the openssl package
    // landed). So there is nothing to port here, unlike IPAddr — only the
    // types, plus the `require "net/http"` that `project::BUNDLED` writes.
    //
    // ONLY THE SURFACE BOTH LANES IMPLEMENT IS REGISTERED. spinel's
    // client is a declared subset — no keep-alive, no proxy, no redirect
    // following, and no STREAMING body (`#request` with a block,
    // `#read_body` with a block) — so those names are deliberately absent
    // here even though CRuby has them. Registering them would make
    // `Opengraph::Fetch` type clean and fail at run time on spinel, which
    // is worse than the honest gap it currently reports. Same rule as
    // IPAddr: a call beyond the implemented surface stays a gap.
    let http_response = Ty::Class {
        id: ClassId(Symbol::from("Net::HTTPResponse")),
        args: vec![],
    };
    let str_or_nil = Ty::Union { variants: vec![Ty::Str, Ty::Nil] };
    register_stdlib_class(classes, "Net::HTTP", &[
        ("get", Ty::Str),
        ("get_response", http_response.clone()),
        ("post_form", http_response.clone()),
    ], &[
        // Writers answer the value assigned. `use_ssl=` really is a bool
        // (`uri.scheme == "https"`); the timeouts take whatever the app
        // hands them — campfire assigns an ActiveSupport::Duration — so
        // asserting Int there would be a narrower answer than the truth.
        ("use_ssl=", Ty::Bool),
        ("open_timeout=", Ty::Untyped),
        ("read_timeout=", Ty::Untyped),
        ("use_ssl?", Ty::Bool), ("started?", Ty::Bool),
        ("address", Ty::Str), ("port", Ty::Int),
        ("finish", Ty::Nil),
        ("request", http_response.clone()),
        ("get", http_response.clone()),
        ("post", http_response.clone()),
    ]);
    // The EXCEPTION FAMILY. A `rescue => e` binds `e` as
    // `Class{StandardError}` (see `analyze::body`'s rescue arm) and a
    // `rescue Foo => e` binds the class named — and nothing registered
    // what any of them ANSWER, so the single most common thing anyone
    // does with a rescued exception, `e.message`, typed as a gap.
    //
    // It surfaced twice in one afternoon: once in a runtime file, where
    // `tests/runtime_src_integration.rs` refuses an untyped
    // sub-expression, and once in a gem façade whose whole job was to
    // report an exception. Registering the shape is cheaper than either
    // workaround, and it is the same shape for every member.
    //
    // The registry does no inheritance for stdlib classes, so each name
    // carries the surface — the same reason the `Net::HTTP::<verb>`
    // loop below repeats itself. The list is Ruby's own hierarchy under
    // `StandardError` plus `Exception` itself, which `rescue Exception`
    // names and campfire's `MessagesHelper` actually writes.
    for exc in [
        "Exception", "StandardError", "RuntimeError", "ArgumentError",
        "TypeError", "NameError", "NoMethodError", "IndexError",
        "KeyError", "RangeError", "IOError", "NotImplementedError",
        "FrozenError", "ZeroDivisionError", "StopIteration",
        // Both CRuby's bundled libraries and Spinel's uri/net packages
        // define these exception classes; emitted requires load them.
        "URI::InvalidURIError", "Net::OpenTimeout", "Net::ReadTimeout",
        "OpenSSL::OpenSSLError", "JSON::ParserError",
    ] {
        register_stdlib_class(classes, exc, &[], &[
            ("message", Ty::Str),
            ("to_s", Ty::Str),
            ("full_message", Ty::Str),
            ("inspect", Ty::Str),
            ("backtrace", Ty::Array { elem: Box::new(Ty::Str) }),
        ]);
    }
    let exception_surface = [
        ("message", Ty::Str),
        ("to_s", Ty::Str),
        ("full_message", Ty::Str),
        ("inspect", Ty::Str),
        ("backtrace", Ty::Array { elem: Box::new(Ty::Str) }),
    ];
    for (exc, extra) in [
        ("ActiveRecord::RecordNotFound", None),
        ("ActiveRecord::RecordNotUnique", None),
        ("ActiveRecord::ValueTooLong", None),
        // Not `ActiveRecord::Base`: no instance surface is registered there, so `e.record.errors` would still fail.
        ("ActiveRecord::RecordInvalid", Some(("record", Ty::Untyped))),
        ("ActionController::ParameterMissing", Some(("param", Ty::Str))),
        ("ActionController::UnpermittedParameters", None),
        ("ActionController::UnknownFormat", None),
    ] {
        let mut methods = exception_surface.to_vec();
        methods.extend(extra);
        let methods: Vec<(&str, Ty)> = methods;
        register_stdlib_class(classes, exc, &[], &methods);
    }
    // `ActiveModel::Type::Boolean.new.cast(v)`: nil for a blank value, else the boolean.
    register_stdlib_class(classes, "ActiveModel::Type::Boolean", &[], &[
        ("cast", Ty::Union { variants: vec![Ty::Bool, Ty::Nil] }),
    ]);
    // Not a typed store: a thread-local slot holds whatever the caller put there, so `[]` answers untyped.
    let thread = Ty::Class { id: ClassId(Symbol::from("Thread")), args: vec![] };
    register_stdlib_class(classes, "Thread", &[("current", thread.clone())], &[
        ("[]", Ty::Untyped),
        ("[]=", Ty::Untyped),
    ]);
    // The spinel `csv` package's writer surface: `CSV.generate { |csv| csv << row }` answers the accumulated String.
    let csv = Ty::Class { id: ClassId(Symbol::from("CSV")), args: vec![] };
    register_stdlib_class(classes, "CSV", &[("generate", Ty::Str), ("generate_line", Ty::Str)], &[
        ("<<", csv.clone()),
        ("add_row", csv.clone()),
        ("string", Ty::Str),
    ]);
    // The response. `code` is a String here as it is in CRuby ("200",
    // not 200) — campfire compares `response.code == "200"`, which folds
    // to a constant false against an Int.
    register_stdlib_class(classes, "Net::HTTPResponse", &[], &[
        ("code", Ty::Str), ("body", Ty::Str), ("message", Ty::Str),
        ("http_version", Ty::Str),
        ("content_type", str_or_nil.clone()),
        ("[]", str_or_nil.clone()),
        ("key?", Ty::Bool),
    ]);
    // The request family. CRuby builds these as `Net::HTTP::Post.new(…)`
    // and the surface lives on a shared parent; the registry does no
    // inheritance for stdlib classes, so each verb carries it.
    for verb in ["Get", "Post", "Put", "Delete", "Head"] {
        register_stdlib_class(classes, &format!("Net::HTTP::{verb}"), &[], &[
            ("body=", Ty::Str), ("body", Ty::Str),
            ("[]=", Ty::Str), ("[]", str_or_nil.clone()),
            ("content_type=", Ty::Str),
            ("set_form_data", Ty::Str),
            ("method", Ty::Str), ("path", Ty::Str),
        ]);
    }
    // `Mime::Type` — the class `runtime/ruby/mime.rb` ports from
    // actionpack. `lookup` is deliberately NON-nilable: upstream answers
    // a Type for any well-formed string (symbol nil when unregistered)
    // and raises for the rest, which is what makes
    // `if t = Mime::Type.lookup(ct)` a safe guard in app code. Giving it
    // a `| Nil` arm here would be a lie the type system can act on.
    let mime_type = Ty::Class { id: ClassId(Symbol::from("Mime::Type")), args: vec![] };
    register_stdlib_class(classes, "Mime::Type", &[
        ("lookup", mime_type.clone()),
        ("lookup_by_extension", Ty::Union {
            variants: vec![mime_type.clone(), Ty::Nil],
        }),
        ("valid?", Ty::Bool),
    ], &[
        ("symbol", Ty::Union { variants: vec![Ty::Sym, Ty::Nil] }),
        ("to_sym", Ty::Union { variants: vec![Ty::Sym, Ty::Nil] }),
        ("to_s", Ty::Str), ("to_str", Ty::Str), ("inspect", Ty::Str),
        ("hash", Ty::Int), ("==", Ty::Bool), ("eql?", Ty::Bool),
    ]);
    // `URI.parse` returns a URI object we don't model; `Untyped` lets
    // chained `.scheme` / `.host` flow gradually instead of erroring.
    register_stdlib_class(classes, "URI", &[
        ("parse", Ty::Untyped), ("join", Ty::Untyped),
        ("escape", Ty::Str), ("unescape", Ty::Str),
        ("encode_www_form", Ty::Str), ("decode_www_form", Ty::Untyped),
    ], &[]);
    // A class test such as `URI.parse(url).is_a?(URI::HTTP)` names the
    // real bundled class, without claiming any extra instance methods.
    register_stdlib_class(classes, "URI::HTTP", &[], &[]);
    for response in ["Net::HTTPRedirection", "Net::HTTPOK"] {
        register_stdlib_class(classes, response, &[], &[]);
    }
    // The implementation is bundled on Ruby/Spinel. Other targets
    // report the missing runtime at project emission.
    let string_io = Ty::Class { id: ClassId(Symbol::from("StringIO")), args: vec![] };
    register_stdlib_class(classes, "StringIO", &[], &[
        ("string", Ty::Str), ("<<", string_io),
    ]);
    // JSON dispatch is already intrinsic in BodyTyper and the emitters;
    // a source-backed reference must also recognize its exact namespace.
    register_stdlib_class(classes, "JSON", &[], &[]);
    // Ruby and Spinel provide both core classes. The existing constructor
    // rule carries Struct.new's value into an assigned constant, so reads
    // of that constant and Mutex.new retain their source expressions.
    // Do not invent member or synchronization return types here.
    register_stdlib_class(classes, "Struct", &[], &[]);
    register_stdlib_class(classes, "Mutex", &[], &[]);
    // `Array.wrap` is folded by `lower::enumerable_ext` before emit.
    // Registered so the analyzer does not report it as unknown. The
    // element type is not known from a scalar argument.
    register_stdlib_class(
        classes,
        "Array",
        &[("wrap", Ty::Array { elem: Box::new(Ty::Untyped) })],
        &[],
    );
    // CRuby supplies Sets here, the Spinel port supplies Arrays. Both
    // implement the collection operations the app uses; don't invent
    // one concrete representation for the two runtimes.
    register_stdlib_class(classes, "Rails::HTML5::SafeListSanitizer", &[
        ("allowed_tags", Ty::Untyped), ("allowed_attributes", Ty::Untyped),
    ], &[]);
    // `Set` is a value type: `Set.new` yields `Class { Set }` (via the
    // universal `.new`), then these instance methods dispatch on it.
    // Mutators return the receiver (self) for chaining; element-typed
    // accessors are `Untyped` (Set isn't parameterized here).
    let set_self = Ty::Class { id: ClassId(Symbol::from("Set")), args: vec![] };
    // Enumerable's filters and sorts answer an Array, not a Set.
    let untyped_array = Ty::Array { elem: Box::new(Ty::Untyped) };
    register_stdlib_class(classes, "Set", &[("[]", set_self.clone())], &[
        ("<<", set_self.clone()), ("add", set_self.clone()),
        ("delete", set_self.clone()), ("merge", set_self.clone()),
        ("subtract", set_self.clone()), ("clear", set_self.clone()),
        ("keep_if", set_self.clone()), ("delete_if", set_self.clone()),
        ("|", set_self.clone()), ("&", set_self.clone()),
        ("-", set_self.clone()), ("+", set_self.clone()), ("^", set_self.clone()),
        ("union", set_self.clone()), ("intersection", set_self.clone()),
        ("difference", set_self.clone()),
        ("dup", set_self.clone()), ("to_set", set_self.clone()),
        ("add?", Ty::Untyped),
        ("delete?", Ty::Union { variants: vec![set_self.clone(), Ty::Nil] }),
        ("each", Ty::Untyped),
        ("map", untyped_array.clone()), ("flat_map", untyped_array.clone()),
        ("filter_map", untyped_array.clone()),
        ("select", untyped_array.clone()), ("filter", untyped_array.clone()),
        ("reject", untyped_array.clone()),
        ("sort", untyped_array.clone()), ("sort_by", untyped_array.clone()),
        ("partition", Ty::Array { elem: Box::new(untyped_array.clone()) }),
        ("to_a", untyped_array.clone()),
        ("find", Ty::Untyped), ("detect", Ty::Untyped),
        ("first", Ty::Untyped),
        ("max", Ty::Untyped), ("min", Ty::Untyped),
        ("max_by", Ty::Untyped), ("min_by", Ty::Untyped),
        ("sum", Ty::Untyped), ("inject", Ty::Untyped), ("reduce", Ty::Untyped),
        ("each_with_object", Ty::Untyped),
        ("group_by", Ty::Hash {
            key: Box::new(Ty::Untyped),
            value: Box::new(untyped_array.clone()),
        }),
        ("include?", Ty::Bool), ("member?", Ty::Bool), ("empty?", Ty::Bool),
        ("any?", Ty::Bool), ("all?", Ty::Bool), ("none?", Ty::Bool), ("one?", Ty::Bool),
        ("intersect?", Ty::Bool), ("disjoint?", Ty::Bool),
        ("exclude?", Ty::Bool),
        ("size", Ty::Int), ("length", Ty::Int), ("count", Ty::Int),
        ("subset?", Ty::Bool), ("superset?", Ty::Bool),
    ]);

    // Gem / ecosystem catalog (`crate::catalog::gems`). Targeting
    // Rails realistically means targeting its gem ecosystem;
    // rather than enumerate every gem, we register the surface
    // apps actually call (Arel, ROTP, Nokogiri, …) by discovery.
    // Registered like the stdlib singletons — `or_insert`, so a
    // user class of the same name still wins.
    for gem in crate::catalog::GEM_CATALOG {
        let class_methods: Vec<(&str, Ty)> =
            gem.class_methods.iter().map(|(n, k)| (*n, k.to_ty())).collect();
        let instance_methods: Vec<(&str, Ty)> =
            gem.instance_methods.iter().map(|(n, k)| (*n, k.to_ty())).collect();
        register_stdlib_class(classes, gem.name, &class_methods, &instance_methods);
    }
}

fn register_stdlib_class(
    classes: &mut HashMap<ClassId, ClassInfo>,
    name: &str,
    class_methods: &[(&str, Ty)],
    instance_methods: &[(&str, Ty)],
) {
    let cls = classes.entry(ClassId(Symbol::from(name))).or_default();
    for (m, ty) in class_methods {
        cls.class_methods
            .entry(Symbol::from(*m))
            .or_insert_with(|| ty.clone());
    }
    for (m, ty) in instance_methods {
        cls.instance_methods
            .entry(Symbol::from(*m))
            .or_insert_with(|| ty.clone());
    }
}
