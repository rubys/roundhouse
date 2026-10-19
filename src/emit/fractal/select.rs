//! Which source a fractal build may change: the bodies of the classes a
//! person names with `--class`, found by Rubydex.

use crate::App;
use crate::span::Span;

/// The `class`/`module` bodies `--class` selects, and the bodies of
/// other classes nested inside them, which it does not.
pub struct Selection {
    /// (fully qualified name, span of the whole definition), reopenings
    /// included, in file order.
    pub bodies: Vec<(String, Span)>,
    excluded: Vec<Span>,
}

impl Selection {
    /// Every definition of each named class or module. A name the app
    /// does not define is an error: a misspelt `--class` must not quietly
    /// build the identity.
    pub fn resolve(app: &App, classes: &[String]) -> Result<Selection, String> {
        let all = app.const_resolver.for_sources(&app.sources).class_bodies();
        let mut bodies = Vec::new();
        for class in classes {
            let name = class.trim_start_matches("::");
            let found: Vec<_> = all.iter().filter(|(full, _)| full.as_ref() == name).collect();
            if found.is_empty() {
                return Err(format!("--class {class}: no class or module of that name is defined in the app's sources"));
            }
            bodies.extend(found.into_iter().map(|(_, span)| (name.to_string(), *span)));
        }
        bodies.sort_by_key(|(_, span)| (span.file.0, span.start));
        bodies.dedup();
        let excluded = all
            .iter()
            .filter(|(name, span)| {
                !bodies.iter().any(|(selected, _)| selected.as_str() == name.as_ref())
                    && bodies.iter().any(|(_, body)| contains(body, *span) && body != span)
            })
            .map(|(_, span)| *span)
            .collect();
        Ok(Selection { bodies, excluded })
    }

    /// The selected class whose body is the innermost one around `at`,
    /// if that innermost body is selected at all. `--class A --class
    /// A::B::C` over `A::B::C` selects what is written in `A` and in `C`,
    /// but not what is written in `B`.
    pub fn owner_of(&self, at: Span) -> Option<&str> {
        if at.is_synthetic() {
            return None;
        }
        let selected = self.bodies.iter().filter(|(_, s)| contains(s, at)).min_by_key(|(_, s)| s.end - s.start)?;
        let excluded = self.excluded.iter().filter(|s| contains(s, at)).map(|s| s.end - s.start).min();
        match excluded {
            Some(len) if len < selected.1.end - selected.1.start => None,
            _ => Some(selected.0.as_str()),
        }
    }
}

pub(super) fn contains(outer: &Span, inner: Span) -> bool {
    outer.file == inner.file && outer.start <= inner.start && inner.end <= outer.end
}
