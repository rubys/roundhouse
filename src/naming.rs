//! Rails naming conventions (snake_case, camelize, singular/plural).
//!
//! Deliberately naive. Real Rails uses `ActiveSupport::Inflector`'s rule
//! tables; we'll grow this as fixtures demand. If a test fails because of a
//! missed irregular plural, fix the rule here rather than working around it
//! in the caller.

pub fn snake_case(class_name: &str) -> String {
    let mut s = String::with_capacity(class_name.len() + 4);
    for (i, c) in class_name.char_indices() {
        if c.is_uppercase() && i > 0 {
            let prev = class_name.as_bytes()[i - 1] as char;
            if prev.is_lowercase() || prev.is_ascii_digit() {
                s.push('_');
            }
        }
        s.push(c.to_ascii_lowercase());
    }
    s
}

/// Rails `underscore`: like `snake_case`, but `::` becomes a path
/// separator (`ShortId::CandidateId` → `short_id/candidate_id`). Use for
/// file placement of possibly-namespaced classes — a literal `::` in a
/// filename breaks make dependency lists (parsed as a target separator)
/// and diverges from the Rails file convention.
pub fn underscore(class_name: &str) -> String {
    class_name
        .split("::")
        .map(snake_case)
        .collect::<Vec<_>>()
        .join("/")
}

pub fn camelize(snake: &str) -> String {
    let mut out = String::with_capacity(snake.len());
    let mut upper_next = true;
    for c in snake.chars() {
        if c == '_' {
            upper_next = true;
        } else if upper_next {
            out.push(c.to_ascii_uppercase());
            upper_next = false;
        } else {
            out.push(c);
        }
    }
    out
}

/// Rails `camelize` on a `/`-separated path: each segment camelizes,
/// joined by `::` (`mod/activities` → `Mod::Activities`). Inverse of
/// `underscore`; slash-free input degrades to plain `camelize`.
pub fn camelize_path(path: &str) -> String {
    path.split('/')
        .map(camelize)
        .collect::<Vec<_>>()
        .join("::")
}

/// Singularize only the last `/` segment, leaving namespace segments
/// intact (`mod/activities` → `mod/activity`). Slash-free input
/// degrades to plain `singularize`.
pub fn singularize_last(path: &str) -> String {
    match path.rsplit_once('/') {
        Some((ns, last)) => format!("{ns}/{}", singularize(last)),
        None => singularize(path),
    }
}

/// Ruby reserved words that cannot serve as local/parameter names.
/// Instance-variable names aren't keywords (`@for` is legal Ruby —
/// lobsters uses it), so the view lowering's ivar→local rewrite must
/// step around these.
const RESERVED_LOCALS: &[&str] = &[
    "alias", "and", "begin", "break", "case", "class", "def", "defined?",
    "do", "else", "elsif", "end", "ensure", "false", "for", "if", "in",
    "module", "next", "nil", "not", "or", "redo", "rescue", "retry",
    "return", "self", "super", "then", "true", "undef", "unless",
    "until", "when", "while", "yield",
];

/// A name safe to use as a local/param identifier: reserved words get
/// a trailing `_` (`for` → `for_`), everything else passes through.
/// Must be applied at EVERY point an ivar name becomes a view-local
/// identifier (param lists, body rewrites, partial call-site args) so
/// the renamed forms agree; ivar emission sites (`@for`) stay raw.
pub fn safe_local(name: &str) -> String {
    if RESERVED_LOCALS.contains(&name) {
        format!("{name}_")
    } else {
        name.to_string()
    }
}

/// Base (final) segment of a `/`-separated view-dir path or a
/// `::`-namespaced module name — the piece bare record/arg identifiers
/// derive from (`mod/activities` → `activities`, `Mod::Activities` →
/// `Activities`).
pub fn last_segment(name: &str) -> &str {
    name.rsplit(['/', ':']).next().unwrap_or(name)
}

