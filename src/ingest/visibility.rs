//! Resolve local visibility while declaration order and lexical scopes still
//! exist. Never replay these calls from LibraryClass::unknown_calls.

use crate::det::{HashMap, HashSet};

use ruby_prism::{CallNode, Node};

use crate::dialect::{MethodDef, MethodVisibility};
use crate::ident::ClassId;

use super::util::{constant_id_str, flatten_statements, module_name_path, symbol_or_string_value};
use super::{IngestError, IngestResult};

use crate::ingest::prism_api::PrismLocation;

thread_local! {
    /// The statement the walk is at: where a refusal happened
    /// (`super::error_site`).
    static STATEMENT: std::cell::Cell<Option<(usize, usize)>> = const { std::cell::Cell::new(None) };
}
const PRIVATE_HOOKS: &[&str] = &[
    "initialize", "initialize_copy", "initialize_dup", "initialize_clone",
];

#[derive(Default)]
pub(super) struct Visibility {
    // Source statement + method name also identifies synthesized attr/alias
    // definitions, which have no def-name span.
    values: HashMap<(usize, String), MethodVisibility>,
    known: HashMap<(bool, String), Vec<usize>>,
    changed: HashSet<(bool, String)>,
    accessors: HashSet<(bool, String)>,
    defaults: HashMap<usize, MethodVisibility>,
    inherited: Vec<(bool, String, usize)>,
    /// Walking a concern's `ClassMethods` carrier, whose body is the
    /// includers' class side rather than a singleton of its own.
    carrier: bool,
    /// The declaring class's path, which `def Owner.name` spells for `def self.name`.
    owner: Option<String>,
    /// Each def in a load-time conditional's branch: its statement offset
    /// to (the conditional's offset, the branch index). Two definitions
    /// of a name in different branches of one conditional are
    /// alternatives, not a redefinition.
    alternatives: HashMap<usize, (usize, usize)>,
    /// A load-time conditional's offset to its branch count, when it ends
    /// in `else` (some branch always runs).
    exhaustive: HashMap<usize, usize>,
    /// Load-time conditional definitions are admitted: a library class or
    /// module body, whose emit writes the conditional. A model body's is
    /// not, and stays refused.
    conditionals: bool,
}

impl Visibility {
    /// `DataPoint` in svg-graph's `def DataPoint.configure_shape_criteria`, inside `class DataPoint`.
    pub(super) fn names_owner(&self, receiver: &Node<'_>) -> bool {
        let (Some(owner), Some(read)) = (&self.owner, receiver.as_constant_read_node()) else { return false };
        let written = constant_id_str(&read.name());
        owner == written || owner.ends_with(&format!("::{written}"))
    }
}

/// A class-body `if` (with its `elsif`s and `else`) or `unless` (with
/// its `else`), not a ternary or modifier, whose branches hold only
/// method definitions (`def`, or `private def` and the other inline
/// markers), plain accessors, `class` / `module` declarations and nested
/// conditionals of the same shape, at least one: `(unless, [(condition,
/// statements)])`, the `else` branch with no condition. Ruby runs it when
/// the body loads, and only the selected branch defines what it holds.
pub(super) fn conditional_definition<'pr>(node: &Node<'pr>) -> Option<(bool, Vec<(Option<Node<'pr>>, Vec<Node<'pr>>)>)> {
    // Declares nothing in the emitted app (`if defined?(Rails::Console)`).
    if console_only(node) {
        return None;
    }
    let body = |statements: Option<ruby_prism::StatementsNode<'pr>>| -> Vec<Node<'pr>> {
        statements.map(|s| flatten_statements(s.as_node())).unwrap_or_default()
    };
    let mut arms = Vec::new();
    let negated;
    if let Some(branch) = node.as_if_node() {
        // Not a ternary, nor a modifier (`def x = 1 if y`): no `end`.
        branch.if_keyword_loc()?;
        branch.end_keyword_loc()?;
        negated = false;
        arms.push((Some(branch.predicate()), body(branch.statements())));
        let mut next = branch.subsequent();
        while let Some(sub) = next {
            if let Some(elsif) = sub.as_if_node() {
                arms.push((Some(elsif.predicate()), body(elsif.statements())));
                next = elsif.subsequent();
            } else if let Some(other) = sub.as_else_node() {
                arms.push((None, body(other.statements())));
                next = None;
            } else {
                return None;
            }
        }
    } else if let Some(branch) = node.as_unless_node() {
        branch.end_keyword_loc()?;
        negated = true;
        arms.push((Some(branch.predicate()), body(branch.statements())));
        if let Some(other) = branch.else_clause() {
            arms.push((None, body(other.statements())));
        }
    } else {
        return None;
    }
    let defines = arms.iter().any(|(_, stmts)| !stmts.is_empty());
    let only_defs = arms.iter().all(|(_, stmts)| {
        stmts.iter().all(|stmt| {
            definition(stmt).is_some()
                || conditional_accessor(stmt).is_some()
                || conditional_definition(stmt).is_some()
                || stmt.as_class_node().is_some()
                || stmt.as_module_node().is_some()
        })
    });
    (defines && only_defs).then_some((negated, arms))
}

