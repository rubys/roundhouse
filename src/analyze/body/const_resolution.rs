//! Ruby constant references indexed from the application's Ruby sources.
//!
//! Rubydex resolves Ruby lexical names once, then releases its graph.
//! Roundhouse retains source-position answers and keys inferred values
//! by Rubydex declaration IDs.

use std::collections::{HashMap, hash_map::Entry};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;

use rubydex::indexing::local_graph::LocalGraph;
use rubydex::indexing::{IndexerBackend, LanguageId, build_local_graph};
use rubydex::model::built_in::BUILT_IN_URI;
use rubydex::model::declaration::Declaration;
use rubydex::model::document::Document as RubydexDocument;
use rubydex::model::definitions::Definition;
use rubydex::model::graph::Graph;
use rubydex::model::identity_maps::IdentityHashMap;
use rubydex::model::name::ParentScope;
use rubydex::model::ids::{DeclarationId, UriId};
use rubydex::resolution::Resolver;

use crate::ident::{ClassId, Symbol};
use crate::span::{FileId, SourceFile, Span};
use crate::ty::Ty;

// Rubydex's minimal built-ins stop at Object/Module/Class. These are
// Ruby core classes already modeled by Roundhouse's primitive dispatch,
// declared as RBS so the source graph resolves them by Ruby name.
const CORE_RBS: &str = "\
class Numeric < Object\nend\n\
class Integer < Numeric\nend\n\
class Float < Numeric\nend\n\
class String < Object\nend\n\
class Array < Object\nend\n\
class Hash < Object\nend\n\
class Symbol < Object\nend\n\
class TrueClass < Object\nend\n\
class FalseClass < Object\nend\n\
class NilClass < Object\nend\n\
class Regexp < Object\nend\n\
class Exception < Object\nend\n\
class StandardError < Exception\nend\n";

const CORE_URI: &str = "roundhouse-core:rbs";
const RUNTIME_URI_PREFIX: &str = "roundhouse-runtime:";

pub(super) enum ResolvedConstant {
    Namespace { class: Arc<ClassId>, runtime: bool },
    Value { declaration: DeclarationId, runtime: Option<Arc<Ty>> },
}

/// Rubydex answers for one source file.
#[derive(Default)]
struct FileAnswers {
    /// Keyed by the byte offset where each written constant name starts.
    references: HashMap<u32, Option<ResolvedConstant>>,
    /// Each constant definition as (offset where its name ends, name,
    /// declaration), sorted by offset.
    constants: Vec<(u32, Box<str>, DeclarationId)>,
}

pub(crate) struct ConstResolver {
    /// Indexed by `FileId - 1`. `None` marks a source that Rubydex did
    /// not index, such as an ERB template.
    files: Vec<Option<FileAnswers>>,
    /// Includes paths, full text and order because answers use file IDs
    /// and byte offsets. Retain no second copy of the source snapshot.
    source_fingerprint: u64,
}

fn is_indexed(source: &SourceFile) -> bool {
    source.path.ends_with(".rb")
}

fn source_fingerprint(sources: &[SourceFile]) -> u64 {
    let mut hash = DefaultHasher::new();
    sources.len().hash(&mut hash);
    for source in sources {
        source.path.hash(&mut hash);
        source.text.hash(&mut hash);
    }
    hash.finish()
}

/// One Rubydex input document. The indexing thread builds its URI.
struct Document<'a> {
    uri_prefix: &'static str,
    path: &'a str,
    text: &'a str,
    language: LanguageId,
}

impl Document<'_> {
    fn index(&self) -> LocalGraph {
        let uri: Box<str> = [self.uri_prefix, self.path].concat().into();
        constant_graph(
            build_local_graph(uri, self.text, &self.language, IndexerBackend::RubyIndexer),
            self.text,
        )
    }
}

