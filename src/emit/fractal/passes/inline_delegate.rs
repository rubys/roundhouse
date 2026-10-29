//! `inline-delegate`: write a `delegate` call out as the methods Active
//! Support would generate from it.
//!
//! `delegate :name, to: :shop` builds Ruby source for each method at boot
//! and `module_eval`s it, in the class, at the `delegate` call's file and
//! line. This pass puts that exact source where the call was, so Rails
//! does not have to generate it, Bootsnap can cache it with the rest of the
//! file, and the JIT sees a plain `def`.
//!
//! Only the simplest form is rewritten, where the generated source is fully
//! known from the call itself: a bare statement of a `class`, `module` or
//! `class << self` body, naming methods with symbol literals and the target
//! with `to:` and a symbol literal that names a method. The generated
//! methods go on the call's first line, which is the line Active Support
//! gives them too, and the rest of the call's lines are left empty so every
//! line after it stays where it was.
//! Every other `delegate` call is declined with the reason. The generated
//! source depends on the Active Support version, so the pass also declines
//! unless `Gemfile.lock` resolves the version it was checked against, and
//! unless the app's own source defines no `delegate` method that a class
//! body could call instead of Active Support's
//! ([`super::delegate_definitions`]).

use std::cell::OnceCell;

use ruby_prism::Visit;

use crate::emit::fractal::pass::{Context, Edit, Pass, Proposal, Unit};
use crate::emit::fractal::scope::Scope;
use crate::ingest::prism_api::PrismLocation;
use crate::span::Span;

/// The `activesupport` version whose `ActiveSupport::Delegation.generate`
/// this pass reproduces, checked against rails/rails main. Every other
/// version is declined rather than assumed to generate the same source.
pub const VERIFIED_ACTIVESUPPORT_VERSION: &str = "8.2.0.alpha";

/// Names Active Support rewrites to `self.<name>` when they are the
/// target, or that are Ruby keywords, as `ActiveSupport::Delegation`'s
/// `RESERVED_METHOD_NAMES`. A target or method with one of these names is
/// declined rather than reproduced.
const RESERVED_METHOD_NAMES: &[&str] = &[
    "__ENCODING__", "__LINE__", "__FILE__", "alias", "and", "BEGIN", "begin", "break", "case", "class", "def", "defined?", "do",
    "else", "elsif", "END", "end", "ensure", "false", "for", "if", "in", "module", "next", "nil", "not", "or", "redo", "rescue",
    "retry", "return", "self", "super", "then", "true", "undef", "unless", "until", "when", "while", "yield", "_", "arg", "args",
    "block",
];

/// Calls that, with no arguments, change the visibility of every `def`
/// after them in the same body.
const VISIBILITY_SECTIONS: &[&str] = &["private", "protected", "public", "module_function"];

/// Calls that invoke the method named by their first argument.
const SEND_CALLS: &[&str] = &["send", "__send__", "public_send"];

#[derive(Default)]
pub struct InlineDelegate {
    /// [`app_blocker`]'s answer, computed once per build: it reads the
    /// whole app.
    app_blocker: OnceCell<Option<String>>,
}

impl Pass for InlineDelegate {
    fn name(&self) -> &'static str {
        "inline-delegate"
    }

    fn summary(&self) -> &'static str {
        "Writes `delegate :name, to: :target` out as the methods Active Support would generate from it at boot."
    }

    /// Everything the pass needs it reads from source through [`Context`]:
    /// the compiled file, the lockfile, and the app's Ruby files.
    fn scope(&self) -> Scope {
        Scope::Files(Vec::new())
    }

    fn run(&self, context: &Context<'_>, unit: &Unit<'_>) -> Proposal {
        let parsed = ruby_prism::parse(unit.text.as_bytes());
        let mut finder = DelegateFinder { unit, candidates: Vec::new(), direct_starts: Vec::new() };
        finder.visit(&parsed.node());
        let mut proposal = Proposal::default();
        if finder.candidates.is_empty() {
            return proposal;
        }
        // A def compiled in this file sees its `using` refinements; the
        // source Active Support module_evals does not.
        let mut refinements = RefinementFinder { found: false };
        refinements.visit(&parsed.node());
        let file_blocker = refinements
            .found
            .then(|| "the file activates a refinement with `using`, which a def written here would see and Active Support's module_eval'd methods do not".to_string());
        let blocker = self.app_blocker.get_or_init(|| app_blocker(context));
        for candidate in finder.candidates {
            let outcome = match (file_blocker.as_ref().or(blocker.as_ref()), candidate.rewrite) {
                (_, Err(reason)) => Err(reason),
                (Some(reason), Ok(_)) => Err(reason.clone()),
                (None, Ok(text)) => Ok(text),
            };
            match outcome {
                Ok(text) => proposal.edits.push(Edit { span: candidate.span, text }),
                Err(reason) => proposal.declined.push((candidate.span, reason)),
            }
        }
        proposal
    }
}