/// A class-body `if` / `unless` whose branches define a method but hold
/// a statement [`conditional_definition`] does not admit: that statement,
/// and why it keeps the conditional refused.
fn refused_conditional_definition<'pr>(node: &Node<'pr>) -> Option<(Node<'pr>, &'static str)> {
    if console_only(node) || conditional_definition(node).is_some() {
        return None;
    }
    let mut arms: Vec<Vec<Node<'pr>>> = Vec::new();
    let body = |statements: Option<ruby_prism::StatementsNode<'pr>>| -> Vec<Node<'pr>> {
        statements.map(|s| flatten_statements(s.as_node())).unwrap_or_default()
    };
    if let Some(branch) = node.as_if_node() {
        branch.if_keyword_loc()?;
        branch.end_keyword_loc()?;
        arms.push(body(branch.statements()));
        let mut next = branch.subsequent();
        while let Some(sub) = next {
            if let Some(elsif) = sub.as_if_node() {
                arms.push(body(elsif.statements()));
                next = elsif.subsequent();
            } else {
                arms.push(body(sub.as_else_node()?.statements()));
                next = None;
            }
        }
    } else if let Some(branch) = node.as_unless_node() {
        branch.end_keyword_loc()?;
        arms.push(body(branch.statements()));
        if let Some(other) = branch.else_clause() {
            arms.push(body(other.statements()));
        }
    } else {
        return None;
    }
    if !arms.iter().flatten().any(|stmt| definition(stmt).is_some()) {
        return None;
    }
    let admitted = |stmt: &Node<'_>| {
        definition(stmt).is_some()
            || conditional_accessor(stmt).is_some()
            || conditional_definition(stmt).is_some()
            || stmt.as_class_node().is_some()
            || stmt.as_module_node().is_some()
    };
    let stmt = arms.into_iter().flatten().find(|stmt| !admitted(stmt))?;
    let why = if stmt.as_call_node().is_some_and(|c| c.receiver().is_none() && marker(&c)) {
        "a visibility declaration inside a load-time conditional definition is not modeled \
         (the visibility in effect after it would depend on the branch taken)"
    } else if stmt.as_if_node().is_some() || stmt.as_unless_node().is_some() || stmt.as_case_node().is_some() {
        "a conditional nested in a load-time conditional definition is not modeled"
    } else if stmt.as_singleton_class_node().is_some() {
        "`class << self` inside a load-time conditional definition is not modeled"
    } else if stmt.as_constant_write_node().is_some() {
        "a constant assigned in a load-time conditional definition is not modeled"
    } else if stmt.as_alias_method_node().is_some()
        || stmt.as_call_node().is_some_and(|c| {
            c.receiver().is_none() && matches!(constant_id_str(&c.name()), "attr_reader" | "attr_writer" | "attr_accessor" | "alias_method")
        })
    {
        "an accessor or alias declared in a load-time conditional definition is not modeled"
    } else {
        "a load-time conditional definition whose branch holds a statement other than a `def` is not modeled"
    };
    Some((stmt, why))
}

/// A bare `attr_reader` / `attr_writer` / `attr_accessor` of literal
/// names. In a load-time conditional's branch it defines its methods
/// only when the branch runs, as a `def` there does (activemerchant
/// 1.137.0's `Connection` has `attr_accessor :min_version` under `if
/// Net::HTTP.instance_methods.include?(:min_version=)`).
pub(super) fn conditional_accessor<'pr>(stmt: &Node<'pr>) -> Option<CallNode<'pr>> {
    let call = stmt.as_call_node()?;
    (call.receiver().is_none()
        && call.block().is_none()
        && matches!(constant_id_str(&call.name()), "attr_reader" | "attr_writer" | "attr_accessor")
        && call.arguments().is_some_and(|args| args.arguments().iter().all(|arg| symbol_or_string_value(&arg).is_some())))
    .then_some(call)
}

/// Whether `node` declares only for the Rails console
/// (`if defined?(Rails::Console)`), which the emitted app never runs.
pub(super) fn console_only_node(node: &Node<'_>) -> bool {
    console_only(node)
}

pub(super) fn marker(call: &CallNode<'_>) -> bool {
    matches!(
        constant_id_str(&call.name()),
        "public" | "protected" | "private" | "public_class_method" | "private_class_method"
    )
}

/// `private def helper … end` defines a method, not a default for later
/// definitions. Leave all other forms alone for the resolution/error gate.
pub(super) fn definition<'pr>(node: &Node<'pr>) -> Option<ruby_prism::DefNode<'pr>> {
    if let Some(call) = node.as_call_node() {
        if marker(&call) && call.receiver().is_none() && call.block().is_none() {
            if let Some(args) = call.arguments() {
                let args: Vec<_> = args.arguments().iter().collect();
                if let [arg] = args.as_slice() {
                    return arg.as_def_node();
                }
            }
        }
    }
    node.as_def_node()
}

/// Does this `included do … end` body mix a visibility marker (bare or
/// named — either spelling makes the marker's reach matter) with some
/// OTHER receiverless call this file's visibility tracking does not
/// itself attribute a method to? `def`s and the marker calls themselves
/// are always fine; `attr_reader`/`attr_writer`/`attr_accessor`,
/// `alias_method`, and nested `class_methods`/`included`/
/// `module_function` are the declaration forms `Visibility::walk`
/// already tracks, so they're fine too. Anything else — `delegate`,
/// `scope`, `has_many`, an arbitrary DSL block — might generate a
/// method whose visibility this walk cannot verify, so its presence
/// alongside a marker is reported, not silently accepted.
/// Declaration forms `Visibility::walk` already attributes a method
/// (or methods) to, besides a plain `def`. Anything else — `delegate`,
/// `scope`, `has_many`, an arbitrary DSL — might generate a method
/// whose visibility this walk cannot verify.
const TRACKED_DECLARATIONS: &[&str] = &[
    "attr_reader",
    "attr_writer",
    "attr_accessor",
    "alias_method",
    "class_methods",
    "included",
    "module_function",
];

/// Thor's command-annotation DSL: these describe the NEXT command and
/// define no methods of their own, so a visibility marker coexisting
/// with one is never the soundness hole `marker_mixed_with_unmodeled_call`
/// otherwise guards against — but only the bare, receiverless spelling
/// Thor's own DSL uses; see [`marker_mixed_with_unmodeled_call`] for why
/// a self-qualified or conditionally-guarded one still isn't exempt.
const THOR_DSL_ANNOTATIONS: &[&str] = &["desc", "long_desc", "option", "method_option", "class_option"];

