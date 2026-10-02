//! Alba 4's bounded, declarative source-property serialization subset.
//! Expand once before inference into ordinary methods; no external runtime,
//! target DSL replay, or asserted types. Source validation is necessary because
//! library capture can omit conditionals and failed expression ingests.

use std::collections::{HashMap, HashSet};

use crate::dialect::{LibraryClass, LibraryClassOrigin};
use crate::expr::Expr;
use crate::ident::{ClassId, Symbol};
use crate::span::{SourceFile, Span};

use super::util::{
    class_name_path, constant_id_str, constant_path_of, flatten_statements, symbol_value,
};
use super::{IngestError, IngestResult};

const RESOURCE: &str = "Alba::Resource";

#[derive(Clone)]
struct Field {
    name: Symbol,
    resource: Option<ClassId>,
}

struct ResourceDecl {
    fields: Vec<Field>,
    span: Span,
}

pub(super) fn lower_alba_resources(
    app: &mut crate::App,
    sources: &[crate::span::SourceFile],
) -> IngestResult<()> {
    let resources = selected_resources(&app.library_classes);
    if resources.is_empty() {
        return Ok(());
    }
    let declarations = collect_resource_declarations(sources, &resources)?;
    let flattened = validate_resource_graph(app, sources, &resources, &declarations)?;
    for class in &mut app.library_classes {
        if let Some(fields) = flattened.get(&class.name) {
            synthesize_methods(class, fields, declarations[&class.name].span)?;
        }
    }
    Ok(())
}

fn selected_resources(classes: &[LibraryClass]) -> HashSet<ClassId> {
    let mut candidates: HashSet<_> = classes
        .iter()
        .filter(|c| c.includes.iter().any(|i| i.0.as_str() == RESOURCE))
        .map(|c| c.name.clone())
        .collect();
    loop {
        let before = candidates.len();
        for class in classes {
            if class
                .parent
                .as_ref()
                .is_some_and(|p| candidates.contains(p))
            {
                candidates.insert(class.name.clone());
            }
        }
        if candidates.len() == before {
            break;
        }
    }
    // Include-only ancestry remains the generic gem-attribution contract.
    // Select declaration families, including their empty bases and children.
    let mut selected: HashSet<_> = classes
        .iter()
        .filter(|c| candidates.contains(&c.name) && !c.unknown_calls.is_empty())
        .map(|c| c.name.clone())
        .collect();
    loop {
        let before = selected.len();
        for class in classes.iter().filter(|c| candidates.contains(&c.name)) {
            if let Some(parent) = &class.parent {
                if selected.contains(&class.name) {
                    selected.insert(parent.clone());
                }
                if selected.contains(parent) {
                    selected.insert(class.name.clone());
                }
            }
        }
        if selected.len() == before {
            break;
        }
    }
    selected
}

/// Strict source identities: unlike the shared convenience resolver, an
/// expression-bearing constant parent must not disappear into a bare name.
fn literal_constant(node: &ruby_prism::Node<'_>) -> Option<Vec<String>> {
    if let Some(c) = node.as_constant_read_node() {
        return Some(vec![constant_id_str(&c.name()).to_owned()]);
    }
    let path = node.as_constant_path_node()?;
    let mut parts = match path.parent() {
        Some(parent) => literal_constant(&parent)?,
        None => Vec::new(),
    };
    parts.push(constant_id_str(&path.name()?).to_owned());
    Some(parts)
}

/// Count original fragments, including ones the ordinary finder drops, and
/// refuse explicit out-of-class modifications/aliases of selected resources.
struct SourceGuard<'a> {
    resources: &'a HashSet<ClassId>,
    counts: HashMap<ClassId, usize>,
    mutation: Option<usize>,
}

impl SourceGuard<'_> {
    fn resource_ref(&self, node: &ruby_prism::Node<'_>) -> bool {
        constant_path_of(node).is_some_and(|p| {
            self.resources
                .contains(&ClassId(Symbol::from(p.join("::"))))
        })
    }
}

