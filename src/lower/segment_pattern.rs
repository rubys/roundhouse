//! Compile a Rails route `constraints:` regex (the raw source text
//! between `/…/`/`%r{…}` delimiters — see `ingest::routes::regex_source`)
//! into a small, backtrack-free "segment pattern" that every target's
//! router can check with PLAIN CHAR COMPARISONS, no regex engine
//! required. `runtime/ruby/action_dispatch/router.rb` stays
//! regex-free on purpose (its own comment: "the router stays
//! cross-target lowerable") — Elixir's emitter in particular has no
//! `Regexp#match?`/`=~` dispatch at all, and `Router.match` is a
//! forced tree-shake root on every strict target, so anything it
//! calls must lower everywhere, for every app, constrained or not.
//!
//! # The supported subset
//!
//! A pattern is a sequence of ITEMS, each a literal string or a
//! character class, each with a quantifier (exactly-one, `?`, `*`,
//! `+`). Supported atoms:
//!   - a literal character (escaped or not): `a`, `\-`, `\/`, `\.`, …
//!   - `\d` — the digit class `0-9`
//!   - `[...]`/`[^...]` — a bracket expression: literal members and
//!     `a-b` ranges, optionally negated. Ranges are EXPANDED to their
//!     member characters at compile time, so the runtime check never
//!     needs range syntax, only "is this char in (or not in) this
//!     set".
//!   - `.` — matches any character.
//!   - `(X)` — a group wrapping exactly ONE atom above (no
//!     alternation, no nested groups); only its OWN quantifier
//!     matters, the parens themselves carry nothing.
//!
//! NOT supported (each refuses the whole pattern, compile returns
//! `None`): anchors (`^ $ \A \z \Z \b \B` — Rails itself rejects
//! anchors inside a routing requirement, so refusing here mirrors
//! Rails), backreferences (`\1`, `\k<...>`), lookaround (`(?=` `(?!`
//! `(?<=` `(?<!`), alternation (`|`), any other backslash escape
//! (`\w \s \D \S \W`, …), and a lazy quantifier (`+?` `*?` `??`) that
//! ISN'T the pattern's last item (lazy vs. greedy only coincide, for
//! a whole-segment anchored match with nothing after, when nothing
//! comes after — checked explicitly below).
//!
//! # Why no backtracking is needed
//!
//! The runtime check (`router.rb`'s `segment_pattern_match`) walks
//! items strictly left to right, greedily consuming as much as each
//! quantified item allows, never revisiting an earlier item. That is
//! only CORRECT when a quantified item (`?`/`*`/`+`) can never
//! over-consume a character the NEXT item actually needed — so
//! `check_safe_adjacency` requires every quantified item's character
//! reach to be disjoint from the very next item's. A pattern where
//! that doesn't hold is refused (`None`), never matched approximately.

use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Quant {
    One,
    Opt,
    Star,
    Plus,
}

impl Quant {
    fn code(self) -> char {
        match self {
            Quant::One => '1',
            Quant::Opt => '?',
            Quant::Star => '*',
            Quant::Plus => '+',
        }
    }