/// Roundhouse asks Rubydex only about constants written in the source.
/// Instance methods, variables, attributes, visibility calls, and method
/// aliases do not change how a constant resolves, so they leave the graph
/// before the merge. Methods with a receiver stay, because `def self.x`
/// and `def X.y` create singleton classes. The indexer also records a
/// `<X>` reference to the singleton class of each method call receiver.
/// No written name resolves through those references, and in a large
/// app they are 38% of all references, so they leave too. The rebuilt
/// document lists only the kept entries.
fn constant_graph(local: LocalGraph, text: &str) -> LocalGraph {
    let (uri_id, document, definitions, strings, names, constant_references, _, _) =
        local.into_parts();
    let document = RubydexDocument::new(document.uri().into(), text);
    let mut graph = LocalGraph::from_parts(uri_id, document, strings, names);
    for definition in definitions.into_values() {
        let keep = match &definition {
            Definition::Class(_)
            | Definition::SingletonClass(_)
            | Definition::Module(_)
            | Definition::Constant(_)
            | Definition::ConstantAlias(_)
            | Definition::ConstantVisibility(_) => true,
            Definition::Method(method) => method.receiver().is_some(),
            _ => false,
        };
        if keep {
            graph.add_definition(definition);
        }
    }
    for reference in constant_references.into_values() {
        let singleton_receiver = matches!(
            graph.names().get(reference.name_id()).map(|name| name.parent_scope()),
            Some(ParentScope::Attached(_))
        );
        if !singleton_receiver {
            graph.add_constant_reference(reference);
        }
    }
    graph
}

/// Rubydex work runs on at most this many threads. The caller thread
/// merges local graphs one at a time. On an app with 60 MB of Ruby, more
/// than 8 parsers made the merge no faster and held more parse memory.
#[cfg(not(target_arch = "wasm32"))]
const MAX_WORKERS: usize = 8;

