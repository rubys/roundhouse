//! Ruby constant references indexed from the application's Ruby sources.
//!
//! Rubydex resolves Ruby lexical names once, then releases its graph.
//! Roundhouse retains source-position answers and keys inferred values
//! by Rubydex declaration IDs.

use crate::ingest::prism_api::PrismLocation;
mod hoist;

use std::collections::hash_map::Entry;
use crate::det::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::{Arc, OnceLock};

use rubydex::diagnostic::{Diagnostic as RubydexDiagnostic, Rule};
use rubydex::indexing::local_graph::LocalGraph;
use rubydex::indexing::{IndexerBackend, LanguageId, build_local_graph};
use rubydex::model::built_in::BUILT_IN_URI;
use rubydex::model::declaration::{Ancestor, Declaration};
use rubydex::model::document::Document as RubydexDocument;
use rubydex::model::definitions::{Definition, Receiver};
use rubydex::model::graph::Graph;
use rubydex::model::identity_maps::IdentityHashMap;
use rubydex::model::name::ParentScope;
use rubydex::model::ids::{DeclarationId, NameId, UriId};
use rubydex::resolution::Resolver;

use crate::ident::{ClassId, Symbol};
use crate::span::{FileId, SourceFile, Span};
use crate::ty::Ty;
use hoist::{hoisted_name, nested_namespaces_index, runtime_value_types};

// Rubydex's minimal built-ins stop at Object/Module/Class. Classes
// without value Consts live in `CORE_RBS_BASE`; value Consts are one
// table (`CORE_VALUE_CONSTS`) that builds both the RBS fields and the
// typed answer — a second copy is how the type drifts from the graph.
const CORE_RBS_BASE: &str = "\
class Numeric < Object\nend\n\
class Integer < Numeric\nend\n\
class String < Object\nend\n\
class Array < Object\nend\n\
class Hash < Object\nend\n\
class Symbol < Object\nend\n\
class Data < Object\nend\n\
class TrueClass < Object\nend\n\
class FalseClass < Object\nend\n\
class NilClass < Object\nend\n\
class Regexp < Object\nend\n\
class Exception < Object\nend\n\
class StandardError < Exception\nend\n";

/// `(owner, field, type)` — Ruby's `Owner::FIELD` value Consts.
/// `core_rbs()` emits the RBS; `core_value_type` answers the type.
const CORE_VALUE_CONSTS: &[(&str, &str, fn() -> Ty)] = &[
    ("Float", "INFINITY", || Ty::Float),
    ("Float", "NAN", || Ty::Float),
    ("Float", "EPSILON", || Ty::Float),
    ("Float", "MAX", || Ty::Float),
    ("Float", "MIN", || Ty::Float),
    ("IO", "NULL", || Ty::Str),
    // `Regexp.new(selector, Regexp::IGNORECASE)` — lobsters' domain selectors.
    ("Regexp", "IGNORECASE", || Ty::Int),
    ("Regexp", "EXTENDED", || Ty::Int),
    ("Regexp", "MULTILINE", || Ty::Int),
    ("File", "NULL", || Ty::Str),
    // `File.open(path, File::RDWR | File::CREAT, 0644)` and
    // `file.flock(File::LOCK_EX | File::LOCK_NB)` — campfire's WAL
    // checkpointer. Ruby defines them on `File::Constants`, which `File`
    // includes; CRuby and spinel answer the same POSIX values.
    ("File", "RDONLY", || Ty::Int),
    ("File", "WRONLY", || Ty::Int),
    ("File", "RDWR", || Ty::Int),
    ("File", "CREAT", || Ty::Int),
    ("File", "EXCL", || Ty::Int),
    ("File", "TRUNC", || Ty::Int),
    ("File", "APPEND", || Ty::Int),
    ("File", "LOCK_SH", || Ty::Int),
    ("File", "LOCK_EX", || Ty::Int),
    ("File", "LOCK_NB", || Ty::Int),
    ("File", "LOCK_UN", || Ty::Int),
    // `path.start_with?(root + File::SEPARATOR)`, a `PATH` joined with
    // `File::PATH_SEPARATOR`, and `File.open(path, File::RDONLY |
    // File::NOFOLLOW | File::NONBLOCK)`; CRuby and spinel answer the same
    // values.
    ("File", "SEPARATOR", || Ty::Str),
    ("File", "PATH_SEPARATOR", || Ty::Str),
    ("File", "NOFOLLOW", || Ty::Int),
    ("File", "NONBLOCK", || Ty::Int),
    ("File", "NOCTTY", || Ty::Int),
    ("File", "SYNC", || Ty::Int),
    ("File", "DSYNC", || Ty::Int),
    ("File", "BINARY", || Ty::Int),
    ("Encoding", "UTF_8", encoding_ty),
    ("Encoding", "BINARY", encoding_ty),
    ("Encoding", "ASCII_8BIT", encoding_ty),
    ("Encoding", "US_ASCII", encoding_ty),
    // `http.verify_mode = OpenSSL::SSL::VERIFY_PEER` — lobsters' Sponge.
    // The openssl library's on the ruby family, spinel's
    // `packages/openssl` (same values) on spinel.
    ("OpenSSL::SSL", "VERIFY_NONE", || Ty::Int),
    ("OpenSSL::SSL", "VERIFY_PEER", || Ty::Int),
    // `URI::RFC2396_PARSER.escape(url)` — lobsters' webmention job. The
    // uri library's on the ruby family, spinel's `packages/uri` on spinel;
    // DEFAULT_PARSER is RFC 3986's, as on Ruby 3.4.
    // `Math.cos(deg * Math::PI / 180)` — svg-graph's label rotation.
    ("Math", "PI", || Ty::Float),
    ("Math", "E", || Ty::Float),
    ("URI", "DEFAULT_PARSER", uri_rfc3986_parser),
    ("URI", "RFC3986_PARSER", uri_rfc3986_parser),
    ("URI", "RFC2396_PARSER", uri_rfc2396_parser),
];

