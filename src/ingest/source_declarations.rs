//! A syntactic declaration inventory for explicitly supplied Ruby or RBS.
//!
//! The projection uses the RBI reader's grammar: named namespaces, literal
//! ancestry, constant names and aliases. It reads no files and executes no code.
//! The check-only input uses a boundary-aware Ruby projection that retains
//! uncertainty about omitted effects. Source selection and loading policy
//! belong to the caller. An inventory is not proof a constant exists.

use ruby_prism::{Node, Visit};
use std::collections::BTreeSet;

/// Project direct Ruby declarations from UTF-8 or US-ASCII source.
///
/// Reject the whole input with a reason on syntax errors, non-UTF-8 bytes or
/// unsupported encoding declarations. No partial inventory or replacement
/// characters are returned. Callers must retain the rejection reason.
///
/// Method bodies, blocks, conditionals, singleton scopes, computed parents,
/// multiple assignments, parenthesized assignments, plain `begin` wrappers,
/// `self.include` and `const_set` are omitted. Factory assignments keep only
/// an opaque constant name. Value expressions become `nil` placeholders,
/// not claims about their type; consumers must treat those values as opaque.
pub fn read_ruby(source: &[u8]) -> Result<String, String> {
    ruby_encoding(source)?;
    let parsed = super::prism::parse_silent(source);
    if let Some(error) = parsed.errors().next() {
        return Err(format!(
            "Ruby syntax error: {}",
            String::from_utf8_lossy(error.message())
        ));
    }
    let program = parsed.node();
    let program = program.as_program_node().ok_or("expected a Ruby program")?;
    let mut out = String::new();
    ruby_statements(&program.statements(), &mut out);
    Ok(out)
}

fn ruby_encoding(source: &[u8]) -> Result<(), String> {
    std::str::from_utf8(source).map_err(|error| {
        format!(
            "non-UTF-8 Ruby source at byte {} is not supported",
            error.valid_up_to()
        )
    })?;
    // Only the first line, or the line after a shebang, can set Ruby's encoding.
    let mut lines = source
        .strip_prefix(b"\xef\xbb\xbf")
        .unwrap_or(source)
        .split(|byte| *byte == b'\n');
    let first = lines.next().unwrap_or_default();
    let header = if first.starts_with(b"#!") {
        // Prism can still read a non-Ruby shebang as an encoding comment.
        // Reject its encoding marker before a second line can mask it.
        if encoding_marker(first) {
            return Err("unsupported Ruby source encoding declaration in shebang".into());
        }
        lines.next().unwrap_or_default()
    } else {
        first
    };
    let header = header.trim_ascii_start();
    if !header.starts_with(b"#") {
        return Ok(());
    }
    // Let Prism interpret the supported spellings rather than decoding names
    // using a different encoding from the source. Parse only this header so a
    // later, ineffective encoding comment cannot mask the active declaration.
    let parsed = ruby_prism::parse(header);
    let mut declared = false;
    for comment in parsed.magic_comments() {
        if comment.key().eq_ignore_ascii_case(b"encoding")
            || comment.key().eq_ignore_ascii_case(b"coding")
        {
            declared = true;
            let encoding = comment.value();
            if ![b"UTF-8".as_slice(), b"UTF8", b"US-ASCII", b"ASCII"]
                .iter()
                .any(|supported| encoding.eq_ignore_ascii_case(supported))
            {
                return Err(format!(
                    "unsupported Ruby source encoding {}; only UTF-8 and US-ASCII are supported",
                    String::from_utf8_lossy(encoding)
                ));
            }
        }
    }
    // Prism also accepts fallback forms (for example Vim's fileencoding=)
    // which it does not expose as magic comments. Reject such an unclassified
    // header instead of silently projecting legacy bytes as UTF-8 names.
    if !declared && encoding_marker(header) {
        return Err("unsupported Ruby source encoding comment; use an explicit UTF-8 or US-ASCII declaration".into());
    }
    Ok(())
}

fn encoding_marker(header: &[u8]) -> bool {
    header.windows(6).enumerate().any(|(offset, word)| {
        word.eq_ignore_ascii_case(b"coding")
            && matches!(
                header[offset + 6..].trim_ascii_start().first(),
                Some(b':' | b'=')
            )
    })
}

fn constant(node: &Node<'_>) -> Option<String> {
    if let Some(read) = node.as_constant_read_node() {
        return Some(super::util::constant_id_str(&read.name()).to_string());
    }
    let path = node.as_constant_path_node()?;
    let leaf = path.name()?;
    let leaf = super::util::constant_id_str(&leaf);
    Some(match path.parent() {
        Some(parent) => format!("{}::{leaf}", constant(&parent)?),
        None => format!("::{leaf}"),
    })
}

fn ruby_body(body: Option<Node<'_>>, out: &mut String) {
    if let Some(body) = body.and_then(|node| node.as_statements_node()) {
        ruby_statements(&body, out);
    }
}

fn ruby_statements(body: &ruby_prism::StatementsNode<'_>, out: &mut String) {
    for node in body.body().iter() {
        if let Some(class) = node.as_class_node() {
            let Some(name) = constant(&class.constant_path()) else {
                continue;
            };
            // A computed superclass cannot be replaced with Object.
            let parent = match class.superclass() {
                Some(parent) => match constant(&parent) {
                    Some(parent) => format!(" < {parent}"),
                    None => continue,
                },
                None => String::new(),
            };
            out.push_str(&format!("class {name}{parent}\n"));
            ruby_body(class.body(), out);
            out.push_str("end\n");
        } else if let Some(module) = node.as_module_node() {
            let Some(name) = constant(&module.constant_path()) else {
                continue;
            };
            out.push_str(&format!("module {name}\n"));
            ruby_body(module.body(), out);
            out.push_str("end\n");
        } else if let Some(write) = node.as_constant_write_node() {
            let name = write.name();
            let name = super::util::constant_id_str(&name);
            let value = constant(&write.value()).unwrap_or_else(|| "nil".into());
            out.push_str(&format!("{name} = {value}\n"));
        } else if let Some(write) = node.as_constant_path_write_node() {
            let Some(name) = constant(&write.target().as_node()) else {
                continue;
            };
            let value = constant(&write.value()).unwrap_or_else(|| "nil".into());
            out.push_str(&format!("{name} = {value}\n"));
        } else if let Some(call) = node.as_call_node() {
            if call.receiver().is_some()
                || call.block().is_some()
                || !matches!(call.name().as_slice(), b"include" | b"prepend" | b"extend")
            {
                continue;
            }
            let Some(args) = call.arguments() else {
                continue;
            };
            let targets: Option<Vec<_>> =
                args.arguments().iter().map(|arg| constant(&arg)).collect();
            if let Some(targets) = targets.filter(|targets| !targets.is_empty()) {
                // Keep grouped mixins in Ruby's written order; splitting
                // include A, B into two calls reverses lookup precedence.
                let name = call.name();
                out.push_str(&format!(
                    "{} {}\n",
                    super::util::constant_id_str(&name),
                    targets.join(", ")
                ));
            }
        }
    }
}

/// Project namespace declarations from an explicitly supplied RBS contract.
///
/// Method types remain the existing signature reader's responsibility.
/// Interfaces and their mixins, and type aliases, do not declare Ruby constants.
/// Reject any `use` directive: erasing an import could change the binding of a
/// superclass, mixin or alias. Import resolution belongs to a future policy.
pub fn read_rbs(source: &str) -> Result<String, String> {
    let parsed = ruby_rbs::node::parse(source)?;
    if parsed
        .directives()
        .iter()
        .any(|node| matches!(node, ruby_rbs::node::Node::Use(_)))
    {
        return Err("RBS `use` imports are not supported by the declaration reader".into());
    }
    let mut out = String::new();
    rbs_members(parsed.declarations().iter(), &mut out);
    Ok(out)
}

fn rbs_name(name: &ruby_rbs::node::TypeNameNode<'_>) -> String {
    let namespace = name.namespace();
    let mut parts: Vec<_> = namespace
        .path()
        .iter()
        .filter_map(|node| match node {
            ruby_rbs::node::Node::Symbol(symbol) => Some(symbol.as_str().to_string()),
            _ => None,
        })
        .collect();
    parts.push(name.name().as_str().to_string());
    format!(
        "{}{}",
        if namespace.absolute() { "::" } else { "" },
        parts.join("::")
    )
}

