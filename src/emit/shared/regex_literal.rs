//! Rendering a Ruby regex into a `/…/`-delimited target literal.
//!
//! Ingest stores prism's UNESCAPED pattern bytes (see
//! `dialect::Literal::Regex`), so a source `\/` arrives here as a bare
//! `/`. Pasting that between `/` delimiters closes the literal early:
//! `/[ …,\/:;…]/` in Ruby became `/[ …,/:;…]/` in Crystal, which reports
//! `invalid regex: missing terminating ] for character class` — at the
//! exact offset of that slash.
//!
//! Every target whose regex literal is `/`-delimited needs this: ruby,
//! crystal, roda, elixir's `~r/…/`. Targets that render the pattern into
//! a STRING literal instead (python's `re.compile("…")`, go, kotlin,
//! csharp) do not — their own string escaping already covers it.

/// Semantic bits of a Ruby regex flag string this build understands.
///
/// Ruby's `m` is only "dot matches newline"; `^`/`$` are always line
/// anchors. Targets that split those modes (RE2, Python, .NET, …) must
/// turn `dot_matches_newline` into *both* their dot-all and their
/// line-anchor options. `extended` (`x`) is carried for targets that
/// can spell it (Python); format-validation ingest gaps it before emit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RubyRegexFlagBits {
    pub ignore_case: bool,
    pub dot_matches_newline: bool,
    pub extended: bool,
}

/// Parse a Ruby flag string into the bits every options-style emitter
/// shares. Unknown letters are ignored (ingest never ships them on the
/// format-validation path).
pub fn parse_ruby_regex_flags(flags: &str) -> RubyRegexFlagBits {
    let mut bits = RubyRegexFlagBits::default();
    for c in flags.chars() {
        match c {
            'i' => bits.ignore_case = true,
            'm' => bits.dot_matches_newline = true,
            'x' => bits.extended = true,
            _ => {}
        }
    }
    bits
}

/// Ruby's `m` flag is "dot matches newline" (what most other regex
/// flavors spell `s` / `DOTALL`); Ruby has no flag for "`^`/`$` match
/// at line boundaries" at all (that's always on, unflagged), so a
/// flagged `m` maps to both `s` and the target's own `m`. A target
/// whose own `m` flag means the PCRE-style "`^`/`$` at line
/// boundaries" instead — Rust's `regex` crate, Go's `regexp` (both
/// RE2 syntax), and JS — silently changes a flagged pattern's meaning
/// if Ruby's `m` is pasted straight through: `/\Aa.b\z/m` would turn
/// into "multi-line anchors" instead of "dot matches newline", which
/// is not just a different flag but a different, wrong, match result.
/// `i` (case-insensitive) is spelled the same everywhere and passes
/// through unchanged; `x` never reaches a renderer (format-validation
/// ingest gaps it, and it isn't valid on these three targets' own `m`
/// mapping) but is left alone here rather than silently dropped on the
/// off chance some other caller passes it.
pub fn translate_ruby_flags_for_re2_style_m(flags: &str) -> String {
    // Ruby's `^`/`$` are line anchors whether or not `m` is given, so a
    // flagged `m` needs the target's line-anchor mode (`m`) as well as
    // its dot-matches-newline (`s`). (An unflagged pattern's `^`/`$` is
    // the same gap, left as it was: changing it would change every
    // regex's emit.)
    let mut out = String::new();
    for c in flags.chars() {
        if c == 'm' {
            out.push_str("sm");
        } else {
            out.push(c);
        }
    }
    out
}

/// Pattern string for an RE2-style `Regex::new` / `MustCompile` call:
/// empty Ruby flags stay bare (an empty `(?)` prefix is a parse error);
/// otherwise prepend `(?<translated>)`.
pub fn re2_style_regex_source(pattern: &str, ruby_flags: &str) -> String {
    let flags = translate_ruby_flags_for_re2_style_m(ruby_flags);
    if flags.is_empty() {
        pattern.to_string()
    } else {
        format!("(?{flags}){pattern}")
    }
}

