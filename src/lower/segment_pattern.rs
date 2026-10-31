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
//!   - `(X)` / `(?:X)` — a capturing or non-capturing group. The
//!     parens carry nothing (a constraint has no captures to read);
//!     the group's own quantifier applies to its body.
//!   - `|` — ordered alternation. Each branch is compiled into its
//!     own backtrack-free sequence and the runtime tries those
//!     sequences against the WHOLE segment, first success wins. A
//!     group alternation distributes over the rest of the pattern
//!     (`(?:ABR|SNB)\d{12}` is `ABR`+twelve digits, or `SNB`+twelve
//!     digits), so a later failure does not have to rewind into an
//!     earlier branch.
//!   - `{n}`, `{n,m}`, `{n,}` — bounded and open repeats, within a
//!     fixed cap (`MAX_REPEAT` / `MAX_ALTERNATIVES`). An exact repeat
//!     is copies of the atom; a closed range is one alternative per
//!     length; `{n,}` on a single character or class is `n - 1`
//!     exact copies plus a `+`.
//!
//! NOT supported (each refuses the whole pattern, compile returns
//! `None`): anchors (`^ $ \A \z \Z \b \B` — Rails itself rejects
//! anchors inside a routing requirement, so refusing here mirrors
//! Rails), backreferences (`\1`, `\k<...>`), lookaround and other
//! `(?…)` groups besides `(?:…)`, any other backslash escape
//! (`\w \s \D \S \W`, …), a lazy quantifier (`+?` `*?` `??`) that
//! ISN'T the pattern's last item, an unbounded quantifier (`+` `*`
//! `{n,}`) on a multi-character atom or an alternation, and an
//! expansion that would need more than `MAX_ALTERNATIVES` whole
//! patterns (the cross product of several ranges). A `{` that is
//! not a Ruby quantifier stays a literal, as it does in Ruby.
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
    /// `lazy` is recorded only so a non-final lazy quantifier can be
    /// refused; the encoded program is the greedy reading, which
    /// agrees with lazy on a whole-segment match.
    Literal {
        text: String,
        quant: Quant,
        lazy: bool,
    },
    /// A character class: `chars` lists its members (ranges already
    /// expanded); `negated` flips membership; `any` (dot) ignores
    /// `chars`/`negated` and matches every character.
    Class {
        chars: Vec<char>,
        negated: bool,
        any: bool,
        quant: Quant,
        lazy: bool,
    },
}

impl Item {
    fn quant(&self) -> Quant {
        match self {
            Item::Literal { quant, .. } => *quant,
            Item::Class { quant, .. } => *quant,
        }
    }

