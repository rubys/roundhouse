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
    for m in ["application", "logger", "cache", "configuration", "root", "public_path"] {
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

    // GlobalID mint + Locator — unsigned `param`/`uri`/`signed` and the
    // locate / locate_signed class methods. Return types for locate*
    // with a literal `only:` are refined in `body/send.rs` to the named
    // model (nilable); Untyped here is the gradual fallback for a
    // computed `only:`.
    {
        let mut gid = ClassInfo::default();
        for m in ["param", "uri", "signed"] {
            gid.class_methods.insert(Symbol::from(m), Ty::Str);
        }
        classes.insert(ClassId(Symbol::from("GlobalID")), gid);
        let mut locator = ClassInfo::default();
        locator
            .class_methods
            .insert(Symbol::from("locate"), Ty::Untyped);
        locator
            .class_methods
            .insert(Symbol::from("locate_signed"), Ty::Untyped);
        classes.insert(ClassId(Symbol::from("GlobalID::Locator")), locator);
    }

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

    // `ActionController::InvisibleCaptcha.spam?(params)` — the honeypot
    // gate `ingest::invisible_captcha` synthesizes into a controller
    // body; runtime/ruby/action_controller/invisible_captcha.rb answers
    // it. Registered for the same reason as RateLimiter / BrowserBlocker.
    let mut captcha = ClassInfo::default();
    captcha.class_methods.insert(Symbol::from("spam?"), Ty::Bool);
    classes.insert(ClassId(Symbol::from("ActionController::InvisibleCaptcha")), captcha);

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
    // Psych/YAML — campfire's Purchaser loads `config/purchased_by.yml`
    // via `YAML.load_file`. Register the class-side surface the corpus
    // writes so the constant is modeled (not a silent raise stub) and
    // ruby-family emit keeps the call. Return types stay gradual: the
    // document shape is whatever the file held.
    register_stdlib_class(classes, "YAML", &[
        ("load", Ty::Untyped), ("load_file", Ty::Untyped),
        ("safe_load", Ty::Untyped), ("safe_load_file", Ty::Untyped),
        ("dump", Ty::Str),
    ], &[]);
    // Framework modules an app `include`s that emit already handles
    // (Attachable sgid, Turbo stream names, ActiveModel callbacks,
    // SanitizeHelper). Empty module markers so the unresolved-include
    // gate does not refuse known seams; methods come from lowering /
    // other registry entries.
    for name in [
        "ActionText::Attachable",
        "Turbo::Streams::StreamName",
        "Turbo::Streams::StreamName::ClassMethods",
        "ActiveModel::Validations::Callbacks",
        "ActionView::Helpers::SanitizeHelper",
    ] {
        let info = classes.entry(ClassId(Symbol::from(name))).or_default();
        info.is_module = true;
    }
    // SanitizeHelper readers Opengraph::Metadata calls after include.
    if let Some(sanitize) = classes.get_mut(&ClassId(Symbol::from("ActionView::Helpers::SanitizeHelper"))) {
        for m in ["sanitize", "strip_tags", "sanitize_css"] {
            sanitize.instance_methods.entry(Symbol::from(m)).or_insert(Ty::Str);
        }
    }
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
    register_stdlib_class(classes, "Regexp", &[
        // Class side: escaping is a pure String -> String function, `last_match` reads the
        // `$~` of the previous match, `union` builds a Regexp.
        ("escape", Ty::Str),
        ("quote", Ty::Str),
        ("last_match", Ty::Union { variants: vec![match_data.clone(), Ty::Nil] }),
        ("union", Ty::Class { id: ClassId(Symbol::from("Regexp")), args: vec![] }),
    ], &[
        ("match", Ty::Union { variants: vec![match_data.clone(), Ty::Nil] }),
        ("match?", Ty::Bool),
        ("=~", Ty::Union { variants: vec![Ty::Int, Ty::Nil] }),
        ("===", Ty::Bool),
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
    // `gzip` is Ruby's own on the ruby family and spinel's
    // `packages/zlib` on spinel, so it types here and a strict target,
    // whose port has no deflate, refuses the call by name
    // (`project::RUBY_SPINEL_ONLY_METHODS`).
    register_stdlib_class(classes, "Zlib", &[
        ("crc32", Ty::Int),
        ("gzip", Ty::Str),
    ], &[]);
    // `SQLite3::Database` — the sqlite3 gem on the ruby family, and
    // `runtime/spinel/sqlite3_database.rb` (the same surface over the FFI)
    // on spinel. ONLY that surface: campfire's `ResponseCache` keeps a
    // read-only observer (`new(path, readonly: true)`,
    // `get_first_value("PRAGMA data_version")`, `close`) and its WAL
    // checkpointer opens a block-form connection, sets
    // `busy_handler_timeout=` and `execute`s a pragma. A row is an Array
    // of column values.
    let sqlite_db = Ty::Class { id: ClassId(Symbol::from("SQLite3::Database")), args: vec![] };
    register_stdlib_class(classes, "SQLite3::Database", &[
        ("new", sqlite_db.clone()),
        ("open", sqlite_db.clone()),
    ], &[
        ("execute", Ty::Array { elem: Box::new(Ty::Array { elem: Box::new(Ty::Untyped) }) }),
        ("get_first_row", Ty::Union { variants: vec![Ty::Array { elem: Box::new(Ty::Untyped) }, Ty::Nil] }),
        ("get_first_value", Ty::Untyped),
        ("close", Ty::Nil),
        ("closed?", Ty::Bool),
        ("readonly?", Ty::Bool),
        ("busy_timeout=", Ty::Int),
        ("busy_handler_timeout=", Ty::Int),
    ]);
    // `ActiveSupport::Cache` — `expand_cache_key` and the bounded
    // `MemoryStore`, in runtime/spinel/active_support_cache.rb (the ruby
    // family and spinel). A cached value is whatever was written.
    register_stdlib_class(classes, "ActiveSupport::Cache", &[
        ("expand_cache_key", Ty::Str),
    ], &[]);
    let memory_store = Ty::Class { id: ClassId(Symbol::from("ActiveSupport::Cache::MemoryStore")), args: vec![] };
    register_stdlib_class(classes, "ActiveSupport::Cache::MemoryStore", &[
        ("new", memory_store),
    ], &[
        ("read", Ty::Untyped),
        ("write", Ty::Bool),
        ("fetch", Ty::Untyped),
        ("delete", Ty::Bool),
        ("exist?", Ty::Bool),
        ("clear", Ty::Untyped),
        ("cleanup", Ty::Nil),
        ("prune", Ty::Nil),
        ("read_multi", Ty::Hash { key: Box::new(Ty::Untyped), value: Box::new(Ty::Untyped) }),
        ("write_multi", Ty::Bool),
    ]);

    // `FileUtils` — a default gem / spinel's `packages/fileutils`.
    register_stdlib_class(classes, "FileUtils", &[
        ("mkdir_p", Ty::Untyped), ("makedirs", Ty::Untyped),
        ("rm_rf", Ty::Untyped), ("rm_f", Ty::Untyped),
        ("remove_entry", Ty::Nil), ("touch", Ty::Untyped),
    ], &[]);
    // `ActiveSupport::JSON` — Rails' coder, in `runtime/ruby/
    // active_support_ext.rb` (the ruby family and spinel). `encode`
    // answers the document; `decode` whatever the document holds.
    register_stdlib_class(classes, "ActiveSupport::JSON", &[
        ("encode", Ty::Str),
        ("decode", Ty::Untyped),
    ], &[]);
    // `Rack::Utils` — the two encoding-negotiation functions, ported
    // into `runtime/ruby/rack_utils.rb` (the rack gem's own on the ruby
    // family). ONLY that surface, the IPAddr rule. A pair is
    // `[name, quality]`; the name is nil for an empty header part.
    let q_pair = Ty::Tuple {
        elems: vec![Ty::Union { variants: vec![Ty::Str, Ty::Nil] }, Ty::Float],
    };
    register_stdlib_class(classes, "Rack::Utils", &[
        ("q_values", Ty::Array { elem: Box::new(q_pair) }),
        ("select_best_encoding", Ty::Union { variants: vec![Ty::Str, Ty::Nil] }),
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
    // `cause` is the exception being handled when this one was raised, or
    // nil; `exception` answers the exception itself (`raise e` and
    // `e.exception("msg")` both go through it).
    let exception_surface = [
        ("message", Ty::Str),
        ("to_s", Ty::Str),
        ("full_message", Ty::Str),
        ("detailed_message", Ty::Str),
        ("inspect", Ty::Str),
        ("backtrace", Ty::Array { elem: Box::new(Ty::Str) }),
        ("backtrace_locations", Ty::Array { elem: Box::new(Ty::Untyped) }),
        ("set_backtrace", Ty::Untyped),
        ("exception", Ty::Untyped),
        (
            "cause",
            Ty::Union {
                variants: vec![
                    Ty::Class { id: ClassId(Symbol::from("Exception")), args: vec![] },
                    Ty::Nil,
                ],
            },
        ),
    ];
    for exc in [
        "Exception", "StandardError", "RuntimeError", "ArgumentError",
        "TypeError", "NameError", "NoMethodError", "IndexError",
        "KeyError", "RangeError", "IOError", "NotImplementedError",
        "FrozenError", "ZeroDivisionError", "StopIteration",
        "ThreadError", "ClosedQueueError",
        // CRuby's bundled libraries and Spinel's uri/net/json packages
        // recognize these exception names; emitted requires load them.
        "URI::Error", "URI::InvalidURIError", "Net::OpenTimeout", "Net::ReadTimeout",
        "SocketError", "OpenSSL::OpenSSLError", "JSON::ParserError", "JSON::GeneratorError",
        // Campfire tip: `rescue SystemCallError` / `OpenSSL::SSL::SSLError`
        // on pooled web-push connections; `rescue Vips::Error` beside
        // ActiveStorage::PreviewError when drawing attachment variants.
        "SystemCallError", "OpenSSL::SSL::SSLError", "Vips::Error",
        // `Timeout.timeout` / `rescue Timeout::Error` — Campfire unfurl
        // deadline and TimeLimitedVideoPreviewer#capture.
        "Timeout::Error",
        // `rescue EOFError` around `readpartial` on a pipe or a pty.
        "EOFError",
        // `rescue SQLite3::Exception` — campfire's `ResponseCache` drops
        // its observer on any driver error (see `SQLite3::Database`).
        "SQLite3::Exception", "SQLite3::CantOpenException",
        "SQLite3::BusyException", "SQLite3::SQLException",
        // `rescue ArgumentError, RQRCodeCore::QRCodeRunTimeError` —
        // campfire's `QrCodeController#show` answers 400 for data too
        // long to encode. rqrcode_core's classes on the ruby family, the
        // spinel-rqrcode package's (same names, same raise) on spinel.
        "RQRCodeCore::QRCodeRunTimeError", "RQRCodeCore::QRCodeArgumentError",
    ] {
        register_stdlib_class(classes, exc, &[], &exception_surface);
    }
    // `rescue Errno::ENOENT` / `Errno::EIO`: every Errno class that both
    // CRuby (on every POSIX platform) and Spinel's runtime define. The
    // lookup is by exact name, so the family is registered whole.
    register_stdlib_class(classes, "Errno", &[], &[]);
    for name in ERRNO_CLASSES {
        register_stdlib_class(classes, &format!("Errno::{name}"), &[], &exception_surface);
    }
    for (exc, extra) in [
        ("ActiveRecord::RecordNotFound", None),
        ("ActiveRecord::RecordNotUnique", None),
        ("ActiveRecord::ValueTooLong", None),
        ("ActiveRecord::SoleRecordExceeded", None),
        // Not `ActiveRecord::Base`: no instance surface is registered there, so `e.record.errors` would still fail.
        ("ActiveRecord::RecordInvalid", Some(("record", Ty::Untyped))),
        ("ActiveRecord::RecordNotSaved", Some(("record", Ty::Untyped))),
        ("ActiveRecord::ReadOnlyRecord", None),
        // Names overlap `project::RUBY_FAMILY_RUNTIME_CONSTANTS` (emit
        // ledger). Keep extras here — inference needs the readers.
        ("ActionController::ParameterMissing", Some(("param", Ty::Str))),
        ("ActionController::UnpermittedParameters", None),
        ("ActionController::UnknownFormat", None),
        ("ActionController::BadRequest", None),
        ("ActionController::InvalidAuthenticityToken", None),
        ("ActionController::RoutingError", Some(("failures", Ty::Array { elem: Box::new(Ty::Str) }))),
        ("AbstractController::ActionNotFound", None),
    ] {
        let mut methods = exception_surface.to_vec();
        methods.extend(extra);
        let methods: Vec<(&str, Ty)> = methods;
        register_stdlib_class(classes, exc, &[], &methods);
    }
    // `ActiveModel::Type::Boolean.new.cast(v)`: nil for a blank value, else the boolean.
    // Not answered here beyond `new`: the dispatch arm for `ActiveSupport::StringInquirer` types its predicates and String surface.
    register_stdlib_class(classes, "ActiveSupport::StringInquirer", &[("new", Ty::Str)], &[]);
    // `ActiveRecord::Type::Boolean` is a subclass with the same casting; `deserialize` casts the same way for a boolean.
    for name in ["ActiveModel::Type::Boolean", "ActiveRecord::Type::Boolean"] {
        register_stdlib_class(classes, name, &[], &[
            ("cast", Ty::Union { variants: vec![Ty::Bool, Ty::Nil] }),
            ("deserialize", Ty::Union { variants: vec![Ty::Bool, Ty::Nil] }),
        ]);
    }
    // Not a typed store: a thread-local slot holds whatever the caller put there, so `[]` answers untyped.
    // `new` / `pass` / `kill` / `join` — `runtime/ruby/timeout.rb`'s wall-clock
    // port (Spinel lane) starts a worker and kills it past the deadline.
    let thread = Ty::Class { id: ClassId(Symbol::from("Thread")), args: vec![] };
    register_stdlib_class(classes, "Thread", &[
        ("current", thread.clone()),
        ("new", thread.clone()),
        ("pass", Ty::Nil),
    ], &[
        ("[]", Ty::Untyped),
        ("[]=", Ty::Untyped),
        ("kill", thread.clone()),
        ("join", thread.clone()),
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
    register_stdlib_class(classes, "URI::HTTPS", &[], &[]);
    for response in ["Net::HTTPRedirection", "Net::HTTPOK"] {
        register_stdlib_class(classes, response, &[], &[]);
    }
    // The implementation is bundled on Ruby/Spinel. Other targets
    // report the missing runtime at project emission.
    let string_io = Ty::Class { id: ClassId(Symbol::from("StringIO")), args: vec![] };
    register_stdlib_class(classes, "StringIO", &[], &[
        ("string", Ty::Str), ("<<", string_io),
    ]);
    // `IO` / `Process` / `Timeout` — Campfire tip names these as Consts
    // in indexed app source (`time_limited_video_previewer`, web-push
    // connection pool, unfurl deadline). They were listed in
    // `RUBY_TOP_LEVEL` (declared-type noise) but never registered, so
    // Const resolution raised Unsupported. Register the class objects
    // for Const resolution only. Class-method returns for `Process.*`,
    // `Timeout.timeout`, and `IO.popen` / `copy_stream` live in the
    // send special-cases (`body/send.rs`) — catalog entries would win
    // before those cases and kill unit-aware `clock_gettime` (Float for
    // `:millisecond`). Nested *namespace* Consts (`Process::CLOCK_*`)
    // stay empty ClassIds the way `URI::HTTP` is. Value Consts
    // (`IO::NULL` / `File::NULL`) are typed Strings via CORE_RBS — do
    // not also register them here or the ClassId fallback disagrees.
    // Instance methods on an `IO` handle still belong here. `winsize` /
    // `winsize=` wait on carrying `require "io/console"` into the
    // emitted tree; admitting them without that load is check-quiet /
    // runtime `NoMethodError`.
    let io = Ty::Class { id: ClassId(Symbol::from("IO")), args: vec![] };
    register_stdlib_class(classes, "IO", &[], &[
        ("pid", Ty::Int),
        ("read", Ty::Str),
        ("readpartial", Ty::Str),
        ("write", Ty::Int),
        ("closed?", Ty::Bool),
        ("rewind", Ty::Int),
        ("binmode", io.clone()),
        ("close", Ty::Nil),
    ]);
    // `File < IO` — Ruby's hierarchy (not a PTY detail). File's class
    // methods are registered earlier; the parent is set once `IO` exists
    // so instance methods (`readpartial`, `closed?`, …) resolve on a
    // File handle, including `PTY.spawn`'s reader/writer.
    classes
        .get_mut(&ClassId(Symbol::from("File")))
        .expect("File registered above")
        .parent = Some(ClassId(Symbol::from("IO")));
    // `require "pty"`. `PTY.spawn`'s return is a send special case: its
    // block form answers nil; block yields are typed in `block_params_for`.
    register_stdlib_class(classes, "PTY", &[], &[]);
    // A default gem a booted Rails app has already loaded; the BUNDLED
    // row emits its require.
    register_stdlib_class(classes, "Shellwords", &[
        ("escape", Ty::Str), ("shellescape", Ty::Str),
        ("join", Ty::Str), ("shelljoin", Ty::Str),
        ("split", str_arr()), ("shellsplit", str_arr()), ("shellwords", str_arr()),
    ], &[]);
    register_stdlib_class(classes, "Process", &[], &[]);
    register_stdlib_class(classes, "Process::CLOCK_MONOTONIC", &[], &[]);
    register_stdlib_class(classes, "Process::CLOCK_REALTIME", &[], &[]);
    register_stdlib_class(classes, "Process::CLOCK_PROCESS_CPUTIME_ID", &[], &[]);
    register_stdlib_class(classes, "Process::CLOCK_THREAD_CPUTIME_ID", &[], &[]);
    // Module Const only — `timeout` return is the send special-case.
    // Exception is `Timeout::Error` above. CRuby loads via BUNDLED
    // `require "timeout"`; Spinel gets `runtime/ruby/timeout.rb`.
    register_stdlib_class(classes, "Timeout", &[], &[]);
    // JSON dispatch is already intrinsic in BodyTyper and the emitters;
    // a source-backed reference must also recognize its exact namespace.
    register_stdlib_class(classes, "JSON", &[], &[]);
    // Ruby and Spinel provide both core classes. The existing constructor
    // rule carries Struct.new's value into an assigned constant, so reads
    // of that constant and Mutex.new retain their source expressions.
    // Do not invent member or synchronization return types here.
    register_stdlib_class(classes, "Struct", &[], &[]);
    register_stdlib_class(classes, "Mutex", &[], &[]);
    // The queue constructors, Thread's core aliases, and the standard
    // mixins likewise need exact entries for source constant resolution.
    // No queue element, synchronization or mixin method types are added;
    // the Ruby-family runtimes supply the actual behavior.
    for name in ["Queue", "SizedQueue", "Thread::Queue", "Thread::SizedQueue",
        "Thread::Mutex", "Comparable", "Enumerable"] {
        register_stdlib_class(classes, name, &[], &[]);
    }
    for (class, method) in BUILTIN_BLOCK_VALUE_METHODS {
        classes
            .entry(ClassId(Symbol::from(*class)))
            .or_default()
            .block_value_methods
            .insert(Symbol::from(*method));
    }
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
        ("allowed_tags", Ty::Union { variants: vec![Ty::Array { elem: Box::new(Ty::Str) }, Ty::Class { id: ClassId(Symbol::from("Set")), args: vec![] }] }),
        ("allowed_attributes", Ty::Union { variants: vec![Ty::Array { elem: Box::new(Ty::Str) }, Ty::Class { id: ClassId(Symbol::from("Set")), args: vec![] }] }),
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

/// Library methods that answer their block's value, by Ruby's (or
/// ActiveSupport's) definition rather than by inference:
/// `mutex.synchronize { … }` — campfire's `ResponseCache#version` is
/// `@mutex.synchronize { current_version }` — and a cache `fetch`, which
/// answers the block's value on a miss and what such a block wrote on a
/// hit. Only an informative block type is adopted (`block_value_return`).
/// Re-seeded after every harvest of the app's own block-value methods
/// (`Analyzer::harvest_block_value_methods`), which starts from empty.
pub(crate) const BUILTIN_BLOCK_VALUE_METHODS: &[(&str, &str)] = &[
    ("Mutex", "synchronize"),
    ("Thread::Mutex", "synchronize"),
    ("ActiveSupport::Cache::MemoryStore", "fetch"),
];

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

/// The Errno classes CRuby defines on every POSIX platform that Spinel's
/// runtime (`lib/sp_exc.c`) defines too.
const ERRNO_CLASSES: &[&str] = &[
    "EPERM", "ENOENT", "ESRCH", "EINTR", "EIO", "ENXIO", "E2BIG", "ENOEXEC", "EBADF",
    "ECHILD", "EAGAIN", "ENOMEM", "EACCES", "EFAULT", "EBUSY", "EEXIST", "EXDEV", "ENODEV",
    "ENOTDIR", "EISDIR", "EINVAL", "ENFILE", "EMFILE", "ENOTTY", "EFBIG", "ENOSPC", "ESPIPE",
    "EROFS", "EMLINK", "EPIPE", "EDOM", "ERANGE", "EDEADLK", "ENAMETOOLONG", "ENOLCK",
    "ENOSYS", "ENOTEMPTY", "ELOOP", "ENOTSOCK", "EMSGSIZE", "EPROTOTYPE", "ENOPROTOOPT",
    "EPROTONOSUPPORT", "ENOTSUP", "EOPNOTSUPP", "EAFNOSUPPORT", "EADDRINUSE",
    "EADDRNOTAVAIL", "ENETDOWN", "ENETUNREACH", "ENETRESET", "ECONNABORTED", "ECONNRESET",
    "ENOBUFS", "EISCONN", "ENOTCONN", "ETIMEDOUT", "ECONNREFUSED", "EHOSTUNREACH",
    "EALREADY", "EINPROGRESS", "ESTALE", "EDQUOT", "ECANCELED", "EOVERFLOW", "EILSEQ",
];