/// Rails' inflection rules, ported from
/// `activesupport/lib/active_support/inflections.rb` (verified identical
/// between the 8.1.2 gem and today's rails/rails main).
///
/// PORTED, not derived. The hand-rolled approximation this replaces was
/// wrong on **39 of 86 plurals and 47 of 86 singulars** in Rails' OWN
/// test vocabulary (`activesupport/test/inflector_test_cases.rb`): every
/// irregular (person/people, child/children, man/men), every Latin
/// plural (datum/data, analysis/analyses, index/indices), the f→ves
/// family (wife/wives, half/halves) and every uncountable (fish, news,
/// series, money, jeans). The corpus happened to contain none of them,
/// which is exactly why it survived — and its two errors that DID show
/// up (`key`→`keies`, `custom_styles`→`custom_styleses`) each shipped a
/// `require` naming a file the emit never wrote, invisible until
/// campfire arrived.
///
/// Rules are stored in Rails' registration order and applied in
/// REVERSE: `inflect.plural` prepends, so the last registered rule wins
/// and `/$/ => "s"` is the fallback.
///
/// Every rule in Rails' table is suffix-anchored, so no regex engine is
/// needed. A rule is `(alternatives, replacement)`; `|` separates the
/// alternatives of a captured group. Replacement mini-language: `{}`
/// keeps the matched text, `{-x}` keeps it minus a trailing `x`, `{-2}`
/// minus its last two characters, and a bare literal replaces it. The
/// handful of rules needing a character class are named pseudo-patterns
/// handled in `apply_rule`.
const PLURAL_RULES: &[(&str, &str)] = &[
    ("", "{}s"),
    ("s", "s"),
    ("^axis|^testis", "{-is}es"),
    ("octopus|virus", "{-us}i"),
    ("octopi|viri", "{}"),
    ("alias|status", "{}es"),
    ("bus", "{-s}ses"),
    ("buffalo|tomato", "{}es"),
    ("tum|ium", "{-um}a"),
    ("ta|ia", "{}"),
    ("sis", "{-sis}ses"),
    ("FE_VES", ""),
    ("hive", "{}s"),
    ("CONSONANT_Y", ""),
    ("x|ch|ss|sh", "{}es"),
    ("matrix|vertix|indix|matrex|vertex|index", "{-2}ices"),
    ("^mouse|^louse", "{-ouse}ice"),
    ("^mice|^lice", "{}"),
    ("^ox", "{}en"),
    ("^oxen", "{}"),
    ("quiz", "{}zes"),
];

const SINGULAR_RULES: &[(&str, &str)] = &[
    ("s", "{-s}"),
    ("ss", "{}"),
    ("news", "{}"),
    ("ta|ia", "{-a}um"),
    ("SIS_FAMILY", ""),
    ("VES_FE", ""),
    ("hives", "{-s}"),
    ("tives", "{-s}"),
    ("LR_VES", ""),
    ("CONSONANT_IES", ""),
    ("series", "{}"),
    ("movies", "{-s}"),
    ("xes|ches|sses|shes", "{-es}"),
    ("^mice|^lice", "{-ice}ouse"),
    ("buses|bus", "bus"),
    ("oes", "{-es}"),
    ("shoes", "{-s}"),
    ("crisis|crises|testis|testes", "{-2}is"),
    ("^axes|^axis", "axis"),
    ("octopus|virus", "{}"),
    ("octopi|viri", "{-i}us"),
    ("aliases|alias|statuses|status", "{-es}"),
    ("^oxen", "ox"),
    ("vertices|indices", "{-ices}ex"),
    ("matrices", "matrix"),
    ("quizzes", "quiz"),
    ("databases", "{-s}"),
];

/// `inflect.irregular` — matched as a SUFFIX, which is how Rails' own
/// generated rules behave (`salesperson` → `salespeople`, `node_child`
/// → `node_children`). That also reproduces Rails' quirk of inflecting
/// `human` to `humen`; matching Rails is the contract, not English.
const IRREGULAR: &[(&str, &str)] = &[
    ("person", "people"),
    ("man", "men"),
    ("child", "children"),
    ("sex", "sexes"),
    ("move", "moves"),
    ("zombie", "zombies"),
];

const UNCOUNTABLE: &[&str] = &[
    "equipment",
    "information",
    "rice",
    "money",
    "species",
    "series",
    "fish",
    "sheep",
    "jeans",
    "police",
];

/// Rails' uncountable check is `/\b<word>\z/i` — the match must begin at
/// a word BOUNDARY. `_` is a word character, so `funky jeans` is
/// uncountable while `old_news` is not (that one is handled by the
/// explicit `(n)ews$` singular rule instead).
fn uncountable(word: &str) -> bool {
    UNCOUNTABLE.iter().any(|u| {
        word.strip_suffix(u).is_some_and(|head| {
            head.is_empty() || !head.ends_with(|c: char| c.is_alphanumeric() || c == '_')
        })
    })
}

fn irregular_apply(word: &str, from_singular: bool) -> Option<String> {
    for (s, p) in IRREGULAR {
        let (from, to) = if from_singular { (*s, *p) } else { (*p, *s) };
        if let Some(head) = word.strip_suffix(from) {
            return Some(format!("{head}{to}"));
        }
    }
    None
}

