//! A class or module body registering one `autoload` per file of a
//! directory, by a glob over its own source tree:
//!
//! ```ruby
//! module Paygate
//!   load_path = Pathname.new(__FILE__ + "/../..")
//!   Dir[File.dirname(__FILE__) + "/gateways/**/*.rb"].each do |filename|
//!     gateway_name      = File.basename(filename, ".rb")
//!     gateway_classname = "#{gateway_name}_gateway".camelize
//!     gateway_filename  = Pathname.new(filename).relative_path_from(load_path).sub_ext("")
//!     autoload(gateway_classname, gateway_filename)
//!   end
//! end
//! ```
//!
//! (activemerchant's billing/gateways.rb has this shape). Its value is the
//! set of autoloads, which depends only on `__FILE__`, the directory
//! listing and pure string and path functions, so it is evaluated at
//! compile time over the tree's own files: each constant the loop names
//! autoloads the file whose iteration named it, as a literal `autoload
//! :Name, "path"` does. Only that closed vocabulary is read: `__FILE__`,
//! `__dir__`, string and symbol literals and interpolation, `String#+`,
//! `#camelize`, `#to_sym`, `File.dirname`/`basename`/`join`/`expand_path`,
//! `Pathname.new`, `Pathname#relative_path_from`/`sub_ext`/`to_s`, body
//! locals assigned from those, and `Dir[pattern]`/`Dir.glob(pattern)` with
//! `*`, `?` and `**/`. Anything else is not this shape.
//!
//! Ported from Shopify/roundhouse-1's `ingest::autoload_glob`, by Tobi Lütke.

use std::path::Path;

use ruby_prism::Node;

use crate::det::HashMap;

use super::util::{constant_id_str, flatten_statements};

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Value {
    Str(String),
    Path(String),
    Sym(String),
}

impl Value {
    fn text(&self) -> &str {
        match self {
            Value::Str(s) | Value::Path(s) | Value::Sym(s) => s,
        }
    }
}

/// A body's locals the loops may read, as the statements before them set
/// them: `local = <value>` is recorded, any other write forgets it.
#[derive(Default)]
pub(super) struct Locals(HashMap<String, Value>);

impl Locals {
    /// Record `statement` when it is a local write; `true` when it was.
    pub(super) fn record(&mut self, statement: &Node<'_>, file: &str) -> bool {
        let Some(write) = statement.as_local_variable_write_node() else { return false };
        let name = constant_id_str(&write.name()).to_string();
        match eval(&write.value(), file, &self.0) {
            Some(value) => {
                self.0.insert(name, value);
            }
            None => {
                self.0.remove(&name);
            }
        }
        true
    }
}

/// `Dir[pattern].each { |f| … autoload(name, feature) }` in a body: each
/// `(constant name, matched file)` it registers, the files in `Dir`'s
/// order. `None` when the statement is not that shape, or matches no file.
pub(super) fn glob_autoloads(
    statement: &Node<'_>,
    file: &str,
    locals: &Locals,
    list_dir: &dyn Fn(&Path) -> Vec<(String, bool)>,
) -> Option<Vec<(String, String)>> {
    let call = statement.as_call_node()?;
    if constant_id_str(&call.name()) != "each" || call.arguments().is_some() {
        return None;
    }
    let glob = call.receiver()?.as_call_node()?;
    let on_dir = glob.receiver().is_some_and(|r| r.as_constant_read_node().is_some_and(|c| constant_id_str(&c.name()) == "Dir"));
    if !on_dir || !matches!(constant_id_str(&glob.name()), "[]" | "glob") || glob.block().is_some() {
        return None;
    }
    let args: Vec<Node<'_>> = glob.arguments()?.arguments().iter().collect();
    let [pattern] = args.as_slice() else { return None };
    let Value::Str(pattern) = eval(pattern, file, &locals.0)? else { return None };
    let pattern = cleanpath(&pattern);
    if !pattern.starts_with('/') || !supported(&pattern) {
        return None;
    }
    let block = call.block()?.as_block_node()?;
    let param = block_param(&block)?;
    let body: Vec<Node<'_>> = block.body().map(flatten_statements).unwrap_or_default();
    let mut matches: Vec<String> = listing(&pattern, list_dir).into_iter().filter(|f| glob_matches(&pattern, f)).collect();
    matches.sort_by(|a, b| a.split('/').cmp(b.split('/')));
    if matches.is_empty() {
        return None;
    }
    let mut autoloads = Vec::new();
    for path in matches {
        let mut scope = locals.0.clone();
        scope.insert(param.clone(), Value::Str(path.clone()));
        let mut registered = false;
        for statement in &body {
            if let Some(write) = statement.as_local_variable_write_node() {
                let value = eval(&write.value(), file, &scope)?;
                scope.insert(constant_id_str(&write.name()).to_string(), value);
                continue;
            }
            let (name, _feature) = autoload(statement, file, &scope)?;
            autoloads.push((name, path.clone()));
            registered = true;
        }
        if !registered {
            return None;
        }
    }
    Some(autoloads)
}

