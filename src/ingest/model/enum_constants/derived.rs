//! Enum mappings read through another owner: `Gadget.statuses` (the class
//! method Rails' `enum :status` defines), another owner's Hash constant,
//! and the two Hash methods apps derive a mapping with, `keys` and
//! `merge`. The prepass records each owner's own `enum` declarations as
//! recipes (prism nodes do not outlive their file's parse); a reference
//! evaluates them on demand, with every owner it passes through refused
//! a second time (a load-order cycle).

use ruby_prism::Node;

use super::super::super::util::{constant_id_str, flatten_statements};
use super::EnumConstants;
use crate::expr::Literal;

/// A mapping as Ruby holds it: an Array of labels (an `enum` maps it by
/// position) or a Hash. `indifferent` is the HashWithIndifferentAccess
/// an enum's class method returns, whose keys are always Strings.
#[derive(Clone)]
pub(super) enum Value {
    List(Vec<String>),
    Map { pairs: Vec<(String, Literal)>, indifferent: bool },
}

impl Value {
    pub(super) fn into_mapping(self) -> Vec<(String, Literal)> {
        match self {
            Value::List(labels) => labels
                .into_iter()
                .enumerate()
                .map(|(i, label)| (label, Literal::Int { value: i as i64 }))
                .collect(),
            Value::Map { pairs, .. } => pairs,
        }
    }

    /// What `enum` stores and its class method answers: labels as
    /// Strings, one entry per label (the first position, the last value).
    fn into_enum_values(self) -> Value {
        let mut out: Vec<(String, Literal)> = Vec::new();
        for (label, value) in self.into_mapping() {
            match out.iter_mut().find(|(l, _)| *l == label) {
                Some(slot) => slot.1 = value,
                None => out.push((label, value)),
            }
        }
        Value::Map { pairs: out, indifferent: true }
    }
}

/// How to compute a mapping once every owner is recorded.
#[derive(Clone)]
pub(super) enum Recipe {
    Fixed(Value),
    /// A constant, resolved from `owners` (innermost first).
    Const { path: String, owners: Vec<String> },
    /// `Owner.method` with no arguments: an enum's class method.
    EnumOf { path: String, owners: Vec<String>, method: String },
    Keys(Box<Recipe>),
    Merge(Box<Recipe>, Box<Recipe>),
}

/// One `enum` declaration an owner's body makes, by column.
pub(super) struct Declaration {
    pub(super) column: String,
    /// `None`: a mapping this pass cannot compute; it still answers the
    /// method, so it is never skipped over.
    pub(super) recipe: Option<Recipe>,
}

/// Calls whose receiver could reach an owner's singleton class and so
/// define or replace the method an enum defines there.
pub(super) const SINGLETON_METHODS: &[&str] = &["define_singleton_method", "singleton_class"];

/// Calls whose block runs with another `self`.
pub(super) const EVAL_METHODS: &[&str] =
    &["instance_eval", "instance_exec", "class_eval", "class_exec", "module_eval", "module_exec"];

impl Recipe {
    /// The recipe for a mapping expression. `table` is the declaring
    /// body's own constants (a bare name Ruby finds there first).
    pub(super) fn of(
        node: &Node<'_>,
        owners: &[String],
        table: Option<&super::super::ClassConsts>,
        file: &str,
    ) -> Option<Recipe> {
        if let Some(parentheses) = node.as_parentheses_node() {
            let statements = flatten_statements(parentheses.body()?);
            let [only] = statements.as_slice() else { return None };
            return Recipe::of(only, owners, table, file);
        }
        if let Some(read) = node.as_constant_read_node() {
            let name = constant_id_str(&read.name());
            if let Some(table) = table {
                if let Some(labels) = table.labels.get::<str>(&name) {
                    return Some(Recipe::Fixed(Value::List(labels.clone())));
                }
                if let Some(mapping) = table.mappings.get::<str>(&name) {
                    return literal_pairs(mapping.clone())
                        .map(|pairs| Recipe::Fixed(Value::Map { pairs, indifferent: false }));
                }
            }
            return Some(Recipe::Const { path: name.to_string(), owners: owners.to_vec() });
        }
        if node.as_constant_path_node().is_some() {
            return Some(Recipe::Const { path: EnumConstants::path(node)?, owners: owners.to_vec() });
        }
        if node.as_array_node().is_some() {
            return super::super::label_list(node).map(|labels| Recipe::Fixed(Value::List(labels)));
        }
        if node.as_hash_node().is_some() || node.as_keyword_hash_node().is_some() {
            let empty;
            let consts = match table {
                Some(table) => table,
                None => {
                    empty = super::super::ClassConsts::collect(&[], file);
                    &empty
                }
            };
            let elements: Vec<Node<'_>> = match node.as_hash_node() {
                Some(hash) => hash.elements().iter().collect(),
                None => node.as_keyword_hash_node()?.elements().iter().collect(),
            };
            if !values_fold(&elements, consts) {
                return None;
            }
            let pairs = super::super::enum_label_pairs(elements.into_iter(), file, consts).ok()??;
            return literal_pairs(pairs).map(|pairs| Recipe::Fixed(Value::Map { pairs, indifferent: false }));
        }
        let call = node.as_call_node()?;
        if call.block().is_some() {
            return None;
        }
        let receiver = call.receiver()?;
        let method = constant_id_str(&call.name());
        let args: Vec<Node<'_>> = call.arguments().map(|a| a.arguments().iter().collect()).unwrap_or_default();
        match (method.as_ref(), args.as_slice()) {
            // Both return the receiver's contents unchanged.
            ("freeze" | "dup", []) => Recipe::of(&receiver, owners, table, file),
            ("keys", []) => Some(Recipe::Keys(Box::new(Recipe::of(&receiver, owners, table, file)?))),
            ("merge", [other]) => Some(Recipe::Merge(
                Box::new(Recipe::of(&receiver, owners, table, file)?),
                Box::new(Recipe::of(other, owners, table, file)?),
            )),
            (_, []) => {
                let path = EnumConstants::path(&receiver)?;
                let lowercase = method.chars().next().is_some_and(|c| c.is_ascii_lowercase() || c == '_');
                (lowercase && method.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
                    .then(|| Recipe::EnumOf { path, owners: owners.to_vec(), method: method.to_string() })
            }
            _ => None,
        }
    }
}