/// Why no `delegate` call in this app can be rewritten, if one applies:
/// an Active Support version the generated source was not checked against,
/// or an app definition of `delegate` that a class body might call instead.
fn app_blocker(context: &Context<'_>) -> Option<String> {
    match context.gem_version("activesupport") {
        None => return Some("Gemfile.lock does not resolve activesupport, so the source `delegate` generates is unknown".to_string()),
        Some(version) if version != VERIFIED_ACTIVESUPPORT_VERSION => {
            return Some(format!(
                "Gemfile.lock resolves activesupport {version}; the generated source was checked only against {VERIFIED_ACTIVESUPPORT_VERSION}"
            ));
        }
        Some(_) => {}
    }
    let sites = super::delegate_definitions::blocking_definitions(context.app_sources(), context.root);
    if !sites.is_empty() {
        return Some(format!(
            "the app defines a `delegate` method a class body could call instead of Active Support's ({})",
            sites.join(", ")
        ));
    }
    None
}

/// One `delegate` call with no receiver, and its rewrite or why not.
struct Candidate {
    span: Span,
    rewrite: Result<String, String>,
}

struct DelegateFinder<'unit> {
    unit: &'unit Unit<'unit>,
    candidates: Vec<Candidate>,
    /// Start offsets of the calls already judged as direct body statements.
    direct_starts: Vec<usize>,
}

impl DelegateFinder<'_> {
    /// Judge the `delegate` calls that are direct statements of a body.
    fn judge_body(&mut self, body: Option<ruby_prism::Node<'_>>) {
        let Some(statements) = body.and_then(|body| body.as_statements_node()) else { return };
        let statements: Vec<ruby_prism::Node<'_>> = statements.body().iter().collect();
        let mut after_visibility_section = false;
        for (index, statement) in statements.iter().enumerate() {
            let Some(call) = statement.as_call_node().filter(|call| call.name().as_slice() == b"delegate" && call.receiver().is_none()) else {
                after_visibility_section |= may_change_default_visibility(statement);
                continue;
            };
            let location = call.location();
            let span = self.unit.span(location.start_offset(), location.end_offset());
            let rewrite = if after_visibility_section {
                Err("follows a statement that may change this body's default visibility (a `private`, `protected`, `public` or `module_function` with no arguments, however it is called): Active Support defines the methods public through module_eval, a def here would take that visibility".to_string())
            } else if index + 1 == statements.len() {
                Err("is the last statement of its body, whose value the class or module expression returns".to_string())
            } else {
                generated_source(self.unit.text, &call)
            };
            self.direct_starts.push(location.start_offset());
            self.candidates.push(Candidate { span, rewrite });
        }
    }
}

impl<'parse> Visit<'parse> for DelegateFinder<'_> {
    fn visit_class_node(&mut self, node: &ruby_prism::ClassNode<'parse>) {
        self.judge_body(node.body());
        ruby_prism::visit_class_node(self, node);
    }

    fn visit_module_node(&mut self, node: &ruby_prism::ModuleNode<'parse>) {
        self.judge_body(node.body());
        ruby_prism::visit_module_node(self, node);
    }

    fn visit_singleton_class_node(&mut self, node: &ruby_prism::SingletonClassNode<'parse>) {
        if node.expression().as_self_node().is_some() {
            self.judge_body(node.body());
        }
        ruby_prism::visit_singleton_class_node(self, node);
    }

    fn visit_call_node(&mut self, node: &ruby_prism::CallNode<'parse>) {
        let start = node.location().start_offset();
        if node.name().as_slice() == b"delegate" && node.receiver().is_none() && !self.direct_starts.contains(&start) {
            let end = node.location().end_offset();
            self.candidates.push(Candidate {
                span: self.unit.span(start, end),
                rewrite: Err("is not a direct statement of a `class`, `module` or `class << self` body, so where Ruby would define a def here is not certain".to_string()),
            });
        }
        ruby_prism::visit_call_node(self, node);
    }
}

