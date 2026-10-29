//! `lower-duration`: write `5.days.ago` as what Active Support's
//! `Duration` does for it, in calls on the time itself, without building
//! the `Duration`.
//!
//! `5.days.ago` allocates an `ActiveSupport::Duration` and its parts hash in
//! `Numeric#days`, then `Duration#ago` calls the private `#sum`, which builds
//! a second hash for `Time#advance`'s keywords. For a `Duration` with a single
//! part, `sum` comes down to one call on `::Time.current`, which is
//! `::Time.zone ? ::Time.zone.now : ::Time.now`, and each of those calls
//! comes down to Ruby's own `Time` arithmetic:
//!
//! - seconds, minutes and hours: `since(sign * n * seconds_per_unit)`. On a
//!   `TimeWithZone` that adds the seconds to its UTC time and wraps the sum
//!   in the zone, so the pass writes that wrap around `::Time.now.utc + s`,
//!   and `::Time.now + s` for a process with no zone;
//! - days, weeks and fortnights: `advance(unit => sign * n)`, which is
//!   calendar arithmetic, so a day across a DST change is still a day of the
//!   wall clock. A `TimeWithZone` hands `advance` to its local time, a UTC
//!   `Time` holding the wall clock, and wraps the result in the zone the way
//!   every forwarded method is wrapped. On a UTC `Time` a day is exactly
//!   86,400 seconds, so the pass forwards Ruby's own `Time#+` instead
//!   (`plus_without_duration`, Active Support's name for it), with the
//!   seconds folded. A process with no zone keeps `::Time.now.advance`: a
//!   local `Time` does have DST;
//! - months and years: `advance(unit => sign * n)` on `::Time.current`. Ruby
//!   has no month arithmetic on `Time`, and doing it with `Date#>>` needs the
//!   time twice, which an expression with no local variable cannot have.
//!
//! The pass writes that call, with the integer folded, in place of the
//! chain: `5.days.ago` becomes
//! `(::Time.zone ? ::Time.zone.now.plus_without_duration(-432000) : ::Time.now.advance(days: -5))`
//! and `2.hours.from_now` becomes
//! `(::Time.zone ? ::ActiveSupport::TimeWithZone.new(::Time.now.utc + 7200, ::Time.zone) : ::Time.now + 7200)`.
//! It handles an integer literal receiver, every unit Active Support defines
//! on `Numeric` and `Integer` (singular and plural), and `ago`, `until`,
//! `before`, `since`, `from_now` and `after` with no argument and no block,
//! on one line. It rewrites only inside a `def`, block or lambda: a chain
//! evaluated once as the file loads (a constant's value) saves nothing.
//! Everything else in the shape `<receiver>.<unit>.<direction>` is declined
//! with the reason.
//!
//! The rewrite rests on the methods it takes out keeping Active Support's
//! definitions, so it declines unless `Gemfile.lock` resolves the version
//! it was checked against, the file activates no refinement, and the app's
//! source defines none of those methods where they could run instead
//! ([`super::duration_definitions`]).

use std::cell::OnceCell;
use std::collections::{BTreeMap, BTreeSet};

use ruby_prism::Visit;

use crate::emit::fractal::pass::{Context, Edit, Pass, Proposal, Unit};
use crate::emit::fractal::scope::Scope;
use crate::ingest::prism_api::PrismLocation;
use crate::span::Span;

/// The `activesupport` version whose `Numeric#days` and its siblings,
/// `Integer#months` and `#years`, `Duration.days` and its siblings,
/// `Duration#initialize`, `#since`, `#ago`, their aliases and `#sum`, and
/// whose `Time.current`, `Time#since`, `#advance` and `#+`, `TimeZone#now`
/// and `TimeWithZone#since`, `#advance` and `#method_missing`, this pass
/// reproduces, checked against rails/rails main. Every other version is
/// declined rather than assumed to behave the same.
pub const VERIFIED_ACTIVESUPPORT_VERSION: &str = "8.2.0.alpha";