/// Does this `included do … end` body mix a visibility marker (bare or
/// named — either spelling makes the marker's reach matter) with some
/// OTHER call, ANYWHERE in the body (including inside `if`/`unless`
/// branches, and however deeply elsif-chained), that this file's
/// visibility tracking does not itself attribute a method to? `def`s
/// and the marker calls themselves are always fine, on any receiver;
/// [`TRACKED_DECLARATIONS`] are fine too, but ONLY when sent to the
/// includer itself — bare (implicit `self`) or an explicit `self.` —
/// since a call on some OTHER receiver (`Other.attr_accessor`) cannot
/// be generating a method on THIS class regardless of what it's named.
///
/// A top-level, bare [`THOR_DSL_ANNOTATIONS`] call is ALSO exempt — but
/// narrowly: only that exact shape. `self.desc …` isn't a spelling
/// Thor's DSL itself produces, so it stays an ordinary unmodeled call
/// (same bucket as any other `self.`-qualified mystery), and one nested
/// inside an `if`/`unless` reaches [`statement_has_unmodeled_call`]
/// through [`branch_has_unmodeled_call`]'s own recursion, never through
/// this exemption, so it stays refused too — conditionally annotating a
/// Thor command isn't a shape its DSL produces either.
///
/// When every marker is bare, a call BEFORE the first one is exempt too:
/// a bare `private` sets the default for the definitions after it, so
/// what an earlier `validates` or `has_many` generates keeps its own
/// visibility. A named marker (`has_many :items; private :items`) can
/// reach back, so with one the whole body still counts.
fn marker_mixed_with_unmodeled_call(body: &Node<'_>) -> bool {
    let Some(statements) = body.as_statements_node() else { return false };
    let stmts = flatten_statements(statements.as_node());
    let is_marker = |stmt: &Node<'_>| stmt.as_call_node().is_some_and(|c| c.receiver().is_none() && marker(&c));
    let Some(first_marker) = stmts.iter().position(is_marker) else {
        return false;
    };
    let all_bare = stmts
        .iter()
        .filter(|stmt| is_marker(stmt))
        .all(|stmt| stmt.as_call_node().is_some_and(|c| c.arguments().is_none()));
    let from = if all_bare { first_marker } else { 0 };
    stmts[from..].iter().any(|stmt| !is_bare_thor_annotation(stmt) && statement_has_unmodeled_call(stmt))
}

/// A direct, receiverless call naming one of [`THOR_DSL_ANNOTATIONS`] —
/// the one shape Thor's own DSL produces. Checked only at the top level
/// of an `included do` body; see `marker_mixed_with_unmodeled_call`'s
/// doc comment for why a self-qualified or nested spelling does not
/// qualify.
fn is_bare_thor_annotation(stmt: &Node<'_>) -> bool {
    stmt.as_call_node().is_some_and(|c| {
        c.receiver().is_none() && THOR_DSL_ANNOTATIONS.contains(&constant_id_str(&c.name()))
    })
}

/// A single statement, or (recursively) anything reachable through an
/// `if`/`unless` branch it opens — an includer guard around a
/// method-generating call is exactly as effective as one written
/// directly in the block.
fn statement_has_unmodeled_call(stmt: &Node<'_>) -> bool {
    if stmt.as_def_node().is_some() {
        return false;
    }
    if let Some(call) = stmt.as_call_node() {
        if marker(&call) {
            return false;
        }
        let targets_includer = call.receiver().is_none_or(|r| r.as_self_node().is_some());
        if !targets_includer {
            return false;
        }
        return !TRACKED_DECLARATIONS.contains(&constant_id_str(&call.name()));
    }
    branch_has_unmodeled_call(stmt)
}

/// Descend into an `if`/`unless` node's branches (both arms, and an
/// elsif chain) looking for [`statement_has_unmodeled_call`]. Anything
/// that is not itself an `if`/`unless` node is not a branch at all —
/// `false`, same as `statement_has_unmodeled_call`'s own fallthrough.
fn branch_has_unmodeled_call(node: &Node<'_>) -> bool {
    let body_has = |body: Option<ruby_prism::StatementsNode<'_>>| {
        body.is_some_and(|s| flatten_statements(s.as_node()).iter().any(statement_has_unmodeled_call))
    };
    if let Some(branch) = node.as_if_node() {
        body_has(branch.statements())
            || branch.subsequent().is_some_and(|sub| {
                if let Some(else_node) = sub.as_else_node() {
                    body_has(else_node.statements())
                } else {
                    // `elsif` — another `IfNode`, chained via `subsequent`.
                    branch_has_unmodeled_call(&sub)
                }
            })
    } else if let Some(branch) = node.as_unless_node() {
        body_has(branch.statements())
            || branch.else_clause().is_some_and(|clause| body_has(clause.statements()))
    } else {
        false
    }
}

impl Visibility {
    /// Recover lexical defaults for source declarations that a later pass
    /// synthesizes into methods. Parse and walk each class body once per file.
    pub(super) fn declaration_defaults(
        source: &str,
        file: &str,
    ) -> HashMap<usize, MethodVisibility> {
        // Classification re-parse: primary ingest already reported syntax
        // errors via `prism::parse`. Reporting again under the same
        // `prism::scope` would duplicate located Parse diagnostics.
        let parsed = super::prism::parse_silent(source.as_bytes());
        let root = parsed.node();
        let mut defaults = HashMap::default();
        let bodies = super::util::find_all_classes_with_scope(&root)
            .into_iter()
            .filter_map(|(_, class)| class.body())
            .chain(
                super::util::find_all_module_declarations_with_scope(&root)
                    .into_iter()
                    .filter_map(|(_, module)| module.body()),
            );
        for body in bodies {
            // Not `resolve`: forem's `private :has_role?` names a method rolify defines, which
            // refuses the class body there but leaves the default at every later line known.
            if let Ok(visibility) = Self::resolve_carrying_inherited(Some(&body), file, None) {
                defaults.extend(visibility.defaults);
            }
            for statement in flatten_statements(body) {
                let Some(call) = statement.as_call_node() else { continue };
                if call.receiver().is_some() || constant_id_str(&call.name()) != "included" {
                    continue;
                }
                let block_body = call
                    .block()
                    .and_then(|block| block.as_block_node())
                    .and_then(|block| block.body());
                if let Ok(visibility) = Self::resolve(block_body.as_ref(), file, None) {
                    defaults.extend(visibility.defaults);
                }
            }
        }
        defaults
    }

    pub(super) fn resolve(
        body: Option<&Node<'_>>,
        file: &str,
        module_owner: Option<&ClassId>,
    ) -> IngestResult<Self> {
        Self::resolve_owned(body, file, module_owner, None)
    }

    /// [`Self::resolve_owned`] for a library class or module body, whose
    /// load-time conditional definitions are admitted
    /// (`conditional_definition`).
    pub(super) fn resolve_library(
        body: Option<&Node<'_>>,
        file: &str,
        module_owner: Option<&ClassId>,
        owner: Option<&ClassId>,
    ) -> IngestResult<Self> {
        let out = Self::resolve_carrying_inherited_with(body, file, module_owner, owner, true)?;
        match out.inherited.first() {
            Some((_, name, _)) => Err(Self::undefined(file, name)),
            None => Ok(out),
        }
    }