    fn lazy(&self) -> bool {
        match self {
            Item::Literal { lazy, .. } | Item::Class { lazy, .. } => *lazy,
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
        Item::Literal { text, .. } => Reach::Finite(text.chars().take(1).collect()),
        Item::Class {
            chars,
            negated,
            any,
            ..
        } => {
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
                // An escaped punctuation char inside a bracket is
                // literal (`\/`, `\-`, `\]`, …). A letter escape is a
                // class or a control char in Ruby, never the letter:
                // `[\w-]` is word chars and `-`, not `w` and `-`.
                // `\d` and `\w` expand to their ASCII members (Ruby's
                // meaning without the `u` flag); any other letter
                // escape refuses the pattern, as it does outside.
                *i += 1;
                let c = *chars.get(*i)?;
                *i += 1;
                if c.is_ascii_alphanumeric() {
                    match c {
                        'd' => members.extend('0'..='9'),
                        'w' => {
                            members.extend('a'..='z');
                            members.extend('A'..='Z');
                            members.extend('0'..='9');
                            members.push('_');
                        }
                        _ => return None,
                    }
                    continue;
                }
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
                if chars.get(*i) == Some(&'-') && chars.get(*i + 1) != Some(&']') && chars.get(*i + 1).is_some()
                {
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

/// How many times the preceding atom is repeated. `AtLeast` is `{n,}`;
/// `Between` is `{n,m}`.
#[derive(Clone, Copy)]
enum Rep {
    One,
    Opt,
    Star,
    Plus,
    Exact(u16),
    AtLeast(u16),
    Between(u16, u16),
}

/// An unexpanded pattern. Alternation and bounded repeats become
/// several [`Item`] sequences in [`expand`]; the runtime never sees
/// this tree.
#[derive(Clone)]
enum Ast {
    Empty,
    Leaf(Item),
    Concat(Vec<Ast>),
    Alt(Vec<Ast>),
}

/// Expansion caps. A route constraint that needs more than this is
/// refused rather than turned into a large program. Twelve-digit
/// repeats and a handful of literal alternatives (the shapes route
/// files actually write) sit well under both.
const MAX_REPEAT: usize = 64;
const MAX_ALTERNATIVES: usize = 64;

fn literal_item(text: impl Into<String>) -> Item {
    Item::Literal {
        text: text.into(),
        quant: Quant::One,
        lazy: false,
    }
}

fn class_item(chars: Vec<char>, negated: bool, any: bool) -> Item {
    Item::Class {
        chars,
        negated,
        any,
        quant: Quant::One,
        lazy: false,
    }
}

fn with_quant(item: &Item, quant: Quant, lazy: bool) -> Item {
    match item {
        Item::Literal { text, .. } => Item::Literal {
            text: text.clone(),
            quant,
            lazy,
        },
        Item::Class {
            chars,
            negated,
            any,
            ..
        } => Item::Class {
            chars: chars.clone(),
            negated: *negated,
            any: *any,
            quant,
            lazy,
        },
    }
}

/// A single unquantified character or class — the only atoms `?`,
/// `+`, `*`, and `{n,}` can attach to without enumerating copies.
/// A multi-character literal (`(?:ab)?`) is not one: the runtime's
/// quantified literal match compares a single character.
fn repeatable_atom(ast: &Ast) -> Option<Item> {
    let seqs = expand(ast)?;
    if seqs.len() != 1 {
        return None;
    }
    let mut seq = seqs.into_iter().next()?;
    if seq.len() != 1 {
        return None;
    }
    let item = seq.pop()?;
    if item.quant() != Quant::One {
        return None;
    }
    match &item {
        Item::Literal { text, .. } if text.chars().count() == 1 => Some(item),
        Item::Class { .. } => Some(item),
        _ => None,
    }
}

fn apply_rep(ast: Ast, rep: Rep, lazy: bool) -> Option<Ast> {
    match rep {
        Rep::One => Some(ast),
        Rep::Opt => {
            if let Some(item) = repeatable_atom(&ast) {
                Some(Ast::Leaf(with_quant(&item, Quant::Opt, lazy)))
            } else {
                Some(Ast::Alt(vec![ast, Ast::Empty]))
            }
        }
        Rep::Star => unbounded(ast, Quant::Star, lazy),
        Rep::Plus => unbounded(ast, Quant::Plus, lazy),
        Rep::Exact(0) => Some(Ast::Empty),
        Rep::Exact(n) => {
            if n as usize > MAX_REPEAT {
                return None;
            }
            Some(Ast::Concat((0..n).map(|_| ast.clone()).collect()))
        }
        Rep::AtLeast(0) => unbounded(ast, Quant::Star, lazy),
        Rep::AtLeast(1) => unbounded(ast, Quant::Plus, lazy),
        Rep::AtLeast(n) => {
            if n as usize > MAX_REPEAT {
                return None;
            }
            let item = repeatable_atom(&ast)?;
            let mut parts = Vec::new();
            for _ in 0..n - 1 {
                parts.push(Ast::Leaf(with_quant(&item, Quant::One, false)));
            }
            parts.push(Ast::Leaf(with_quant(&item, Quant::Plus, lazy)));
            Some(Ast::Concat(parts))
        }
        Rep::Between(lo, hi) => {
            if hi < lo || (hi - lo) as usize > MAX_REPEAT {
                return None;
            }
            let mut alts = Vec::new();
            for n in lo..=hi {
                alts.push(apply_rep(ast.clone(), Rep::Exact(n), false)?);
            }
            Some(Ast::Alt(alts))
        }
    }
}

fn unbounded(ast: Ast, quant: Quant, lazy: bool) -> Option<Ast> {
    let item = repeatable_atom(&ast)?;
    Some(Ast::Leaf(with_quant(&item, quant, lazy)))
}

/// Merge adjacent exact literals so `SEE` is one item, not three.
fn coalesce(items: &mut Vec<Item>) {
    let mut out: Vec<Item> = Vec::new();
    for item in items.drain(..) {
        if let Item::Literal {
            text,
            quant: Quant::One,
            lazy: false,
        } = &item
            && let Some(Item::Literal {
                text: prev,
                quant: Quant::One,
                lazy: false,
            }) = out.last_mut()
        {
            prev.push_str(text);
            continue;
        }
        out.push(item);
    }
    *items = out;
}

/// Every way `ast` can match, as backtrack-free item sequences.
/// `None` when the cross product would pass [`MAX_ALTERNATIVES`].
fn expand(ast: &Ast) -> Option<Vec<Vec<Item>>> {
    match ast {
        Ast::Empty => Some(vec![vec![]]),
        Ast::Leaf(item) => Some(vec![vec![item.clone()]]),
        Ast::Concat(parts) => {
            let mut acc: Vec<Vec<Item>> = vec![vec![]];
            for part in parts {
                let next = expand(part)?;
                let mut combined = Vec::new();
                for left in &acc {
                    for right in &next {
                        if combined.len() >= MAX_ALTERNATIVES {
                            return None;
                        }
                        let mut row = left.clone();
                        row.extend(right.iter().cloned());
                        coalesce(&mut row);
                        combined.push(row);
                    }
                }
                acc = combined;
            }
            Some(acc)
        }
        Ast::Alt(parts) => {
            let mut out = Vec::new();
            for part in parts {
                for seq in expand(part)? {
                    if out.len() >= MAX_ALTERNATIVES {
                        return None;
                    }
                    out.push(seq);
                }
            }
            if out.is_empty() { None } else { Some(out) }
        }
    }
}

struct Parser {
    chars: Vec<char>,
    i: usize,
}

impl Parser {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.i).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.i += 1;
        Some(c)
    }

    /// `{n}`, `{n,}`, or `{n,m}` starting at `i`. `{,m}` is not a Ruby
    /// quantifier — the brace stays a literal character.
    fn brace_is_quantifier(&self) -> bool {
        if self.peek() != Some('{') {
            return false;
        }
        let mut j = self.i + 1;
        if !self.chars.get(j).is_some_and(|c| c.is_ascii_digit()) {
            return false;
        }
        while self.chars.get(j).is_some_and(|c| c.is_ascii_digit()) {
            j += 1;
        }
        if self.chars.get(j) == Some(&',') {
            j += 1;
            while self.chars.get(j).is_some_and(|c| c.is_ascii_digit()) {
                j += 1;
            }
        }
        self.chars.get(j) == Some(&'}')
    }

    fn parse_number(&mut self) -> Option<u16> {
        let mut n: u32 = 0;
        let mut digits = 0u32;
        while let Some(c) = self.peek() {
            if !c.is_ascii_digit() {
                break;
            }
            digits += 1;
            self.bump();
            n = n
                .saturating_mul(10)
                .saturating_add(u32::from(c as u8 - b'0'));
        }
        if digits == 0 {
            return None;
        }
        u16::try_from(n).ok()
    }

    fn parse_brace(&mut self) -> Option<Rep> {
        self.bump()?; // '{'
        let lo = self.parse_number()?;
        let rep = if self.peek() == Some(',') {
            self.bump();
            if self.peek().is_some_and(|c| c.is_ascii_digit()) {
                Rep::Between(lo, self.parse_number()?)
            } else {
                Rep::AtLeast(lo)
            }
        } else {
            Rep::Exact(lo)
        };
        if self.bump() != Some('}') {
            return None;
        }
        Some(rep)
    }

    fn parse_rep(&mut self) -> Option<(Rep, bool)> {
        let rep = match self.peek() {
            Some('+') => {
                self.bump();
                Rep::Plus
            }
            Some('*') => {
                self.bump();
                Rep::Star
            }
            Some('?') => {
                self.bump();
                Rep::Opt
            }
            Some('{') if self.brace_is_quantifier() => self.parse_brace()?,
            _ => return Some((Rep::One, false)),
        };
        let lazy = if self.peek() == Some('?') {
            self.bump();
            true
        } else {
            false
        };
        Some((rep, lazy))
    }

    fn parse_escape(&mut self) -> Option<Ast> {
        let escaped = self.bump()?;
        if escaped == 'd' {
            return Some(Ast::Leaf(class_item(('0'..='9').collect(), false, false)));
        }
        // `\h`, `\n`, `\w`, `\1`, and the rest are not "the character
        // after the slash". Only a non-alphanumeric escape is literal.
        if escaped.is_ascii_alphanumeric() {
            return None;
        }
        Some(Ast::Leaf(literal_item(escaped.to_string())))
    }

    fn parse_atom(&mut self) -> Option<Ast> {
        match self.peek()? {
            '(' => {
                self.bump();
                // `(?:…)` groups. Every other `(?…)` form (lookaround,
                // named captures, flags) is outside the subset.
                if self.peek() == Some('?') {
                    self.bump();
                    if self.bump() != Some(':') {
                        return None;
                    }
                }
                let inner = self.parse_alt()?;
                if self.bump() != Some(')') {
                    return None;
                }
                Some(inner)
            }
            '[' => {
                self.bump();
                let (members, negated) = parse_bracket(&self.chars, &mut self.i)?;
                Some(Ast::Leaf(class_item(members, negated, false)))
            }
            '.' => {
                self.bump();
                Some(Ast::Leaf(class_item(Vec::new(), false, true)))
            }
            '\\' => {
                self.bump();
                self.parse_escape()
            }
            '^' | '$' | ')' | '|' | '+' | '*' | '?' => None,
            '{' if self.brace_is_quantifier() => None,
            c => {
                self.bump();
                Some(Ast::Leaf(literal_item(c.to_string())))
            }
        }
    }

    fn parse_piece(&mut self) -> Option<Ast> {
        let atom = self.parse_atom()?;
        let (rep, lazy) = self.parse_rep()?;
        apply_rep(atom, rep, lazy)
    }

    fn parse_concat(&mut self) -> Option<Ast> {
        let mut parts = Vec::new();
        loop {
            match self.peek() {
                None | Some('|' | ')') => break,
                _ => parts.push(self.parse_piece()?),
            }
        }
        match parts.len() {
            0 => Some(Ast::Empty),
            1 => parts.pop(),
            _ => Some(Ast::Concat(parts)),
        }
    }

    fn parse_alt(&mut self) -> Option<Ast> {
        let mut alts = vec![self.parse_concat()?];
        while self.peek() == Some('|') {
            self.bump();
            alts.push(self.parse_concat()?);
        }
        if alts.len() == 1 {
            alts.pop()
        } else {
            Some(Ast::Alt(alts))
        }
    }
}

fn parse_pattern(src: &str) -> Option<Ast> {
    let mut parser = Parser {
        chars: src.chars().collect(),
        i: 0,
    };
    let ast = parser.parse_alt()?;
    if parser.i != parser.chars.len() {
        return None;
    }
    Some(ast)
}

fn encode_item(item: &Item) -> String {
    match item {
        Item::Literal { text, quant, .. } => {
            format!("L{}00{}.{}", quant.code(), text.chars().count(), text)
        }
        Item::Class {
            chars,
            negated,
            any,
            quant,
            ..
        } => {
            let neg = if *negated { '1' } else { '0' };
            let any_c = if *any { '1' } else { '0' };
            let set: String = chars.iter().collect();
            format!(
                "C{}{}{}{}.{}",
                quant.code(),
                neg,
                any_c,
                set.chars().count(),
                set
            )
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
fn encode_alternatives(alts: &[String]) -> String {
    let mut out = String::from("A");
    for alt in alts {
        out.push_str(&alt.chars().count().to_string());
        out.push('.');
        out.push_str(alt);
    }
    out
}

/// A lazy quantifier only coincides with its greedy reading when
/// nothing follows it — both are forced to consume the rest of the
/// segment. A lazy flag on an earlier item is refused.
fn lazy_is_terminal(items: &[Item]) -> bool {
    let last = items.len().saturating_sub(1);
    items
        .iter()
        .enumerate()
        .all(|(idx, item)| !item.lazy() || idx == last)
}

/// An alternation of no sequences: the pattern no value matches
/// (`segment_pattern_alt_match` tries none). `format: false` is the
/// `(.:format)` requirement no extension meets.
pub const NEVER: &str = "A";

pub fn compile(src: &str) -> Option<String> {
    let ast = parse_pattern(src)?;
    let seqs = expand(&ast)?;
    if seqs.is_empty() || (seqs.len() == 1 && seqs[0].is_empty()) {
        return None;
    }
    let mut encoded = Vec::with_capacity(seqs.len());
    for seq in &seqs {
        if !lazy_is_terminal(seq) {
            return None;
        }
        check_safe_adjacency(seq)?;
        encoded.push(encode(seq));
    }
    if encoded.len() == 1 {
        return Some(encoded.pop()?);
    }
    Some(encode_alternatives(&encoded))
}

// A copy of router.rb's `format_extension` list, not read from it: the runtime is Ruby source, not data lowering can load.
const ROUTER_FORMATS: &str = "html text js css ics csv vcf vtt png jpeg jpg gif bmp tiff svg webp mpeg mp3 mp4 ogg webm otf ttf woff woff2 pdf zip gzip xml rss atom yaml json turbo_stream";

/// A requirement that stops a lazy segment before one of the listed
/// extensions, the end of the segment or the next `/` (lobsters'
/// `DOMAINS_IDENTIFIER = /([^\/]+?)(?=\.json|\.rss|$|\/)/`): the segment
/// itself is the greedy body (`[^\/]+`), and a trailing listed extension
/// is the `(.:format)` Rails splits off there. Answers the body's source
/// and the extensions, or None for any other shape.
pub fn split_format_lookahead(src: &str) -> Option<(String, Vec<String>)> {
    let (body, look) = src.split_once("(?=")?;
    let look = look.strip_suffix(')')?;
    let mut exts = Vec::new();
    for alt in look.split('|') {
        match alt {
            "$" | "/" | "\\/" => {}
            _ => {
                let ext = alt.strip_prefix("\\.")?;
                // Not any word: `Router.match` peels only a registered format, so another listed suffix would stay in the param.
                if !ROUTER_FORMATS.split(' ').any(|f| f == ext) {
                    return None;
                }
                exts.push(ext.to_string());
            }
        }
    }
    // Not without `$`: then the segment has to end in one of the extensions, a rule the body alone does not say.
    if !look.split('|').any(|a| a == "$") || exts.is_empty() {
        return None;
    }
    let body = body.strip_prefix('(').and_then(|b| b.strip_suffix(')')).unwrap_or(body);
    let body = body.strip_suffix("+?").map(|b| format!("{b}+"))?;
    Some((body, exts))
}

/// A requirement whose body is `.+` (greedy) or `.+?` (lazy), which
/// Rails matches across `/` (lobsters' `ORIGINS_IDENTIFIER =
/// /(.+)(?=\.json|\.rss|$|\/)/` takes `github.com/user`). The param is
/// then a span of segments, as a `*glob` is.
#[derive(Debug, PartialEq, Eq)]
pub struct DotSpan {
    /// The extensions split off as the format when the param ends the
    /// path: none for the greedy body, which keeps every extension
    /// (`/origins/github.com.rss` is the identifier `github.com.rss`).
    pub exts: Vec<String>,
    /// Whether the span may be followed by `/`: false when a lookahead
    /// leaves `/` out, so the param can only end the path.
    pub slash_follows: bool,
}

/// The `.+` / `.+?` requirement `DotSpan` reads, or None for any other
/// shape. A lookahead must list `$`; the lazy body needs one, since
/// without it Rails splits ANY trailing extension off, which the
/// router's registered-format peel cannot follow.
pub fn dot_span(src: &str) -> Option<DotSpan> {
    let (body, look) = match src.split_once("(?=") {
        Some((body, look)) => (body, Some(look.strip_suffix(')')?)),
        None => (src, None),
    };
    let body = body
        .strip_prefix("(?:")
        .or_else(|| body.strip_prefix('('))
        .and_then(|b| b.strip_suffix(')'))
        .unwrap_or(body);
    let lazy = match body {
        ".+" => false,
        ".+?" => true,
        _ => return None,
    };
    let Some(look) = look else {
        return (!lazy).then_some(DotSpan { exts: Vec::new(), slash_follows: true });
    };
    let mut exts = Vec::new();
    let mut end = false;
    let mut slash_follows = false;
    for alt in look.split('|') {
        match alt {
            "$" => end = true,
            "/" | "\\/" => slash_follows = true,
            _ => {
                let ext = alt.strip_prefix("\\.")?;
                if ext.is_empty() || !ext.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                    return None;
                }
                exts.push(ext.to_string());
            }
        }
    }
    if !end || (lazy && exts.is_empty()) {
        return None;
    }
    // Not any word: a lazy span leaves the listed extension to the router's peel, which takes only these.
    if lazy && exts.iter().any(|ext| !ROUTER_FORMATS.split(' ').any(|f| f == ext)) {
        return None;
    }
    if !lazy {
        exts.clear();
    }
    Some(DotSpan { exts, slash_follows })
}

/// The `seg_patterns` key that marks `name` as a span of segments: the
/// route's dispatch pattern holds it as `*name` (`dispatch_path`).
pub fn span_key(name: &str) -> String {
    format!("*{name}")
}

/// A route's path as the router matches it: each param marked by
/// `span_key` becomes the `*name` segment the router spans.
pub fn dispatch_path(path: &str, entries: &[(String, String)]) -> String {
    path.split('/')
        .map(|seg| match seg.strip_prefix(':') {
            Some(name) if entries.iter().any(|(k, _)| *k == span_key(name)) => span_key(name),
            _ => seg.to_string(),
        })
        .collect::<Vec<_>>()
        .join("/")
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
        if name.starts_with('*') {
            continue;
        }
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
    #[test]
    fn a_dot_body_spans_segments() {
        use super::{DotSpan, dot_span};
        let span = |exts: &[&str], slash_follows| {
            Some(DotSpan { exts: exts.iter().map(|e| e.to_string()).collect(), slash_follows })
        };
        assert_eq!(dot_span(r"(.+)(?=\.json|\.rss|$|\/)"), span(&[], true));
        assert_eq!(dot_span(r"(.+?)(?=\.json|\.rss|$|\/)"), span(&["json", "rss"], true));
        assert_eq!(dot_span(r".+"), span(&[], true));
        assert_eq!(dot_span(r"(.+)(?=\.json|$)"), span(&[], false));
        // Not without `$`, not a lazy body with no extension to split, not any other body.
        assert_eq!(dot_span(r"(.+)(?=\.json|\.rss)"), None);
        assert_eq!(dot_span(r".+?"), None);
        assert_eq!(dot_span(r"(.+?)(?=$|\/)"), None);
        assert_eq!(dot_span(r"([^\/]+?)(?=\.json|\.rss|$|\/)"), None);
        assert_eq!(dot_span(r".*"), None);
        assert_eq!(dot_span(r"(.+?)(?=\.foo|$)"), None);
        assert_eq!(dot_span(r"(.+)(?=\.foo|$)"), span(&[], false));
    }

    #[test]
    fn a_format_lookahead_splits_into_its_body_and_extensions() {
        assert_eq!(
            super::split_format_lookahead(r"([^\/]+?)(?=\.json|\.rss|$|\/)"),
            Some((r"[^\/]+".to_string(), vec!["json".to_string(), "rss".to_string()]))
        );
        // Not a greedy body (it would swallow the extension), and not without `$`.
        assert_eq!(super::split_format_lookahead(r"(.+)(?=\.json|\.rss|$|\/)"), None);
        assert_eq!(super::split_format_lookahead(r"([^\/]+?)(?=\.json|\.rss)"), None);
        // An extension the router does not peel would stay in the param.
        assert_eq!(super::split_format_lookahead(r"([^\/]+?)(?=\.foo|$)"), None);
    }

    use super::*;

    fn ok(src: &str) -> String {
        compile(src).unwrap_or_else(|| panic!("expected {src:?} to compile"))
    }

    fn refused(src: &str) {
        assert!(
            compile(src).is_none(),
            "expected {src:?} to be refused, got {:?}",
            compile(src)
        );
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
    fn a_word_escape_inside_a_bracket_is_the_word_class() {
        // `[\w-]+` is word characters and `-`, not the letters `w` and `-`.
        let enc = ok(r"[\w-]+");
        for c in ['a', 'Z', '0', '_', '-'] {
            assert!(enc.contains(c), "{enc} lacks {c:?}");
        }
        assert_eq!(ok(r"[\d]+"), ok(r"[0-9]+"));
    }

    #[test]
    fn other_letter_escapes_inside_a_bracket_are_refused() {
        refused(r"[\s]+");
        refused(r"[\W-]+");
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
    fn alternation_of_literals_compiles_to_whole_segment_alternatives() {
        assert_eq!(ok(r"foo|bar"), "A9.L1003.foo9.L1003.bar");
    }

    #[test]
    fn grouped_alternation_distributes_over_an_exact_repeat() {
        let enc = ok(r"(?:ABR|SNB)\d{12}");
        assert!(enc.starts_with('A'), "{enc}");
        assert!(enc.contains("ABR") && enc.contains("SNB"), "{enc}");
        let digits = "C10010.0123456789";
        assert_eq!(
            enc.matches(digits).count(),
            24,
            "each branch repeats the class twelve times: {enc}"
        );
        // A capturing group is the same constraint: nothing reads the capture.
        assert_eq!(ok(r"(ABR|SNB)\d{12}"), enc);
    }

    #[test]
    fn an_exact_brace_repeat_is_copies_of_the_atom() {
        let digits = "C10010.0123456789";
        let enc = ok(r"SEE\d{12}");
        assert!(enc.starts_with("L1003.SEE"), "{enc}");
        assert_eq!(enc.matches(digits).count(), 12, "{enc}");
        assert_eq!(ok(r"a{3}"), "L1003.aaa");
    }

    #[test]
    fn a_closed_range_is_one_alternative_per_length() {
        let enc = ok(r"a{2,4}");
        assert!(
            enc.contains("L1002.aa") && enc.contains("L1003.aaa") && enc.contains("L1004.aaaa"),
            "{enc}"
        );
    }

    #[test]
    fn an_open_repeat_keeps_a_single_backtrack_free_sequence() {
        let enc = ok(r"\d{2,}");
        assert!(
            !enc.starts_with('A'),
            "a single class does not need alternatives: {enc}"
        );
        assert!(enc.contains("C+"), "{enc}");
    }

    #[test]
    fn different_length_branches_stay_ordered_whole_matches() {
        assert_eq!(ok(r"(?:a|ab)c"), "A8.L1002.ac9.L1003.abc");
    }

    #[test]
    fn an_unclosed_brace_stays_a_literal() {
        assert_eq!(ok(r"a{"), "L1002.a{");
        assert_eq!(ok(r"a{,2}"), "L1005.a{,2}");
    }

    #[test]
    fn unbounded_repetition_of_an_alternation_is_refused() {
        refused(r"(?:a|b)+");
        refused(r"(?:ab){2,}");
    }

    #[test]
    fn a_reversed_brace_range_is_refused() {
        refused(r"a{2,1}");
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