/// Whether every value of a Hash mapping folds to a literal, checked
/// before `enum_label_pairs` would ingest one that does not: that ingest
/// records a survey gap, and a recipe is only ever literal, so a mapping
/// read for another owner must not report its values a second time (or
/// under no file at all).
fn values_fold(elements: &[Node<'_>], consts: &super::super::ClassConsts) -> bool {
    elements.iter().all(|element| {
        element.as_assoc_node().is_some_and(|assoc| {
            let value = assoc.value();
            match value.as_constant_write_node() {
                Some(write) => consts.scalar(&write.value()).is_some(),
                None => value.as_nil_node().is_some() || consts.scalar(&value).is_some(),
            }
        })
    })
}

/// [`values_fold`] for a whole mapping expression: a Hash literal (or a
/// frozen one) must fold; anything else ingests no values.
pub(super) fn mapping_values_fold(node: &Node<'_>, consts: &super::super::ClassConsts) -> bool {
    if let Some(call) = node.as_call_node()
        && constant_id_str(&call.name()) == "freeze"
        && call.arguments().is_none()
        && call.block().is_none()
        && let Some(receiver) = call.receiver()
    {
        return mapping_values_fold(&receiver, consts);
    }
    match (node.as_hash_node(), node.as_keyword_hash_node()) {
        (Some(hash), _) => values_fold(&hash.elements().iter().collect::<Vec<_>>(), consts),
        (_, Some(hash)) => values_fold(&hash.elements().iter().collect::<Vec<_>>(), consts),
        _ => true,
    }
}

pub(super) fn literal_pairs(stored: Vec<(String, super::super::EnumStored)>) -> Option<Vec<(String, Literal)>> {
    stored
        .into_iter()
        .map(|(label, value)| match value {
            super::super::EnumStored::Lit(value) => Some((label, value)),
            super::super::EnumStored::Expr(_) => None,
        })
        .collect()
}

impl Recipe {
    fn reads_enum_method(&self) -> bool {
        match self {
            Recipe::EnumOf { .. } => true,
            Recipe::Keys(inner) => inner.reads_enum_method(),
            Recipe::Merge(left, right) => left.reads_enum_method() || right.reads_enum_method(),
            Recipe::Fixed(_) | Recipe::Const { .. } => false,
        }
    }
}

impl EnumConstants {
    pub(in crate::ingest) fn enum_key_list(&self, node: &Node<'_>, owners: &[String]) -> Option<Vec<Literal>> {
        let recipe = Recipe::of(node, owners, None, "")?;
        if !recipe.reads_enum_method() {
            return None;
        }
        // Inclusion currently reads stored attributes. A recipient with
        // enum attributes can expose a label different from its stored
        // value, so keep that case outside this extension.
        if owners.first().is_some_and(|owner| self.has_enum_attributes(owner, &mut crate::det::HashSet::default())) {
            return None;
        }
        let mut loading: Vec<String> = owners.first().cloned().into_iter().collect();
        match self.evaluate(&recipe, &mut loading)? {
            Value::List(labels) => Some(labels.into_iter().map(|value| Literal::Str { value }).collect()),
            Value::Map { .. } => None,
        }
    }

    fn has_enum_attributes(&self, owner: &str, visited: &mut crate::det::HashSet<String>) -> bool {
        if !visited.insert(owner.to_string()) { return false; }
        if self.enum_declarations.get(owner).is_some_and(|declarations| !declarations.is_empty()) {
            return true;
        }
        self.parents.get(owner).is_some_and(|parents| parents.iter().any(|parent| {
            self.resolve_name(&parent.path, &parent.owners)
                .is_some_and(|parent| self.has_enum_attributes(&parent, visited))
        }))
    }

    /// For a mapping folded before the consumed sources were read: it
    /// read another owner's enum method, which those sources now say may
    /// answer something else.
    pub(in crate::ingest) fn enum_method_read_refused(&self, node: &Node<'_>, owners: &[String]) -> bool {
        Recipe::of(node, owners, None, "").is_some_and(|recipe| recipe.reads_enum_method())
            && self.derived(node, owners).is_none()
    }

    /// The mapping a derived expression evaluates to, when every owner
    /// it reads answers statically. `owners`: the reference's lexical
    /// owners, innermost first; the innermost is the body being loaded.
    pub(super) fn derived(&self, node: &Node<'_>, owners: &[String]) -> Option<Vec<(String, Literal)>> {
        let recipe = Recipe::of(node, owners, None, "")?;
        let mut loading: Vec<String> = owners.first().cloned().into_iter().collect();
        Some(self.evaluate(&recipe, &mut loading)?.into_mapping())
    }

    /// `loading`: the owners whose bodies are mid-evaluation along this
    /// chain. Ruby defines an enum's method only when its declaration
    /// runs, so reading one of them again depends on statement and load
    /// order: refused.
    fn evaluate(&self, recipe: &Recipe, loading: &mut Vec<String>) -> Option<Value> {
        match recipe {
            Recipe::Fixed(value) => Some(value.clone()),
            Recipe::Const { path, owners } => {
                let name = self.resolve_name(path, owners)?;
                if self.namespace_chain_valid(&name)
                    && let Some(super::EnumConstant::Mapping(mapping)) = self.values.get(&name)
                {
                    return Some(Value::List(mapping.iter().map(|(label, _)| label.clone()).collect()));
                }
                if !self.snapshot_constant_is_stable(path, owners) {
                    return None;
                }
                let (_, member, table) = self.owner_constant(path, owners)?;
                if let Some(labels) = table.labels.get(&member) {
                    return Some(Value::List(labels.clone()));
                }
                let pairs = literal_pairs(table.mappings.get(&member)?.clone())?;
                Some(Value::Map { pairs, indifferent: false })
            }
            Recipe::EnumOf { path, owners, method } => {
                let owner = self.resolve_name(path, owners)?;
                if !matches!(self.values.get(&owner), Some(super::EnumConstant::Namespace))
                    || !self.namespace_chain_valid(&owner)
                    || loading.contains(&owner)
                    || !self.singleton_answers_enum(&owner, method)
                {
                    return None;
                }
                let declarations: Vec<&Declaration> = self
                    .enum_declarations
                    .get(&owner)?
                    .iter()
                    .filter(|d| crate::naming::pluralize_snake(&d.column) == *method)
                    .collect();
                let [declaration] = declarations.as_slice() else { return None };
                let recipe = declaration.recipe.as_ref()?;
                loading.push(owner);
                let value = self.evaluate(recipe, loading);
                loading.pop();
                Some(value?.into_enum_values())
            }
            Recipe::Keys(inner) => match self.evaluate(inner, loading)? {
                Value::Map { pairs, .. } => Some(Value::List(pairs.into_iter().map(|(label, _)| label).collect())),
                Value::List(_) => None, // Array#keys: NoMethodError.
            },
            Recipe::Merge(left, right) => {
                // Only the indifferent hash: a plain Hash keeps `:a` and
                // `"a"` as two keys, which labels as Strings cannot show.
                let Value::Map { pairs: mut out, indifferent: true } = self.evaluate(left, loading)? else {
                    return None;
                };
                let Value::Map { pairs, .. } = self.evaluate(right, loading)? else {
                    return None; // Hash#merge(Array): TypeError.
                };
                for (label, value) in pairs {
                    match out.iter_mut().find(|(l, _)| *l == label) {
                        Some(slot) => slot.1 = value,
                        None => out.push((label, value)),
                    }
                }
                Some(Value::Map { pairs: out, indifferent: true })
            }
        }
    }

    /// Whether `owner.method` can only be the method its own `enum`
    /// defined: an ActiveRecord-shaped class (it has a superclass, and
    /// defines no `enum` of its own), no singleton definition of that name
    /// on it (or, by name, on an owner this pass cannot name: `def x.m`,
    /// a `def` inside `x.instance_eval do`, `define_singleton_method(:m)`),
    /// nothing opening its singleton class, and no reflection rooted at
    /// it (`Owner.class_eval`, `Owner.send`). The mapping itself is frozen,
    /// so unlike a constant's value nothing can mutate it: a reflective
    /// call on `Object` (`Object.const_get(name)`) only refuses the
    /// singleton definitions above, as the enum constant inputs do.
    fn singleton_answers_enum(&self, owner: &str, method: &str) -> bool {
        let last = owner.rsplit("::").next().unwrap_or(owner);
        self.parents.contains_key(owner)
            && !self.singletons_unknown
            && !self.uncertain_singleton_methods.contains(method)
            && !self.singleton_methods.contains(&(last.to_string(), method.to_string()))
            && !self.singleton_methods.contains(&(last.to_string(), "enum".to_string()))
            && !self.open_singletons.contains(last)
            && !owner.split("::").any(|segment| self.reflected.contains(segment))
    }
}