    /// Whether this body's load-time conditional definitions are admitted.
    pub(super) fn admits_conditionals(&self) -> bool {
        self.conditionals
    }

    pub(super) fn resolve_owned(
        body: Option<&Node<'_>>,
        file: &str,
        module_owner: Option<&ClassId>,
        owner: Option<&ClassId>,
    ) -> IngestResult<Self> {
        let out = Self::resolve_carrying_inherited_owned(body, file, module_owner, owner)?;
        match out.inherited.first() {
            Some((_, name, _)) => Err(Self::undefined(file, name)),
            None => Ok(out),
        }
    }

    pub(super) fn resolve_carrying_inherited(
        body: Option<&Node<'_>>,
        file: &str,
        module_owner: Option<&ClassId>,
    ) -> IngestResult<Self> {
        Self::resolve_carrying_inherited_owned(body, file, module_owner, None)
    }

    fn resolve_carrying_inherited_owned(
        body: Option<&Node<'_>>,
        file: &str,
        module_owner: Option<&ClassId>,
        owner: Option<&ClassId>,
    ) -> IngestResult<Self> {
        Self::resolve_carrying_inherited_with(body, file, module_owner, owner, false)
    }

    fn resolve_carrying_inherited_with(
        body: Option<&Node<'_>>,
        file: &str,
        module_owner: Option<&ClassId>,
        owner: Option<&ClassId>,
        conditionals: bool,
    ) -> IngestResult<Self> {
        let mut out = Self { owner: owner.map(|o| o.0.to_string()), conditionals, ..Self::default() };
        if let Some(body) = body {
            let Some(statements) = body.as_statements_node() else {
                return Err(Self::unsupported(
                    file,
                    "visibility requires a static declaration body",
                ));
            };
            out.walk(Some(statements.as_node()), false, file, module_owner)?;
        }
        // Ruby raises NameError at the marker for a later `def` or one only on the other side, so those are no inherited method.
        if let Some((_, name, _)) = out.inherited.iter().find(|(class_side, name, _)| {
            out.known.contains_key(&(*class_side, name.clone())) || out.known.contains_key(&(!*class_side, name.clone()))
        }) {
            return Err(Self::undefined(file, name));
        }
        Ok(out)
    }

    // Not dropped with the nested scope that wrote it: `class << self; private :recent; end` has no body statement to carry it.
    pub(super) fn uncarried_inherited(&self, carried: &crate::det::HashSet<usize>, file: &str) -> IngestResult<()> {
        match self.inherited.iter().find(|(_, _, at)| !carried.contains(at)) {
            Some((_, name, _)) => Err(Self::undefined(file, name)),
            None => Ok(()),
        }
    }

    pub(super) fn names_inherited(&self, statement: &Node<'_>) -> bool {
        let offset = statement.location().start_offset();
        self.inherited.iter().any(|(_, _, at)| *at == offset)
    }

    fn undefined(file: &str, name: &str) -> IngestError {
        Self::unsupported(
            file,
            format!("visibility change for `{name}` requires an already defined local method (not forward or inherited)"),
        )
    }

    /// `private_class_method` / `public_class_method` named this method
    /// after `module_function` copied it to the class side.
    pub(super) fn class_side_changed(&self, name: &str) -> bool {
        self.changed.contains(&(true, name.to_string()))
    }

    pub(super) fn apply(&self, statement: &Node<'_>, method: &mut MethodDef) {
        let offset = statement.location().start_offset();
        let name = method.name.to_string();
        // A `module_function` class copy is recorded one past the def so an
        // instance `private :name` cannot overwrite it. Prefer that copy
        // only after a class-method visibility change named this method.
        if self.class_side_changed(&name) {
            if let Some(value) = self.values.get(&(offset.wrapping_add(1), name.clone())) {
                method.visibility = *value;
                return;
            }
        }
        if let Some(value) = self.values.get(&(offset, name)) {
            method.visibility = *value;
        }
    }

    pub(super) fn check_model_item(&self, statement: &Node<'_>, file: &str) -> IngestResult<()> {
        if self.values.iter().any(|((offset, _), value)| {
            *offset == statement.location().start_offset() && *value != MethodVisibility::Public
        }) {
            return Err(Self::unsupported(
                file,
                "visibility of model accessors or aliases requires a local MethodDef",
            ));
        }
        Ok(())
    }

    fn unsupported(file: &str, message: impl Into<String>) -> IngestError {
        let err = IngestError::Unsupported {
            file: file.into(),
            message: message.into(),
        };
        if let Some((start, end)) = STATEMENT.get() {
            super::error_site::note(&err, start, end);
        }
        err
    }

    fn define(
        &mut self,
        offset: usize,
        name: String,
        class_side: bool,
        visibility: MethodVisibility,
        file: &str,
    ) -> IngestResult<()> {
        let key = (class_side, name.clone());
        let alternative = |a: usize, b: usize| {
            matches!((self.alternatives.get(&a), self.alternatives.get(&b)), (Some((sa, ba)), Some((sb, bb))) if sa == sb && ba != bb)
        };
        if let Some(previous) = self.known.get(&key).filter(|previous| !previous.iter().all(|p| alternative(*p, offset))) {
            // A repeated ordinary constructor has the same implicit
            // visibility on every definition. Keep the pre-existing duplicate
            // bodies so semantic consumers (e.g. Alba's evidence gate) can
            // reject them; a visibility marker still makes them ambiguous.
            let implicit = if !class_side && PRIVATE_HOOKS.contains(&name.as_str()) {
                MethodVisibility::Private
            } else {
                MethodVisibility::Public
            };
            if self.changed.contains(&key)
                || visibility != implicit
                || previous
                    .iter()
                    .any(|p| self.values[&(*p, name.clone())] != implicit)
            {
                return Err(Self::unsupported(
                    file,
                    format!("ambiguous visibility for redefined method `{name}`"),
                ));
            }
        }
        self.known.entry(key).or_default().push(offset);
        self.values.insert((offset, name), visibility);
        Ok(())
    }