/// How `sum` applies one part of a Duration to a time.
#[derive(Clone, Copy)]
enum Application {
    /// `time.since(sign * number * seconds)`.
    Since { seconds: i64 },
    /// `time.advance(part => sign * number)`, where `number` is the receiver
    /// times `factor`: a whole number of days of `days` each, which a
    /// `TimeWithZone`'s wall-clock `Time` (a UTC one) can add as seconds.
    Days { part: &'static str, factor: i64, days: i64 },
    /// `time.advance(part => sign * number)`, where `number` is the receiver
    /// times `factor`: months, whose length varies.
    Advance { part: &'static str, factor: i64 },
}

impl Application {
    /// Active Support methods, besides the Duration's, that the original
    /// calls and the rewrite does not, or the other way around. The app
    /// defining one of these on `Time`, `TimeWithZone` or `TimeZone` blocks
    /// the rewrite ([`super::duration_definitions::TARGETS`]).
    fn time_methods(self) -> &'static [&'static str] {
        match self {
            Application::Since { .. } => &["current", "now", "since", "+", "in_time_zone"],
            Application::Days { .. } => &["current", "advance", "change", "plus_without_duration"],
            Application::Advance { .. } => &[],
        }
    }
}

/// A unit method: the `Duration` constructor it calls and how `sum` applies
/// the part that constructor builds.
struct UnitMethod {
    spelled: &'static str,
    constructor: &'static str,
    application: Application,
}

const UNITS: &[UnitMethod] = &[
    UnitMethod { spelled: "second", constructor: "seconds", application: Application::Since { seconds: 1 } },
    UnitMethod { spelled: "seconds", constructor: "seconds", application: Application::Since { seconds: 1 } },
    UnitMethod { spelled: "minute", constructor: "minutes", application: Application::Since { seconds: 60 } },
    UnitMethod { spelled: "minutes", constructor: "minutes", application: Application::Since { seconds: 60 } },
    UnitMethod { spelled: "hour", constructor: "hours", application: Application::Since { seconds: 3600 } },
    UnitMethod { spelled: "hours", constructor: "hours", application: Application::Since { seconds: 3600 } },
    UnitMethod { spelled: "day", constructor: "days", application: Application::Days { part: "days", factor: 1, days: 1 } },
    UnitMethod { spelled: "days", constructor: "days", application: Application::Days { part: "days", factor: 1, days: 1 } },
    UnitMethod { spelled: "week", constructor: "weeks", application: Application::Days { part: "weeks", factor: 1, days: 7 } },
    UnitMethod { spelled: "weeks", constructor: "weeks", application: Application::Days { part: "weeks", factor: 1, days: 7 } },
    UnitMethod { spelled: "fortnight", constructor: "weeks", application: Application::Days { part: "weeks", factor: 2, days: 7 } },
    UnitMethod { spelled: "fortnights", constructor: "weeks", application: Application::Days { part: "weeks", factor: 2, days: 7 } },
    UnitMethod { spelled: "month", constructor: "months", application: Application::Advance { part: "months", factor: 1 } },
    UnitMethod { spelled: "months", constructor: "months", application: Application::Advance { part: "months", factor: 1 } },
    UnitMethod { spelled: "year", constructor: "years", application: Application::Advance { part: "years", factor: 1 } },
    UnitMethod { spelled: "years", constructor: "years", application: Application::Advance { part: "years", factor: 1 } },
];

/// `Duration`'s direction methods and the sign `sum` applies.
const DIRECTIONS: &[(&str, i64)] = &[("ago", -1), ("until", -1), ("before", -1), ("since", 1), ("from_now", 1), ("after", 1)];

/// Methods every rewrite takes out besides the unit and direction it
/// names: `sum`, which both directions call, and the constructor's.
const ALWAYS_RELIED_ON: &[&str] = &["sum", "new", "initialize"];

#[derive(Default)]
pub struct LowerDuration {
    /// [`app_blockers`]'s answer, computed once per build: it reads the
    /// whole app.
    app_blockers: OnceCell<AppBlockers>,
}

/// Why rewrites cannot be made in this app: for every site
/// (`everywhere`), or for sites relying on a given method name.
#[derive(Default)]
struct AppBlockers {
    everywhere: Option<String>,
    by_name: BTreeMap<String, Vec<String>>,
}

impl Pass for LowerDuration {
    fn name(&self) -> &'static str {
        "lower-duration"
    }