impl<'pr> ruby_prism::Visit<'pr> for SourceGuard<'_> {
    fn visit_class_node(&mut self, node: &ruby_prism::ClassNode<'pr>) {
        if let Some(path) = class_name_path(node) {
            let id = ClassId(Symbol::from(path.join("::")));
            if self.resources.contains(&id) {
                *self.counts.entry(id).or_default() += 1;
                // The direct declaration collector validates this entire body,
                // including superclass/association identities. Outside those
                // declarations, a resource may only be a literal new receiver.
                return;
            }
        }
        ruby_prism::visit_class_node(self, node);
    }
    fn visit_call_node(&mut self, node: &ruby_prism::CallNode<'pr>) {
        if node.receiver().is_some_and(|r| {
            self.resource_ref(&r)
                && literal_constant(&r).is_some()
                && constant_id_str(&node.name()) == "new"
        }) {
            if let Some(args) = node.arguments() {
                self.visit(&args.as_node());
            }
            if let Some(block) = node.block() {
                self.visit(&block);
            }
            return;
        }
        ruby_prism::visit_call_node(self, node);
    }
    fn visit_constant_read_node(&mut self, node: &ruby_prism::ConstantReadNode<'pr>) {
        if self.resource_ref(&node.as_node()) {
            self.mutation = Some(node.location().start_offset());
        }
    }
    fn visit_constant_path_node(&mut self, node: &ruby_prism::ConstantPathNode<'pr>) {
        if self.resource_ref(&node.as_node()) {
            self.mutation = Some(node.location().start_offset());
        }
        ruby_prism::visit_constant_path_node(self, node);
    }
    fn visit_constant_write_node(&mut self, node: &ruby_prism::ConstantWriteNode<'pr>) {
        if self
            .resources
            .contains(&ClassId(Symbol::from(constant_id_str(&node.name()))))
        {
            self.mutation = Some(node.location().start_offset());
        }
        ruby_prism::visit_constant_write_node(self, node);
    }
    fn visit_constant_and_write_node(&mut self, node: &ruby_prism::ConstantAndWriteNode<'pr>) {
        if self
            .resources
            .contains(&ClassId(Symbol::from(constant_id_str(&node.name()))))
        {
            self.mutation = Some(node.location().start_offset());
        }
        ruby_prism::visit_constant_and_write_node(self, node);
    }
    fn visit_constant_or_write_node(&mut self, node: &ruby_prism::ConstantOrWriteNode<'pr>) {
        if self
            .resources
            .contains(&ClassId(Symbol::from(constant_id_str(&node.name()))))
        {
            self.mutation = Some(node.location().start_offset());
        }
        ruby_prism::visit_constant_or_write_node(self, node);
    }
    fn visit_constant_operator_write_node(
        &mut self,
        node: &ruby_prism::ConstantOperatorWriteNode<'pr>,
    ) {
        if self
            .resources
            .contains(&ClassId(Symbol::from(constant_id_str(&node.name()))))
        {
            self.mutation = Some(node.location().start_offset());
        }
        ruby_prism::visit_constant_operator_write_node(self, node);
    }
    fn visit_constant_target_node(&mut self, node: &ruby_prism::ConstantTargetNode<'pr>) {
        if self
            .resources
            .contains(&ClassId(Symbol::from(constant_id_str(&node.name()))))
        {
            self.mutation = Some(node.location().start_offset());
        }
    }
    fn visit_constant_path_target_node(&mut self, node: &ruby_prism::ConstantPathTargetNode<'pr>) {
        let mut path = match node.parent() {
            Some(parent) => literal_constant(&parent).unwrap_or_default(),
            None => Vec::new(),
        };
        if let Some(name) = node.name() {
            path.push(constant_id_str(&name).to_owned());
        }
        if self
            .resources
            .contains(&ClassId(Symbol::from(path.join("::"))))
        {
            self.mutation = Some(node.location().start_offset());
        }
        ruby_prism::visit_constant_path_target_node(self, node);
    }
}