/// Rubydex parses each document independently, so workers build the
/// local graphs in parallel. This thread merges them in input order, so
/// the graph does not depend on thread timing. Every URI is new in this
/// analysis: a bulk merge skips incremental invalidation.
#[cfg(not(target_arch = "wasm32"))]
fn index_documents(documents: &[Document<'_>]) -> Graph {
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;

    let workers = std::thread::available_parallelism()
        .map_or(1, std::num::NonZeroUsize::get)
        .min(MAX_WORKERS)
        .min(documents.len());
    let next = AtomicUsize::new(0);
    let mut graph = Graph::new();
    std::thread::scope(|scope| {
        let (sender, receiver) = mpsc::channel();
        for _ in 0..workers {
            let sender = sender.clone();
            let next = &next;
            std::thread::Builder::new()
                .name("rubydex-index".into())
                // The indexer recurses through each expression, like the
                // compiler walkers, so it gets the same stack budget.
                .stack_size(crate::stack::COMPILER_STACK_BYTES)
                .spawn_scoped(scope, move || {
                    loop {
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        let Some(document) = documents.get(index) else { break };
                        if sender.send((index, document.index())).is_err() {
                            break;
                        }
                    }
                })
                .expect("spawn a Rubydex indexing thread");
        }
        drop(sender);
        let mut pending = BTreeMap::new();
        let mut merged = 0;
        for (index, local) in receiver {
            pending.insert(index, local);
            while let Some(local) = pending.remove(&merged) {
                graph.extend(local);
                merged += 1;
            }
        }
    });
    graph
}

/// The browser build has no threads.
#[cfg(target_arch = "wasm32")]
fn index_documents(documents: &[Document<'_>]) -> Graph {
    let mut graph = Graph::new();
    for document in documents {
        graph.extend(document.index());
    }
    graph
}

/// Roundhouse models the Ruby core: the RBS block above, the Ruby
/// runtime, and the classes that Rubydex itself installs (`BasicObject`,
/// `Kernel`, `Object`, `Module`, `Class`).
fn is_runtime_declaration(graph: &Graph, declaration: &Declaration) -> bool {
    declaration.definitions().iter().any(|id| {
        graph
            .definitions()
            .get(id)
            .and_then(|definition| graph.documents().get(definition.uri_id()))
            .is_some_and(|document| {
                let uri = document.uri();
                uri == CORE_URI || uri == BUILT_IN_URI || uri.starts_with(RUNTIME_URI_PREFIX)
            })
    })
}

/// Rubydex promotes `X = <method call>` to a module when code calls a
/// method on `X`, because the call could build a class (`Struct.new`).
/// Roundhouse types the assigned value instead. A `class` or `module`
/// definition, including `Class.new` and `Module.new`, keeps the namespace.
fn is_assigned_value(graph: &Graph, declaration: &Declaration) -> bool {
    let mut assigned = false;
    for id in declaration.definitions() {
        match graph.definitions().get(id) {
            Some(Definition::Constant(_)) => assigned = true,
            Some(Definition::Class(_) | Definition::Module(_) | Definition::SingletonClass(_)) => {
                return false;
            }
            _ => {}
        }
    }
    assigned
}

/// Literal runtime values come from the same embedded implementations
/// the name graph indexes. Keep their full owners, not suffix aliases,
/// and share types between references instead of copying their trees.
fn runtime_value_types() -> &'static HashMap<String, Arc<Ty>> {
    static VALUES: std::sync::OnceLock<HashMap<String, Arc<Ty>>> = std::sync::OnceLock::new();
    VALUES.get_or_init(|| {
        let mut values: HashMap<String, Arc<Ty>> = HashMap::new();
        let mut ambiguous = std::collections::HashSet::new();
        for (_, text) in crate::runtime_files::ruby_sources() {
            let (_, owners) = crate::runtime_src::parse_module_constant_tables(text, true);
            for (owner, constants) in owners {
                for (name, ty) in constants {
                    let name = format!("{}::{}", owner.0.as_str(), name.as_str());
                    if ambiguous.contains(&name) {
                        continue;
                    }
                    if values.get(&name).is_some_and(|previous| **previous != ty) {
                        values.remove(&name);
                        ambiguous.insert(name);
                    } else {
                        values.insert(name, Arc::new(ty));
                    }
                }
            }
        }
        values
    })
}

impl ConstResolver {
    /// Index real Ruby app sources with their original 1-based file IDs.
    pub(crate) fn from_app_sources(sources: &[SourceFile]) -> Self {
        let mut graph = crate::timings::phase("rubydex: index", || index_sources(sources));
        // Rubydex resolves all source references after every file enters
        // its graph, including declarations in later files.
        crate::timings::phase("rubydex: resolve", || Resolver::new(&mut graph).resolve());
        let files = crate::timings::phase("rubydex: answers", || collect_answers(&graph, sources));
        drop(graph);
        Self { files, source_fingerprint: source_fingerprint(sources) }
    }

    fn file(&self, file: FileId) -> Option<&FileAnswers> {
        let index = file.0.checked_sub(1)?;
        self.files.get(index as usize)?.as_ref()
    }

    pub(super) fn has_source_file(&self, file: FileId) -> bool {
        self.file(file).is_some()
    }

    /// IR spans cover the entire `A::B::C` path. Rubydex records the
    /// final name at the span end minus the name's byte length. The typer
    /// can qualify a relative path, but the final name stays the same.
    pub(super) fn reference(
        &self,
        span: Span,
        path: &[Symbol],
    ) -> Option<Option<&ResolvedConstant>> {
        let length = u32::try_from(path.last()?.as_str().len()).ok()?;
        let start = span.end.checked_sub(length)?;
        self.file(span.file)?.references.get(&start).map(Option::as_ref)
    }

    /// The declaration that a constant assignment defines. The IR keeps
    /// the assigned value, and Ruby writes the name just before it, so
    /// the definition is the last one in the file whose name ends at or
    /// before the value. Ingest can give the value another IR owner (a
    /// file-level constant goes to the first class of its file), but
    /// the source position does not change.
    pub(crate) fn constant_declaration(&self, value: Span, name: &str) -> Option<DeclarationId> {
        if value.is_synthetic() {
            return None;
        }
        let constants = &self.file(value.file)?.constants;
        let index = constants.partition_point(|(end, ..)| *end <= value.start).checked_sub(1)?;
        let (_, defined, id) = &constants[index];
        (defined.as_ref() == name).then_some(*id)
    }
}

/// Rubydex answers that ingest resolved for `App::sources`. They derive
/// from the sources, so `App` does not serialize or compare them.
#[derive(Clone, Default)]
pub struct PreparedConstResolver(Option<Arc<ConstResolver>>);

impl PreparedConstResolver {
    /// The answers that ingest prepared, if they cover these sources.
    /// Cloning an app retains the cache, but callers can edit its public
    /// sources. Rebuild when paths, text or file order have changed.
    pub(crate) fn for_sources(&self, sources: &[SourceFile]) -> Arc<ConstResolver> {
        match &self.0 {
            Some(resolver) if resolver.source_fingerprint == source_fingerprint(sources) => {
                Arc::clone(resolver)
            }
            _ => Arc::new(ConstResolver::from_app_sources(sources)),
        }
    }
}

impl PartialEq for PreparedConstResolver {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl std::fmt::Debug for PreparedConstResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = if self.0.is_some() { "ready" } else { "absent" };
        write!(f, "PreparedConstResolver({state})")
    }
}