fn uri_rfc2396_parser() -> Ty {
    Ty::Class { id: ClassId(Symbol::from("URI::RFC2396_Parser")), args: vec![].into() }
}

fn uri_rfc3986_parser() -> Ty {
    Ty::Class { id: ClassId(Symbol::from("URI::RFC3986_Parser")), args: vec![].into() }
}

/// Owners of value Consts that Ruby defines as modules, not classes.
const CORE_VALUE_MODULES: &[&str] = &["URI", "Math"];

/// Ruby's top-level value Consts: `STDERR.puts "…"` (discourse's
/// warnings) reads the process's own stream, which `$stderr` is only
/// until something reassigns it.
const CORE_TOP_CONSTS: &[&str] = &["STDIN", "STDOUT", "STDERR"];

/// `if RUBY_VERSION >= "2.7.1"` (aasm, hashie): the running Ruby's own
/// version, which a compile-time answer would freeze to the wrong one.
const CORE_TOP_STR_CONSTS: &[&str] = &["RUBY_VERSION"];

fn io_ty() -> Ty {
    Ty::Class { id: ClassId(Symbol::from("IO")), args: vec![].into() }
}

/// Classes that own rows in `CORE_VALUE_CONSTS`, with their RBS parents.
const CORE_VALUE_CLASSES: &[(&str, &str)] = &[
    ("Float", "Numeric"),
    ("IO", "Object"),
    ("File", "IO"),
    ("Encoding", "Object"),
    ("OpenSSL::SSL", "Object"),
    ("Regexp", "Object"),
];

fn encoding_ty() -> Ty {
    Ty::Class { id: ClassId(Symbol::from("Encoding")), args: vec![].into() }
}

fn rbs_type_name(ty: &Ty) -> &'static str {
    match ty {
        Ty::Float => "Float",
        Ty::Int => "Integer",
        Ty::Str => "String",
        Ty::Class { id, .. } if id.0.as_str() == "Encoding" => "Encoding",
        _ => "untyped",
    }
}

fn core_rbs() -> &'static str {
    static RBS: OnceLock<String> = OnceLock::new();
    RBS.get_or_init(|| {
        let mut out = String::from(CORE_RBS_BASE);
        for (class, parent) in CORE_VALUE_CLASSES {
            out.push_str("class ");
            out.push_str(class);
            out.push_str(" < ");
            out.push_str(parent);
            out.push('\n');
            for (owner, field, ty) in CORE_VALUE_CONSTS {
                if owner == class {
                    out.push_str("  ");
                    out.push_str(field);
                    out.push_str(": ");
                    out.push_str(rbs_type_name(&ty()));
                    out.push('\n');
                }
            }
            out.push_str("end\n");
        }
        for module in CORE_VALUE_MODULES {
            out.push_str("module ");
            out.push_str(module);
            out.push('\n');
            for (owner, field, ty) in CORE_VALUE_CONSTS {
                if owner == module {
                    out.push_str("  ");
                    out.push_str(field);
                    out.push_str(": ");
                    out.push_str(rbs_type_name(&ty()));
                    out.push('\n');
                }
            }
            out.push_str("end\n");
        }
        for name in CORE_TOP_CONSTS {
            out.push_str(name);
            out.push_str(": IO\n");
        }
        for name in CORE_TOP_STR_CONSTS {
            out.push_str(name);
            out.push_str(": String\n");
        }
        out.push_str("interface _RoundhouseEnvironment\nend\nENV: _RoundhouseEnvironment\n");
        out.push_str(STDLIB_NAMESPACES_RBS);
        out
    })
    .as_str()
}

/// Library namespaces an app `include`s to name their classes bare
/// (svg-graph's `include REXML`, then `Document.new`, `Comment.new`): Ruby
/// looks the name up in the included module before `Object`, so the app's
/// own top-level `Comment` model is not the one meant. Only the classes
/// spinel's package defines too (`packages/rexml`).
const STDLIB_NAMESPACES_RBS: &str = "\
module REXML\n\
  class Child\n  end\n\
  class Parent < Child\n  end\n\
  class Element < Parent\n  end\n\
  class Document < Element\n  end\n\
  class Text < Child\n  end\n\
  class CData < Text\n  end\n\
  class Comment < Child\n  end\n\
  class Instruction < Child\n  end\n\
  class XMLDecl < Child\n  end\n\
  class DocType < Parent\n  end\n\
  module Formatters\n\
    class Default\n    end\n\
    class Pretty < Default\n    end\n\
  end\n\
  class Attribute\n  end\n\
  class Attributes\n  end\n\
  class Elements\n  end\n\
  class ParseException < StandardError\n  end\n\
end\n";

/// The value contract contributed by a constant declared in `core_rbs()`.
fn core_value_type(name: &str) -> Option<Ty> {
    if name.trim_start_matches("Object::") == "ENV" {
        return Some(Ty::Class { id: ClassId(Symbol::from("_RoundhouseEnvironment")), args: vec![].into() });
    }
    if CORE_TOP_CONSTS.contains(&name.trim_start_matches("Object::")) {
        return Some(io_ty());
    }
    if CORE_TOP_STR_CONSTS.contains(&name.trim_start_matches("Object::")) {
        return Some(Ty::Str);
    }
    CORE_VALUE_CONSTS.iter().find_map(|(owner, field, ty)| {
        let need = owner.len() + 2 + field.len();
        (name.len() == need
            && name.as_bytes().get(..owner.len()) == Some(owner.as_bytes())
            && name.as_bytes().get(owner.len()..owner.len() + 2) == Some(b"::".as_slice())
            && name.as_bytes().get(owner.len() + 2..) == Some(field.as_bytes()))
        .then(|| ty())
    })
}

const CORE_URI: &str = "roundhouse-core:rbs";
const RUNTIME_URI_PREFIX: &str = "roundhouse-runtime:";

pub(super) enum ResolvedConstant {
    Namespace { class: Arc<ClassId>, runtime: bool },
    /// `runtime_decl`: declared only by the runtime's sources, so a
    /// registry class of the same exact name describes this constant.
    /// `singleton`: declared in a `class << self` (`X::<X>::NAME`), whose
    /// segment `name` leaves out.
    Value { declaration: DeclarationId, name: Arc<ClassId>, runtime: Option<Arc<Ty>>, runtime_decl: bool, singleton: bool },
}

