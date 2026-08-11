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

/// Escape unescaped `/` so `pattern` can sit inside `/…/` delimiters.
///
/// An already-escaped `\/` is left alone, and a literal backslash `\\`
/// does not make the following `/` look escaped — which is why this
/// tracks `prev_backslash` as a toggle rather than a simple lookback.
pub fn escape_regex_delimiters(pattern: &str) -> String {
    let mut out = String::with_capacity(pattern.len() + 4);
    let mut prev_backslash = false;
    for c in pattern.chars() {
        if c == '/' && !prev_backslash {
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
