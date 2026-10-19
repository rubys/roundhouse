pub(super) struct CharSet {
    pub negated: bool,
    pub ranges: Vec<(char, char)>,
}

// A wider `tr` spec is refused rather than written out as a mapping table.
const MAX_EXPANDED: usize = 256;

pub(super) fn parse(spec: &str) -> Option<CharSet> {
    let chars: Vec<char> = spec.chars().collect();
    let negated = chars.len() > 1 && chars[0] == '^';
    let mut i = usize::from(negated);
    let mut ranges = Vec::new();
    while i < chars.len() {
        if chars[i] == '\\' && i + 1 < chars.len() {
            i += 1;
        }
        let lo = chars[i];
        i += 1;
        // Ruby reads a range's upper end as written: `\` does not escape it.
        if i + 1 < chars.len() && chars[i] == '-' {
            let hi = chars[i + 1];
            if lo > hi {
                return None;
            }
            ranges.push((lo, hi));
            i += 2;
        } else {
            ranges.push((lo, lo));
        }
    }
    Some(CharSet { negated, ranges })
}

impl CharSet {
    pub fn class(&self) -> String {
        let mut out = String::from(if self.negated { "[^" } else { "[" });
        for &(lo, hi) in &self.ranges {
            push_escaped(&mut out, lo);
            if hi != lo {
                out.push('-');
                push_escaped(&mut out, hi);
            }
        }
        out.push(']');
        out
    }

    pub fn expand(&self) -> Option<Vec<char>> {
        let mut out = Vec::new();
        for &(lo, hi) in &self.ranges {
            for c in lo..=hi {
                out.push(c);
                if out.len() > MAX_EXPANDED {
                    return None;
                }
            }
        }
        Some(out)
    }
}

pub(super) fn intersection(sets: &[CharSet]) -> String {
    let (last, rest) = sets.split_last().expect("at least one set");
    let mut out = String::new();
    for set in rest {
        out.push_str("(?=");
        out.push_str(&set.class());
        out.push(')');
    }
    out.push_str(&last.class());
    out
}

pub(super) fn push_escaped(out: &mut String, c: char) {
    // `u`-mode rejects an identity escape on anything but a syntax character.
    match c {
        '\\' | ']' | '[' | '^' | '-' | '/' | '.' | '*' | '+' | '?' | '(' | ')' | '{' | '}' | '|' | '$' => {
            out.push('\\');
            out.push(c);
        }
        c if c.is_control() || c == '\u{2028}' || c == '\u{2029}' => {
            out.push_str(&format!("\\u{{{:x}}}", c as u32));
        }
        c => out.push(c),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn class(spec: &str) -> String {
        parse(spec).unwrap().class()
    }

    #[test]
    fn reads_ranges_negation_and_escapes_as_ruby_does() {
        assert_eq!(class("a-z"), "[a-z]");
        assert_eq!(class("^aeiou"), "[^aeiou]");
        assert_eq!(class("^"), "[\\^]");
        assert_eq!(class("a-"), "[a\\-]");
        assert_eq!(class("-a"), "[\\-a]");
        assert_eq!(class("a\\-z"), "[a\\-z]");
        assert_eq!(class("\n"), "[\\u{a}]");
        assert!(parse("z-a").is_none());
        assert_eq!(parse("a-ce").unwrap().expand().unwrap(), vec!['a', 'b', 'c', 'e']);
    }
}
