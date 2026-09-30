//! App-declared surfaces of unknown gems. The declaration provides
//! evidence more specific than a fixed gem surface or receiver ancestry;
//! it does not provide types or an emitted implementation.

use std::collections::BTreeSet;

use ruby_prism::{CallNode, Node};

mod state_machines;

/// Instance/class dispatch is not distinguished by the diagnostic IR,
/// so these names describe a declaration's combined surface.
pub(super) struct Generated {
    pub dsl: &'static str,
    pub surfaces: Vec<Surface>,
    pub requires_concern: bool,
}

/// Some names require an integration, not just the declaring gem.
/// Requirements are evaluated on the receiver, including declarations
/// contributed by a Concern, never on the declaration owner's kind.
pub(super) struct Surface {
    pub names: BTreeSet<String>,
    pub ancestor: Option<&'static str>,
    pub provider: Option<&'static str>,
}

/// Generator policy is separate from DSL-to-gem ownership in
/// `gems::GENERATING_DSLS`. Add declaration families here, not public
/// attribution passes or call-site exceptions.
const GENERATORS: &[(
    &str,
    for<'pr> fn(&CallNode<'pr>, Option<Node<'pr>>) -> Vec<Surface>,
)] = &[
    ("aasm", state_machines::aasm),
    ("state_machine", state_machines::state_machine),
];

pub(super) fn dsl_for_call(call: &CallNode<'_>) -> Option<&'static str> {
    GENERATORS
        .iter()
        .find(|(dsl, _)| dsl.as_bytes() == call.name().as_slice())
        .map(|(dsl, _)| *dsl)
}

/// Positive evidence is limited to direct class-body declarations and
/// Rails' explicit wrappers. Unsupported executable shapes contribute
/// only vetoes: not seeing their effects does not establish isolation.
pub(super) fn harvest_body(body: &Node<'_>, out: &mut Vec<Generated>) {
    let Some(statements) = body.as_statements_node() else {
        return;
    };
    for statement in statements.body().iter() {
        let Some(call) = statement.as_call_node() else {
            record_uncertain(&statement, out);
            continue;
        };
        if call.receiver().is_some() {
            record_uncertain(&statement, out);
            continue;
        }
        let name = call.name();
        let block = call.block().and_then(|b| b.as_block_node());
        if matches!(name.as_slice(), b"included" | b"with_options") {
            if let Some(body) = block.and_then(|block| block.body()) {
                let start = out.len();
                harvest_body(&body, out);
                // Wrapper options can change a generator's naming
                // configuration. Keep occurrences as veto evidence,
                // without guessing how Rails merges those options.
                for generated in &mut out[start..] {
                    if name.as_slice() == b"included" {
                        generated.requires_concern = true;
                    }
                    if call.arguments().is_some() {
                        generated.surfaces.clear();
                    }
                }
            }
        } else if let Some((dsl, generate)) = GENERATORS
            .iter()
            .find(|(dsl, _)| dsl.as_bytes() == name.as_slice())
        {
            out.push(Generated {
                dsl,
                requires_concern: false,
                // Blockless/opaque calls may reopen/configure a prior
                // declaration. They veto isolation but claim no names.
                surfaces: if let Some(block) = block {
                    generate(&call, block.body())
                } else {
                    Vec::new()
                },
            });
        } else {
            record_uncertain(&statement, out);
        }
    }
}

/// A conditional or arbitrary block may configure an existing machine.
/// Inspect it for uncertainty, never for positive generated names. Method
/// bodies and nested declarations have a different execution/owner scope.
fn record_uncertain(node: &Node<'_>, out: &mut Vec<Generated>) {
    struct Uncertain<'a>(&'a mut Vec<Generated>);
    impl<'pr> ruby_prism::Visit<'pr> for Uncertain<'_> {
        fn visit_call_node(&mut self, call: &CallNode<'pr>) {
            if call
                .receiver()
                .is_none_or(|receiver| receiver.as_self_node().is_some())
            {
                if let Some(dsl) = dsl_for_call(call) {
                    self.0.push(Generated {
                        dsl,
                        surfaces: Vec::new(),
                        requires_concern: false,
                    });
                }
            }
            ruby_prism::visit_call_node(self, call);
        }
        fn visit_def_node(&mut self, _: &ruby_prism::DefNode<'pr>) {}
        fn visit_class_node(&mut self, _: &ruby_prism::ClassNode<'pr>) {}
        fn visit_module_node(&mut self, _: &ruby_prism::ModuleNode<'pr>) {}
    }
    ruby_prism::Visit::visit(&mut Uncertain(out), node);
}

fn statements<'pr>(body: Option<Node<'pr>>) -> Vec<Node<'pr>> {
    body.and_then(|b| b.as_statements_node())
        .map(|s| s.body().iter().collect())
        .unwrap_or_default()
}