/// Could `statement`, run in a class body, change the default visibility of
/// the defs after it? Any `private`, `protected`, `public` or
/// `module_function` call that may have no arguments (none, or only splats)
/// counts, whatever its receiver, behind any condition, in a block, or
/// through `send` with a literal symbol or string name. Only the
/// bodies of nested `def`s, classes and modules, which have their own
/// visibility, are skipped.
fn may_change_default_visibility(statement: &ruby_prism::Node<'_>) -> bool {
    let mut finder = VisibilityFinder { found: false };
    finder.visit(statement);
    finder.found
}

struct VisibilityFinder {
    found: bool,
}

/// The method `call` invokes and the arguments it passes, looking through
/// `send`, `__send__` and `public_send` with a literal symbol or string name.
fn invoked<'parse>(call: &ruby_prism::CallNode<'parse>) -> (String, Vec<ruby_prism::Node<'parse>>) {
    let name = String::from_utf8_lossy(call.name().as_slice()).into_owned();
    let mut arguments: Vec<ruby_prism::Node<'parse>> = call.arguments().map(|arguments| arguments.arguments().iter().collect()).unwrap_or_default();
    if SEND_CALLS.contains(&name.as_str())
        && let Some(sent) = arguments.first().and_then(super::super::definitions::literal_name)
    {
        arguments.remove(0);
        return (sent, arguments);
    }
    (name, arguments)
}

impl<'parse> Visit<'parse> for VisibilityFinder {
    fn visit_call_node(&mut self, node: &ruby_prism::CallNode<'parse>) {
        let (name, arguments) = invoked(node);
        // Splats can expand to nothing (`private(*[])`), so a call with only
        // splats may have no arguments.
        let may_have_no_arguments = arguments.iter().all(|argument| argument.as_splat_node().is_some());
        if VISIBILITY_SECTIONS.contains(&name.as_str()) && may_have_no_arguments {
            self.found = true;
        }
        ruby_prism::visit_call_node(self, node);
    }