    fn change(
        &mut self,
        name: String,
        class_side: bool,
        visibility: MethodVisibility,
        file: &str,
        statement: usize,
    ) -> IngestResult<()> {
        let key = (class_side, name.clone());
        let Some(positions) = self.known.get(&key) else {
            self.inherited.push((class_side, name, statement));
            return Ok(());
        };
        // Every definition in a different branch of one conditional: the
        // change applies to whichever branch ran.
        // Only when every branch defines it and some branch always runs:
        // otherwise Ruby raises NameError here when the branch that
        // defines it did not.
        if positions.iter().any(|p| self.alternatives.contains_key(p)) {
            let statement = self.alternatives.get(&positions[0]).map(|(s, _)| *s);
            let same = positions.iter().all(|p| self.alternatives.get(p).map(|(s, _)| *s) == statement);
            let branches: HashSet<usize> = positions.iter().filter_map(|p| self.alternatives.get(p).map(|(_, b)| *b)).collect();
            let covered = statement.and_then(|s| self.exhaustive.get(&s)).is_some_and(|n| branches.len() == *n && branches.len() == positions.len());
            if !same {
                return Err(Self::unsupported(file, "conditional or dynamic visibility declarations are not modeled"));
            }
            if !covered {
                return Err(Self::unsupported(
                    file,
                    format!("visibility change for `{name}`, which a load-time conditional defines in only some branches, is not modeled"),
                ));
            }
        }
        let alternatives = positions.len() > 1 && positions.iter().all(|p| self.alternatives.contains_key(p));
        if alternatives {
            for position in positions.clone() {
                self.values.insert((position, name.clone()), visibility);
            }
            self.changed.insert(key);
            return Ok(());
        }
        let [position] = positions.as_slice() else {
            return Err(Self::unsupported(
                file,
                format!("ambiguous visibility for redefined method `{name}`"),
            ));
        };
        self.values.insert((*position, name), visibility);
        self.changed.insert(key);
        Ok(())
    }

    /// A load-time conditional's definitions (and those of the
    /// conditionals nested in its branches): each takes the visibility in
    /// effect at the statement, or its inline marker's, and the defs in
    /// different branches of one conditional are alternatives.
    fn walk_conditional(&mut self, statement: &Node<'_>, class_side: bool, default: MethodVisibility, file: &str) -> IngestResult<()> {
        let offset = statement.location().start_offset();
        let Some((_, arms)) = conditional_definition(statement) else { return Ok(()) };
        if arms.last().is_some_and(|(condition, _)| condition.is_none()) {
            self.exhaustive.insert(offset, arms.len());
        }
        for (index, (_, stmts)) in arms.iter().enumerate() {
            for stmt in stmts {
                let at = stmt.location().start_offset();
                STATEMENT.set(Some((at, stmt.location().end_offset())));
                self.defaults.insert(at, default);
                if conditional_definition(stmt).is_some() {
                    self.walk_conditional(stmt, class_side, default, file)?;
                    continue;
                }
                // A declaration has its own pass and namespace, but not
                // one on the singleton class.
                if stmt.as_class_node().is_some() || stmt.as_module_node().is_some() {
                    if class_side {
                        return Err(Self::unsupported(
                            file,
                            "a class or module declared in a load-time conditional inside `class << self` is not modeled",
                        ));
                    }
                    continue;
                }
                if let Some(call) = conditional_accessor(stmt) {
                    let kw = constant_id_str(&call.name());
                    for arg in call.arguments().into_iter().flat_map(|a| a.arguments().iter()) {
                        let Some(name) = symbol_or_string_value(&arg) else { continue };
                        self.alternatives.insert(at, (offset, index));
                        if kw != "attr_writer" {
                            self.define(at, name.clone(), class_side, default, file)?;
                        }
                        if kw != "attr_reader" {
                            self.define(at, format!("{name}="), class_side, default, file)?;
                        }
                    }
                    continue;
                }
                let Some(def) = definition(stmt) else { continue };
                let inline = stmt.as_call_node().filter(marker).map(|call| match constant_id_str(&call.name()) {
                    "public" | "public_class_method" => MethodVisibility::Public,
                    "protected" => MethodVisibility::Protected,
                    _ => MethodVisibility::Private,
                });
                if def.receiver().is_some_and(|r| r.as_self_node().is_none() && !self.names_owner(&r)) || (class_side && def.receiver().is_some()) {
                    return Err(Self::unsupported(file, "visibility of a definition on a foreign receiver is not modeled"));
                }
                let name = constant_id_str(&def.name()).to_string();
                let side = class_side || def.receiver().is_some();
                let visibility = inline.unwrap_or_else(|| {
                    if !side && PRIVATE_HOOKS.contains(&name.as_str()) {
                        MethodVisibility::Private
                    } else if def.receiver().is_some() {
                        MethodVisibility::Public
                    } else {
                        default
                    }
                });
                self.alternatives.insert(at, (offset, index));
                self.define(at, name, side, visibility, file)?;
            }
        }
        Ok(())
    }

    fn walk_carrier(&mut self, body: Option<Node<'_>>, file: &str) -> IngestResult<()> {
        // A Concern's ClassMethods module is NOT the concern's own singleton
        // class. Their methods only share a bucket after flattening.
        let mut carrier = Self { carrier: true, ..Self::default() };
        carrier.walk(body, true, file, None)?;
        if let Some((_, name, _)) = carrier.inherited.first() {
            return Err(Self::undefined(file, name));
        }
        self.values.extend(carrier.values);
        Ok(())
    }