fn collect_resource_declarations(
    sources: &[crate::span::SourceFile],
    resources: &HashSet<ClassId>,
) -> IngestResult<HashMap<ClassId, ResourceDecl>> {
    let mut declarations = HashMap::new();
    for source in sources {
        // Mutation-only files must be included, not merely declaration files.
        if !resources
            .iter()
            .any(|id| source.text.contains(id.0.as_str()))
        {
            continue;
        }
        let parsed = ruby_prism::parse(source.text.as_bytes());
        let mut guard = SourceGuard {
            resources,
            counts: HashMap::new(),
            mutation: None,
        };
        ruby_prism::Visit::visit(&mut guard, &parsed.node());
        if let Some(offset) = guard.mutation {
            return Err(refuse(
                source,
                offset,
                "resource mutation or alias outside its declaration",
            ));
        }
        let mut direct = HashMap::new();
        if let Some(program) = parsed.node().as_program_node() {
            for stmt in program.statements().body().iter() {
                let Some(node) = stmt.as_class_node() else {
                    continue;
                };
                let Some(path) = class_name_path(&node) else {
                    continue;
                };
                let id = ClassId(Symbol::from(path.join("::")));
                if !resources.contains(&id) {
                    continue;
                }
                let offset = node.location().start_offset();
                if path.len() != 1
                    || declarations.contains_key(&id)
                    || literal_constant(&node.constant_path()) != Some(path)
                {
                    return Err(refuse(
                        source,
                        offset,
                        "resource must be an unreopened literal top-level class",
                    ));
                }
                if let Some(parent) = node.superclass() {
                    if !literal_constant(&parent).is_some_and(|p| p.len() == 1) {
                        return Err(refuse(
                            source,
                            parent.location().start_offset(),
                            "superclass must be an exact top-level resource",
                        ));
                    }
                }
                let fields = collect_fields(&node, source)?;
                declarations.insert(
                    id.clone(),
                    ResourceDecl {
                        fields,
                        span: Span {
                            file: super::sources::file_id(&source.path),
                            start: offset as u32,
                            end: offset as u32,
                        },
                    },
                );
                *direct.entry(id).or_insert(0) += 1;
            }
        }
        if guard.counts.iter().any(|(id, n)| direct.get(id) != Some(n)) {
            return Err(refuse(
                source,
                0,
                "resource fragments must be direct top-level classes",
            ));
        }
    }
    Ok(declarations)
}

fn collect_fields(
    node: &ruby_prism::ClassNode<'_>,
    source: &SourceFile,
) -> IngestResult<Vec<Field>> {
    let mut fields = Vec::new();
    let mut included = false;
    if let Some(body) = node.body() {
        for statement in flatten_statements(body) {
            let offset = statement.location().start_offset();
            let fail = |reason| refuse(source, offset, reason);
            let Some(call) = statement.as_call_node() else {
                return Err(fail("resource body must contain only literal declarations"));
            };
            if call.receiver().is_some() || call.block().is_some() {
                return Err(fail("receiver or block in resource declaration"));
            }
            let args: Vec<_> = call
                .arguments()
                .map(|a| a.arguments().iter().collect())
                .unwrap_or_default();
            if constant_id_str(&call.name()) == "include" {
                if included
                    || node.superclass().is_some()
                    || !fields.is_empty()
                    || args.len() != 1
                    || literal_constant(&args[0])
                        != Some(vec!["Alba".to_owned(), "Resource".to_owned()])
                {
                    return Err(fail(
                        "only include Alba::Resource once before root declarations is supported",
                    ));
                }
                included = true;
            } else {
                if node.superclass().is_none() && !included {
                    return Err(fail(
                        "root declarations require include Alba::Resource first",
                    ));
                }
                let Some(mut next) = declaration(constant_id_str(&call.name()), &args) else {
                    return Err(fail("unsupported Alba declaration"));
                };
                fields.append(&mut next);
            }
        }
    }
    Ok(fields)
}

fn validate_resource_graph(
    app: &crate::App,
    sources: &[crate::span::SourceFile],
    resources: &HashSet<ClassId>,
    declarations: &HashMap<ClassId, ResourceDecl>,
) -> IngestResult<HashMap<ClassId, Vec<Field>>> {
    let mut flattened = HashMap::new();
    for id in resources {
        let Some(decl) = declarations.get(id) else {
            return Err(IngestError::Unsupported {
                file: "<alba>".into(),
                message: format!("cannot establish complete source for {}", id.0),
            });
        };
        let source = &sources[decl.span.file.0 as usize - 1];
        let fail = |reason| refuse(source, decl.span.start as usize, reason);
        if app
            .library_classes
            .iter()
            .any(|c| c.name.0.as_str() == "Alba" || c.name.0.as_str().ends_with("Alba::Resource"))
        {
            return Err(fail(
                "source-defined Alba namespace is not the external Alba contract",
            ));
        }
        let mut chain = Vec::new();
        let mut seen = HashSet::new();
        let mut cursor = Some(id);
        while let Some(current) = cursor {
            if !seen.insert(current.clone()) {
                return Err(fail("cyclic resource ancestry"));
            }
            let classes: Vec<_> = app
                .library_classes
                .iter()
                .filter(|c| &c.name == current)
                .collect();
            if classes.len() != 1 {
                return Err(fail("reopened resource"));
            }
            let class = classes[0];
            if class.is_module || !class.methods.is_empty() || !class.constants.is_empty() {
                return Err(fail("custom resource methods or constants are unsupported"));
            }
            if !resources.contains(current) {
                return Err(fail("non-resource superclass"));
            }
            chain.push(current);
            cursor = class.parent.as_ref();
        }
        let mut all = Vec::<Field>::new();
        for ancestor in chain.into_iter().rev() {
            let Some(own) = declarations.get(ancestor) else {
                return Err(fail("resource source unavailable"));
            };
            for field in &own.fields {
                if all.iter().any(|f| f.name == field.name) {
                    return Err(fail("duplicate resource field"));
                }
                all.push(field.clone());
            }
        }
        if all.iter().filter(|f| f.resource.is_some()).count() > 1 {
            return Err(fail("only one nested resource is supported"));
        }
        flattened.insert(id.clone(), all);
    }
    for (id, all) in &flattened {
        for target in all.iter().filter_map(|f| f.resource.as_ref()) {
            let decl = &declarations[id];
            let source = &sources[decl.span.file.0 as usize - 1];
            let Some(nested) = flattened.get(target) else {
                return Err(refuse(
                    source,
                    decl.span.start as usize,
                    "association resource must name an exact known Alba resource",
                ));
            };
            if nested.iter().any(|f| f.resource.is_some()) {
                return Err(refuse(
                    source,
                    decl.span.start as usize,
                    "nested resource must be a leaf",
                ));
            }
        }
    }
    Ok(flattened)
}