/// Escape unescaped `/` so `pattern` can sit inside `/…/` delimiters,
/// and an unescaped `#` that starts an interpolation there (`#{`, `#@`,
/// `#$`): a pattern folded from a String (`'x#@name'`) holds the text
/// itself, which a Ruby literal would read as `@name`'s value. `\#` is
/// a literal `#` in Ruby, Crystal (PCRE2) and Elixir alike.
///
/// An already-escaped `\/` is left alone, and a literal backslash `\\`
/// does not make the following `/` look escaped — which is why this
/// tracks `prev_backslash` as a toggle rather than a simple lookback.
pub fn escape_regex_delimiters(pattern: &str) -> String {
    let mut out = String::with_capacity(pattern.len() + 4);
    let mut prev_backslash = false;
    let mut chars = pattern.chars().peekable();
    while let Some(c) = chars.next() {
        let interpolation = c == '#' && matches!(chars.peek(), Some('{' | '@' | '$'));
        if (c == '/' || interpolation) && !prev_backslash {
            out.push('\\');
        }
        out.push(c);
        prev_backslash = c == '\\' && !prev_backslash;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `#{`, `#@` and `#$` would interpolate inside a `/…/` literal; an
    /// escaped one, a lone `#`, and `#` before anything else stay as they
    /// are.
    #[test]
    fn an_interpolation_start_is_escaped() {
        assert_eq!(escape_regex_delimiters("x#@name"), r"x\#@name");
        assert_eq!(escape_regex_delimiters("x#$name"), r"x\#$name");
        assert_eq!(escape_regex_delimiters("x#{y}"), r"x\#{y}");
        assert_eq!(escape_regex_delimiters(r"x\#{y}"), r"x\#{y}");
        assert_eq!(escape_regex_delimiters(r"x\\#@y"), r"x\\\#@y");
        assert_eq!(escape_regex_delimiters("a#b#"), "a#b#");
    }

    /// Ruby's `^`/`$` are always line anchors; its `m` only adds
    /// dot-matches-newline. A flagged `m` keeps both on RE2-style
    /// engines: `/^b.c$/m` matches "a\nb\nc\nd".
    #[test]
    fn rubys_m_maps_to_dotall_and_line_anchors() {
        assert_eq!(translate_ruby_flags_for_re2_style_m("m"), "sm");
        assert_eq!(translate_ruby_flags_for_re2_style_m("im"), "ism");
        assert_eq!(translate_ruby_flags_for_re2_style_m("i"), "i");
    }

    #[test]
    fn parse_bits_split_ruby_flag_letters() {
        assert_eq!(
            parse_ruby_regex_flags("imx"),
            RubyRegexFlagBits {
                ignore_case: true,
                dot_matches_newline: true,
                extended: true,
            }
        );
        assert_eq!(parse_ruby_regex_flags(""), RubyRegexFlagBits::default());
    }

    #[test]
    fn re2_source_skips_empty_flag_group() {
        assert_eq!(re2_style_regex_source("a.b", ""), "a.b");
        assert_eq!(re2_style_regex_source("a.b", "m"), "(?sm)a.b");
        assert_eq!(re2_style_regex_source("^b.c$", "im"), "(?ism)^b.c$");
    }

    #[test]
    fn bare_slash_is_escaped() {
        assert_eq!(escape_regex_delimiters("a/b"), "a\\/b");
    }

    #[test]
    fn already_escaped_slash_is_left_alone() {
        assert_eq!(escape_regex_delimiters("a\\/b"), "a\\/b");
    }

    #[test]
    fn escaped_backslash_does_not_shield_the_next_slash() {
        // `\\/` is a literal backslash then a delimiter — the slash
        // still needs escaping.
        assert_eq!(escape_regex_delimiters("a\\\\/b"), "a\\\\\\/b");
    }

    #[test]
    fn character_class_with_a_slash_survives() {
        // The url_encode pattern that broke the crystal build.
        let got = escape_regex_delimiters("[ !\"\\#$%&'()*+,/:;<=>?@]");
        assert!(got.contains("\\/"), "{got}");
    }
}