/// Rubydex answers for one source file.
#[derive(Default)]
struct FileAnswers {
    /// Keyed by the byte offset where each written constant name starts.
    references: HashMap<u32, Option<ResolvedConstant>>,
    class_references: HashSet<u32>,
    /// Canonical class/module body owners, keyed by the end of their header name.
    namespace_bodies: HashMap<u32, ClassId>,
    /// Each constant definition as (offset where its name ends, name,
    /// declaration), sorted by offset.
    constants: Vec<(u32, Box<str>, DeclarationId)>,
    /// Exact source declaration names, including their lexical owners.
    constant_classes: IdentityHashMap<DeclarationId, ClassId>,
    namespace_definitions: HashSet<Box<str>>,
    class_definitions: HashSet<Box<str>>,
    /// Each `class`/`module` definition as (full name, start, end): the
    /// byte range of the whole definition, keyword through `end`.
    class_bodies: Vec<(Box<str>, u32, u32)>,
    alias_definitions: HashSet<Box<str>>,
    runtime_namespace_aliases: HashMap<Box<str>, Box<str>>,
    /// Source-only factory admission facts; they do not alter name resolution.
    sole_namespaces: HashSet<Box<str>>,
    singleton_methods: HashMap<Box<str>, HashSet<Box<str>>>,
    singleton_owners: HashSet<Box<str>>,
}

