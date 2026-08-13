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