/// The analysis/basis/diagnosis family, both directions of
/// `((a)naly|(b)a|(d)iagno|…)(sis|ses)$ => '\1sis'`.
fn sis_family(word: &str) -> Option<String> {
    const STEMS: &[&str] = &[
        "analy", "ba", "diagno", "parenthe", "progno", "synop", "the",
    ];
    let head = word.strip_suffix("ses").or_else(|| word.strip_suffix("sis"))?;
    STEMS
        .iter()
        .find(|st| head.ends_with(*st))
        .map(|_| format!("{head}sis"))
}

fn apply_rule(word: &str, alts: &str, repl: &str) -> Option<String> {
    match alts {
        // /(?:([^f])fe|([lr])f)$/ => '\1\2ves'
        "FE_VES" => {
            if let Some(h) = word.strip_suffix("fe") {
                if !h.is_empty() && !h.ends_with('f') {
                    return Some(format!("{h}ves"));
                }
            }
            let h = word.strip_suffix('f')?;
            return (h.ends_with('l') || h.ends_with('r')).then(|| format!("{h}ves"));
        }
        // /([^aeiouy]|qu)y$/ => '\1ies'
        "CONSONANT_Y" => {
            let h = word.strip_suffix('y')?;
            return (h.ends_with("qu") || h.ends_with(|c: char| !"aeiouy".contains(c)))
                .then(|| format!("{h}ies"));
        }
        "SIS_FAMILY" => return sis_family(word),
        // /([^f])ves$/ => '\1fe'
        "VES_FE" => {
            let h = word.strip_suffix("ves")?;
            return (!h.is_empty() && !h.ends_with('f')).then(|| format!("{h}fe"));
        }
        // /([lr])ves$/ => '\1f'
        "LR_VES" => {
            let h = word.strip_suffix("ves")?;
            return (h.ends_with('l') || h.ends_with('r')).then(|| format!("{h}f"));
        }
        // /([^aeiouy]|qu)ies$/ => '\1y'
        "CONSONANT_IES" => {
            let h = word.strip_suffix("ies")?;
            return (h.ends_with("qu") || h.ends_with(|c: char| !"aeiouy".contains(c)))
                .then(|| format!("{h}y"));
        }
        _ => {}
    }
    if alts.is_empty() {
        return Some(expand(word, "", repl));
    }
    // A leading `^` marks a rule Rails anchors at both ends
    // (`/^(ox)$/`, `/^(m|l)ice$/`): it matches the WHOLE word, never a
    // tail. Without that, `box` hit the `ox` rule and pluralized to
    // `boxen`, and `slice` hit `lice` and stayed `slice`.
    let hit = alts.split('|').find(|a| match a.strip_prefix('^') {
        Some(whole) => word == whole,
        None => word.ends_with(a),
    })?;
    Some(expand(word, hit.trim_start_matches('^'), repl))
}

fn expand(word: &str, matched: &str, repl: &str) -> String {
    let head = &word[..word.len() - matched.len()];
    let Some(body) = repl.strip_prefix('{') else {
        return format!("{head}{repl}");
    };
    let (inner, tail) = body.split_once('}').unwrap_or((body, ""));
    let kept = if inner == "-2" {
        &matched[..matched.len().saturating_sub(2)]
    } else if let Some(drop) = inner.strip_prefix('-') {
        matched.strip_suffix(drop).unwrap_or(matched)
    } else {
        matched
    };
    format!("{head}{kept}{tail}")
}

fn inflect(word: &str, rules: &[(&str, &str)], from_singular: bool) -> String {
    if word.is_empty() || uncountable(word) {
        return word.to_string();
    }
    if let Some(hit) = irregular_apply(word, from_singular) {
        return hit;
    }
    // Reverse registration order: `inflect.plural`/`inflect.singular`
    // prepend, so Rails tries the LAST rule in the file first.
    for (alts, repl) in rules.iter().rev() {
        if let Some(out) = apply_rule(word, alts, repl) {
            return out;
        }
    }
    word.to_string()
}

pub fn pluralize_snake(class_name: &str) -> String {
    inflect(&snake_case(class_name), PLURAL_RULES, true)
}

pub fn singularize(plural: &str) -> String {
    inflect(plural, SINGULAR_RULES, false)
}


pub fn singularize_camelize(plural_symbol: &str) -> String {
    camelize(&singularize(plural_symbol))
}

pub fn habtm_join_table(owner_class: &str, target_plural_sym: &str) -> String {
    let a = pluralize_snake(owner_class);
    let b = target_plural_sym.to_string();
    if a < b { format!("{a}_{b}") } else { format!("{b}_{a}") }
}