pub(crate) struct ConstResolver {
    /// Indexed by `FileId - 1`. `None` marks a source that Rubydex did
    /// not index, such as an ERB template.
    files: Vec<Option<FileAnswers>>,
    /// Each source class or module that others are nested in by name,
    /// with those nested ones (`A` to `A::B` and `A::B::C`).
    nested_namespaces: HashMap<Box<str>, Vec<Box<str>>>,
    /// The names `has_source_namespace` answers yes for, gathered once.
    source_namespaces: HashSet<Box<str>>,
    /// Includes paths, full text and order because answers use file IDs
    /// and byte offsets. Retain no second copy of the source snapshot.
    source_fingerprint: u64,
    /// Complete, canonical ancestry from the same graph, for source admission.
    class_ancestry: HashMap<ClassId, Vec<ClassId>>,
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
    let mut constant_document = RubydexDocument::new(document.uri().into(), text);
    for diagnostic in document.diagnostics().iter()
        .filter(|diagnostic| *diagnostic.rule() == Rule::DynamicSingletonDefinition)
    {
        constant_document.add_diagnostic(RubydexDiagnostic::new(
            *diagnostic.rule(), *diagnostic.uri_id(), diagnostic.offset().clone(), diagnostic.message().into(),
        ));
    }
    let mut graph = LocalGraph::from_parts(uri_id, constant_document, strings, names);
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
            graph
                .names()
                .get(reference.name_id())
                .map(|name| name.parent_scope()),
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
                        let Some(document) = documents.get(index) else {
                            break;
                        };
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

fn resolved_namespace(graph: &Graph, name: NameId) -> Option<&Declaration> {
    let id = graph.name_id_to_declaration_id(name)?;
    let id = graph.resolve_alias(id).unwrap_or(*id);
    graph.declarations().get(&id).filter(|declaration| declaration.as_namespace().is_some())
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

/// Where each class and module is defined, and what its ancestors are,
/// read from Rubydex alone. A scoped fractal build asks this before it
/// ingests anything, to decide which files to ingest.
#[derive(Debug, Default)]
pub struct DefinitionIndex {
    /// Fully qualified name -> the source paths that define it
    /// (`class`/`module` bodies, reopenings included), sorted.
    pub files: HashMap<Box<str>, Vec<String>>,
    /// Fully qualified name -> its resolved ancestors, nearest first,
    /// itself excluded. A gem's ancestor that left the chain incomplete
    /// ends it.
    pub ancestors: HashMap<Box<str>, Vec<Box<str>>>,
}

/// Index `sources` with Rubydex and keep only [`DefinitionIndex`]'s
/// answers. Cheaper than ingest: it parses each file once for constants
/// and resolves names, and keeps nothing else.
pub(crate) fn definition_index(sources: &[SourceFile]) -> DefinitionIndex {
    let mut graph = crate::timings::phase("rubydex: index", || index_sources(sources));
    crate::timings::phase("rubydex: resolve", || Resolver::new(&mut graph).resolve());
    let app_paths: HashSet<&str> = sources.iter().map(|source| source.path.as_str()).collect();
    let mut index = DefinitionIndex::default();
    for declaration in graph.declarations().values() {
        let Some(namespace) = declaration.as_namespace() else { continue };
        let name: Box<str> = hoisted_name(declaration.name()).as_ref().into();
        let mut paths: Vec<String> = declaration
            .definitions()
            .iter()
            .filter_map(|id| graph.definitions().get(id))
            .filter(|definition| matches!(definition, Definition::Class(_) | Definition::Module(_)))
            .filter_map(|definition| graph.documents().get(definition.uri_id()))
            .map(|document| document.uri())
            .filter(|uri| app_paths.contains(uri))
            .map(str::to_string)
            .collect();
        paths.sort();
        paths.dedup();
        let mut ancestors = Vec::new();
        for ancestor in namespace.ancestors() {
            let Ancestor::Complete(id) = ancestor else { break };
            let Some(ancestor) = graph.declarations().get(id) else { continue };
            let ancestor: Box<str> = hoisted_name(ancestor.name()).as_ref().into();
            if ancestor != name && !ancestors.contains(&ancestor) {
                ancestors.push(ancestor);
            }
        }
        if !paths.is_empty() {
            index.files.entry(name.clone()).or_default().extend(paths);
        }
        index.ancestors.entry(name).or_default().extend(ancestors);
    }
    for paths in index.files.values_mut() {
        paths.sort();
        paths.dedup();
    }
    index
}

/// Does any `class`/`module` definition in `sources` name its namespace
/// with a relative constant path (`class B::C` written inside `module A`)
/// whose namespace the indexed sources do not define? Ruby looks that
/// namespace up lexically first (`A::B`), so the fully qualified name
/// depends on files the graph has not seen. A scoped fractal build asks
/// this of the files it compiles, to know when naming their bodies needs
/// the rest of the app.
pub(crate) fn has_ambiguous_namespace(sources: &[SourceFile]) -> bool {
    if sources.is_empty() {
        return false;
    }
    let mut graph = index_sources(sources);
    Resolver::new(&mut graph).resolve();
    sources.iter().filter(|source| is_indexed(source)).any(|source| {
        let Some(document) = graph.documents().get(&UriId::from(source.path.as_str())) else { return false };
        document.definitions().iter().any(|definition_id| {
            let Some(definition @ (Definition::Class(_) | Definition::Module(_))) = graph.definitions().get(definition_id) else {
                return false;
            };
            definition.name_id().is_some_and(|name_id| namespace_is_ambiguous(&graph, *name_id))
        })
    })
}

/// Is `name_id` a relative path (`B::C`) written inside a body, whose
/// namespace (`B`) no indexed `class`, `module` or constant defines?
/// Rubydex still creates a declaration for such a namespace; it just has
/// no definitions.
fn namespace_is_ambiguous(graph: &Graph, name_id: NameId) -> bool {
    let Some(name) = graph.names().get(&name_id) else { return false };
    let ParentScope::Some(namespace_id) = name.parent_scope() else { return false };
    let mut leftmost_id = *namespace_id;
    loop {
        let Some(segment) = graph.names().get(&leftmost_id) else { return false };
        match segment.parent_scope() {
            ParentScope::Some(next_id) => leftmost_id = *next_id,
            // `::B::C`, or a path on a singleton, resolves without lexical scope.
            ParentScope::TopLevel | ParentScope::Attached(_) => return false,
            ParentScope::None => {
                if segment.nesting().is_none() {
                    // Written at the top level: `B` can only be `::B`.
                    return false;
                }
                break;
            }
        }
    }
    graph
        .name_id_to_declaration_id(*namespace_id)
        .and_then(|declaration_id| graph.declarations().get(declaration_id))
        .is_none_or(|namespace| namespace.definitions().is_empty())
}

impl ConstResolver {
    /// Index real Ruby app sources with their original 1-based file IDs.
    pub(crate) fn from_app_sources(sources: &[SourceFile]) -> Self {
        let mut graph = crate::timings::phase("rubydex: index", || index_sources(sources));
        // Rubydex resolves all source references after every file enters
        // its graph, including declarations in later files.
        crate::timings::phase("rubydex: resolve", || Resolver::new(&mut graph).resolve());
        // Match ingest and reference answers even for singleton-class nesting.
        let class_ancestry = graph.declarations().values().filter_map(|declaration| {
            let namespace = declaration.as_namespace()?;
            if namespace.kind() != "Class" { return None; }
            let mut ancestors = vec![ClassId(Symbol::from(hoisted_name(declaration.name()).as_ref()))];
            for ancestor in namespace.ancestors() {
                // Mutation reachability needs the known edges even when a
                // gem superclass leaves the rest of the ancestry incomplete.
                let Ancestor::Complete(id) = ancestor else { continue };
                let Some(ancestor) = graph.declarations().get(id) else { continue };
                if matches!(ancestor.kind(), "Class" | "Module") {
                    let id = ClassId(Symbol::from(hoisted_name(ancestor.name()).as_ref()));
                    if !ancestors.contains(&id) { ancestors.push(id); }
                }
            }
            Some((ClassId(Symbol::from(hoisted_name(declaration.name()).as_ref())), ancestors))
        }).collect();
        let files = crate::timings::phase("rubydex: answers", || collect_answers(&graph, sources));
        drop(graph);
        let mut declared: Vec<&str> = files.iter().flatten().flat_map(|file| file.class_definitions.iter().map(|name| name.as_ref())).collect();
        declared.sort_unstable();
        declared.dedup();
        let nested_namespaces = nested_namespaces_index(&declared);
        let class_definitions: HashSet<&str> = declared.into_iter().collect();
        let mut source_namespaces: HashSet<Box<str>> = files.iter().flatten()
            .flat_map(|file| file.namespace_definitions.iter().cloned()).collect();
        for (alias, target) in files.iter().flatten().flat_map(|file| file.runtime_namespace_aliases.iter()) {
            if class_definitions.contains(alias.as_ref()) {
                source_namespaces.insert(target.clone());
            }
        }
        Self { files, nested_namespaces, source_namespaces, source_fingerprint: source_fingerprint(sources), class_ancestry }
    }

    fn file(&self, file: FileId) -> Option<&FileAnswers> {
        let index = file.0.checked_sub(1)?;
        self.files.get(index as usize)?.as_ref()
    }

    /// Every `class`/`module` definition in the app, as (fully qualified
    /// name, span of the whole definition), reopenings included, in file
    /// order. A definition's span contains the definitions nested in it.
    pub(crate) fn class_bodies(&self) -> Vec<(Box<str>, Span)> {
        let mut out = Vec::new();
        for (index, file) in self.files.iter().enumerate() {
            let Some(file) = file else { continue };
            for (name, start, end) in &file.class_bodies {
                out.push((name.clone(), Span { file: FileId(index as u32 + 1), start: *start, end: *end }));
            }
        }
        out.sort_by_key(|(_, span)| (span.file.0, span.start));
        out
    }

    /// Every constant (fully qualified) `file` declares: its classes and
    /// modules, and its assigned constants and aliases.
    pub(crate) fn namespaces_declared_in(&self, file: FileId) -> impl Iterator<Item = &str> {
        self.file(file).into_iter().flat_map(|file| file.namespace_definitions.iter().map(|name| name.as_ref()))
    }

    /// `namespaces_declared_in` less the aliases: `Alias = Bar` names
    /// `Bar`'s class, not one `file` makes.
    pub(crate) fn receivers_declared_in(&self, file: FileId) -> impl Iterator<Item = &str> {
        self.file(file).into_iter().flat_map(|file| {
            file.namespace_definitions
                .iter()
                .filter(|name| !file.alias_definitions.contains(*name))
                .map(|name| name.as_ref())
        })
    }

    pub(super) fn has_source_file(&self, file: FileId) -> bool {
        self.file(file).is_some()
    }

    pub(super) fn has_registered_source(&self, file: FileId) -> bool {
        file.0
            .checked_sub(1)
            .is_some_and(|index| (index as usize) < self.files.len())
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
        self.file(span.file)?
            .references
            .get(&start)
            .map(Option::as_ref)
    }

    /// App and runtime sources define this reference; RBI declarations do
    /// not enter this graph and cannot establish source ownership.
    pub(crate) fn has_definition(&self, span: Span, path: &[Symbol]) -> bool {
        self.reference(span, path).is_some_and(|resolved| resolved.is_some())
    }

    /// Exact resolved source names for declaration edges without expression
    /// spans. The borrowed identifiers remain owned by this snapshot.
    pub(crate) fn defined_names(&self) -> HashSet<&str> {
        self.files.iter().flatten().flat_map(|file| file.references.values()).filter_map(|resolved| {
            match resolved.as_ref()? {
                ResolvedConstant::Namespace { class, .. } => Some(class.0.as_str()),
                ResolvedConstant::Value { name, .. } => Some(name.0.as_str()),
            }
        }).collect()
    }

    /// Exact source-resolved namespace, also used by ingest admission gates
    /// that must distinguish lexical namesakes from the activated concern.
    pub(crate) fn namespace(&self, span: Span, path: &[Symbol]) -> Option<&ClassId> {
        match self.reference(span, path)?? {
            ResolvedConstant::Namespace { class, .. } => Some(class),
            ResolvedConstant::Value { .. } => None,
        }
    }

    /// A declaration header is not a constant reference. Its body still runs
    /// on the resolved namespace, including qualified reopenings and aliases.
    pub(in crate::analyze) fn namespace_body(&self, span: Span) -> Option<&ClassId> {
        self.file(span.file)?.namespace_bodies.get(&span.end)
    }

    /// The fully qualified declaration a reference resolves to, whether a
    /// namespace or a value.
    pub(crate) fn declaration_name(&self, span: Span, path: &[Symbol]) -> Option<&str> {
        match self.reference(span, path)?? {
            ResolvedConstant::Namespace { class, .. } => Some(class.0.as_str()),
            ResolvedConstant::Value { declaration, .. } => self
                .files
                .iter()
                .flatten()
                .find_map(|file| file.constant_classes.get(declaration))
                .map(|name| name.0.as_str()),
        }
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
        let index = constants
            .partition_point(|(end, ..)| *end <= value.start)
            .checked_sub(1)?;
        let (_, defined, id) = &constants[index];
        (defined.as_ref() == name).then_some(*id)
    }

    pub(crate) fn constant_class(&self, value: Span, name: &str) -> Option<ClassId> {
        let declaration = self.constant_declaration(value, name)?;
        self.file(value.file)?.constant_classes.get(&declaration).cloned()
    }

    /// The source classes and modules declared inside the source class
    /// or module `name` (`A::B` and `A::B::C` for `A`).
    pub(crate) fn nested_source_namespaces(&self, name: &str) -> &[Box<str>] {
        self.nested_namespaces.get(name).map_or(&[], Vec::as_slice)
    }

    pub(crate) fn has_source_namespace(&self, name: &str) -> bool {
        self.source_namespaces.contains(name)
    }

    pub(crate) fn is_runtime_class(&self, span: Span, path: &[Symbol], name: &str) -> bool {
        matches!(self.reference(span, path),
            Some(Some(ResolvedConstant::Namespace { class, runtime: true }))
                if class.0.as_str() == name)
    }

    pub(in crate::analyze) fn sole_namespace(&self, file: FileId, name: &str) -> bool {
        self.file(file).is_some_and(|file| file.sole_namespaces.contains(name))
    }

    pub(in crate::analyze) fn is_class_namespace(&self, span: Span, path: &[Symbol]) -> bool {
        path.last().and_then(|name| span.end.checked_sub(name.as_str().len() as u32))
            .is_some_and(|start| self.file(span.file).is_some_and(|file| file.class_references.contains(&start)))
    }

    pub(in crate::analyze) fn has_source_singleton_body(&self, owner: &str) -> bool {
        self.files.iter().flatten().any(|file| {
            file.singleton_owners.contains(owner) || file.singleton_methods.contains_key(owner)
        })
    }

    pub(in crate::analyze) fn source_singleton_method(&self, owner: &str, method: &str) -> bool {
        self.files.iter().flatten().any(|file| {
            file.singleton_methods.get(owner).is_some_and(|methods| methods.contains(method))
        })
    }

    pub(in crate::analyze) fn class_ancestry(&self, name: &ClassId) -> Option<&[ClassId]> {
        self.class_ancestry.get(name).map(Vec::as_slice)
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
        text: core_rbs(),
        language: LanguageId::Rbs,
    }];
    documents.extend(
        crate::runtime_files::ruby_sources().map(|(path, text)| Document {
            uri_prefix: RUNTIME_URI_PREFIX,
            path,
            text,
            language: LanguageId::Ruby,
        }),
    );
    documents.extend(
        sources
            .iter()
            .filter(|source| is_indexed(source))
            .map(|source| Document {
                uri_prefix: "",
                path: &source.path,
                text: &source.text,
                language: LanguageId::Ruby,
            }),
    );
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
                chunk
                    .join()
                    .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
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

// Rubydex skips parenthesized receivers; retain only literal constant `.define` mutations.
fn parenthesized_factory_receivers(text: &str) -> HashSet<u32> {
    struct Receivers(HashSet<u32>);
    impl<'pr> ruby_prism::Visit<'pr> for Receivers {
        fn visit_def_node(&mut self, node: &ruby_prism::DefNode<'pr>) {
            if node.name_loc().as_slice() == b"define"
                && let Some(receiver) = node.receiver()
                && let Some(parentheses) = receiver.as_parentheses_node()
                && let Some(receiver) = parentheses.body()
            {
                let location = if let Some(path) = receiver.as_constant_path_node() {
                    Some(path.name_loc())
                } else {
                    receiver.as_constant_read_node().map(|_| receiver.location())
                };
                if let Some(location) = location {
                    self.0.insert(location.start_offset() as u32);
                }
            }
            if let Some(body) = node.body() {
                ruby_prism::Visit::visit(self, &body);
            }
        }
    }
    let parsed = ruby_prism::parse(text.as_bytes());
    let mut receivers = Receivers(HashSet::default());
    if parsed.errors().next().is_none() {
        ruby_prism::Visit::visit(&mut receivers, &parsed.node());
    }
    receivers.0
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
    let parenthesized_receivers = if document.diagnostics().iter()
        .any(|diagnostic| *diagnostic.rule() == Rule::DynamicSingletonDefinition)
    {
        parenthesized_factory_receivers(&source.text)
    } else {
        HashSet::default()
    };
    // Positions a whole-path reference filled, which an exact one replaces.
    let mut path_keys = HashSet::default();
    for reference_id in document.constant_references() {
        let Some(reference) = graph.constant_references().get(reference_id) else {
            continue;
        };
        let offset = reference.offset();
        // A superclass or mixin reference spans its whole `A::B` path,
        // so it starts at the same offset as the `A` reference. Its
        // answer belongs to where its final name starts, which is where
        // `reference` looks for an IR span covering the whole path; a
        // reference whose span is exactly its written name keeps its
        // own start.
        let written = source
            .text
            .get(offset.start() as usize..offset.end() as usize);
        let name = graph
            .names()
            .get(reference.name_id())
            .and_then(|name| graph.strings().get(name.str()));
        let Some((key, exact)) = name.and_then(|name| {
            let (name, written) = (name.as_str(), written?);
            if written == name {
                Some((offset.start(), true))
            } else {
                let prefix = written.strip_suffix(name)?;
                prefix.ends_with("::").then(|| (offset.end() - name.len() as u32, false))
            }
        }) else {
            continue;
        };
        // A path reference only fills its final name's position; the
        // reference that Rubydex recorded there itself, if any, wins.
        if !exact && answers.references.contains_key(&key) {
            continue;
        }
        if exact && parenthesized_receivers.contains(&offset.start())
            && let Some(target) = resolved_namespace(graph, *reference.name_id())
                .filter(|target| is_runtime_declaration(graph, target))
        {
            answers.namespace_definitions.insert(hoisted_name(target.name()).into());
        }
        let target = graph
            .name_id_to_declaration_id(*reference.name_id())
            .and_then(|id| graph.declarations().get(id).map(|decl| (*id, decl)))
            .map(|(id, declaration)| {
                if declaration.kind() == "Class" {
                    answers.class_references.insert(key);
                }
                if declaration.as_namespace().is_some() && !is_assigned_value(graph, declaration) {
                    let (class, runtime) = class_cache.entry(id).or_insert_with(|| {
                        (
                            Arc::new(ClassId(Symbol::from(hoisted_name(declaration.name()).as_ref()))),
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
                    let name = declaration.name();
                    let builtin = core_value_type(name).filter(|_| {
                        declaration.definitions().iter().all(|id| {
                            graph
                                .definitions()
                                .get(id)
                                .and_then(|definition| graph.documents().get(definition.uri_id()))
                                .is_some_and(|document| document.uri() == CORE_URI)
                        })
                    });
                    let runtime_decl = !app_write && is_runtime_declaration(graph, declaration);
                    let runtime = if let Some(ty) = builtin {
                        Some(Arc::new(ty))
                    } else if runtime_decl {
                        runtime_value_types().get(declaration.name()).cloned()
                    } else {
                        None
                    };
                    ResolvedConstant::Value { declaration: id, name: Arc::new(ClassId(Symbol::from(hoisted_name(name).as_ref()))), runtime, runtime_decl, singleton: name.contains("::<") }
                }
            });
        match answers.references.entry(key) {
            Entry::Vacant(slot) => {
                if !exact {
                    path_keys.insert(key);
                }
                slot.insert(target);
            }
            Entry::Occupied(mut slot) => {
                if (exact && path_keys.remove(&key)) || (slot.get().is_none() && target.is_some()) {
                    slot.insert(target);
                }
            }
        }
    }
    for definition_id in document.definitions() {
        let definition = graph.definitions().get(definition_id);
        if let Some(definition @ (Definition::Class(_) | Definition::Module(_))) = definition
            && let Some(offset) = definition.name_offset()
            && let Some(id) = graph.definition_to_declaration_id(definition)
            && let Some(declaration) = graph.declarations().get(&graph.resolve_alias(id).unwrap_or(*id))
        {
            answers.namespace_bodies.insert(
                offset.end(),
                ClassId(Symbol::from(hoisted_name(declaration.name()).as_ref())),
            );
        }
        if let Some(name) = definition
            .filter(|definition| matches!(definition,
                Definition::Class(_) | Definition::Module(_) | Definition::Constant(_) | Definition::ConstantAlias(_)))
            .and_then(Definition::name_id)
            .and_then(|id| graph.names().get(id))
        {
            let parent = match name.parent_scope() {
                ParentScope::TopLevel => None,
                ParentScope::None => name.nesting().as_ref(),
                ParentScope::Some(id) | ParentScope::Attached(id) => Some(id),
            };
            if let Some(written) = graph.strings().get(name.str()) {
                let owner = parent.and_then(|id| graph.name_id_to_declaration_id(*id))
                    .and_then(|id| graph.declarations().get(id));
                let full = owner.map_or_else(|| written.to_string(), |owner| format!("{}::{}", hoisted_name(owner.name()), written.as_str()));
                if let Some(class @ (Definition::Class(_) | Definition::Module(_))) = definition {
                    answers.class_definitions.insert(full.clone().into());
                    let offset = class.offset();
                    answers.class_bodies.push((full.clone().into(), offset.start(), offset.end()));
                }
                if let Some(Definition::ConstantAlias(alias)) = definition {
                    answers.alias_definitions.insert(full.clone().into());
                    if let Some(target) = resolved_namespace(graph, *alias.target_name_id())
                        .filter(|target| is_runtime_declaration(graph, target))
                    {
                        answers.runtime_namespace_aliases.insert(full.clone().into(), hoisted_name(target.name()).into());
                    }
                }
                answers.namespace_definitions.insert(full.into());
            }
            if let Some(declaration) = definition
                .and_then(|definition| graph.definition_to_declaration_id(definition))
                .and_then(|id| graph.declarations().get(id))
                && declaration.definitions().len() == 1
                && declaration.kind() == "Class"
                && !is_runtime_declaration(graph, declaration)
            {
                answers.sole_namespaces.insert(hoisted_name(declaration.name()).into());
            }
        }
        if let Some(Definition::SingletonClass(singleton)) = definition
            && let Some(ParentScope::Attached(receiver)) = graph.names().get(singleton.name_id())
                .map(|name| name.parent_scope())
            && let Some(target) = resolved_namespace(graph, *receiver)
        {
            answers.singleton_owners.insert(hoisted_name(target.name()).into());
            if is_runtime_declaration(graph, target) {
                answers.namespace_definitions.insert(hoisted_name(target.name()).into());
            }
        }
        if let Some(Definition::Method(method)) = definition {
            let owner = match method.receiver() {
                Some(Receiver::ConstantReceiver(receiver)) => resolved_namespace(graph, *receiver),
                Some(Receiver::SelfReceiver(receiver)) => graph.definitions().get(receiver)
                    .and_then(|definition| graph.definition_to_declaration_id(definition))
                    .and_then(|id| graph.declarations().get(id)),
                None => None,
            };
            if let Some(owner) = owner
                && let Some(name) = graph.strings().get(method.str_id())
            {
                answers.singleton_methods.entry(hoisted_name(owner.name()).into()).or_default()
                    .insert(name.as_str().trim_end_matches("()").into());
            }
            if let Some(Receiver::ConstantReceiver(receiver)) = method.receiver()
                && graph.strings().get(method.str_id()).is_some_and(|name| name.as_str() == "define()")
                && let Some(target) = resolved_namespace(graph, *receiver)
                    .filter(|target| is_runtime_declaration(graph, target))
            {
                // A named singleton definition can override a core factory without a class reopen.
                answers.namespace_definitions.insert(hoisted_name(target.name()).into());
            }
        }
        let (name_id, offset) = match definition {
            Some(Definition::Constant(it)) => (it.name_id(), it.offset()),
            Some(Definition::ConstantAlias(it)) => (it.name_id(), it.offset()),
            _ => continue,
        };
        let name = graph
            .names()
            .get(name_id)
            .and_then(|name| graph.strings().get(name.str()));
        let id = definition.and_then(|definition| graph.definition_to_declaration_id(definition));
        if let (Some(name), Some(id)) = (name, id) {
            answers.constants.push((offset.end(), name.as_str().into(), *id));
            if let Some(declaration) = graph.declarations().get(id) {
                answers.constant_classes.insert(*id, ClassId(Symbol::from(hoisted_name(declaration.name()).as_ref())));
            }
        }
    }
    answers.constants.sort_unstable_by_key(|(end, ..)| *end);
    answers
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn singleton_namespace_answers_use_hoisted_names() {
        let sources = vec![SourceFile {
            path: "app/services/outer.rb".into(),
            text: "\
class Outer
  class << self
    class << self
      module M; end
      class Base; end
      class Parent < Base
        include M
        def self.inherited(child); end
      end
      class BodyOwner; end
      class << BodyOwner; end
      Factory = Class.new
      Parent
    end
  end
end
".into(),
        }];
        let resolver = ConstResolver::from_app_sources(&sources);
        assert_eq!(resolved_name(&resolver, &sources, 0, "Parent"), "Outer::Parent");
        let ancestry = resolver.class_ancestry(&ClassId(Symbol::from("Outer::Parent"))).unwrap();
        for name in ["Outer::Parent", "Outer::Base", "Outer::M"] {
            assert!(ancestry.contains(&ClassId(Symbol::from(name))), "{name}: {ancestry:?}");
        }
        for (header, owner) in [("class Parent", "Outer::Parent"), ("module M", "Outer::M")] {
            let end = (sources[0].text.find(header).unwrap() + header.len()) as u32;
            assert_eq!(resolver.namespace_body(Span { file: FileId(1), start: 0, end }),
                Some(&ClassId(Symbol::from(owner))));
        }
        assert!(resolver.sole_namespace(FileId(1), "Outer::Factory"));
        assert!(resolver.has_source_singleton_body("Outer::BodyOwner"));
        assert!(resolver.source_singleton_method("Outer::Parent", "inherited"));
    }

    #[test]
    fn source_namespace_index_matches_original_file_and_runtime_alias_scan() {
        for reopen in [false, true] {
            let mut sources = vec![
                SourceFile { path: "nested.rb".into(), text: "module Catalog\n class Reader; end\nend\n".into() },
                SourceFile { path: "alias.rb".into(), text: "DataAlias = Data\nValue = 3\n".into() },
            ];
            if reopen {
                sources.push(SourceFile { path: "reopen.rb".into(), text: "class DataAlias\n def marker; 1; end\nend\n".into() });
            }
            let resolver = ConstResolver::from_app_sources(&sources);
            let mut queries = vec!["Catalog", "Catalog::Reader", "DataAlias", "Data", "Value", "Missing"];
            queries.extend(resolver.files.iter().flatten().flat_map(|file|
                file.namespace_definitions.iter().map(|name| name.as_ref())));
            queries.extend(resolver.files.iter().flatten().flat_map(|file|
                file.runtime_namespace_aliases.values().map(|name| name.as_ref())));
            for name in queries {
                let scanned = resolver.files.iter().flatten().any(|file| file.namespace_definitions.contains(name))
                    || resolver.files.iter().flatten().any(|file| {
                        file.runtime_namespace_aliases.iter().any(|(alias, target)| {
                            target.as_ref() == name && resolver.files.iter().flatten()
                                .any(|source| source.class_definitions.contains(alias.as_ref()))
                        })
                    });
                assert_eq!(resolver.has_source_namespace(name), scanned, "{name}, reopen={reopen}");
            }
        }
    }

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

    fn source(path: &str, name: &str) -> SourceFile {
        SourceFile {
            path: path.into(),
            text: format!("class {name}; end; {name}"),
        }
    }
    fn resolved_name(
        resolver: &ConstResolver,
        sources: &[SourceFile],
        index: usize,
        name: &str,
    ) -> String {
        let text = &sources[index].text;
        let start = text.rfind(name).unwrap() as u32;
        let span = Span {
            file: FileId(index as u32 + 1),
            start,
            end: start + name.len() as u32,
        };
        match resolver.reference(span, &[Symbol::from(name)]) {
            Some(Some(ResolvedConstant::Namespace { class, .. })) => class.0.to_string(),
            _ => panic!("source reference must be indexed at its original FileId"),
        }
    }

    #[test]
    fn missing_documents_and_missing_source_answers_are_not_generated_ir() {
        use super::super::{BodyTyper, ClassInfo, Ctx};
        use crate::{Expr, ExprNode};
        let sources = vec![source("app/services/first.rb", "First")];
        let resolver = Arc::new(ConstResolver::from_app_sources(&sources));
        let classes = HashMap::from_iter([(ClassId(Symbol::from("Object")), ClassInfo::default())]);
        let typer = BodyTyper::new(&classes).with_const_resolver(resolver);
        let make = |file| {
            Expr::new(
                Span {
                    file: FileId(file),
                    start: 0,
                    end: 6,
                },
                ExprNode::Const {
                    path: vec![Symbol::from("Object")],
                },
            )
        };
        for file in [1, 99] {
            let mut expr = make(file);
            typer.analyze_expr(&mut expr, &Ctx::default());
            assert!(
                expr.diagnostic().is_some(),
                "source FileId({file}) cannot use a generated fallback"
            );
        }
        let mut generated = make(99);
        generated.decisions |= crate::expr::GENERATED_CONST_REF;
        typer.analyze_expr(&mut generated, &Ctx::default());
        assert!(generated.diagnostic().is_none());
        assert!(matches!(generated.ty, Some(crate::ty::Ty::Class { .. })));
    }

    #[test]
    fn prepared_answers_reject_same_count_source_changes_and_reordering() {
        let mut sources = vec![
            source("app/services/first.rb", "First"),
            source("components/payments/lib/second.rb", "Second"),
        ];
        let prepared = ConstResolverTask::start(Arc::new(sources.clone())).finish();
        sources[0] = source("app/services/first.rb", "Third");
        let changed = prepared.for_sources(&sources);
        assert_eq!(resolved_name(&changed, &sources, 0, "Third"), "Third");
        sources.swap(0, 1);
        let reordered = prepared.for_sources(&sources);
        assert_eq!(resolved_name(&reordered, &sources, 0, "Second"), "Second");
        assert_eq!(resolved_name(&reordered, &sources, 1, "Third"), "Third");
    }

    /// An app that reads a scalar constant of a runtime module
    /// (`ActiveRecord::SignedId::SALT = "active_record/signed_id"`, nested
    /// two modules deep) must get the constant's literal type, or the
    /// read is refused as an unsupported constant.
    #[test]
    fn a_runtime_module_scalar_constant_resolves_with_its_literal_type() {
        let text = "module SaltProbe\n  R = \"#{ActiveRecord::SignedId::SALT}\".freeze\nend\n";
        let sources = vec![SourceFile { path: "lib/salt_probe.rb".into(), text: text.into() }];
        let resolver = ConstResolver::from_app_sources(&sources);
        let end = (text.find("SALT}").unwrap() + "SALT".len()) as u32;
        let span = Span { file: FileId(1), start: end - 40, end };
        let path: Vec<Symbol> = ["ActiveRecord", "SignedId", "SALT"].into_iter().map(Symbol::from).collect();
        match resolver.reference(span, &path) {
            Some(Some(ResolvedConstant::Value { name, runtime, .. })) => {
                assert_eq!(name.0.as_str(), "ActiveRecord::SignedId::SALT");
                assert_eq!(runtime.as_deref(), Some(&crate::ty::Ty::Str));
            }
            _ => panic!("ActiveRecord::SignedId::SALT must resolve to the runtime value"),
        }
    }

    /// A superclass or mixin path is one Rubydex reference spanning the
    /// whole `A::B`. Its answer is where an IR span covering that path
    /// looks, the final name, while the `A` reference keeps its own.
    #[test]
    fn superclass_and_mixin_paths_answer_at_their_final_name() {
        let text = "module A\n  class B; end\n  module H; end\nend\nclass X < A::B\n  include A::H\nend\nclass Y < ::A::B; end\n";
        let sources = vec![SourceFile { path: "x.rb".into(), text: text.into() }];
        let resolver = ConstResolver::from_app_sources(&sources);
        let path = |written: &str| -> Vec<Symbol> { written.split("::").filter(|s| !s.is_empty()).map(Symbol::from).collect() };
        let at = |needle: &str, written: &str| {
            let start = text.find(needle).unwrap() + needle.len() - written.len();
            let span = Span { file: FileId(1), start: start as u32, end: (start + written.len()) as u32 };
            resolver.namespace(span, &path(written)).map(|class| class.0.to_string())
        };
        assert_eq!(at("X < A::B", "A::B").as_deref(), Some("A::B"));
        assert_eq!(at("include A::H", "A::H").as_deref(), Some("A::H"));
        assert_eq!(at("Y < ::A::B", "::A::B").as_deref(), Some("A::B"));
        assert_eq!(at("X < A", "A").as_deref(), Some("A"));
    }
}