    fn summary(&self) -> &'static str {
        "Writes `5.days.ago` as the Ruby `Time` arithmetic Active Support's Duration comes down to, without building the Duration."
    }

    /// Everything the pass needs it reads from source through [`Context`]:
    /// the compiled file, the lockfile, and the app's Ruby files.
    fn scope(&self) -> Scope {
        Scope::Files(Vec::new())
    }

    fn changes_bytecode(&self) -> bool {
        true
    }

    fn run(&self, context: &Context<'_>, unit: &Unit<'_>) -> Proposal {
        let parsed = ruby_prism::parse(unit.text.as_bytes());
        let mut finder = ChainFinder { unit, candidates: Vec::new(), callable_depth: 0 };
        finder.visit(&parsed.node());
        let mut proposal = Proposal::default();
        if finder.candidates.is_empty() {
            return proposal;
        }
        let mut refinements = super::inline_delegate::RefinementFinder { found: false };
        refinements.visit(&parsed.node());
        let file_blocker = refinements.found.then(|| "the file activates a refinement with `using`, which could refine the unit or Duration method".to_string());
        let blockers = self.app_blockers.get_or_init(|| app_blockers(context));
        for candidate in finder.candidates {
            let outcome = candidate.rewrite.and_then(|(text, relied_on)| {
                if let Some(reason) = file_blocker.as_ref().or(blockers.everywhere.as_ref()) {
                    return Err(reason.clone());
                }
                let defined: Vec<&str> = relied_on.iter().copied().filter(|name| blockers.by_name.contains_key(*name)).collect();
                if defined.is_empty() {
                    return Ok(text);
                }
                let mut sites: Vec<&str> = defined.iter().flat_map(|name| &blockers.by_name[*name]).map(String::as_str).collect();
                sites.sort();
                sites.dedup();
                let names: Vec<String> = defined.iter().map(|name| format!("`{name}`")).collect();
                Err(format!("the app defines {} where it could run instead of Active Support's ({})", names.join(", "), sites.join(", ")))
            });
            match outcome {
                Ok(text) => proposal.edits.push(Edit { span: candidate.span, text }),
                Err(reason) => proposal.declined.push((candidate.span, reason)),
            }
        }
        proposal
    }
}

/// Every method name a rewrite might rely on.
fn relied_on_names() -> BTreeSet<String> {
    UNITS
        .iter()
        .flat_map(|unit| [unit.spelled, unit.constructor])
        .chain(UNITS.iter().flat_map(|unit| unit.application.time_methods().iter().copied()))
        .chain(DIRECTIONS.iter().map(|(name, _)| *name))
        .chain(ALWAYS_RELIED_ON.iter().copied())
        .map(str::to_string)
        .collect()
}

fn app_blockers(context: &Context<'_>) -> AppBlockers {
    let everywhere = match context.gem_version("activesupport") {
        None => Some("Gemfile.lock does not resolve activesupport, so what the Duration would call is unknown".to_string()),
        Some(version) if version != VERIFIED_ACTIVESUPPORT_VERSION => {
            Some(format!("Gemfile.lock resolves activesupport {version}; the Duration methods were checked only against {VERIFIED_ACTIVESUPPORT_VERSION}"))
        }
        Some(_) => None,
    };
    if everywhere.is_some() {
        return AppBlockers { everywhere, by_name: BTreeMap::new() };
    }
    AppBlockers { everywhere: None, by_name: super::duration_definitions::blocking_definitions(context.app_sources(), context.root, &relied_on_names()) }
}

/// One `<receiver>.<unit>.<direction>` chain, and its rewrite (with the
/// method names it relies on) or why not.
struct Candidate {
    span: Span,
    rewrite: Result<(String, Vec<&'static str>), String>,
}

struct ChainFinder<'unit> {
    unit: &'unit Unit<'unit>,
    candidates: Vec<Candidate>,
    /// How many `def`s, blocks and lambdas the current node is inside.
    callable_depth: usize,
}

