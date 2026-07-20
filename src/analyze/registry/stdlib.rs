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

    // Date / DateTime singletons — analogous to Time. Same
    // rationale: structural typing of these classes hasn't been
    // wired, but the call shape needs to resolve.
    for name in ["Date", "DateTime"] {
        let mut cls = ClassInfo::default();
        cls.class_methods.insert(Symbol::from("current"), Ty::Untyped);
        cls.class_methods.insert(Symbol::from("today"), Ty::Untyped);
        cls.class_methods.insert(Symbol::from("now"), Ty::Untyped);
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
    // `URI.parse` returns a URI object we don't model; `Untyped` lets
    // chained `.scheme` / `.host` flow gradually instead of erroring.
    register_stdlib_class(classes, "URI", &[
        ("parse", Ty::Untyped), ("join", Ty::Untyped),
        ("escape", Ty::Str), ("unescape", Ty::Str),
        ("encode_www_form", Ty::Str), ("decode_www_form", Ty::Untyped),
    ], &[]);
    // `Set` is a value type: `Set.new` yields `Class { Set }` (via the
    // universal `.new`), then these instance methods dispatch on it.
    // Mutators return the receiver (self) for chaining; element-typed
    // accessors are `Untyped` (Set isn't parameterized here).
    let set_self = Ty::Class { id: ClassId(Symbol::from("Set")), args: vec![] };
    register_stdlib_class(classes, "Set", &[], &[
        ("<<", set_self.clone()), ("add", set_self.clone()),
        ("delete", set_self.clone()), ("merge", set_self.clone()),
        ("add?", Ty::Untyped), ("each", Ty::Untyped),
        ("map", Ty::Array { elem: Box::new(Ty::Untyped) }),
        ("include?", Ty::Bool), ("member?", Ty::Bool), ("empty?", Ty::Bool),
        ("size", Ty::Int), ("length", Ty::Int), ("count", Ty::Int),
        ("to_a", Ty::Array { elem: Box::new(Ty::Untyped) }),
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
