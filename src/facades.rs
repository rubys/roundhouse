//! Vendored-extras façades — the table, shared by ingest and emit.
//!
//! This lives outside `emit/` because the façade is not only an emit-time
//! substitution: its hand-written `.rbs` is the typing contract ANALYSIS
//! must see. Leaving the table in the emitter meant the contract could
//! only ever be applied after inference had already run, which is how
//! `OpenSSL::Random.random_bytes` stayed untyped and cost lobsters two C
//! errors ten inference links downstream.

/// App-defined classes whose bodies drive un-modeled native/stdlib
/// surface or a spinel-refused control-flow shape. Spinel AOT prices
/// every method body in the reachable require graph, and these bodies
/// cannot compile — so the scaffold base swaps their emitted files for
/// hand-written raising façades at the SAME emit path, leaving the
/// require graph untouched. The CRuby tree, where the real gems/stdlib
/// exist and the source runs as written, restores the verbatim emit via
/// `restore_extras_facades`. Same raise-loudly contract as
/// runtime/ruby/gem_facades.rb.
///
/// - Sponge = Net::HTTP + Resolv + IPAddr + OpenSSL (pending the stdlib
///   spin packages).
/// - Markdowner walks a Markly DOM with a recursive block-driving helper
///   (`walk_text_nodes` forwards `&block` through `Markly::Node#each`);
///   that identity-forwarding block-through-a-yielding-method recursion
///   is a deliberate spinel always-inline boundary (matz/spinel#2948).
///   All consumers are write-path (`markeddown_*` precomputed on save),
///   so the read benchmark never renders markdown. Real fix = a
///   Commonmarker façade over the gem's iterative `Node#walk`.
/// - FlaggedCommenters computes flag statistics with MySQL-only SQL
///   (stddev(), if()) under `Rails.cache.fetch` blocks whose bodies
///   also carry un-modeled calls (`exec_query().first.symbolize_keys!`,
///   select-alias readers); the lobsters-bench capture disables the
///   feature rather than port that SQL to SQLite. Constructor and
///   readers stay real; the statistics methods raise.
pub struct Facade {
    /// Emit path stem, no extension (`app/models/sponge`).
    pub stem: &'static str,
    /// The class the façade defines, as written. Explicit rather than
    /// camelized from `stem`: a rule table beats a derivation, and the
    /// inflector has no business in a four-row list.
    pub class_name: &'static str,
    pub rb: &'static str,
    pub rbs: &'static str,
}

pub const EXTRAS_FACADES: &[Facade] = &[
    Facade {
        stem: "app/models/sponge",
        class_name: "Sponge",
        rb: include_str!("../runtime/spinel/facades/sponge.rb"),
        rbs: include_str!("../runtime/spinel/facades/sponge.rbs"),
    },
    Facade {
        stem: "app/models/markdowner",
        class_name: "Markdowner",
        rb: include_str!("../runtime/spinel/facades/markdowner.rb"),
        rbs: include_str!("../runtime/spinel/facades/markdowner.rbs"),
    },
    Facade {
        stem: "app/models/flagged_commenters",
        class_name: "FlaggedCommenters",
        rb: include_str!("../runtime/spinel/facades/flagged_commenters.rb"),
        rbs: include_str!("../runtime/spinel/facades/flagged_commenters.rbs"),
    },
    Facade {
        stem: "app/models/html_encoder",
        class_name: "HtmlEncoder",
        rb: include_str!("../runtime/spinel/facades/html_encoder.rb"),
        rbs: include_str!("../runtime/spinel/facades/html_encoder.rbs"),
    },
];

/// The façade contracts, parsed into analysis signatures, for those
/// façades whose class this app actually defines. Gated the same way
/// `apply_extras_facades` is a no-op on an absent path — an app with no
/// `Sponge` must not acquire one's signatures.
///
/// Merged into `app.rbs_signatures`, which the analyzer already overlays
/// onto its hardcoded catalog (see `analyzer_rbs_signatures_overlay_the_hardcoded_catalog`).
pub fn signatures_for(
    app: &crate::App,
) -> std::collections::HashMap<crate::ClassId, std::collections::HashMap<crate::Symbol, crate::Ty>>
{
    let mut out: std::collections::HashMap<
        crate::ClassId,
        std::collections::HashMap<crate::Symbol, crate::Ty>,
    > = std::collections::HashMap::new();
    for f in EXTRAS_FACADES {
        let defined = app
            .library_classes
            .iter()
            .any(|lc| lc.name.0.as_str() == f.class_name);
        if !defined {
            continue;
        }
        // A malformed façade contract is a build-time authoring error in
        // THIS repo, not app input; skipping keeps ingest total, and the
        // emitted `.rbs` still carries the text for spinel to read.
        if let Ok(sigs) = crate::rbs::parse_app_signatures(f.rbs) {
            for (class_id, methods) in sigs {
                out.entry(class_id).or_default().extend(methods);
            }
        }
    }
    out
}