/// Rubydex resolution that runs while ingest finishes the passes that
/// register no sources. The browser build has no threads, so it
/// resolves before those passes.
pub(crate) struct ConstResolverTask {
    #[cfg(not(target_arch = "wasm32"))]
    thread: std::thread::JoinHandle<ConstResolver>,
    #[cfg(target_arch = "wasm32")]
    resolver: ConstResolver,
}

impl ConstResolverTask {
    pub(crate) fn start(sources: Arc<Vec<SourceFile>>) -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let thread = std::thread::Builder::new()
                .name("rubydex".into())
                .stack_size(crate::stack::COMPILER_STACK_BYTES)
                .spawn(move || ConstResolver::from_app_sources(&sources))
                .expect("spawn the Rubydex thread");
            Self { thread }
        }
        #[cfg(target_arch = "wasm32")]
        Self {
            resolver: ConstResolver::from_app_sources(&sources),
        }
    }

    pub(crate) fn finish(self) -> PreparedConstResolver {
        #[cfg(not(target_arch = "wasm32"))]
        let resolver = self
            .thread
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
        #[cfg(target_arch = "wasm32")]
        let resolver = self.resolver;
        PreparedConstResolver(Some(Arc::new(resolver)))
    }
}

/// Framework declarations come from embedded Ruby implementations and
/// Ruby core RBS, never from fabricated suffix aliases. App sources keep
/// their paths as URIs so answers map back to their file IDs.
fn index_sources(sources: &[SourceFile]) -> Graph {
    let mut documents = vec![Document {
        uri_prefix: CORE_URI,
        path: "",
        text: CORE_RBS,
        language: LanguageId::Rbs,
    }];
    documents.extend(crate::runtime_files::ruby_sources().map(|(path, text)| Document {
        uri_prefix: RUNTIME_URI_PREFIX,
        path,
        text,
        language: LanguageId::Ruby,
    }));
    documents.extend(sources.iter().filter(|source| is_indexed(source)).map(|source| Document {
        uri_prefix: "",
        path: &source.path,
        text: &source.text,
        language: LanguageId::Ruby,
    }));
    index_documents(&documents)
}

/// Rubydex resolves names, ownership, and lexical scope. Keep only its
/// immutable answers for positions Roundhouse will type; the full graph
/// leaves memory before the inference fixpoint starts. The answers for
/// each file are independent, so workers collect them in parallel.
#[cfg(not(target_arch = "wasm32"))]
fn collect_answers(graph: &Graph, sources: &[SourceFile]) -> Vec<Option<FileAnswers>> {
    let workers = std::thread::available_parallelism()
        .map_or(1, std::num::NonZeroUsize::get)
        .min(MAX_WORKERS);
    let chunk = sources.len().div_ceil(workers).max(1);
    std::thread::scope(|scope| {
        let chunks: Vec<_> = sources
            .chunks(chunk)
            .map(|chunk| scope.spawn(move || answer_files(graph, chunk)))
            .collect();
        chunks
            .into_iter()
            .flat_map(|chunk| {
                chunk.join().unwrap_or_else(|panic| std::panic::resume_unwind(panic))
            })
            .collect()
    })
}

/// The browser build has no threads.
#[cfg(target_arch = "wasm32")]
fn collect_answers(graph: &Graph, sources: &[SourceFile]) -> Vec<Option<FileAnswers>> {
    answer_files(graph, sources)
}

fn answer_files(graph: &Graph, sources: &[SourceFile]) -> Vec<Option<FileAnswers>> {
    let mut class_cache = IdentityHashMap::default();
    sources
        .iter()
        .map(|source| is_indexed(source).then(|| answer_file(graph, source, &mut class_cache)))
        .collect()
}