fn refuse(source: &SourceFile, offset: usize, reason: &str) -> IngestError {
    let (line, column) = source.line_col(offset as u32);
    IngestError::Unsupported {
        file: source.path.clone(),
        message: format!("{line}:{column}: Alba source-property subset: {reason}"),
    }
}

fn property(node: &ruby_prism::Node<'_>) -> Option<Symbol> {
    let value = symbol_value(node)?;
    let mut chars = value.chars();
    if !chars
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c == '_')
        || !chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return None;
    }
    Some(Symbol::from(value))
}

fn declaration(method: &str, args: &[ruby_prism::Node<'_>]) -> Option<Vec<Field>> {
    match method {
        "attributes" if !args.is_empty() => args
            .iter()
            .map(|a| {
                Some(Field {
                    name: property(a)?,
                    resource: None,
                })
            })
            .collect(),
        "one" if args.len() == 2 => {
            let name = property(&args[0])?;
            let hash = args[1].as_keyword_hash_node()?;
            let entries: Vec<_> = hash.elements().iter().collect();
            if entries.len() != 1 {
                return None;
            }
            let assoc = entries[0].as_assoc_node()?;
            if property(&assoc.key())?.as_str() != "resource" {
                return None;
            }
            let path = literal_constant(&assoc.value())?;
            if path.len() != 1 {
                return None;
            }
            Some(vec![Field {
                name,
                resource: Some(ClassId(Symbol::from(path[0].as_str()))),
            }])
        }
        _ => None,
    }
}

fn synthesize_methods(class: &mut LibraryClass, fields: &[Field], span: Span) -> IngestResult<()> {
    let entries = fields
        .iter()
        .map(|field| {
            let name = field.name.as_str();
            let value = match &field.resource {
                Some(resource) => format!("{}.new(object.{name}).to_h", resource.0),
                None => format!("object.{name}"),
            };
            format!("{} => {value}", serde_json::to_string(name).unwrap())
        })
        .collect::<Vec<_>>()
        .join(", ");
    // Same ordinary-Ruby-to-method-IR construction used by delegate/current.
    // The executable object guard narrows conservative nullable ivar reads;
    // it does not erase nullable source properties or stamp return types.
    let source = format!(
        "class {}\n def initialize(object)\n @__alba_object = object\n end\n def to_h\n object = @__alba_object || raise(\"Alba source object required\")\n {{{entries}}}\n end\nend\n",
        class.name.0
    );
    let (parsed, diags) =
        super::prism::scope(|| super::ingest_library_classes(source.as_bytes(), "<alba>"));
    let mut parsed = parsed?;
    if !diags.is_empty() {
        return Err(IngestError::Parse {
            file: "<alba>".into(),
            message: format!("invalid synthesized serializer: {diags:?}"),
        });
    }
    fn mark(expr: &mut Expr, span: Span) {
        expr.span = span;
        expr.node.for_each_child_mut(&mut |child| mark(child, span));
    }
    for method in &mut parsed[0].methods {
        mark(&mut method.body, span);
        method.name_span = span;
    }
    class.methods.append(&mut parsed.remove(0).methods);
    class.origin = Some(LibraryClassOrigin::AlbaResource {
        declaration_span: span,
    });
    class.includes.retain(|i| i.0.as_str() != RESOURCE);
    class.unknown_calls.clear();
    Ok(())
}