fn rbs_mixin(keyword: &str, name: &ruby_rbs::node::TypeNameNode<'_>, out: &mut String) {
    // RBS interface names start with `_`; they are not Ruby module constants,
    // regardless of qualification or generic arguments.
    if !name.name().as_str().starts_with('_') {
        out.push_str(&format!("{keyword} {}\n", rbs_name(name)));
    }
}

fn rbs_members<'a>(nodes: impl Iterator<Item = ruby_rbs::node::Node<'a>>, out: &mut String) {
    use ruby_rbs::node::Node;
    for node in nodes {
        match node {
            Node::Class(class) => {
                out.push_str(&format!("class {}", rbs_name(&class.name())));
                if let Some(parent) = class.super_class() {
                    out.push_str(&format!(" < {}", rbs_name(&parent.name())));
                }
                out.push('\n');
                rbs_members(class.members().iter(), out);
                out.push_str("end\n");
            }
            Node::Module(module) => {
                out.push_str(&format!("module {}\n", rbs_name(&module.name())));
                rbs_members(module.members().iter(), out);
                out.push_str("end\n");
            }
            Node::Constant(value) => out.push_str(&format!("{} = nil\n", rbs_name(&value.name()))),
            Node::Include(edge) => rbs_mixin("include", &edge.name(), out),
            Node::Prepend(edge) => rbs_mixin("prepend", &edge.name(), out),
            Node::Extend(edge) => rbs_mixin("extend", &edge.name(), out),
            Node::ClassAlias(alias) => out.push_str(&format!(
                "{} = {}\n",
                rbs_name(&alias.new_name()),
                rbs_name(&alias.old_name())
            )),
            Node::ModuleAlias(alias) => out.push_str(&format!(
                "{} = {}\n",
                rbs_name(&alias.new_name()),
                rbs_name(&alias.old_name())
            )),
            _ => {}
        }
    }
}

/// Index-only placeholder, rewritten to an impossible Ruby name by the resolver.
pub(crate) const OPAQUE_CONSTANTS: &str = "RoundhouseIncompleteConstantsA6";

pub(crate) const RECEIVER_EFFECT: &str = "__roundhouse_constant_effect__";
pub(crate) const MIXIN_OVERRIDE: &str = "__roundhouse_mixin_override__";
pub(crate) const UNBOUNDED_EFFECT: &str = "__roundhouse_unbounded_constant_effect__";

const NAMED_EFFECT: &str = "RoundhouseUncertainNameA8_";
pub(crate) enum ConstantEffect {
    All,
    Name(crate::ident::Symbol),
}
pub(crate) fn constant_effect(name: &str) -> Option<ConstantEffect> {
    if matches!(
        name,
        OPAQUE_CONSTANTS | "<roundhouse:incomplete-constant-namespace>"
    ) {
        return Some(ConstantEffect::All);
    }
    if let Some(name) = name
        .strip_prefix("<roundhouse:uncertain-name:")
        .and_then(|n| n.strip_suffix('>'))
    {
        return Some(ConstantEffect::Name(crate::ident::Symbol::new(name)));
    }
    let hex = name.strip_prefix(NAMED_EFFECT)?;
    if hex.len() % 2 != 0 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let bytes = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
        .collect::<Option<Vec<_>>>()?;
    let name = String::from_utf8(bytes).ok()?;
    Some(ConstantEffect::Name(crate::ident::Symbol::new(name)))
}
pub(crate) fn named_marker(name: &str) -> String {
    format!(
        "{NAMED_EFFECT}{}",
        name.as_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    )
}
fn literal_constant_name(node: &Node<'_>) -> Option<String> {
    let name = super::util::symbol_value(node).or_else(|| super::util::string_value(node))?;
    let mut chars = name.bytes();
    if !chars.next()?.is_ascii_uppercase() || !chars.all(|c| c.is_ascii_alphanumeric() || c == b'_')
    {
        return None;
    }
    Some(name)
}

/// Project literal declarations and conservative barriers for omitted effects.
/// `nil` represents an opaque value name, never a claim about its type.
/// Loading remains unverified even for declarations retained in the inventory.
/// Apply the same whole-file rejection policy as `read_ruby`.
pub(crate) fn read_ruby_for_check(source: &[u8]) -> Result<String, String> {
    ruby_encoding(source)?;
    let parsed = super::prism::parse_silent(source);
    if let Some(error) = parsed.errors().next() {
        return Err(format!(
            "Ruby syntax error: {}",
            String::from_utf8_lossy(error.message())
        ));
    }
    let node = parsed.node();
    node.as_program_node().ok_or("expected a Ruby program")?;
    let mut producer = RubyProducer(
        String::new(),
        vec![false],
        false,
        vec![false],
        vec![BTreeSet::new()],
        vec![BTreeSet::new()],
    );
    producer.visit(&node);
    if producer.1[0] {
        producer.0.push_str(&format!(
            "class ::Object\ninclude ::{OPAQUE_CONSTANTS}\nend\n"
        ));
    } else {
        for name in &producer.4[0] {
            producer.0.push_str(&format!(
                "class ::Object\ninclude ::{}\nend\n",
                named_marker(name)
            ));
        }
    }
    Ok(producer.0)
}

// These contracts change method tables/visibility or class variables, not
// constants. Arguments and blocks still get visited; spelling alone never
// makes an arbitrary callback or computed argument harmless.
fn declaration_call(name: &[u8]) -> bool {
    matches!(
        name,
        b"public"
            | b"private"
            | b"protected"
            | b"module_function"
            | b"public_class_method"
            | b"private_class_method"
            | b"ruby2_keywords"
            | b"attr"
            | b"attr_reader"
            | b"attr_writer"
            | b"attr_accessor"
            | b"alias_method"
            | b"remove_method"
            | b"undef_method"
            | b"delegate"
            | b"delegate_missing_to"
            | b"class_attribute"
            | b"mattr_reader"
            | b"mattr_writer"
            | b"mattr_accessor"
            | b"cattr_reader"
            | b"cattr_writer"
            | b"cattr_accessor"
    )
}
fn effect_contract(name: &[u8]) -> bool {
    declaration_call(name)
        || matches!(
            name,
            b"const_set"
                | b"autoload"
                | b"include"
                | b"prepend"
                | b"extend"
                | b"respond_to?"
                | b"method_defined?"
                | b"private_method_defined?"
                | b"protected_method_defined?"
                | b"public_method_defined?"
                | b"block_given?"
        )
}
fn effect_hook(name: &[u8]) -> bool {
    matches!(
        name,
        b"method_added"
            | b"singleton_method_added"
            | b"const_added"
            | b"inherited"
            | b"included"
            | b"extended"
            | b"prepended"
            | b"append_features"
            | b"prepend_features"
            | b"extend_object"
            | b"const_missing"
    )
}
fn effect_free_call(node: &ruby_prism::CallNode<'_>) -> bool {
    let name = node.name();
    let implicit_self =
        node.receiver().is_none() || node.receiver().is_some_and(|n| n.as_self_node().is_some());
    if implicit_self
        && (declaration_call(name.as_slice())
            || matches!(
                name.as_slice(),
                b"respond_to?"
                    | b"method_defined?"
                    | b"private_method_defined?"
                    | b"protected_method_defined?"
                    | b"public_method_defined?"
                    | b"block_given?"
            ))
    {
        return true;
    }
    // Ordinary value calls (including #freeze on literals) stay uncertain:
    // source code may have replaced even a core receiver's implementation.
    false
}