fn answer_file(
    graph: &Graph,
    source: &SourceFile,
    class_cache: &mut IdentityHashMap<DeclarationId, (Arc<ClassId>, bool)>,
) -> FileAnswers {
    let mut answers = FileAnswers::default();
    let Some(document) = graph.documents().get(&UriId::from(source.path.as_str())) else {
        return answers;
    };
    for reference_id in document.constant_references() {
        let Some(reference) = graph.constant_references().get(reference_id) else {
            continue;
        };
        let offset = reference.offset();
        // A superclass or mixin reference spans its whole `A::B` path,
        // so it starts at the same offset as the `A` reference. Only a
        // reference whose span is exactly its written name belongs to
        // that offset.
        let written = source.text.get(offset.start() as usize..offset.end() as usize);
        let name = graph
            .names()
            .get(reference.name_id())
            .and_then(|name| graph.strings().get(name.str()));
        if !name.is_some_and(|name| written == Some(name.as_str())) {
            continue;
        }
        let target = graph
            .name_id_to_declaration_id(*reference.name_id())
            .and_then(|id| graph.declarations().get(id).map(|decl| (*id, decl)))
            .map(|(id, declaration)| {
                if declaration.as_namespace().is_some() && !is_assigned_value(graph, declaration) {
                    let (class, runtime) = class_cache.entry(id).or_insert_with(|| {
                        (
                            Arc::new(ClassId(Symbol::from(declaration.name()))),
                            is_runtime_declaration(graph, declaration),
                        )
                    });
                    ResolvedConstant::Namespace {
                        class: Arc::clone(class),
                        runtime: *runtime,
                    }
                } else {
                    // An app write to the same declaration must never
                    // borrow the runtime's previous literal type.
                    let app_write = declaration.definitions().iter().any(|id| {
                        graph.definitions().get(id).is_some_and(|definition| {
                            matches!(definition, Definition::Constant(_) | Definition::ConstantAlias(_))
                                && graph.documents().get(definition.uri_id()).is_some_and(|document| {
                                    !document.uri().starts_with(RUNTIME_URI_PREFIX)
                                })
                        })
                    });
                    let runtime = (!app_write && is_runtime_declaration(graph, declaration))
                        .then(|| runtime_value_types().get(declaration.name()).cloned())
                        .flatten();
                    ResolvedConstant::Value { declaration: id, runtime }
                }
            });
        match answers.references.entry(offset.start()) {
            Entry::Vacant(slot) => {
                slot.insert(target);
            }
            Entry::Occupied(mut slot) => {
                if slot.get().is_none() && target.is_some() {
                    slot.insert(target);
                }
            }
        }
    }
    for definition_id in document.definitions() {
        let definition = graph.definitions().get(definition_id);
        let (name_id, offset) = match definition {
            Some(Definition::Constant(it)) => (it.name_id(), it.offset()),
            Some(Definition::ConstantAlias(it)) => (it.name_id(), it.offset()),
            _ => continue,
        };
        let name = graph.names().get(name_id).and_then(|name| graph.strings().get(name.str()));
        let id = definition.and_then(|definition| graph.definition_to_declaration_id(definition));
        if let (Some(name), Some(id)) = (name, id) {
            answers.constants.push((offset.end(), name.as_str().into(), *id));
        }
    }
    answers.constants.sort_unstable_by_key(|(end, ..)| *end);
    answers
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepared_answers_require_the_same_paths_text_and_file_order() {
        let mut sources = vec![
            SourceFile {
                path: "first.rb".into(),
                text: "module Alpha\n  class Widget; end\n  def self.value; Widget; end\nend\n".into(),
            },
            SourceFile {
                path: "other.rb".into(),
                text: "module Other\n  class Widget; end\n  def self.value; Widget; end\nend\n".into(),
            },
        ];
        let snapshot = sources.clone();
        let prepared = ConstResolverTask::start(Arc::new(snapshot.clone())).finish();
        let original = prepared.for_sources(&sources);
        assert!(Arc::ptr_eq(&original, &prepared.clone().for_sources(&sources)));
        let resolved = |resolver: &ConstResolver, source: &SourceFile| {
            let start = source.text.rfind("Widget").unwrap() as u32;
            let span = Span { file: FileId(1), start, end: start + 6 };
            match resolver.reference(span, &[Symbol::new("Widget")]) {
                Some(Some(ResolvedConstant::Namespace { class, .. })) => class.0.as_str().to_owned(),
                _ => panic!("expected a resolved Widget"),
            }
        };
        assert_eq!(resolved(&original, &sources[0]), "Alpha::Widget");

        // Same source count, byte length and reference offset, different binding.
        sources[0].text = sources[0].text.replace("Alpha", "Bravo");
        let changed = prepared.clone().for_sources(&sources);
        assert_eq!(resolved(&changed, &sources[0]), "Bravo::Widget");
        assert!(!Arc::ptr_eq(&original, &changed));

        // FileId(1) must follow input order, not the cached first file.
        sources = snapshot.clone();
        sources.swap(0, 1);
        assert_eq!(resolved(&prepared.for_sources(&sources), &sources[0]), "Other::Widget");

        // A path-only change can make the same text non-indexed.
        sources = snapshot;
        sources[0].path = "first.md".into();
        assert!(!prepared.for_sources(&sources).has_source_file(FileId(1)));
    }
}