    /// Quantifiers whose greedy choice can "steal" a character the
    /// next item needed, and so must be checked for safe adjacency.
    fn variable(self) -> bool {
        matches!(self, Quant::Opt | Quant::Star | Quant::Plus)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Item {
    /// A fixed string, matched exactly once (`quant` is always `One`
    /// here — multi-char literals never repeat), OR a single
    /// character under its own quantifier (`text` is one char).
    Literal { text: String, quant: Quant },
    /// A character class: `chars` lists its members (ranges already
    /// expanded); `negated` flips membership; `any` (dot) ignores
    /// `chars`/`negated` and matches every character.
    Class { chars: Vec<char>, negated: bool, any: bool, quant: Quant },
}

impl Item {
    fn quant(&self) -> Quant {
        match self {
            Item::Literal { quant, .. } => *quant,
            Item::Class { quant, .. } => *quant,
        }
    }
}

/// An item's possible first-matched characters, abstractly: either a
/// known finite set, or "every character except this finite set"
/// (a negated class), or "every character" (`.`).
enum Reach {
    Finite(BTreeSet<char>),
    Complement(BTreeSet<char>),
    Any,
}

fn reach_of(item: &Item) -> Reach {
    match item {
        Item::Literal { text, .. } => {
            Reach::Finite(text.chars().take(1).collect())
        }
        Item::Class { chars, negated, any, .. } => {
            if *any {
                Reach::Any
            } else if *negated {
                Reach::Complement(chars.iter().copied().collect())
            } else {
                Reach::Finite(chars.iter().copied().collect())
            }
        }
    }
}

/// True when `a` (the earlier, variable-length item) can never
/// consume a character `b` (the next item) actually required —
/// the precondition for a single greedy left-to-right pass to be
/// correct with no backtracking.
fn disjoint(a: &Reach, b: &Reach) -> bool {
    match (a, b) {
        (Reach::Any, _) | (_, Reach::Any) => false,
        (Reach::Complement(_), Reach::Complement(_)) => false,
        (Reach::Finite(fa), Reach::Finite(fb)) => fa.is_disjoint(fb),
        (Reach::Finite(f), Reach::Complement(c)) | (Reach::Complement(c), Reach::Finite(f)) => {
            // Finite ∩ Complement(C) = Finite \ C; disjoint iff Finite ⊆ C.
            f.is_subset(c)
        }
    }
}

fn nullable(item: &Item) -> bool {
    matches!(item.quant(), Quant::Opt | Quant::Star)
}

/// A variable item must be disjoint from every item it can meet next:
/// the following item, and past any item that can match nothing, the
/// one after that, up to the first required item. In `a*b?a`, `b?`
/// can be skipped, so `a*` meets the final `a`; a greedy pass would
/// consume it and reject `"a"`, which the regex accepts.
fn check_safe_adjacency(items: &[Item]) -> Option<()> {
    for (idx, item) in items.iter().enumerate() {
        if !item.quant().variable() {
            continue;
        }
        for next in &items[idx + 1..] {
            if !disjoint(&reach_of(item), &reach_of(next)) {
                return None;
            }
            if !nullable(next) {
                break;
            }
        }
    }
    Some(())
}

/// Parse a quantifier (`+ * ?` or none) starting at `*i`, plus a
/// trailing lazy `?` marker. Returns `(quant, lazy)`.
fn parse_quant(chars: &[char], i: &mut usize) -> (Quant, bool) {
    let base = match chars.get(*i) {
        Some('+') => {
            *i += 1;
            Quant::Plus
        }
        Some('*') => {
            *i += 1;
            Quant::Star
        }
        Some('?') => {
            *i += 1;
            Quant::Opt
        }
        _ => return (Quant::One, false),
    };
    if chars.get(*i) == Some(&'?') {
        *i += 1;
        (base, true)
    } else {
        (base, false)
    }
}

/// Parse a `[...]` bracket expression starting just after the `[` (at
/// `*i`). Returns `(members, negated)`; advances `*i` past the `]`.
/// Ranges (`a-b`) are expanded into their member characters.
fn parse_bracket(chars: &[char], i: &mut usize) -> Option<(Vec<char>, bool)> {
    let negated = if chars.get(*i) == Some(&'^') {
        *i += 1;
        true
    } else {
        false
    };
    let mut members: Vec<char> = Vec::new();
    loop {
        match chars.get(*i) {
            None => return None,
            Some(']') => {
                *i += 1;
                break;
            }
            Some('\\') => {
                // An escaped char inside a bracket is always literal
                // (`\/`, `\-`, `\]`, …) — no `\d`/anchors supported
                // here, keeping brackets a closed, simple shape.
                *i += 1;
                let c = *chars.get(*i)?;
                *i += 1;
                if chars.get(*i) == Some(&'-') && chars.get(*i + 1) != Some(&']') {
                    let (lo, hi) = (c, *chars.get(*i + 1)?);
                    if lo > hi {
                        return None;
                    }
                    members.extend(lo..=hi);
                    *i += 2;
                } else {
                    members.push(c);
                }
            }
            Some(&c) => {
                *i += 1;
                // `a-b` range, but `-` right before the closing `]`
                // (or at the very start, already handled by it simply
                // being a member) is a literal hyphen, matching Ruby.
                if chars.get(*i) == Some(&'-') && chars.get(*i + 1) != Some(&']') && chars.get(*i + 1).is_some() {
                    let hi = *chars.get(*i + 1)?;
                    if c > hi {
                        return None;
                    }
                    members.extend(c..=hi);
                    *i += 2;
                } else {
                    members.push(c);
                }
            }
        }
    }
    Some((members, negated))
}

/// Parse the body of a `(...)` group: exactly one atom (a bracket
/// class, `\d`, or `.`), no quantifier of its own (the group's
/// quantifier, parsed by the caller, applies instead). Anything else
/// — alternation, a literal, a nested group, a lookaround marker —
/// is out of the supported subset.
fn parse_group_atom(chars: &[char], i: &mut usize) -> Option<Item> {
    if chars.get(*i) == Some(&'?') {
        return None; // (?=…) (?!…) (?<…) (?:…) — none supported.
    }
    let item = match chars.get(*i) {
        Some('[') => {
            *i += 1;
            let (members, negated) = parse_bracket(chars, i)?;
            Item::Class { chars: members, negated, any: false, quant: Quant::One }
        }
        Some('.') => {
            *i += 1;
            Item::Class { chars: Vec::new(), negated: false, any: true, quant: Quant::One }
        }
        Some('\\') => {
            *i += 1;
            match chars.get(*i) {
                Some('d') => {
                    *i += 1;
                    Item::Class { chars: ('0'..='9').collect(), negated: false, any: false, quant: Quant::One }
                }
                _ => return None,
            }
        }
        _ => return None,
    };
    if chars.get(*i) != Some(&')') {
        return None;
    }
    *i += 1;
    Some(item)
}

fn parse_items(src: &str) -> Option<Vec<Item>> {
    let chars: Vec<char> = src.chars().collect();
    let mut i = 0usize;
    let mut items: Vec<Item> = Vec::new();
    let mut lazy_flags: Vec<bool> = Vec::new();
    let mut pending = String::new();

    while i < chars.len() {
        match chars[i] {
            '^' | '$' => return None,
            '|' => return None,
            ')' => return None,
            '+' | '*' | '?' => return None, // quantifier with nothing before it
            '\\' => {
                i += 1;
                match *chars.get(i)? {
                    'A' | 'z' | 'Z' | 'b' | 'B' => return None,
                    '1'..='9' => return None, // backreference
                    'k' => return None,       // \k<name> backreference
                    'w' | 's' | 'D' | 'S' | 'W' => return None,
                    'd' => {
                        if !pending.is_empty() {
                            items.push(Item::Literal { text: std::mem::take(&mut pending), quant: Quant::One });
                            lazy_flags.push(false);
                        }
                        i += 1;
                        let (quant, lazy) = parse_quant(&chars, &mut i);
                        items.push(Item::Class {
                            chars: ('0'..='9').collect(),
                            negated: false,
                            any: false,
                            quant,
                        });
                        lazy_flags.push(lazy);
                    }
                    esc => {
                        i += 1;
                        let (quant, lazy) = parse_quant(&chars, &mut i);
                        if quant == Quant::One {
                            pending.push(esc);
                        } else {
                            if !pending.is_empty() {
                                items.push(Item::Literal {
                                    text: std::mem::take(&mut pending),
                                    quant: Quant::One,
                                });
                                lazy_flags.push(false);
                            }
                            items.push(Item::Literal { text: esc.to_string(), quant });
                            lazy_flags.push(lazy);
                        }
                    }
                }
            }
            '.' => {
                if !pending.is_empty() {
                    items.push(Item::Literal { text: std::mem::take(&mut pending), quant: Quant::One });
                    lazy_flags.push(false);
                }
                i += 1;
                let (quant, lazy) = parse_quant(&chars, &mut i);
                items.push(Item::Class { chars: Vec::new(), negated: false, any: true, quant });
                lazy_flags.push(lazy);
            }
            '[' => {
                if !pending.is_empty() {
                    items.push(Item::Literal { text: std::mem::take(&mut pending), quant: Quant::One });
                    lazy_flags.push(false);
                }
                i += 1;
                let (members, negated) = parse_bracket(&chars, &mut i)?;
                let (quant, lazy) = parse_quant(&chars, &mut i);
                items.push(Item::Class { chars: members, negated, any: false, quant });
                lazy_flags.push(lazy);
            }
            '(' => {
                if !pending.is_empty() {
                    items.push(Item::Literal { text: std::mem::take(&mut pending), quant: Quant::One });
                    lazy_flags.push(false);
                }
                i += 1;
                let atom = parse_group_atom(&chars, &mut i)?;
                let (quant, lazy) = parse_quant(&chars, &mut i);
                let item = match atom {
                    Item::Class { chars: cs, negated, any, .. } => {
                        Item::Class { chars: cs, negated, any, quant }
                    }
                    Item::Literal { text, .. } => Item::Literal { text, quant },
                };
                items.push(item);
                lazy_flags.push(lazy);
            }
            c => {
                i += 1;
                let (quant, lazy) = parse_quant(&chars, &mut i);
                if quant == Quant::One {
                    pending.push(c);
                } else {
                    if !pending.is_empty() {
                        items.push(Item::Literal { text: std::mem::take(&mut pending), quant: Quant::One });
                        lazy_flags.push(false);
                    }
                    items.push(Item::Literal { text: c.to_string(), quant });
                    lazy_flags.push(lazy);
                }
            }
        }
    }
    if !pending.is_empty() {
        items.push(Item::Literal { text: pending, quant: Quant::One });
        lazy_flags.push(false);
    }
    if items.is_empty() {
        return None;
    }
    // A lazy quantifier only coincides with its greedy reading when
    // nothing follows it (both are forced to consume everything by
    // the whole-segment anchor) — refuse anywhere else.
    let last = items.len() - 1;
    for (idx, lazy) in lazy_flags.iter().enumerate() {
        if *lazy && idx != last {
            return None;
        }
    }
    Some(items)
}

fn encode_item(item: &Item) -> String {
    match item {
        Item::Literal { text, quant } => {
            format!("L{}00{}.{}", quant.code(), text.chars().count(), text)
        }
        Item::Class { chars, negated, any, quant } => {
            let neg = if *negated { '1' } else { '0' };
            let any_c = if *any { '1' } else { '0' };
            let set: String = chars.iter().collect();
            format!("C{}{}{}{}.{}", quant.code(), neg, any_c, set.chars().count(), set)
        }
    }
}

fn encode(items: &[Item]) -> String {
    items.iter().map(encode_item).collect()
}

/// Compile `src` (a regex's raw source text, escapes verbatim — same
/// string `ingest::routes::regex_source` captures) into the encoded
/// segment-pattern form `router.rb`'s `segment_pattern_match` reads,
/// or `None` when it needs more than the supported subset (the route
/// using it must not be emitted — see `lower::routes`).
pub fn compile(src: &str) -> Option<String> {
    let items = parse_items(src)?;
    check_safe_adjacency(&items)?;
    Some(encode(&items))
}

/// Combine a route's already-compiled `(param_name, encoded_pattern)`
/// pairs into the ONE string a `Route` carries as its optional
/// `seg_constraints` positional (mirrors how `int_params` stays a
/// single scalar string rather than an array, so the optional tail is
/// one type on every strict target). Self-delimiting via length
/// prefixes, so neither name nor pattern needs escaping: `router.rb`'s
/// `segment_pattern_for` reads `<namelen>.<name><patlen>.<pattern>`
/// entries back to back until the string is exhausted.
pub fn encode_route_constraints(entries: &[(String, String)]) -> String {
    let mut out = String::new();
    for (name, pattern) in entries {
        out.push_str(&name.chars().count().to_string());
        out.push('.');
        out.push_str(name);
        out.push_str(&pattern.chars().count().to_string());
        out.push('.');
        out.push_str(pattern);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(src: &str) -> String {
        compile(src).unwrap_or_else(|| panic!("expected {src:?} to compile"))
    }

    fn refused(src: &str) {
        assert!(compile(src).is_none(), "expected {src:?} to be refused, got {:?}", compile(src));
    }

    #[test]
    fn digit_plus_compiles() {
        ok(r"\d+");
    }

    #[test]
    fn optional_minus_then_digits_compiles() {
        ok(r"-?\d+");
    }

    #[test]
    fn digits_dash_digits_compiles() {
        ok(r"[0-9]+-[0-9]+");
    }

    #[test]
    fn negated_single_exclusion_compiles() {
        ok(r"[^/]+");
    }

    #[test]
    fn grouped_lazy_class_compiles() {
        ok(r"([^/])+?");
    }

    #[test]
    fn negated_multi_exclusion_compiles() {
        ok(r"[^@/.]+");
    }

    #[test]
    fn literal_prefix_then_any_star_compiles() {
        ok(r"%40.*");
    }

    #[test]
    fn lookahead_is_refused() {
        refused(r"(?=foo)");
    }

    #[test]
    fn backreference_is_refused() {
        refused(r"(\d)\1");
    }

    #[test]
    fn alternation_is_refused() {
        refused(r"foo|bar");
    }

    #[test]
    fn anchors_are_refused() {
        refused(r"\A\d+\z");
        refused(r"^\d+$");
    }

    #[test]
    fn non_terminal_lazy_quantifier_is_refused() {
        refused(r"a+?b");
    }

    #[test]
    fn ambiguous_adjacent_classes_are_refused() {
        // Both classes can match 'a' — a correct engine would need to
        // backtrack; this compiler refuses rather than guess.
        refused(r"[a-z]+[a-m]");
    }

    #[test]
    fn overlap_past_a_nullable_item_is_refused() {
        // `b?` can match nothing, so `a*` also meets the final `a`.
        refused(r"a*b?a");
        refused(r"[0-9]*-?[0-9]");
    }
}
