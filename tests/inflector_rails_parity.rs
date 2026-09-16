//! `naming::pluralize_snake` / `singularize` against Rails' OWN inflector
//! test vocabulary.
//!
//! The table in `src/naming.rs` is ported from
//! `activesupport/lib/active_support/inflections.rb`; these pairs are
//! lifted from `activesupport/test/inflector_test_cases.rb`
//! (`SingularToPlural`), so this file is the oracle that keeps the port
//! honest rather than a restatement of the rules.
//!
//! The hand-rolled approximation this replaced was wrong on 39 of these
//! 86 plurals and 47 of the singulars.

const PAIRS: &[(&str, &str)] = &[
    ("search", "searches"), ("switch", "switches"), ("fix", "fixes"),
    ("box", "boxes"), ("process", "processes"), ("address", "addresses"),
    ("case", "cases"), ("stack", "stacks"), ("wish", "wishes"),
    ("fish", "fish"), ("jeans", "jeans"), ("funky jeans", "funky jeans"),
    ("my money", "my money"), ("category", "categories"), ("query", "queries"),
    ("ability", "abilities"), ("agency", "agencies"), ("movie", "movies"),
    ("archive", "archives"), ("index", "indices"), ("wife", "wives"),
    ("safe", "saves"), ("half", "halves"), ("move", "moves"),
    ("salesperson", "salespeople"), ("person", "people"),
    ("spokesman", "spokesmen"), ("man", "men"), ("woman", "women"),
    ("basis", "bases"), ("diagnosis", "diagnoses"), ("diagnosis_a", "diagnosis_as"),
    ("datum", "data"), ("medium", "media"), ("stadium", "stadia"),
    ("analysis", "analyses"), ("my_analysis", "my_analyses"),
    ("node_child", "node_children"), ("child", "children"),
    ("experience", "experiences"), ("day", "days"), ("comment", "comments"),
    ("foobar", "foobars"), ("newsletter", "newsletters"),
    ("old_news", "old_news"), ("news", "news"), ("series", "series"),
    ("miniseries", "miniseries"), ("species", "species"), ("quiz", "quizzes"),
    ("perspective", "perspectives"), ("ox", "oxen"), ("photo", "photos"),
    ("buffalo", "buffaloes"), ("tomato", "tomatoes"), ("dwarf", "dwarves"),
    ("elf", "elves"), ("information", "information"), ("equipment", "equipment"),
    ("bus", "buses"), ("status", "statuses"), ("mouse", "mice"),
    ("louse", "lice"), ("house", "houses"), ("octopus", "octopi"),
    ("virus", "viri"), ("alias", "aliases"), ("portfolio", "portfolios"),
    ("vertex", "vertices"), ("matrix", "matrices"), ("axis", "axes"),
    ("taxi", "taxis"), ("testis", "testes"), ("crisis", "crises"),
    ("rice", "rice"), ("shoe", "shoes"), ("horse", "horses"),
    ("prize", "prizes"), ("edge", "edges"), ("database", "databases"),
];

#[test]
fn pluralize_matches_rails() {
    let bad: Vec<String> = PAIRS
        .iter()
        .filter_map(|(s, p)| {
            let got = roundhouse::naming::pluralize_snake(s);
            (got != *p).then(|| format!("{s} -> want {p}, got {got}"))
        })
        .collect();
    assert!(bad.is_empty(), "{} wrong:\n  {}", bad.len(), bad.join("\n  "));
}

#[test]
fn singularize_matches_rails() {
    let bad: Vec<String> = PAIRS
        .iter()
        .filter_map(|(s, p)| {
            let got = roundhouse::naming::singularize(p);
            (got != *s).then(|| format!("{p} -> want {s}, got {got}"))
        })
        .collect();
    assert!(bad.is_empty(), "{} wrong:\n  {}", bad.len(), bad.join("\n  "));
}