    fn visit_def_node(&mut self, _node: &ruby_prism::DefNode<'parse>) {}

    fn visit_class_node(&mut self, _node: &ruby_prism::ClassNode<'parse>) {}

    fn visit_module_node(&mut self, _node: &ruby_prism::ModuleNode<'parse>) {}

    fn visit_singleton_class_node(&mut self, _node: &ruby_prism::SingletonClassNode<'parse>) {}
}

/// Finds any call to `using` in a file, direct or through a send with a
/// literal name.
pub(crate) struct RefinementFinder {
    pub(crate) found: bool,
}

impl<'parse> Visit<'parse> for RefinementFinder {
    fn visit_call_node(&mut self, node: &ruby_prism::CallNode<'parse>) {
        if invoked(node).0 == "using" {
            self.found = true;
        }
        ruby_prism::visit_call_node(self, node);
    }
}

/// The source `ActiveSupport::Delegation.generate` builds for `call`, for
/// the one form this pass handles, or why `call` is not that form.
fn generated_source(text: &str, call: &ruby_prism::CallNode<'_>) -> Result<String, String> {
    let location = call.location();
    let line_breaks = text[location.start_offset()..location.end_offset()].matches('\n').count();
    if call.block().is_some() {
        return Err("passes a block".to_string());
    }
    let arguments: Vec<ruby_prism::Node<'_>> = call.arguments().map(|arguments| arguments.arguments().iter().collect()).unwrap_or_default();
    let Some((last, positional)) = arguments.split_last() else {
        return Err("has no arguments".to_string());
    };
    let Some(options) = last.as_keyword_hash_node() else {
        return Err("has no `to:` option".to_string());
    };
    let mut target = None;
    for element in options.elements().iter() {
        let Some(pair) = element.as_assoc_node() else {
            return Err("splats its options".to_string());
        };
        let Some(key) = pair.key().as_symbol_node().map(|symbol| text_of(symbol.unescaped())) else {
            return Err("has an option whose key is not a symbol literal".to_string());
        };
        if key != "to" {
            return Err(format!("passes `{key}:`; only `to:` alone is rewritten"));
        }
        let Some(value) = pair.value().as_symbol_node().map(|symbol| text_of(symbol.unescaped())) else {
            return Err("names its target with something other than a symbol literal".to_string());
        };
        target = Some(value);
    }
    let Some(target) = target else {
        return Err("has no `to:` option".to_string());
    };
    if !is_plain_method_name(&target, false) {
        return Err(format!("delegates to `{target}`, which is not a plain method name (an instance variable, a constant or a reserved name)"));
    }
    if positional.is_empty() {
        return Err("names no methods".to_string());
    }
    let mut methods = Vec::new();
    for argument in positional {
        let Some(method) = argument.as_symbol_node().map(|symbol| text_of(symbol.unescaped())) else {
            return Err("names a method with something other than a symbol literal".to_string());
        };
        if !is_plain_method_name(&method, true) {
            return Err(format!("delegates `{method}`, which is a writer, an operator or a reserved name"));
        }
        methods.push(method);
    }
    // `ActiveSupport::Delegation.generate` with no prefix, no allow_nil, no
    // private, and a target that is neither a module nor `self.class`: the
    // `...` signature, and the rescue that turns a nil target into a
    // DelegationError. It joins every line of every method with `;`.
    let definitions: Vec<String> = methods
        .iter()
        .map(|method| {
            [
                format!("def {method}(...)"),
                format!("  _ = {target}"),
                format!("  _.{method}(...)"),
                "rescue ::NoMethodError => e".to_string(),
                format!("  if _.nil? && e.name == :{method}"),
                format!("    ::Kernel.raise ::ActiveSupport::DelegationError.nil_target(:{method}, :'{target}')"),
                "  else".to_string(),
                "    ::Kernel.raise".to_string(),
                "  end".to_string(),
                "end".to_string(),
            ]
            .join(";")
        })
        .collect();
    // Every method on the call's first line, where Active Support's
    // module_eval puts them; the call's other lines stay, empty.
    Ok(definitions.join(";") + &"\n".repeat(line_breaks))
}

/// A lowercase identifier that is not reserved, with a trailing `?` or `!`
/// allowed when `allow_suffix` (method names, not targets).
fn is_plain_method_name(name: &str, allow_suffix: bool) -> bool {
    let stem = if allow_suffix { name.strip_suffix(['?', '!']).unwrap_or(name) } else { name };
    let mut characters = stem.chars();
    let starts_lowercase = characters.next().is_some_and(|first| first.is_ascii_lowercase() || first == '_');
    starts_lowercase && characters.all(|character| character.is_ascii_alphanumeric() || character == '_') && !RESERVED_METHOD_NAMES.contains(&name)
}

fn text_of(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_visibility_section_counts_however_it_is_called() {
        for (statement, changes) in [
            ("private", true),
            ("self.private", true),
            ("private if ENABLED", true),
            ("send(:protected)", true),
            ("__send__(\"public\")", true),
            ("private(*[])", true),
            ("module_function", true),
            ("[1].each { private }", true),
            ("private def helper; end", false),
            ("private :helper", false),
            ("def helper; private; end", false),
            ("class Inner; private; end", false),
        ] {
            let parsed = ruby_prism::parse(statement.as_bytes());
            let program = parsed.node();
            let first = program.as_program_node().and_then(|program| program.statements().body().iter().next()).expect("one statement");
            assert_eq!(may_change_default_visibility(&first), changes, "{statement}");
        }
    }

    #[test]
    fn plain_method_names() {
        assert!(is_plain_method_name("shop", false));
        assert!(is_plain_method_name("_shop2", false));
        assert!(!is_plain_method_name("shop?", false));
        assert!(is_plain_method_name("paid?", true));
        assert!(is_plain_method_name("save!", true));
        for rejected in ["@shop", "Shop", "name=", "[]", "class", "self", "", "?", "arg"] {
            assert!(!is_plain_method_name(rejected, true), "{rejected}");
        }
    }
}