    /// `included do … end` runs under `class_eval` on the includer: its
    /// defs land on the includer's INSTANCE side, not a singleton of
    /// their own. Walk it as its own lexical scope that starts public —
    /// the same bucket-isolation `walk_carrier` gives `class_methods do
    /// … end`, just on the instance side (`class_side: false`).
    ///
    /// Narrowly scoped to defs and the markers themselves (plus the
    /// declaration helpers `walk` already tracks — `attr_*`,
    /// `alias_method`, nested `class_methods`/`included`/
    /// `module_function`): a marker's effect on anything else this body
    /// might generate a method from (`delegate :email, to: :author`,
    /// `scope`, `has_many`, an arbitrary DSL) is NOT modeled, and
    /// silently proceeding would risk shipping that method PUBLIC where
    /// the source marks it private — `attachment_model_lowering.rs`'s
    /// `a_private_delegate_in_an_included_block_is_rejected_at_ingest`
    /// is exactly that case. So a marker coexisting with any other
    /// receiverless, non-marker call keeps the same blanket rejection
    /// this whole block used to raise for ANY marker, same as a named
    /// marker (`private :helper`) naming something NOT defined within
    /// this same `included do` (the one-sided-accessor shape a
    /// separate, already-deferred gap owns) — any error here folds back
    /// to that one message rather than surfacing a new, more specific
    /// one that would read as a second construct now being claimed
    /// supported.
    fn walk_included(&mut self, body: Option<Node<'_>>, file: &str) -> IngestResult<()> {
        if body.as_ref().is_some_and(marker_mixed_with_unmodeled_call) {
            return Err(Self::unsupported(
                file,
                "conditional or dynamic visibility declarations are not modeled",
            ));
        }
        let mut included = Self::default();
        included
            .walk(body, false, file, None)
            .map_err(|_| Self::unsupported(file, "conditional or dynamic visibility declarations are not modeled"))?;
        if !included.inherited.is_empty() {
            return Err(Self::unsupported(file, "conditional or dynamic visibility declarations are not modeled"));
        }
        self.values.extend(included.values);
        Ok(())
    }

    pub(super) fn reject_conditional_declaration(node: &Node<'_>, file: &str) -> IngestResult<()> {
        Self::reject_dynamic_declarations(node, file)
    }

    /// An `if` / `unless` whose body holds a `def` or a visibility marker.
    /// A modifier (`return x if x`) does not.
    pub(super) fn hides_declaration(node: &Node<'_>) -> bool {
        struct Declarations {
            invalid: bool,
        }
        impl<'pr> ruby_prism::Visit<'pr> for Declarations {
            fn visit_branch_node_enter(&mut self, node: Node<'pr>) {
                let body_hides = |body: Option<Node<'pr>>| {
                    body.is_some_and(|body| {
                        super::util::flatten_statements(body).iter().any(|stmt| {
                            stmt.as_def_node().is_some()
                                || stmt.as_call_node().is_some_and(|c| c.receiver().is_none() && marker(&c))
                        })
                    })
                };
                self.invalid |= if let Some(branch) = node.as_if_node() {
                    body_hides(branch.statements().map(|s| s.as_node()))
                        || branch.subsequent().is_some_and(|sub| {
                            sub.as_else_node()
                                .and_then(|e| e.statements())
                                .is_some_and(|s| body_hides(Some(s.as_node())))
                                || sub.as_if_node().is_some()
                        })
                } else if let Some(branch) = node.as_unless_node() {
                    body_hides(branch.statements().map(|s| s.as_node()))
                        || branch
                            .else_clause()
                            .and_then(|clause| clause.statements())
                            .is_some_and(|s| body_hides(Some(s.as_node())))
                } else {
                    false
                };
            }
            fn visit_def_node(&mut self, _node: &ruby_prism::DefNode<'pr>) {}
        }
        let mut declarations = Declarations { invalid: false };
        ruby_prism::Visit::visit(&mut declarations, node);
        declarations.invalid
    }