/// The two that reached production: campfire's `resource :key` and
/// `resource :custom_styles`, each of which named a controller file the
/// emit never wrote.
#[test]
fn the_two_that_shipped_a_broken_require() {
    assert_eq!(roundhouse::naming::pluralize_snake("key"), "keys");
    assert_eq!(roundhouse::naming::pluralize_snake("custom_styles"), "custom_styles");
    assert_eq!(roundhouse::naming::pluralize_snake("refresh"), "refreshes");
}

/// The app's own `config/initializers/inflections.rb` is consulted
/// ahead of Rails' table — Rails itself singularizes `leaves` to
/// `leafe`, and writebook's `has_many :leaves` only names `Leaf`
/// because its initializer says `inflect.irregular "leaf", "leaves"`.
/// Acronyms camelize as themselves; uncountables stop inflecting. An
/// empty set restores the defaults, and regex rules are counted.
#[test]
fn app_inflections_override_the_defaults() {
    use roundhouse::naming::{self, AppInflections};
    assert_eq!(naming::singularize("leaves"), "leafe", "Rails' own answer, no initializer");

    naming::install_app_inflections(AppInflections {
        irregular: vec![("leaf".into(), "leaves".into())],
        uncountable: vec!["equipment".into(), "firmware".into()],
        acronym: vec!["API".into(), "HTML".into()],
        singular: vec![("data".into(), "data".into()), ("quotas".into(), "quota".into())],
        plural: vec![("quota".into(), "quotas".into())],
        regex_rules: 0,
    });
    assert_eq!(naming::singularize("data"), "data", "Mastodon's string rule beats datum");
    assert_eq!(naming::singularize("quotas"), "quota");
    assert_eq!(naming::pluralize_snake("quota"), "quotas");
    assert_eq!(naming::singularize("leaves"), "leaf");
    assert_eq!(naming::pluralize_snake("leaf"), "leaves");
    assert_eq!(naming::singularize_camelize("leaves"), "Leaf");
    assert_eq!(naming::singularize("firmware"), "firmware");
    assert_eq!(naming::pluralize_snake("firmware"), "firmware");
    assert_eq!(naming::camelize("api_keys"), "APIKeys");
    assert_eq!(naming::camelize("html_parser"), "HTMLParser");
    assert_eq!(naming::camelize("article"), "Article");

    naming::install_app_inflections(AppInflections::default());
    assert_eq!(naming::singularize("leaves"), "leafe");
    assert_eq!(naming::camelize("api_keys"), "ApiKeys");
}

/// The initializer as written (the block form, `%w()` and string
/// arguments, a regex rule) parses into the declarations.
#[test]
fn inflections_initializer_is_read() {
    let dir = std::env::temp_dir().join(format!("rh-inflect-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("config/initializers")).unwrap();
    std::fs::write(
        dir.join("config/initializers/inflections.rb"),
        "# Be sure to restart your server when you modify this file.\n\
         ActiveSupport::Inflector.inflections(:en) do |inflect|\n\
         \x20 inflect.irregular \"leaf\", \"leaves\"\n\
         \x20 inflect.uncountable %w( fish sheep )\n\
         \x20 inflect.acronym 'API'\n\
         \x20 inflect.singular \"quotas\", \"quota\"\n\
         \x20 inflect.plural /^(ox)$/i, '\\1en'\n\
         end\n",
    )
    .unwrap();
    let got = roundhouse::ingest::app::ingest_inflections(&roundhouse::vfs::FsVfs::new(), &dir);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(got.irregular, vec![("leaf".to_string(), "leaves".to_string())]);
    assert_eq!(got.uncountable, vec!["fish".to_string(), "sheep".to_string()]);
    assert_eq!(got.acronym, vec!["API".to_string()]);
    assert_eq!(got.singular, vec![("quotas".to_string(), "quota".to_string())]);
    assert_eq!(got.regex_rules, 1, "the regex rule is counted, not carried");
}