/// The files under the pattern's literal directory (its segments before
/// the first wildcard), as absolute paths.
fn listing(pattern: &str, list_dir: &dyn Fn(&Path) -> Vec<(String, bool)>) -> Vec<String> {
    let fixed: Vec<&str> = pattern.split('/').take_while(|seg| !seg.contains(['*', '?'])).collect();
    let base = fixed.join("/");
    fn walk(at: &str, list_dir: &dyn Fn(&Path) -> Vec<(String, bool)>, out: &mut Vec<String>, depth: usize) {
        if depth > 32 {
            return;
        }
        for (name, is_dir) in list_dir(Path::new(at)) {
            let path = format!("{at}/{name}");
            if is_dir {
                walk(&path, list_dir, out, depth + 1);
            } else {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(if base.is_empty() { "/" } else { &base }, list_dir, &mut out, 0);
    out
}

/// ActiveSupport's `String#camelize` on a lower-case `a_b/c_d` name
/// (`crate::naming::camelize_path`, with the app's acronyms). Another
/// spelling is not read.
fn camelize(s: &str) -> Option<String> {
    let plain = !s.is_empty()
        && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '/')
        && !s.starts_with('/')
        && !s.ends_with('/')
        && !s.contains("//");
    plain.then(|| crate::naming::camelize_path(s))
}

/// `File.dirname`.
fn dirname(p: &str) -> String {
    let trimmed = p.trim_end_matches('/');
    if trimmed.is_empty() {
        return if p.starts_with('/') { "/".into() } else { ".".into() };
    }
    match trimmed.rfind('/') {
        None => ".".into(),
        Some(i) => {
            let dir = trimmed[..i].trim_end_matches('/');
            if dir.is_empty() { "/".into() } else { dir.into() }
        }
    }
}

/// `File.basename(p[, ext])`.
fn basename(p: &str, ext: Option<&str>) -> String {
    let trimmed = p.trim_end_matches('/');
    if trimmed.is_empty() {
        return if p.is_empty() { String::new() } else { "/".into() };
    }
    let base = trimmed.rsplit('/').next().unwrap_or(trimmed);
    match ext {
        Some(".*") => match base.rfind('.') {
            Some(i) if i > 0 => base[..i].into(),
            _ => base.into(),
        },
        Some(ext) if !ext.is_empty() && base.len() > ext.len() && base.ends_with(ext) => base[..base.len() - ext.len()].into(),
        _ => base.into(),
    }
}

/// `File.join`.
fn join<'a>(parts: impl Iterator<Item = &'a str>) -> String {
    let mut out = String::new();
    for (i, part) in parts.enumerate() {
        if i == 0 {
            out.push_str(part);
            continue;
        }
        match (out.ends_with('/'), part.starts_with('/')) {
            (true, true) => out.push_str(&part[1..]),
            (false, false) => {
                out.push('/');
                out.push_str(part);
            }
            _ => out.push_str(part),
        }
    }
    out
}

/// `Pathname#cleanpath`: lexical, `.` and `..` folded.
fn cleanpath(p: &str) -> String {
    let absolute = p.starts_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for segment in p.split('/') {
        match segment {
            "" | "." => {}
            ".." => match parts.last() {
                Some(&last) if last != ".." => {
                    parts.pop();
                }
                _ if absolute => {}
                _ => parts.push(".."),
            },
            other => parts.push(other),
        }
    }
    let joined = parts.join("/");
    match (absolute, joined.is_empty()) {
        (true, _) => format!("/{joined}"),
        (false, true) => ".".into(),
        (false, false) => joined,
    }
}

/// `Pathname#relative_path_from`; `None` where Ruby raises
/// (`ArgumentError`: one absolute and one relative, or `base` reaching
/// above what `p` shares with it).
fn relative_path_from(p: &str, base: &str) -> Option<String> {
    let (dest, base) = (cleanpath(p), cleanpath(base));
    if dest.starts_with('/') != base.starts_with('/') {
        return None;
    }
    let split = |s: &str| -> Vec<String> {
        s.split('/').filter(|seg| !seg.is_empty() && *seg != ".").map(str::to_string).collect()
    };
    let (dest, base) = (split(&dest), split(&base));
    let common = dest.iter().zip(&base).take_while(|(a, b)| a == b).count();
    if base[common..].iter().any(|seg| seg == "..") {
        return None;
    }
    let mut parts: Vec<String> = std::iter::repeat_n("..".to_string(), base.len() - common).collect();
    parts.extend(dest[common..].iter().cloned());
    Some(if parts.is_empty() { ".".into() } else { parts.join("/") })
}

/// `Pathname#sub_ext`.
fn sub_ext(p: &str, ext: &str) -> String {
    let (dir, base) = match p.rfind('/') {
        Some(i) => (&p[..=i], &p[i + 1..]),
        None => ("", p),
    };
    let stem = match base.rfind('.') {
        Some(i) if i > 0 => &base[..i],
        _ => base,
    };
    format!("{dir}{stem}{ext}")
}

fn supported(pattern: &str) -> bool {
    !pattern.contains(['{', '[', '\\'])
        && pattern.split('/').all(|seg| seg != "." && seg != ".." && (seg == "**" || !seg.contains("**")))
}

/// `File.fnmatch`-style `Dir.glob` over a file list: `**/` is any number
/// of directories, `*` any run and `?` one character within a segment,
/// neither matching a leading `.`.
fn glob_matches(pattern: &str, path: &str) -> bool {
    if !supported(pattern) {
        return false;
    }
    let pattern: Vec<&str> = pattern.split('/').collect();
    let path: Vec<&str> = path.split('/').collect();
    segments(&pattern, &path)
}

fn segments(pattern: &[&str], path: &[&str]) -> bool {
    match (pattern.first(), path.first()) {
        (None, None) => true,
        (Some(&"**"), _) => {
            segments(&pattern[1..], path)
                || (path.len() > 1 && !path[0].starts_with('.') && segments(pattern, &path[1..]))
        }
        (Some(p), Some(s)) => segment(p.as_bytes(), s.as_bytes(), true) && segments(&pattern[1..], &path[1..]),
        _ => false,
    }
}

fn segment(p: &[u8], s: &[u8], start: bool) -> bool {
    match (p.first(), s.first()) {
        (None, None) => true,
        (Some(b'*'), _) => {
            if start && s.first() == Some(&b'.') {
                return false;
            }
            segment(&p[1..], s, false) || (!s.is_empty() && segment(p, &s[1..], false))
        }
        (Some(b'?'), Some(c)) => !(start && *c == b'.') && segment(&p[1..], &s[1..], false),
        (Some(a), Some(b)) => a == b && segment(&p[1..], &s[1..], false),
        _ => false,
    }
}

fn block_param(block: &ruby_prism::BlockNode<'_>) -> Option<String> {
    let params = block.parameters()?.as_block_parameters_node()?;
    if params.locals().iter().next().is_some() {
        return None;
    }
    let params = params.parameters()?;
    let requireds: Vec<Node<'_>> = params.requireds().iter().collect();
    let [only] = requireds.as_slice() else { return None };
    let plain = params.optionals().iter().next().is_none()
        && params.rest().is_none()
        && params.posts().iter().next().is_none()
        && params.keywords().iter().next().is_none()
        && params.keyword_rest().is_none()
        && params.block().is_none();
    plain.then(|| only.as_required_parameter_node().map(|p| constant_id_str(&p.name()).to_string()))?
}

/// `autoload(name, feature)` on the body's own self.
fn autoload(statement: &Node<'_>, file: &str, scope: &HashMap<String, Value>) -> Option<(String, String)> {
    let call = statement.as_call_node()?;
    if constant_id_str(&call.name()) != "autoload" || call.receiver().is_some_and(|r| r.as_self_node().is_none()) || call.block().is_some() {
        return None;
    }
    let args: Vec<Node<'_>> = call.arguments()?.arguments().iter().collect();
    let [name, feature] = args.as_slice() else { return None };
    let name = match eval(name, file, scope)? {
        Value::Str(s) | Value::Sym(s) => s,
        Value::Path(_) => return None,
    };
    // Module#autoload's own check: a constant name, one segment.
    let mut chars = name.chars();
    if !chars.next().is_some_and(|c| c.is_ascii_uppercase()) || !chars.all(|c| c == '_' || c.is_ascii_alphanumeric()) {
        return None;
    }
    let feature = match eval(feature, file, scope)? {
        Value::Str(s) | Value::Path(s) => s,
        Value::Sym(_) => return None,
    };
    (!feature.is_empty()).then_some((name, feature))
}

pub(super) fn eval(node: &Node<'_>, file: &str, scope: &HashMap<String, Value>) -> Option<Value> {
    if node.as_source_file_node().is_some() {
        return Some(Value::Str(file.to_string()));
    }
    if let Some(s) = node.as_string_node() {
        return Some(Value::Str(String::from_utf8_lossy(s.unescaped()).into_owned()));
    }
    if let Some(s) = node.as_symbol_node() {
        return Some(Value::Sym(String::from_utf8_lossy(s.unescaped()).into_owned()));
    }
    if let Some(read) = node.as_local_variable_read_node() {
        return scope.get(constant_id_str(&read.name())).cloned();
    }
    if let Some(parens) = node.as_parentheses_node() {
        let body = flatten_statements(parens.body()?);
        let [only] = body.as_slice() else { return None };
        return eval(only, file, scope);
    }
    if let Some(interpolated) = node.as_interpolated_string_node() {
        let mut out = String::new();
        for part in interpolated.parts().iter() {
            if let Some(s) = part.as_string_node() {
                out.push_str(&String::from_utf8_lossy(s.unescaped()));
            } else if let Some(embedded) = part.as_embedded_statements_node() {
                let body = flatten_statements(embedded.statements()?.as_node());
                let [only] = body.as_slice() else { return None };
                out.push_str(eval(only, file, scope)?.text());
            } else {
                return None;
            }
        }
        return Some(Value::Str(out));
    }
    let call = node.as_call_node()?;
    if call.block().is_some() {
        return None;
    }
    let method = constant_id_str(&call.name()).to_string();
    let args: Vec<Node<'_>> = call.arguments().map(|a| a.arguments().iter().collect()).unwrap_or_default();
    let mut values = Vec::with_capacity(args.len());
    for arg in &args {
        values.push(eval(arg, file, scope)?);
    }
    // `__dir__`: the directory of `__FILE__` (Ruby resolves symlinks
    // first, which a tree path has none of).
    if call.receiver().is_none() && method == "__dir__" && values.is_empty() {
        return Some(Value::Str(dirname(file)));
    }
    let receiver = call.receiver()?;
    let constant = receiver.as_constant_read_node().map(|c| constant_id_str(&c.name()).to_string());
    match (constant.as_deref(), method.as_str(), values.as_slice()) {
        (Some("File"), "dirname", [Value::Str(p) | Value::Path(p)]) => Some(Value::Str(dirname(p))),
        (Some("File"), "basename", [Value::Str(p) | Value::Path(p)]) => Some(Value::Str(basename(p, None))),
        (Some("File"), "basename", [Value::Str(p) | Value::Path(p), Value::Str(ext)]) => Some(Value::Str(basename(p, Some(ext)))),
        (Some("File"), "join", parts) if !parts.is_empty() && parts.iter().all(|v| !matches!(v, Value::Sym(_))) => {
            Some(Value::Str(join(parts.iter().map(Value::text))))
        }
        // `File.expand_path(path, dir)` from an absolute `dir` (one read
        // off `__FILE__`, an absolute tree path here): lexical, as Ruby's
        // is with no `~` and no symlink.
        (Some("File"), "expand_path", [Value::Str(p) | Value::Path(p), Value::Str(base) | Value::Path(base)])
            if base.starts_with('/') && !p.starts_with('~') =>
        {
            Some(Value::Str(if p.starts_with('/') { cleanpath(p) } else { cleanpath(&join([base.as_str(), p.as_str()].into_iter())) }))
        }
        (Some("Pathname"), "new", [Value::Str(p) | Value::Path(p)]) => Some(Value::Path(p.clone())),
        (Some(_), _, _) => None,
        (None, _, _) => {
            let recv = eval(&receiver, file, scope)?;
            match (recv, method.as_str(), values.as_slice()) {
                (Value::Str(a), "+", [Value::Str(b)]) => Some(Value::Str(a + b)),
                (Value::Str(s), "camelize", []) => camelize(&s).map(Value::Str),
                // `String#to_sym` (offsite_payments 3.0.1's integrations.rb
                // names each autoload `filename.camelize.to_sym`).
                (Value::Str(s), "to_sym", []) => Some(Value::Sym(s)),
                (Value::Path(p), "relative_path_from", [Value::Path(base) | Value::Str(base)]) => {
                    relative_path_from(p.as_str(), base).map(Value::Path)
                }
                (Value::Path(p), "sub_ext", [Value::Str(ext)]) => Some(Value::Path(sub_ext(&p, ext))),
                (Value::Path(p) | Value::Str(p), "to_s", []) => Some(Value::Str(p)),
                _ => None,
            }
        }
    }
}