    fn reject_dynamic_declarations(node: &Node<'_>, file: &str) -> IngestResult<()> {
        if console_only(node) {
            return Ok(());
        }
        struct Declarations {
            invalid: bool,
            reject_defs: bool,
        }
        impl<'pr> ruby_prism::Visit<'pr> for Declarations {
            fn visit_branch_node_enter(&mut self, node: Node<'pr>) {
                // A modifier (`return value if value.is_a?(Kind)`) is one
                // expression, not a declaration that sometimes runs. Only a
                // branch whose body can hold a visibility marker is dynamic.
                let body_has_marker = |body: Option<Node<'pr>>| {
                    body.is_some_and(|body| {
                        super::util::flatten_statements(body).iter().any(|stmt| {
                            stmt.as_call_node().is_some_and(|c| c.receiver().is_none() && marker(&c))
                        })
                    })
                };
                self.invalid |= if let Some(branch) = node.as_if_node() {
                    body_has_marker(branch.statements().map(|s| s.as_node()))
                        || branch.subsequent().is_some_and(|sub| {
                            sub.as_else_node()
                                .and_then(|e| e.statements())
                                .is_some_and(|s| body_has_marker(Some(s.as_node())))
                                || sub.as_else_node().is_none() && body_has_marker(Some(sub))
                        })
                } else if let Some(branch) = node.as_unless_node() {
                    // `statements` is the body; `else_clause` is the else.
                    body_has_marker(branch.statements().map(|s| s.as_node()))
                        || branch
                            .else_clause()
                            .and_then(|clause| clause.statements())
                            .is_some_and(|s| body_has_marker(Some(s.as_node())))
                } else {
                    node.as_call_node().is_some_and(|c| c.receiver().is_none() && marker(&c))
                };
            }
            fn visit_def_node(&mut self, _node: &ruby_prism::DefNode<'pr>) {
                // DSL blocks (e.g. has_many extensions) own their defs, not
                // the enclosing class. Never inspect ordinary def bodies.
                self.invalid |= self.reject_defs;
            }
        }
        // A def inside a call's block belongs to that block's owner, not
        // to this class: a DSL block (has_many extensions), or the class
        // a constant is assigned from — `ContentKey = Data.define(:digest)
        // do def cache_key … end end` (campfire's FragmentCache),
        // `Struct.new(…) do … end`.
        let owns_defs = node.as_call_node().is_some()
            || node
                .as_constant_write_node()
                .is_some_and(|w| w.value().as_call_node().is_some_and(|c| c.block().is_some()));
        let mut declarations = Declarations {
            invalid: false,
            reject_defs: !owns_defs,
        };
        ruby_prism::Visit::visit(&mut declarations, node);
        if declarations.invalid {
            return Err(Self::unsupported(
                file,
                "conditional or dynamic visibility declarations are not modeled",
            ));
        }
        Ok(())
    }

    /// Resolve static method visibility within one lexical body, starting public.
    /// Nested classes and recognized Data factory blocks have separate declaration
    /// passes, so their methods and visibility markers do not affect this scope.
    fn walk(
        &mut self,
        body: Option<Node<'_>>,
        class_side: bool,
        file: &str,
        module_owner: Option<&ClassId>,
    ) -> IngestResult<()> {
        let outer = STATEMENT.get();
        let walked = self.walk_statements(body, class_side, file, module_owner);
        STATEMENT.set(outer);
        walked
    }

    fn walk_statements(
        &mut self,
        body: Option<Node<'_>>,
        class_side: bool,
        file: &str,
        module_owner: Option<&ClassId>,
    ) -> IngestResult<()> {
        let Some(body) = body else { return Ok(()) };
        // A new lexical body always starts public. A marker in the enclosing
        // class must not privatize def self.x or leak into class_methods.
        let mut default = MethodVisibility::Public;
        let mut module_function = false;
        for statement in flatten_statements(body) {
            STATEMENT.set(Some((statement.location().start_offset(), statement.location().end_offset())));
            let offset = statement.location().start_offset();
            self.defaults.insert(offset, default);
            // A load-time conditional's definitions take the visibility in
            // effect at the statement, or their inline marker's.
            if self.conditionals && conditional_definition(&statement).is_some() {
                self.walk_conditional(&statement, class_side, default, file)?;
                continue;
            }
            let def = definition(&statement);
            let node = &statement;
            let mut inline = None;
            if let Some(call) = statement.as_call_node().filter(marker) {
                if call.receiver().is_some() || call.block().is_some() {
                    return Err(Self::unsupported(
                        file,
                        "visibility requires a receiverless, static declaration",
                    ));
                }
                let kw = call.name();
                let kw = constant_id_str(&kw);
                let visibility = match kw {
                    "public" | "public_class_method" => MethodVisibility::Public,
                    "protected" => MethodVisibility::Protected,
                    _ => MethodVisibility::Private,
                };
                let named_class = kw.ends_with("_class_method");
                if class_side && named_class {
                    // `class << self` has no further singleton to address.
                    // A module's `def self` is already that singleton, so
                    // `private_class_method :jwks` after it is the same
                    // change a class body makes.
                    if module_owner.is_none() {
                        return Err(Self::unsupported(
                            file,
                            "class-method visibility on a nested singleton level is not modeled",
                        ));
                    }
                }
                if let Some(def) = &def {
                    let side = class_side || def.receiver().is_some();
                    if side != (class_side || named_class) {
                        return Err(Self::unsupported(
                            file,
                            "visibility wrapper targets the wrong method receiver",
                        ));
                    }
                    inline = Some(visibility);
                } else if let Some(args) = call.arguments() {
                    for arg in args.arguments().iter() {
                        if arg.as_call_node().is_some_and(|call| call.receiver().is_none() && marker(&call)) {
                            break;
                        }
                        let Some(name) = symbol_or_string_value(&arg) else {
                            return Err(Self::unsupported(
                                file,
                                "visibility method names must be literal symbols or strings",
                            ));
                        };
                        // `private_class_method :name` after `def self.name`.
                        // The instance-side lookup misses that normal order.
                        // A forward reference and an instance-only name still fail.
                        let class_copy = named_class
                            && !self.known.contains_key(&(false, name.clone()))
                            && self.known.contains_key(&(true, name.clone()));
                        self.change(name, class_side || named_class || class_copy, visibility, file, offset)?;
                    }
                    continue;
                } else {
                    if named_class {
                        return Err(Self::unsupported(
                            file,
                            "class-method visibility requires local method names",
                        ));
                    }
                    default = visibility;
                    if !class_side && !named_class {
                        // A bare public/protected/private ends Ruby's
                        // module_function mode. The library walker stops
                        // promoting later defs; copy tracking must stop too.
                        module_function = false;
                    }
                    continue;
                }
            }
            if let Some(def) = def {
                // The module's own `class << self` (not a ClassMethods
                // carrier) is where `def included(base)` is the hook.
                let in_singleton_class = class_side && !self.carrier;
                if let Some(inner) = super::library_class::included_hook_class_methods_body(&def, in_singleton_class) {
                    self.walk_carrier(Some(inner), file)?;
                    continue;
                }
                if let Some(receiver) = def.receiver() {
                    if receiver.as_self_node().is_none() && !self.names_owner(&receiver) {
                        return Err(Self::unsupported(
                            file,
                            "visibility of a definition on a foreign receiver is not modeled",
                        ));
                    }
                    if class_side {
                        return Err(Self::unsupported(
                            file,
                            "nested singleton method definition is not modeled",
                        ));
                    }
                }
                let name = constant_id_str(&def.name()).to_string();
                let side = class_side || def.receiver().is_some();
                let key = (side, name.clone());
                // The established attr-reader → custom-def override is
                // unambiguous, unlike multiple source defs around markers.
                if self.accessors.remove(&key) && !self.changed.contains(&key) {
                    self.known.remove(&key);
                }
                let visibility = inline.unwrap_or_else(|| {
                    if !side && PRIVATE_HOOKS.contains(&name.as_str()) {
                        MethodVisibility::Private
                    } else if def.receiver().is_some() {
                        MethodVisibility::Public
                    } else {
                        default
                    }
                });
                self.define(offset, name.clone(), side, visibility, file)?;
                if module_function && !side {
                    // The library walk keeps one class-side method and looks
                    // visibility up by the def offset. A later instance
                    // `private :name` must not clobber that class copy, so
                    // the copy lives at the next offset and `apply` prefers
                    // it once `private_class_method` names the method.
                    self.define(
                        offset.wrapping_add(1),
                        name,
                        true,
                        MethodVisibility::Public,
                        file,
                    )?;
                }
                continue;
            }
            if let Some(sc) = node.as_singleton_class_node() {
                if self.carrier && class_side && sc.expression().as_self_node().is_some() {
                    // Not the includers' class side: the carrier module's
                    // own singleton, whose hooks Ruby runs when
                    // `ActiveSupport::Concern` extends an includer with it.
                    return Err(Self::unsupported(
                        file,
                        "`class << self` in a concern's `ClassMethods` defines the carrier module's own \
                         singleton methods (such as an `extended` hook), which are not modeled",
                    ));
                }
                if class_side || sc.expression().as_self_node().is_none() {
                    return Err(Self::unsupported(
                        file,
                        "visibility of a foreign or nested singleton class is not modeled",
                    ));
                }
                self.walk(sc.body(), true, file, None)?;
                continue;
            }
            // Nested classes have their own declaration pass and namespace.
            if node.as_class_node().is_some() {
                continue;
            }
            if super::data_factory::declaration(node).is_some() {
                continue;
            }
            if let Some(module) = node.as_module_node() {
                if module_name_path(&module).as_deref() == Some(&["ClassMethods".to_string()]) {
                    self.walk_carrier(module.body(), file)?;
                }
                continue;
            }
            let Some(call) = node.as_call_node() else {
                // `X = Data.define(:a) do def … end` defines X's methods,
                // not this body's; `library_class::data_block_classes`
                // gives them a class of their own.
                if node.as_constant_write_node().is_some_and(|cw| {
                    super::library_class::data_define_block(&cw.value()).is_some()
                }) {
                    continue;
                }
                // A `def` is handled above. Anything else — an `if` that
                // wraps a definition, a modifier that does not — still
                // has to be rejected when it hides a marker or a `def`;
                // a conditional definition says which statement keeps it
                // refused.
                if let Err(err) = Self::reject_dynamic_declarations(node, file) {
                    return Err(match self.conditionals.then(|| refused_conditional_definition(node)).flatten() {
                        Some((at, message)) => super::error_site::at(Self::unsupported(file, message), &at),
                        None => err,
                    });
                }
                continue;
            };
            if call.receiver().is_some() {
                continue;
            }
            let name = call.name();
            let name = constant_id_str(&name);
            if !class_side && name == "module_function" && call.arguments().is_none() {
                module_function = true;
                continue;
            }
            if !class_side && name == "module_function" {
                // `module_function :a, :b` copies already-defined instance
                // methods onto the class side. Record those names so a
                // following `private_class_method :a` addresses the copy
                // the library walk actually keeps.
                if let Some(args) = call.arguments() {
                    for arg in args.arguments().iter() {
                        let Some(copied) = symbol_or_string_value(&arg) else {
                            continue;
                        };
                        if self.known.contains_key(&(false, copied.clone()))
                            && !self.known.contains_key(&(true, copied.clone()))
                        {
                            let positions = self.known[&(false, copied.clone())].clone();
                            let copies: Vec<usize> =
                                positions.iter().map(|p| p.wrapping_add(1)).collect();
                            self.known.insert((true, copied.clone()), copies.clone());
                            for position in copies {
                                self.values
                                    .insert((position, copied.clone()), MethodVisibility::Public);
                            }
                        }
                    }
                }
                continue;
            }
            if class_side && name == "module_function" {
                // Its public copy belongs to the carrier itself, not to the
                // includer whose methods we retain after flattening.
                return Err(Self::unsupported(
                    file,
                    "module_function on a nested singleton level or class-method carrier is not modeled",
                ));
            }
            // Only collector-owned candidates have a replacement refusal gate:
            // their unclaimed block context is diagnosed per includer. Do not
            // exempt candidate-free blocks or singleton/class-method carriers.
            if name == "included" && module_owner.is_some_and(|owner| {
                call.block().and_then(|b| b.as_block_node()).and_then(|b| b.body())
                    .is_some_and(|body| super::library_class::included_has_accessor(body, owner, file))
            }) {
                continue;
            }
            if name == "class_methods" {
                if let Some(block) = call.block().and_then(|b| b.as_block_node()) {
                    self.walk_carrier(block.body(), file)?;
                }
            } else if name == "included" {
                // The accessor-shape special case above already handled
                // (and skipped) its candidate; any other `included do …
                // end` is a plain lexical scope on the includer's
                // instance side. A bodyless `included` (no block) has
                // nothing to walk and falls back to the dynamic-marker
                // check, same as any other unrecognized call.
                if let Some(block) = call.block().and_then(|b| b.as_block_node()) {
                    self.walk_included(block.body(), file)?;
                } else {
                    Self::reject_dynamic_declarations(node, file)?;
                }
            } else if matches!(name, "attr_reader" | "attr_writer" | "attr_accessor") {
                if let Some(args) = call.arguments() {
                    for arg in args.arguments().iter() {
                        if let Some(attr) = symbol_or_string_value(&arg) {
                            if name != "attr_writer" {
                                self.define(offset, attr.clone(), class_side, default, file)?;
                                self.accessors.insert((class_side, attr.clone()));
                            }
                            if name != "attr_reader" {
                                self.define(offset, format!("{attr}="), class_side, default, file)?;
                                self.accessors.insert((class_side, format!("{attr}=")));
                            }
                        }
                    }
                }
            } else if name == "alias_method" {
                if let Some(args) = call.arguments() {
                    let args: Vec<_> = args.arguments().iter().collect();
                    if let [to, from] = args.as_slice() {
                        if let (Some(to), Some(from)) =
                            (symbol_or_string_value(to), symbol_or_string_value(from))
                        {
                            if let Some(positions) = self.known.get(&(class_side, from.clone())) {
                                let visibility = self.values[&(*positions.last().unwrap(), from)];
                                self.define(offset, to, class_side, visibility, file)?;
                            }
                        }
                    }
                }
            } else if statement.as_def_node().is_none() {
                Self::reject_dynamic_declarations(node, file)?;
            }
        }
        Ok(())
    }
}

/// `if defined?(Rails::Console) … end` with no `else`: what it declares
/// exists only under `rails console`, never in the app a target emits
/// (a server or its tests), so the branch declares nothing there.
pub(super) fn console_only(node: &Node<'_>) -> bool {
    let Some(branch) = node.as_if_node() else { return false };
    if branch.subsequent().is_some() {
        return false;
    }
    let Some(defined) = branch.predicate().as_defined_node() else { return false };
    let Some(path) = defined.value().as_constant_path_node() else { return false };
    let parent_is_rails = path.parent().is_some_and(|p| {
        p.as_constant_read_node().is_some_and(|c| c.name().as_slice() == b"Rails")
            || p.as_constant_path_node().is_some_and(|cp| cp.parent().is_none() && cp.name().is_some_and(|n| n.as_slice() == b"Rails"))
    });
    parent_is_rails && path.name().is_some_and(|n| n.as_slice() == b"Console")
}