fn positional<'pr>(call: &CallNode<'pr>) -> Vec<Node<'pr>> {
    call.arguments()
        .map(|a| {
            a.arguments()
                .iter()
                .filter(|n| n.as_keyword_hash_node().is_none())
                .collect()
        })
        .unwrap_or_default()
}

fn keywords<'pr>(call: &CallNode<'pr>) -> Vec<ruby_prism::AssocNode<'pr>> {
    let Some(args) = call.arguments() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for arg in args.arguments().iter() {
        let elements = match (arg.as_keyword_hash_node(), arg.as_hash_node()) {
            (Some(kh), _) => kh.elements(),
            (_, Some(h)) => h.elements(),
            _ => continue,
        };
        out.extend(elements.iter().filter_map(|e| e.as_assoc_node()));
    }
    out
}

fn option<'pr>(call: &CallNode<'pr>, key: &str) -> Option<Node<'pr>> {
    keywords(call)
        .into_iter()
        .find(|p| name_of(&p.key()).as_deref() == Some(key))
        .map(|p| p.value())
}

fn name_of(node: &Node<'_>) -> Option<String> {
    if let Some(sym) = node.as_symbol_node() {
        return Some(String::from_utf8_lossy(sym.unescaped()).into_owned());
    }
    node.as_string_node()
        .map(|s| String::from_utf8_lossy(s.unescaped()).into_owned())
}

/// Missing options and unreadable options are different evidence.
/// Reject opaque hashes/splats/duplicate keys instead of treating them
/// as absent and inventing the generator's default configuration.
fn literal_arguments(call: &CallNode<'_>) -> bool {
    let mut keys = BTreeSet::new();
    for arg in call
        .arguments()
        .iter()
        .flat_map(|args| args.arguments().iter())
    {
        if name_of(&arg).is_some() {
            continue;
        }
        let elements = match (arg.as_keyword_hash_node(), arg.as_hash_node()) {
            (Some(hash), _) => hash.elements(),
            (_, Some(hash)) => hash.elements(),
            _ => return false,
        };
        for element in elements.iter() {
            let Some(pair) = element.as_assoc_node() else {
                return false;
            };
            // The gems read symbol configuration keys. A literal string
            // key is not equivalent to the corresponding keyword.
            if pair.key().as_symbol_node().is_none() {
                return false;
            }
            let Some(key) = name_of(&pair.key()) else {
                return false;
            };
            if !keys.insert(key) {
                return false;
            }
            let value = pair.value();
            if name_of(&value).is_none()
                && value.as_true_node().is_none()
                && value.as_false_node().is_none()
                && value.as_nil_node().is_none()
                && value.as_integer_node().is_none()
                && value.as_float_node().is_none()
            {
                return false;
            }
        }
    }
    true
}

#[cfg(test)]
#[test]
fn generator_and_provider_registries_cover_the_same_dsls() {
    let parsers: BTreeSet<_> = GENERATORS.iter().map(|(dsl, _)| *dsl).collect();
    let providers: BTreeSet<_> = crate::gems::GENERATING_DSLS
        .iter()
        .map(|(dsl, _)| *dsl)
        .collect();
    assert_eq!(parsers, providers);
    assert_eq!(parsers.len(), GENERATORS.len(), "duplicate parser key");
    assert_eq!(
        providers.len(),
        crate::gems::GENERATING_DSLS.len(),
        "duplicate provider key"
    );
    assert!(
        crate::gems::GENERATING_DSLS
            .iter()
            .all(|(_, providers)| !providers.is_empty())
    );
}