// An initializer/receiver can mutate its enclosing namespace even when its
// resulting value is serialized opaquely. Inspect syntax, never evaluate it.
// The last flag distinguishes singleton self from the surrounding lexical owner.
struct ValueEffects(bool, bool, bool, String, bool);
impl ValueEffects {
    fn receiver(&mut self, receiver: Option<Node<'_>>, member: Option<&str>, method: &str) {
        if self.4 && receiver.as_ref().is_none_or(|node| node.as_self_node().is_some()) {
            // Singleton self is not the enclosing lexical namespace.
            self.unbounded();
            return;
        }
        let target = match receiver {
            None => "self".to_string(),
            Some(node) if node.as_self_node().is_some() => "self".to_string(),
            Some(node) => match constant(&node) {
                Some(name) => name,
                None => {
                    self.unbounded();
                    return;
                }
            },
        };
        let member = member.map_or("nil".to_string(), |name| format!("{name:?}"));
        self.3.push_str(&format!("{target}.{method}({member})\n"));
    }
    fn write_target(&mut self, target: &ruby_prism::ConstantPathNode<'_>) {
        self.0 = true;
        if let Some(parent) = target.parent() {
            // Evaluating a dynamic target can itself mutate other namespaces.
            self.visit(&parent);
            self.receiver(
                Some(parent),
                target.name().as_ref().map(super::util::constant_id_str),
                RECEIVER_EFFECT,
            );
        } else if let Some(name) = target.name() {
            self.3.push_str(&format!(
                "::Object.{RECEIVER_EFFECT}({:?})\n",
                super::util::constant_id_str(&name)
            ));
        } else {
            self.unbounded();
        }
    }
    fn unbounded(&mut self) {
        self.3.push_str(&format!("::Object.{UNBOUNDED_EFFECT}()\n"));
    }
}
impl<'pr> Visit<'pr> for ValueEffects {
    fn visit_call_node(&mut self, node: &ruby_prism::CallNode<'pr>) {
        self.0 |= !effect_free_call(node);
        if !effect_free_call(node)
            && (self.4 || node.receiver().is_some_and(|n| n.as_self_node().is_none()))
        {
            let member = if matches!(node.name().as_slice(), b"const_set" | b"autoload") {
                node.arguments()
                    .and_then(|a| a.arguments().iter().next())
                    .and_then(|n| literal_constant_name(&n))
            } else {
                None
            };
            self.receiver(node.receiver(), member.as_deref(), RECEIVER_EFFECT);
        }
        if node.name().as_slice() == b"alias_method" {
            let method = node
                .arguments()
                .and_then(|a| a.arguments().iter().next())
                .and_then(|n| super::util::symbol_value(&n));
            if method
                .as_ref()
                .is_none_or(|name| mixin_method(name.as_bytes()))
            {
                self.receiver(node.receiver(), method.as_deref(), MIXIN_OVERRIDE);
            }
            if method
                .as_ref()
                .is_none_or(|name| effect_contract(name.as_bytes()) || effect_hook(name.as_bytes()))
            {
                self.0 = true;
                self.receiver(node.receiver(), None, RECEIVER_EFFECT);
            }
        }
        if matches!(
            node.name().as_slice(),
            b"raise" | b"fail" | b"abort" | b"exit" | b"exit!" | b"exec" | b"throw"
        ) && (node.receiver().is_none()
            || node
                .receiver()
                .as_ref()
                .and_then(constant)
                .is_some_and(|name| name.trim_start_matches("::") == "Kernel"))
        {
            self.1 = true;
        }
        if let Some(receiver) = node.receiver() {
            self.visit(&receiver);
        }
        if let Some(args) = node.arguments() {
            for arg in args.arguments().iter() {
                self.visit(&arg);
            }
        }
        if let Some(block) = node.block() {
            self.visit(&block);
        }
    }
    fn visit_constant_write_node(&mut self, node: &ruby_prism::ConstantWriteNode<'pr>) {
        self.0 = true;
        self.visit(&node.value());
    }
    fn visit_constant_or_write_node(&mut self, node: &ruby_prism::ConstantOrWriteNode<'pr>) {
        self.0 = true;
        self.visit(&node.value());
    }
    fn visit_constant_and_write_node(&mut self, node: &ruby_prism::ConstantAndWriteNode<'pr>) {
        self.0 = true;
        self.visit(&node.value());
    }
    fn visit_constant_operator_write_node(
        &mut self,
        node: &ruby_prism::ConstantOperatorWriteNode<'pr>,
    ) {
        self.0 = true;
        self.visit(&node.value());
    }
    fn visit_constant_path_write_node(&mut self, node: &ruby_prism::ConstantPathWriteNode<'pr>) {
        self.write_target(&node.target());
        self.visit(&node.value());
    }
    fn visit_constant_path_or_write_node(
        &mut self,
        node: &ruby_prism::ConstantPathOrWriteNode<'pr>,
    ) {
        self.write_target(&node.target());
        self.visit(&node.value());
    }
    fn visit_constant_path_and_write_node(
        &mut self,
        node: &ruby_prism::ConstantPathAndWriteNode<'pr>,
    ) {
        self.write_target(&node.target());
        self.visit(&node.value());
    }
    fn visit_constant_path_operator_write_node(
        &mut self,
        node: &ruby_prism::ConstantPathOperatorWriteNode<'pr>,
    ) {
        self.write_target(&node.target());
        self.visit(&node.value());
    }
    fn visit_constant_target_node(&mut self, _: &ruby_prism::ConstantTargetNode<'pr>) {
        self.0 = true;
    }
    fn visit_class_node(&mut self, _: &ruby_prism::ClassNode<'pr>) {
        self.0 = true;
        self.unbounded();
    }
    fn visit_module_node(&mut self, _: &ruby_prism::ModuleNode<'pr>) {
        self.0 = true;
        self.unbounded();
    }
    fn visit_singleton_class_node(&mut self, node: &ruby_prism::SingletonClassNode<'pr>) {
        self.0 = true;
        self.receiver(Some(node.expression()), None, RECEIVER_EFFECT);
        self.visit(&node.expression());
        let previous = self.4;
        self.4 = true;
        if let Some(body) = node.body() {
            self.visit(&body);
        }
        self.4 = previous;
    }
    fn visit_yield_node(&mut self, _: &ruby_prism::YieldNode<'pr>) {
        self.0 = true;
    }
    fn visit_return_node(&mut self, _: &ruby_prism::ReturnNode<'pr>) {
        self.0 = true;
        self.1 = true;
    }
    fn visit_break_node(&mut self, _: &ruby_prism::BreakNode<'pr>) {
        self.0 = true;
        self.1 = true;
    }
    fn visit_next_node(&mut self, _: &ruby_prism::NextNode<'pr>) {
        self.0 = true;
        self.1 = true;
    }
    fn visit_redo_node(&mut self, _: &ruby_prism::RedoNode<'pr>) {
        self.0 = true;
        self.1 = true;
    }
    fn visit_retry_node(&mut self, _: &ruby_prism::RetryNode<'pr>) {
        self.0 = true;
        self.1 = true;
    }
    fn visit_alias_method_node(&mut self, node: &ruby_prism::AliasMethodNode<'pr>) {
        let method = super::util::symbol_value(&node.new_name());
        if method
            .as_ref()
            .is_none_or(|name| mixin_method(name.as_bytes()))
        {
            self.receiver(None, method.as_deref(), MIXIN_OVERRIDE);
        }
        if method
            .as_ref()
            .is_none_or(|name| effect_contract(name.as_bytes()) || effect_hook(name.as_bytes()))
        {
            self.0 = true;
            self.receiver(None, None, RECEIVER_EFFECT);
        }
    }
    fn visit_lambda_node(&mut self, _: &ruby_prism::LambdaNode<'pr>) {}
    fn visit_def_node(&mut self, node: &ruby_prism::DefNode<'pr>) {
        self.0 |= !self.2
            || effect_hook(node.name().as_slice())
            || effect_contract(node.name().as_slice());
        if mixin_method(node.name().as_slice()) {
            self.receiver(
                node.receiver(),
                Some(super::util::constant_id_str(&node.name())),
                MIXIN_OVERRIDE,
            );
        }
        if effect_hook(node.name().as_slice())
            || effect_contract(node.name().as_slice())
            || node.receiver().is_some_and(|n| n.as_self_node().is_none())
        {
            self.receiver(node.receiver(), None, RECEIVER_EFFECT);
        }
        if let Some(receiver) = node.receiver() {
            self.visit(&receiver);
        }
    }
}

fn mixin_method(name: &[u8]) -> bool {
    matches!(name, b"include" | b"prepend" | b"extend")
}

struct MixinOverrides(BTreeSet<String>);
impl<'pr> Visit<'pr> for MixinOverrides {
    fn visit_class_node(&mut self, _: &ruby_prism::ClassNode<'pr>) {}
    fn visit_module_node(&mut self, _: &ruby_prism::ModuleNode<'pr>) {}
    fn visit_def_node(&mut self, node: &ruby_prism::DefNode<'pr>) {
        if mixin_method(node.name().as_slice())
            && node.receiver().is_none_or(|n| n.as_self_node().is_some())
        {
            self.0
                .insert(String::from_utf8_lossy(node.name().as_slice()).into_owned());
        }
    }
    fn visit_alias_method_node(&mut self, node: &ruby_prism::AliasMethodNode<'pr>) {
        if let Some(name) = super::util::symbol_value(&node.new_name()) {
            if mixin_method(name.as_bytes()) {
                self.0.insert(name);
            }
        } else {
            self.0
                .extend(["include", "prepend", "extend"].map(String::from));
        }
    }
    fn visit_call_node(&mut self, node: &ruby_prism::CallNode<'pr>) {
        if node.name().as_slice() == b"alias_method"
            && node.receiver().is_none_or(|n| n.as_self_node().is_some())
        {
            if let Some(name) = node
                .arguments()
                .and_then(|a| a.arguments().iter().next())
                .and_then(|n| super::util::symbol_value(&n))
            {
                if mixin_method(name.as_bytes()) {
                    self.0.insert(name);
                }
            } else {
                self.0
                    .extend(["include", "prepend", "extend"].map(String::from));
            }
        }
        if let Some(receiver) = node.receiver() {
            self.visit(&receiver);
        }
        if let Some(args) = node.arguments() {
            for arg in args.arguments().iter() {
                self.visit(&arg);
            }
        }
        if let Some(block) = node.block() {
            self.visit(&block);
        }
    }
}

struct RubyProducer(
    String,
    Vec<bool>,
    bool,
    Vec<bool>,
    Vec<BTreeSet<String>>,
    Vec<BTreeSet<String>>,
);

impl RubyProducer {
    fn opaque(&mut self) {
        *self.1.last_mut().unwrap() = true;
    }
    fn receiver_effect(&mut self, receiver: Option<Node<'_>>, member: Option<&str>, method: &str) {
        let target = match receiver {
            None => "self".to_string(),
            Some(node) if node.as_self_node().is_some() => "self".to_string(),
            Some(node) => match constant(&node) {
                Some(name) => name,
                None => {
                    self.0.push_str(&format!("::Object.{UNBOUNDED_EFFECT}()\n"));
                    self.opaque();
                    return;
                }
            },
        };
        let member = member.map_or("nil".to_string(), |name| format!("{name:?}"));
        self.0.push_str(&format!("{target}.{method}({member})\n"));
    }
    fn end_scope(&mut self) {
        self.5.pop();
        self.3.pop();
        let names = self.4.pop().unwrap();
        if self.1.pop().unwrap() {
            self.0.push_str(&format!("include ::{OPAQUE_CONSTANTS}\n"));
        } else {
            for name in names {
                self.0
                    .push_str(&format!("include ::{}\n", named_marker(&name)));
            }
        }
        self.0.push_str("end\n");
    }
    fn body(&mut self, body: Option<Node<'_>>) {
        if let Some(body) = body {
            if let Some(statements) = body.as_statements_node() {
                self.visit_statements_node(&statements);
            } else {
                self.opaque();
            }
        }
    }
    fn observe_value(&mut self, value: &Node<'_>) {
        let mut effects = ValueEffects(false, false, false, String::new(), false);
        effects.visit(value);
        self.0.push_str(&effects.3);
        if effects.0 {
            self.opaque();
        }
        self.2 |= effects.1;
    }
    fn value(&mut self, name: &str, value: Option<Node<'_>>) {
        if let Some(value) = &value {
            self.observe_value(value);
        }
        if self.2 {
            return;
        }
        let value = value
            .as_ref()
            .and_then(constant)
            .unwrap_or_else(|| "nil".into());
        self.0.push_str(&format!("{name} = {value}\n"));
    }
}

impl<'pr> Visit<'pr> for RubyProducer {
    fn visit_class_node(&mut self, node: &ruby_prism::ClassNode<'pr>) {
        self.observe_value(&node.constant_path());
        if self.2 {
            return;
        }
        let Some(name) = constant(&node.constant_path()) else {
            self.opaque();
            return;
        };
        if let Some(parent) = node.superclass() {
            self.observe_value(&parent);
        }
        if self.2 {
            return;
        }
        self.0.push_str(&format!("class {name}"));
        if let Some(parent) = node.superclass().as_ref().and_then(constant) {
            self.0.push_str(&format!(" < {parent}"));
        }
        self.0.push('\n');
        self.1.push(
            node.superclass().is_some() && node.superclass().as_ref().and_then(constant).is_none(),
        );
        self.3.push(false);
        self.4.push(BTreeSet::new());
        self.5.push(BTreeSet::new());
        self.body(node.body());
        self.end_scope();
    }

    fn visit_module_node(&mut self, node: &ruby_prism::ModuleNode<'pr>) {
        self.observe_value(&node.constant_path());
        if self.2 {
            return;
        }
        let Some(name) = constant(&node.constant_path()) else {
            self.opaque();
            return;
        };
        self.0.push_str(&format!("module {name}\n"));
        self.1.push(false);
        self.3.push(false);
        self.4.push(BTreeSet::new());
        self.5.push(BTreeSet::new());
        self.body(node.body());
        self.end_scope();
    }

    fn visit_def_node(&mut self, node: &ruby_prism::DefNode<'pr>) {
        let name = node.name();
        if mixin_method(name.as_slice()) {
            if node.receiver().is_none_or(|n| n.as_self_node().is_some()) {
                self.5
                    .last_mut()
                    .unwrap()
                    .insert(String::from_utf8_lossy(name.as_slice()).into_owned());
            }
            self.receiver_effect(
                node.receiver(),
                Some(super::util::constant_id_str(&name)),
                MIXIN_OVERRIDE,
            );
        }
        if effect_hook(name.as_slice()) || effect_contract(name.as_slice()) {
            self.receiver_effect(node.receiver(), None, RECEIVER_EFFECT);
            if node.receiver().is_none_or(|n| n.as_self_node().is_some()) {
                self.opaque();
            }
        } else if node.receiver().is_some_and(|n| n.as_self_node().is_none()) {
            // Defining a singleton method can invoke the receiver's hooks.
            self.receiver_effect(node.receiver(), None, RECEIVER_EFFECT);
        }
        if let Some(receiver) = node.receiver() {
            self.observe_value(&receiver);
        }
    }
    fn visit_singleton_class_node(&mut self, node: &ruby_prism::SingletonClassNode<'pr>) {
        let receiver = node.expression();
        let mut overrides = MixinOverrides(BTreeSet::new());
        if let Some(body) = node.body() {
            overrides.visit(&body);
        }
        for name in overrides.0 {
            self.receiver_effect(Some(node.expression()), Some(&name), MIXIN_OVERRIDE);
            if receiver.as_self_node().is_some() {
                self.5.last_mut().unwrap().insert(name);
            }
        }
        self.observe_value(&receiver);
        if receiver.as_self_node().is_none() {
            self.receiver_effect(Some(receiver), None, RECEIVER_EFFECT);
        }
        // Keep lexical constant receivers in their original scope, but do not
        // mistake singleton self for that lexical owner.
        let mut effects = ValueEffects(false, false, true, String::new(), true);
        if let Some(body) = node.body() {
            effects.visit(&body);
        }
        self.0.push_str(&effects.3);
        if effects.0 {
            self.opaque();
        }
        self.2 |= effects.1;
    }
    fn visit_alias_method_node(&mut self, node: &ruby_prism::AliasMethodNode<'pr>) {
        let name = super::util::symbol_value(&node.new_name());
        if name
            .as_ref()
            .is_none_or(|name| mixin_method(name.as_bytes()))
        {
            let methods: Vec<_> = name.clone().map_or_else(
                || ["include", "prepend", "extend"].map(String::from).to_vec(),
                |name| vec![name],
            );
            for method in methods {
                self.5.last_mut().unwrap().insert(method.clone());
                self.receiver_effect(None, Some(&method), MIXIN_OVERRIDE);
            }
        }
        if name
            .as_ref()
            .is_none_or(|name| effect_contract(name.as_bytes()) || effect_hook(name.as_bytes()))
        {
            self.receiver_effect(None, None, RECEIVER_EFFECT);
            self.opaque();
        }
    }
    fn visit_lambda_node(&mut self, _: &ruby_prism::LambdaNode<'pr>) {}

    fn visit_statements_node(&mut self, node: &ruby_prism::StatementsNode<'pr>) {
        let mut overrides = MixinOverrides(BTreeSet::new());
        overrides.visit_statements_node(node);
        self.5.last_mut().unwrap().extend(overrides.0);
        for statement in node.body().iter() {
            if self.2 {
                break;
            }
            if statement.as_return_node().is_some()
                || statement.as_break_node().is_some()
                || statement.as_next_node().is_some()
                || statement.as_redo_node().is_some()
                || statement.as_retry_node().is_some()
            {
                self.opaque();
                self.2 = true;
                break;
            }
            if let Some(call) = statement.as_call_node() {
                let name = call.name();
                let exits = matches!(
                    name.as_slice(),
                    b"raise" | b"fail" | b"abort" | b"exit" | b"exit!" | b"exec" | b"throw"
                ) && (call.receiver().is_none()
                    || call
                        .receiver()
                        .as_ref()
                        .and_then(constant)
                        .is_some_and(|name| name.trim_start_matches("::") == "Kernel"));
                self.visit_call_node(&call);
                if exits {
                    self.2 = true;
                    break;
                }
            } else if statement.as_class_node().is_some()
                || statement.as_module_node().is_some()
                || statement.as_constant_write_node().is_some()
                || statement.as_constant_path_write_node().is_some()
                || statement.as_constant_or_write_node().is_some()
                || statement.as_constant_path_or_write_node().is_some()
                || statement.as_constant_and_write_node().is_some()
                || statement.as_constant_path_and_write_node().is_some()
                || statement.as_constant_operator_write_node().is_some()
                || statement.as_constant_path_operator_write_node().is_some()
            {
                self.visit(&statement);
            } else if let Some(singleton) = statement.as_singleton_class_node() {
                self.visit_singleton_class_node(&singleton);
            } else if let Some(alias) = statement.as_alias_method_node() {
                self.visit_alias_method_node(&alias);
            } else if let Some(definition) = statement.as_def_node() {
                self.visit_def_node(&definition);
            } else {
                // Inspect possible effects without promoting any names from
                // guarded code. A conditional visibility/method declaration
                // is harmless; any possible constant definition stays opaque.
                let mut effects = ValueEffects(false, false, true, String::new(), false);
                effects.visit(&statement);
                self.0.push_str(&effects.3);
                if effects.0 {
                    self.opaque();
                }
            }
        }
    }

    fn visit_call_node(&mut self, node: &ruby_prism::CallNode<'pr>) {
        let name = node.name();
        if node.receiver().is_none()
            && matches!(name.as_slice(), b"include" | b"prepend" | b"extend")
            && !self
                .5
                .last()
                .unwrap()
                .contains(super::util::constant_id_str(&name))
        {
            let targets: Option<Vec<_>> = node
                .arguments()
                .map(|args| args.arguments().iter().map(|arg| constant(&arg)).collect())
                .flatten();
            if let Some(targets) = targets.filter(|targets| !targets.is_empty()) {
                // Keep the argument group. Repeated single calls reverse its
                // priority. Never retain a partial list with a dynamic arg.
                if name.as_slice() == b"extend" {
                    if targets
                        .iter()
                        .any(|n| n.trim_start_matches("::") == "ActiveSupport::Concern")
                    {
                        *self.3.last_mut().unwrap() = true;
                    }
                    // A custom extension can replace a nil-effect DSL method
                    // on this namespace's singleton class.
                    if targets
                        .iter()
                        .any(|n| n.trim_start_matches("::") != "ActiveSupport::Concern")
                    {
                        self.opaque();
                    }
                }
                self.0.push_str(&format!(
                    "{} {}\n",
                    super::util::constant_id_str(&name),
                    targets.join(", ")
                ));
                return;
            }
        }
        if node.block().is_none()
            && matches!(name.as_slice(), b"const_set" | b"autoload")
            && let Some(args) = node.arguments()
        {
            let args: Vec<_> = args.arguments().iter().collect();
            if args.len() == 2
                && let Some(member) = literal_constant_name(&args[0])
            {
                let mut effects = ValueEffects(false, false, false, String::new(), false);
                effects.visit(&args[1]);
                if !effects.0 {
                    if node.receiver().is_none_or(|n| n.as_self_node().is_some()) {
                        self.4.last_mut().unwrap().insert(member);
                    } else {
                        self.receiver_effect(node.receiver(), Some(&member), RECEIVER_EFFECT);
                    }
                    return;
                }
            }
        }
        if node.receiver().is_none()
            && node.arguments().is_none()
            && matches!(name.as_slice(), b"included" | b"prepended")
            && *self.3.last().unwrap()
            && let Some(block) = node.block().and_then(|n| n.as_block_node())
        {
            // Registration does not execute now. Check every possible branch
            // for future constant effects without promoting any block names.
            let mut effects = ValueEffects(false, false, true, String::new(), false);
            if let Some(body) = block.body() {
                effects.visit(&body);
            }
            self.0.push_str(&effects.3);
            if effects.0 {
                self.opaque();
            }
            return;
        }
        if effect_free_call(node) && node.block().is_none() {
            if name.as_slice() == b"alias_method"
                && node.arguments().is_some_and(|args| {
                    args.arguments()
                        .iter()
                        .next()
                        .and_then(|n| super::util::symbol_value(&n))
                        .is_none_or(|name| effect_contract(name.as_bytes()) || effect_hook(name.as_bytes()))
                })
            {
                self.receiver_effect(node.receiver(), None, RECEIVER_EFFECT);
                self.opaque();
                if let Some(method) = node
                    .arguments()
                    .and_then(|a| a.arguments().iter().next())
                    .and_then(|n| super::util::symbol_value(&n))
                {
                    if mixin_method(method.as_bytes()) {
                        self.5.last_mut().unwrap().insert(method.clone());
                        self.receiver_effect(node.receiver(), Some(&method), MIXIN_OVERRIDE);
                    }
                }
            }
            let mut effects = ValueEffects(false, false, true, String::new(), false);
            if let Some(receiver) = node.receiver() {
                effects.visit(&receiver);
            }
            if let Some(args) = node.arguments() {
                for arg in args.arguments().iter() {
                    effects.visit(&arg);
                }
            }
            self.0.push_str(&effects.3);
            if effects.0 {
                self.opaque();
            }
            self.2 |= effects.1;
            return;
        }
        if node.receiver().is_some_and(|n| n.as_self_node().is_none()) {
            self.receiver_effect(node.receiver(), None, RECEIVER_EFFECT);
        }
        let mut effects = ValueEffects(false, false, true, String::new(), false);
        effects.visit_call_node(node);
        self.0.push_str(&effects.3);
        // An arbitrary omitted call can also affect its lexical namespace.
        // Bounded const_set/autoload calls returned above with a named effect.
        self.opaque();
        // Eval/autoload/const_set and arbitrary blocks may add constants.
        // Their absence must not let lookup skip to a later app ancestor.
    }

    fn visit_constant_write_node(&mut self, node: &ruby_prism::ConstantWriteNode<'pr>) {
        self.value(
            super::util::constant_id_str(&node.name()),
            Some(node.value()),
        );
    }
    fn visit_constant_or_write_node(&mut self, node: &ruby_prism::ConstantOrWriteNode<'pr>) {
        self.observe_value(&node.value());
        self.value(super::util::constant_id_str(&node.name()), None);
    }
    fn visit_constant_and_write_node(&mut self, node: &ruby_prism::ConstantAndWriteNode<'pr>) {
        self.observe_value(&node.as_node());
    }
    fn visit_constant_operator_write_node(
        &mut self,
        node: &ruby_prism::ConstantOperatorWriteNode<'pr>,
    ) {
        self.observe_value(&node.as_node());
    }
    fn visit_constant_target_node(&mut self, node: &ruby_prism::ConstantTargetNode<'pr>) {
        self.value(super::util::constant_id_str(&node.name()), None);
    }
    fn visit_constant_path_write_node(&mut self, node: &ruby_prism::ConstantPathWriteNode<'pr>) {
        self.observe_value(&node.target().as_node());
        self.observe_value(&node.value());
        if let Some(name) = constant(&node.target().as_node()) {
            self.value(&name, Some(node.value()));
        } else {
            self.opaque();
        }
    }
    fn visit_constant_path_or_write_node(
        &mut self,
        node: &ruby_prism::ConstantPathOrWriteNode<'pr>,
    ) {
        self.observe_value(&node.target().as_node());
        self.observe_value(&node.value());
        if let Some(name) = constant(&node.target().as_node()) {
            self.value(&name, None);
        } else {
            self.opaque();
        }
    }
    fn visit_constant_path_and_write_node(
        &mut self,
        node: &ruby_prism::ConstantPathAndWriteNode<'pr>,
    ) {
        self.observe_value(&node.as_node());
    }
    fn visit_constant_path_operator_write_node(
        &mut self,
        node: &ruby_prism::ConstantPathOperatorWriteNode<'pr>,
    ) {
        self.observe_value(&node.as_node());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_foreign_write(effect: &str) {
        for member in ["TOKEN", "OTHER"] {
            let effect = effect.replace("TOKEN", member);
            let text = read_ruby_for_check(format!(
                "module Zork; module Mix; end; module Other; end; {effect}; end"
            ).as_bytes()).unwrap();
            assert!(text.contains(&format!("Mix.{RECEIVER_EFFECT}({member:?})")), "{effect}: {text}");
            let unrelated = if member == "TOKEN" { "OTHER" } else { "TOKEN" };
            assert!(!text.contains(&format!("Mix.{RECEIVER_EFFECT}({unrelated:?})")), "{text}");
            assert!(!text.lines().any(|line| line.starts_with("Mix::TOKEN =")),
                "omitted writes must not become positive declarations: {text}");
        }
    }

    #[test]
    fn review_round2_singleton_body_retains_foreign_receiver_effects() {
        assert_foreign_write("class << Other; Mix.const_set(:TOKEN, 42); end");
        let text = read_ruby_for_check(b"module Zork; class << Other; self.const_set(:TOKEN, 42); end; end").unwrap();
        assert!(text.contains(UNBOUNDED_EFFECT), "singleton self cannot be bound to Zork: {text}");
        assert!(!text.contains(&format!("self.{RECEIVER_EFFECT}(\"TOKEN\")")), "{text}");
        let receiver = read_ruby_for_check(b"module Zork; class << Mix.const_set(:TOKEN, 42); end; end").unwrap();
        assert!(receiver.contains(&format!("Mix.{RECEIVER_EFFECT}(\"TOKEN\")")), "{receiver}");
    }

    #[test]
    fn review_round2_singleton_body_retains_qualified_write_effects() {
        assert_foreign_write("class << Other; Mix::TOKEN = 42; end");
    }

    #[test]
    fn review_round2_conditional_retains_qualified_write_effects() {
        assert_foreign_write("if true; Mix::TOKEN = 42; end");
        for operation in ["=", "||=", "&&=", "+="] {
            assert_foreign_write(&format!("if true; Mix::TOKEN {operation} 42; end"));
        }
    }

    #[test]
    fn review_round2_initializer_retains_qualified_write_effects() {
        assert_foreign_write("HOLDER = (Mix::TOKEN = 42)");
        let text = read_ruby_for_check(b"module Zork; HOLDER = (receiver(Mix.const_set(:TOKEN, 42))::VALUE = Other.const_set(:ITEM, 1)); end").unwrap();
        for effect in [UNBOUNDED_EFFECT.to_string(), format!("Mix.{RECEIVER_EFFECT}(\"TOKEN\")"), format!("Other.{RECEIVER_EFFECT}(\"ITEM\")")] {
            assert!(text.contains(&effect), "lost target or value effect {effect}: {text}");
        }
    }

    #[test]
    fn review_round2_omitted_and_operator_writes_retain_target_and_value_effects() {
        for operation in ["&&=", "+="] {
            assert_foreign_write(&format!("Mix::TOKEN {operation} 42"));
            let local_value = read_ruby_for_check(format!(
                "module Zork; HOLDER {operation} Mix.const_set(:TOKEN, 42); end"
            ).as_bytes()).unwrap();
            assert!(local_value.contains(&format!("Mix.{RECEIVER_EFFECT}(\"TOKEN\")")), "{local_value}");
            let harmless_value = read_ruby_for_check(format!("module Zork; HOLDER {operation} 42; end").as_bytes()).unwrap();
            assert!(!harmless_value.contains(RECEIVER_EFFECT), "{harmless_value}");
            let text = read_ruby_for_check(format!(
                "module Zork; receiver(Mix.const_set(:TOKEN, 42))::VALUE {operation} Other.const_set(:ITEM, 1); end"
            ).as_bytes()).unwrap();
            for effect in [UNBOUNDED_EFFECT.to_string(), format!("Mix.{RECEIVER_EFFECT}(\"TOKEN\")"), format!("Other.{RECEIVER_EFFECT}(\"ITEM\")")] {
                assert!(text.contains(&effect), "lost target or value effect {effect}: {text}");
            }
        }
    }

    #[test]
    fn receiver_effects_retain_literal_ownership_and_unbounded_receivers() {
        let named =
            read_ruby_for_check(b"module Zork; module Mix; end; Mix.const_set(:TOKEN, 42); end")
                .unwrap();
        assert!(
            named.contains(&format!("Mix.{RECEIVER_EFFECT}(\"TOKEN\")")),
            "{named}"
        );
        assert!(
            !named.contains(OPAQUE_CONSTANTS),
            "a bounded name does not poison the enclosing namespace: {named}"
        );
        let other =
            read_ruby_for_check(b"module Zork; module Mix; end; Mix.const_set(:OTHER, 42); end")
                .unwrap();
        assert!(other.contains(&format!("Mix.{RECEIVER_EFFECT}(\"OTHER\")")));
        for effect in [
            "Mix.autoload(:TOKEN, 'missing')",
            "if true; Mix.const_set(:TOKEN, 42); end",
            "HOLDER = Mix.const_set(:TOKEN, 42)",
            "Mix.class_eval('TOKEN = 42')",
            "def Mix.token; end",
            "class << Mix; def token; end; end",
        ] {
            let text = read_ruby_for_check(
                format!("module Zork; module Mix; end; {effect}; end").as_bytes(),
            )
            .unwrap();
            assert!(
                text.contains(&format!("Mix.{RECEIVER_EFFECT}")),
                "{effect}: {text}"
            );
        }
        let unknown =
            read_ruby_for_check(b"module Zork; receiver().const_set(:TOKEN, 42); end").unwrap();
        assert!(unknown.contains(UNBOUNDED_EFFECT), "{unknown}");
    }

    #[test]
    fn overridden_and_aliased_mixin_calls_are_not_literal_ancestry() {
        let mut failures = Vec::new();
        for method in ["include", "prepend", "extend"] {
            let pure =
                read_ruby_for_check(format!("module Mix; {method} Empty; end").as_bytes()).unwrap();
            assert!(pure.lines().any(|line| line == format!("{method} Empty")));
            for declaration in [
                format!("def self.{method}(*); const_set(:TOKEN, 42); end"),
                format!("alias_method :{method}, :custom"),
                format!("alias {method} custom"),
                format!("class << self; def {method}(*); end; end"),
                format!("private_class_method def self.{method}(*); end"),
                format!("private def self.{method}(*); end"),
                format!("(def self.{method}(*); end).to_s"),
            ] {
                let text = read_ruby_for_check(
                    format!("module Mix; {declaration}; {method} Empty; end").as_bytes(),
                )
                .unwrap();
                if text.lines().any(|line| line == format!("{method} Empty"))
                    || !text.contains(MIXIN_OVERRIDE)
                {
                    failures.push(format!("{declaration}: {text}"));
                }
            }
            let unrelated = read_ruby_for_check(format!(
                "module Mix; private_class_method def self.unrelated(*); end; {method} Empty; end"
            ).as_bytes()).unwrap();
            assert!(unrelated.lines().any(|line| line == format!("{method} Empty")), "{unrelated}");
        }
        assert!(failures.is_empty(), "missed wrapped overrides:\n{}", failures.join("\n"));
    }

    fn effect_barriers(text: &str) -> (bool, bool, bool) {
        (text.contains(OPAQUE_CONSTANTS), text.contains(RECEIVER_EFFECT), text.contains(UNBOUNDED_EFFECT))
    }

    const ALIASED_HOOKS: &[&str] = &[
        "const_missing", "const_added", "method_added", "singleton_method_added",
        "inherited", "included", "extended", "prepended", "append_features",
        "prepend_features", "extend_object",
    ];

    #[test]
    fn aliased_effect_hooks_are_incomplete_like_definitions() {
        let mut failures = Vec::new();
        for hook in ALIASED_HOOKS {
            for singleton in [false, true] {
                let project = |body: &str| {
                    let body = format!("def lookup(*); end; {body}");
                    let body = if singleton { format!("class << self; {body}; end") } else { body };
                    read_ruby_for_check(format!("module Hooks; {body}; end").as_bytes()).unwrap()
                };
                let definition = project(&format!("def {hook}(*); end"));
                assert!(definition.contains(OPAQUE_CONSTANTS), "definition control: {definition}");
                for alias in [format!("alias_method :{hook}, :lookup"), format!("alias {hook} lookup")] {
                    let text = project(&alias);
                    if effect_barriers(&text) != effect_barriers(&definition) {
                        failures.push(format!("singleton={singleton}, {alias}: expected {:?}, got {:?}: {text}", effect_barriers(&definition), effect_barriers(&text)));
                    }
                }
            }
        }
        assert!(failures.is_empty(), "aliased hooks differ from definitions:\n{}", failures.join("\n"));
    }

    #[test]
    fn aliased_effect_hooks_in_omitted_values_and_guards_are_not_pure() {
        let mut failures = Vec::new();
        for hook in ALIASED_HOOKS {
            for initializer in [false, true] {
                let project = |body: &str| {
                    let body = if initializer { format!("HOLDER = ({body})") } else { format!("if true; {body}; end") };
                    read_ruby_for_check(format!("module Hooks; def lookup(*); end; {body}; end").as_bytes()).unwrap()
                };
                let definition = project(&format!("def {hook}(*); end"));
                assert!(definition.contains(OPAQUE_CONSTANTS), "definition control: {definition}");
                for alias in [format!("alias_method :{hook}, :lookup"), format!("alias {hook} lookup")] {
                    let text = project(&alias);
                    if effect_barriers(&text) != effect_barriers(&definition) {
                        failures.push(format!("initializer={initializer}, {alias}: expected {:?}, got {:?}: {text}", effect_barriers(&definition), effect_barriers(&text)));
                    }
                }
            }
        }
        assert!(failures.is_empty(), "omitted aliased hooks treated as pure:\n{}", failures.join("\n"));
    }

    #[test]
    fn ordinary_aliases_do_not_add_constant_effects() {
        for alias in ["alias_method :resolve, :lookup", "alias resolve lookup"] {
            for body in [format!("def lookup(*); end; {alias}"),
                format!("class << self; def lookup(*); end; {alias}; end"),
                format!("def lookup(*); end; if true; {alias}; end"),
                format!("def lookup(*); end; HOLDER = ({alias})")]
            {
                let text = read_ruby_for_check(format!("module Hooks; {body}; end").as_bytes()).unwrap();
                assert_eq!(effect_barriers(&text), (false, false, false), "ordinary alias control: {body}: {text}");
            }
        }
    }

    #[test]
    fn check_projection_preserves_the_reader_rejection_policy_and_reasons() {
        for source in [
            b"# encoding: ISO-8859-1\nCAF\xe9 = 1\n".as_slice(),
            b"# coding: ISO-8859-1\nASCII = 1\n",
            b"#!/usr/bin/ruby\n# coding: Shift_JIS\nASCII = 1\n",
            b"#! coding: ISO-8859-1\nASCII = 1\n",
            b"#! coding: ISO-8859-1\n# encoding: UTF-8\nASCII = 1\n",
            b"# vim: set fileencoding=ISO-8859-1 :\nASCII = 1\n",
            b"class Before; end\nclass Broken <\n",
        ] {
            let reason = read_ruby(source).expect_err("reader rejects the whole file");
            assert_eq!(read_ruby_for_check(source), Err(reason));
        }
        for source in [
            "#!/usr/bin/ruby\n# encoding: UTF-8\nCAFé = 1\n",
            "\u{feff}# encoding: UTF-8\nCAFé = 1\n",
            "# coding: US-ASCII\nASCII = 1\n",
        ] {
            let output = read_ruby_for_check(source.as_bytes()).unwrap();
            assert!(output.contains(if source.contains("CAFé") {
                "CAFé = nil"
            } else {
                "ASCII = nil"
            }));
        }
    }

    #[test]
    fn ruby_rejects_encoding_marked_shebang_without_second_header() {
        let source = "#! coding: ISO-8859-1\nCAFé = 1\n";
        assert_eq!(ruby_prism::parse(source.as_bytes()).errors().count(), 0);
        let reason = read_ruby(source.as_bytes())
            .expect_err("encoding-marked shebang must reject the whole input");
        assert!(
            reason.contains("encoding"),
            "rejection must explain the encoding: {reason}"
        );
    }

    #[test]
    fn ruby_rejects_encoding_marked_shebang_before_second_utf8_header() {
        let source = "#! coding: ISO-8859-1\n# encoding: UTF-8\nCAFé = 1\n";
        assert_eq!(ruby_prism::parse(source.as_bytes()).errors().count(), 0);
        let reason = read_ruby(source.as_bytes())
            .expect_err("a second-line UTF-8 comment must not mask the first encoding");
        assert!(
            reason.contains("encoding"),
            "rejection must explain the encoding: {reason}"
        );
    }

    #[test]
    fn ruby_shebang_encoding_controls_preserve_names() {
        for source in [
            "#! coding: ISO-8859-1\n# encoding: ISO-8859-1\nCAFé = 1\n",
            "#!/usr/bin/ruby\n# encoding: ISO-8859-1\nCAFé = 1\n",
        ] {
            let reason = read_ruby(source.as_bytes())
                .expect_err("legacy encoding comments must reject the whole input");
            assert!(
                reason.contains("encoding"),
                "rejection must explain the encoding: {reason}"
            );
        }
        for source in [
            "#!/usr/bin/ruby\nCAFé = 1\n",
            "#!/usr/bin/ruby\n# encoding: UTF-8\nCAFé = 1\n",
            "# encoding: UTF-8\nCAFé = 1\n",
            "\u{feff}# encoding: UTF-8\nCAFé = 1\n",
        ] {
            assert_eq!(read_ruby(source.as_bytes()).unwrap(), "CAFé = nil\n");
        }
        assert_eq!(
            read_ruby(b"#!/usr/bin/ruby\n# encoding: US-ASCII\nASCII = 1\n").unwrap(),
            "ASCII = nil\n"
        );
    }

    #[test]
    fn rbs_interface_mixins_do_not_become_runtime_ancestors() {
        let source = r#"
            interface _Readable
              def read: () -> String
            end
            interface _Generic[A]
              def read: () -> A
            end
            module Features[A]
            end
            module Before
            end
            module ClassMethods
            end
            class Client
              include _Readable
              include ::_Readable
              include Lib::_Readable
              include ::Lib::_Readable
              include _Generic[String]
              include ::Lib::_Generic[String]
              extend _Readable
              extend ::_Readable
              extend Lib::_Readable
              extend ::Lib::_Generic[String]
              include Features[String]
              prepend Before
              extend ClassMethods
            end
            class After
            end
        "#;
        let output = read_rbs(source).unwrap();
        let parsed = ruby_prism::parse(output.as_bytes());
        assert_eq!(parsed.errors().count(), 0, "projection must be valid Ruby");
        drop(parsed);
        assert_eq!(
            output,
            concat!(
                "module Features\nend\nmodule Before\nend\nmodule ClassMethods\nend\n",
                "class Client\ninclude Features\nprepend Before\nextend ClassMethods\nend\nclass After\nend\n"
            )
        );
        let mut boundary = crate::gem_boundary::GemBoundary::default();
        super::super::rbi::read_rbi(output.as_bytes(), "library", &mut boundary);
        assert!(boundary.classes.keys().any(|id| id.0.as_str() == "After"));
        let index = crate::analyze::definition_index(&[crate::span::SourceFile {
            path: "interfaces.rb".into(),
            text: output,
        }]);
        assert_eq!(index.files["After"], ["interfaces.rb"]);
        assert!(
            index.ancestors["Client"]
                .iter()
                .any(|name| &**name == "Features")
        );
        assert!(
            index.ancestors["Client"]
                .iter()
                .all(|name| !name.contains("_Readable"))
        );
    }

    #[test]
    fn rbs_explicit_imports_are_rejected_before_projecting_bindings() {
        for source in [
            "use Lib::Base as Parent\nmodule Lib\nclass Base\nend\nend\nclass Parent\nend\nclass Child < Parent\nend\n",
            "use Lib::Features as Features\nclass Client\ninclude Features\nend\n",
            "use Lib::Base as Parent\nclass Alias = Parent\n",
            "use Lib::Features as Features\nmodule Alias = Features\n",
        ] {
            let reason = read_rbs(source).expect_err("imports cannot be silently erased");
            assert!(
                reason.contains("use"),
                "rejection must explain the unsupported import: {reason}"
            );
        }
    }

    #[test]
    fn rbs_wildcard_imports_are_rejected_before_projecting_bindings() {
        let reason = read_rbs("use Lib::*\nclass Child < Base\nend\n")
            .expect_err("wildcard imports cannot be silently erased");
        assert!(
            reason.contains("use"),
            "rejection must explain the unsupported import: {reason}"
        );
    }

    #[test]
    fn ruby_legacy_encoding_is_rejected_without_panicking() {
        let source = b"# encoding: ISO-8859-1\nCAF\xe9 = 1\n";
        assert_eq!(
            ruby_prism::parse(source).errors().count(),
            0,
            "valid Ruby control"
        );
        let result = std::panic::catch_unwind(|| read_ruby(source))
            .expect("unsupported source encodings must not panic");
        assert!(
            format!("{result:?}").contains("non-UTF-8"),
            "rejection must retain its reason"
        );
        assert_eq!(
            read_ruby("# encoding: UTF-8\nCAFé = 1\n".as_bytes()).unwrap(),
            "CAFé = nil\n"
        );
        for source in [
            "# coding: ISO-8859-1\nASCII = 1\n",
            "#!/usr/bin/ruby\n# -*- coding: Shift_JIS -*-\nASCII = 1\n",
            "# vim: set fileencoding=ISO-8859-1 :\nCAFé = 1\n",
            "# this file uses coding=ISO-8859-1\nCAFé = 1\n# encoding: UTF-8\n",
        ] {
            let reason = format!("{:?}", read_ruby(source.as_bytes()));
            assert!(
                reason.contains("unsupported Ruby source encoding"),
                "rejection must explain the encoding: {reason}"
            );
        }
        assert_eq!(
            read_ruby(b"# coding: US-ASCII\nASCII = 1\n").unwrap(),
            "ASCII = nil\n"
        );
        assert_eq!(read_ruby("CAFé = 1\n".as_bytes()).unwrap(), "CAFé = nil\n");
    }

    #[test]
    fn ruby_keeps_namespace_paths_ancestry_aliases_and_opaque_values() {
        let source = br#"
            module Library
              class Client < ::Base
                include First, Second
                prepend Before
                extend ClassMethods
                Alias = Other::Client
                VALUE = dangerous_initializer()
              end
            end
            Library::TOKEN = 42
            class ::Absolute::Client; end
        "#;
        assert_eq!(
            read_ruby(source).unwrap(),
            concat!(
                "module Library\nclass Client < ::Base\ninclude First, Second\n",
                "prepend Before\nextend ClassMethods\nAlias = Other::Client\nVALUE = nil\nend\nend\n",
                "Library::TOKEN = nil\nclass ::Absolute::Client\nend\n",
            )
        );
    }

    #[test]
    fn ruby_does_not_visit_executable_or_conditional_scopes() {
        let source = br#"
            require "never_loaded"
            File.write("never_written", "no")
            module Library
              def call; METHOD_BODY = 1; end
              class << self; SINGLETON = 1; end
              if false; class Conditional; end; end
              class_eval { class Evaluated; end }
              [1].each { class InBlock; end }
              HANDLER = -> { class InLambda; end }
              include First, dynamic_module
              self.include Other
              class Computed < factory(); class Nested; end; end
              class Visible; end
            end
        "#;
        // A dynamic constant assignment inside a method is invalid Ruby;
        // reject syntax errors rather than retaining Prism's recovery tree.
        assert!(read_ruby(source).is_err());
        let valid = String::from_utf8(source.to_vec())
            .unwrap()
            .replace("METHOD_BODY = 1", "raise 'method body'");
        assert_eq!(
            read_ruby(valid.as_bytes()).unwrap(),
            concat!("module Library\nHANDLER = nil\nclass Visible\nend\nend\n",)
        );
    }

    #[test]
    fn ruby_syntax_errors_do_not_leave_partial_declarations() {
        assert!(read_ruby(b"module Valid; end\nclass Broken <").is_err());
        assert_eq!(read_ruby(b""), Ok(String::new()));
    }

    #[test]
    fn rbs_projects_only_runtime_names() {
        let source = r#"
            module Library
              interface _Reader
                def read: () -> String
              end
              type value = String | Integer
              class Client < ::Base
                include Features
                prepend Before
                extend ClassMethods
                TOKEN: String
                def call: (Integer) -> String
              end
            end
            class Alias = Library::Client
            module AliasModule = Library
        "#;
        assert_eq!(
            read_rbs(source).unwrap(),
            concat!(
                "module Library\nclass Client < ::Base\ninclude Features\nprepend Before\n",
                "extend ClassMethods\nTOKEN = nil\nend\nend\nAlias = Library::Client\nAliasModule = Library\n",
            )
        );
        assert!(read_rbs("class Broken <").is_err());
    }

    #[test]
    fn projection_feeds_existing_rbi_reader_without_methods_or_loading_facts() {
        let declarations = read_ruby(
            br#"
            module Library
              class Client < Base
                include Features
                TOKEN = expensive()
                def call; raise "never executed"; end
              end
            end
        "#,
        )
        .unwrap();
        let mut boundary = crate::gem_boundary::GemBoundary::default();
        super::super::rbi::read_rbi(declarations.as_bytes(), "library", &mut boundary);
        let client = boundary
            .classes
            .iter()
            .find(|(id, _)| id.0.as_str() == "Library::Client")
            .unwrap()
            .1;
        assert_eq!(client.parent.as_ref().unwrap().0.as_str(), "Base");
        assert_eq!(client.includes[0].0.as_str(), "Features");
        assert_eq!(client.constants[0].as_str(), "TOKEN");
        assert!(client.instance_methods.is_empty());
        assert!(client.class_methods.is_empty());
        assert!(boundary.loaded.is_empty());
    }

    #[test]
    fn projection_preserves_lexical_ancestry_for_the_existing_constant_index() {
        let declarations = read_ruby(
            br#"
            module Library
              module Features; end
              class Base; end
              class Child < Base
                include Features
              end
            end
        "#,
        )
        .unwrap();
        let index = crate::analyze::definition_index(&[crate::span::SourceFile {
            path: "declarations.rb".into(),
            text: declarations,
        }]);
        assert_eq!(index.files["Library::Child"], ["declarations.rb"]);
        let ancestors = &index.ancestors["Library::Child"];
        assert!(
            ancestors
                .iter()
                .any(|name| name.as_ref() == "Library::Base")
        );
        assert!(
            ancestors
                .iter()
                .any(|name| name.as_ref() == "Library::Features")
        );
    }
}