impl<'parse> Visit<'parse> for ChainFinder<'_> {
    fn visit_def_node(&mut self, node: &ruby_prism::DefNode<'parse>) {
        self.callable_depth += 1;
        ruby_prism::visit_def_node(self, node);
        self.callable_depth -= 1;
    }

    fn visit_block_node(&mut self, node: &ruby_prism::BlockNode<'parse>) {
        self.callable_depth += 1;
        ruby_prism::visit_block_node(self, node);
        self.callable_depth -= 1;
    }

    fn visit_lambda_node(&mut self, node: &ruby_prism::LambdaNode<'parse>) {
        self.callable_depth += 1;
        ruby_prism::visit_lambda_node(self, node);
        self.callable_depth -= 1;
    }

    fn visit_call_node(&mut self, node: &ruby_prism::CallNode<'parse>) {
        if let Some(direction) = DIRECTIONS.iter().find(|(name, _)| node.name().as_slice() == name.as_bytes())
            && let Some(unit_call) = node.receiver().and_then(|receiver| receiver.as_call_node())
            && let Some(unit_method) = UNITS.iter().find(|unit_method| unit_call.name().as_slice() == unit_method.spelled.as_bytes())
            && unit_call.receiver().is_some()
        {
            let location = node.location();
            let rewrite = if self.callable_depth == 0 {
                Err("runs once, as the file loads, so lowering it saves nothing, and `fractal verify` could not check it".to_string())
            } else {
                lowered(self.unit.text, node, &unit_call, unit_method, *direction)
            };
            self.candidates.push(Candidate { span: self.unit.span(location.start_offset(), location.end_offset()), rewrite });
        }
        ruby_prism::visit_call_node(self, node);
    }
}

/// The call `Duration#sum` makes for `direction_call`, or why the chain is
/// not the form this pass handles.
fn lowered(
    text: &str,
    direction_call: &ruby_prism::CallNode<'_>,
    unit_call: &ruby_prism::CallNode<'_>,
    unit_method: &UnitMethod,
    (direction, sign): (&'static str, i64),
) -> Result<(String, Vec<&'static str>), String> {
    let location = direction_call.location();
    if text[location.start_offset()..location.end_offset()].contains('\n') {
        return Err("spans more than one line, and a rewrite keeps every line where it was".to_string());
    }
    for (call, role) in [(direction_call, "direction"), (unit_call, "unit")] {
        let operator = call.call_operator_loc().map(|operator| String::from_utf8_lossy(operator.as_slice()).into_owned()).unwrap_or_default();
        if operator != "." {
            return Err(format!("calls the {role} method with `{operator}`, not `.`"));
        }
        if call.block().is_some() {
            return Err(format!("passes a block to the {role} method"));
        }
    }
    if unit_call.arguments().is_some() {
        return Err(format!("passes arguments to `{}`", unit_method.spelled));
    }
    if direction_call.arguments().is_some() {
        return Err(format!("passes a time to `{direction}`, which Active Support checks with `acts_like?` first"));
    }
    let receiver = unit_call.receiver().expect("a chain's unit call has a receiver");
    let Some(integer) = receiver.as_integer_node() else {
        return Err(format!("`{}` is called on something other than an integer literal, whose class the source does not settle", unit_method.spelled));
    };
    let number: i32 = integer.value().try_into().map_err(|_| "the integer literal does not fit in 32 bits".to_string())?;
    let number = i64::from(number);
    let call = match unit_method.application {
        Application::Since { seconds } => {
            let seconds = sign * number * seconds;
            format!("(::Time.zone ? ::ActiveSupport::TimeWithZone.new(::Time.now.utc + {seconds}, ::Time.zone) : ::Time.now + {seconds})")
        }
        Application::Days { part, factor, days } => {
            let number = sign * (number * factor);
            format!("(::Time.zone ? ::Time.zone.now.plus_without_duration({}) : ::Time.now.advance({part}: {number}))", number * days * 86_400)
        }
        Application::Advance { part, factor } => format!("::Time.current.advance({part}: {})", sign * (number * factor)),
    };
    let mut relied_on = vec![unit_method.spelled, unit_method.constructor, direction];
    relied_on.extend(ALWAYS_RELIED_ON);
    relied_on.extend(unit_method.application.time_methods());
    relied_on.sort();
    relied_on.dedup();
    Ok((call, relied_on))
}
