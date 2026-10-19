use std::collections::{BTreeMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use ruby_prism::{Node, Visit};
use sha1::{Digest, Sha1};

use super::util::{constant_id_str, flatten_statements, string_value};
use super::{IngestError, IngestResult};
use crate::vfs::Vfs;

static CSS_URL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"url\(\s*["']?([^"'\s?#)]+)([#?][^"')]+)?\s*["']?\)"#).unwrap());
static JS_URL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"RAILS_ASSET_URL\(\s*["']?([^"'\s?#)]+)([#?][^"')]+)?\s*["']?\)"#).unwrap()
});
static PRE_DIGESTED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"-[0-9a-zA-Z_-]{7,128}\.digested").unwrap());
static EXTENSION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\.([0-9A-Za-z_]+(\.map)?)$").unwrap());
static SOURCE_MAP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(//|/\*)# sourceMappingURL=(.+\.map)(\s*?\*/)?\s*?(\n?)$").unwrap()
});

fn version_call(node: &Node<'_>) -> bool {
    let Some(call) = node.as_call_node() else {
        return false;
    };
    constant_id_str(&call.name()) == "version="
        && call
            .receiver()
            .and_then(|r| r.as_call_node())
            .is_some_and(|assets| {
                constant_id_str(&assets.name()) == "assets"
                    && assets
                        .receiver()
                        .and_then(|r| r.as_call_node())
                        .is_some_and(|config| constant_id_str(&config.name()) == "config")
            })
}

fn version_statements(
    nodes: Vec<Node<'_>>,
    value: &mut String,
    read: &mut usize,
) -> Result<(), String> {
    for node in nodes {
        if let Some(class) = node.as_class_node() {
            version_statements(
                class.body().map(flatten_statements).unwrap_or_default(),
                value,
                read,
            )?;
        } else if let Some(module) = node.as_module_node() {
            version_statements(
                module.body().map(flatten_statements).unwrap_or_default(),
                value,
                read,
            )?;
        } else if version_call(&node) {
            let arg = node
                .as_call_node()
                .unwrap()
                .arguments()
                .and_then(|a| a.arguments().iter().next())
                .ok_or("asset version has no value")?;
            *value = if arg.as_nil_node().is_some() {
                String::new()
            } else {
                string_value(&arg)
                    .ok_or("Propshaft asset version is not a literal String or nil")?
            };
            *read += 1;
        } else if let Some(call) = node.as_call_node()
            && constant_id_str(&call.name()) == "configure"
            && call
                .receiver()
                .and_then(|r| r.as_call_node())
                .is_some_and(|r| constant_id_str(&r.name()) == "application")
        {
            let body = call
                .block()
                .and_then(|b| b.as_block_node())
                .and_then(|b| b.body());
            version_statements(
                body.map(flatten_statements).unwrap_or_default(),
                value,
                read,
            )?;
        }
    }
    Ok(())
}

fn apply_version_file<V: Vfs + ?Sized>(
    vfs: &V,
    path: &Path,
    value: &mut String,
) -> IngestResult<()> {
    if !vfs.exists(path) {
        return Ok(());
    }
    let bytes = vfs.read(path)?;
    let parsed = super::prism::parse(&bytes, &path.display().to_string());
    struct Count(usize);
    impl<'pr> Visit<'pr> for Count {
        fn visit_call_node(&mut self, call: &ruby_prism::CallNode<'pr>) {
            if version_call(&call.as_node()) {
                self.0 += 1;
            }
            ruby_prism::visit_call_node(self, call);
        }
    }
    let mut count = Count(0);
    count.visit(&parsed.node());
    let mut read = 0;
    let program = parsed.node().as_program_node().unwrap();
    version_statements(
        program.statements().body().iter().collect(),
        value,
        &mut read,
    )
    .and_then(|()| {
        if count.0 == read {
            Ok(())
        } else {
            Err("Propshaft asset version is outside a supported boot body".to_string())
        }
    })
    .map_err(|message| IngestError::Unsupported {
        file: path.display().to_string(),
        message,
    })
}

pub(super) fn read_version<V: Vfs + ?Sized>(vfs: &V, dir: &Path) -> IngestResult<String> {
    let mut base = "1".to_string();
    apply_version_file(vfs, &dir.join("config/application.rb"), &mut base)?;
    let mut environments = vec![None];
    let environment_dir = dir.join("config/environments");
    if vfs.is_dir(&environment_dir) {
        let mut files = vfs.read_dir(&environment_dir)?;
        files.sort();
        environments.extend(
            files
                .into_iter()
                .filter(|path| path.extension().is_some_and(|ext| ext == "rb"))
                .map(Some),
        );
    }
    let mut initializers = Vec::new();
    let init_dir = dir.join("config/initializers");
    if vfs.is_dir(&init_dir) {
        initializers.extend(super::app::read_rb_files(vfs, &init_dir)?);
    }
    let mut versions = HashSet::new();
    for environment in environments {
        let mut value = base.clone();
        if let Some(path) = environment {
            apply_version_file(vfs, &path, &mut value)?;
        }
        for path in &initializers {
            apply_version_file(vfs, path, &mut value)?;
        }
        versions.insert(value);
    }
    if versions.len() != 1 {
        return Err(IngestError::Unsupported { file: environment_dir.display().to_string(), message: "environment-dependent Propshaft asset versions cannot share one compiled asset table".to_string() });
    }
    Ok(versions.into_iter().next().unwrap())
}

fn collect<V: Vfs + ?Sized>(
    vfs: &V,
    root: &Path,
    dir: &Path,
    assets: &mut BTreeMap<String, Vec<u8>>,
) -> IngestResult<()> {
    let mut entries = vfs.read_dir(dir)?;
    entries.sort();
    for path in entries {
        if vfs.is_symlink(&path) {
            continue;
        }
        if vfs.is_dir(&path) {
            collect(vfs, root, &path, assets)?;
        } else if !path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with('.'))
        {
            let logical = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            if !assets.contains_key(&logical) {
                assets.insert(logical, vfs.read(&path)?);
            }
        }
    }
    Ok(())
}

fn resolve(directory: &str, name: &str) -> Option<String> {
    let path = if name.starts_with('/') {
        PathBuf::from(name.trim_start_matches('/'))
    } else {
        Path::new(directory).join(name)
    };
    let mut parts = Vec::new();
    for part in path.components() {
        match part {
            Component::Normal(name) => parts.push(name.to_string_lossy().to_string()),
            Component::ParentDir => {
                parts.pop()?;
            }
            Component::CurDir => (),
            _ => return None,
        }
    }
    Some(parts.join("/"))
}

fn pattern(logical: &str) -> Option<(&'static Regex, bool)> {
    if logical.ends_with(".css") {
        Some((&CSS_URL, true))
    } else if logical.ends_with(".js") || logical.ends_with(".mjs") {
        Some((&JS_URL, false))
    } else {
        None
    }
}

fn ignored(name: &str, css: bool) -> bool {
    name.starts_with("%23")
        || name.starts_with("//")
        || if css {
            ["data:", "http:", "https:"]
                .iter()
                .any(|prefix| name.starts_with(prefix))
        } else {
            ["data", "http"]
                .iter()
                .any(|prefix| name.starts_with(prefix))
        }
}

fn referenced(
    logical: &str,
    assets: &BTreeMap<String, Vec<u8>>,
    seen: &mut HashSet<String>,
    refs: &mut Vec<String>,
) {
    let Some(bytes) = assets.get(logical) else {
        return;
    };
    let Some((regex, css)) = pattern(logical) else {
        return;
    };
    let text = String::from_utf8_lossy(bytes);
    let directory = Path::new(logical)
        .parent()
        .unwrap_or(Path::new(""))
        .to_string_lossy();
    for captures in regex.captures_iter(&text) {
        let name = &captures[1];
        if ignored(name, css) {
            continue;
        }
        if let Some(path) = resolve(&directory, name)
            && assets.contains_key(&path)
            && seen.insert(path.clone())
        {
            refs.push(path.clone());
            referenced(&path, assets, seen, refs);
        }
    }
}

fn digested_paths(assets: &BTreeMap<String, Vec<u8>>, version: &str) -> BTreeMap<String, String> {
    assets
        .iter()
        .map(|(logical, bytes)| {
            if PRE_DIGESTED.is_match(logical) {
                return (logical.clone(), logical.clone());
            }
            let mut hash = Sha1::new();
            hash.update(bytes);
            if pattern(logical).is_some() {
                let mut refs = Vec::new();
                referenced(logical, assets, &mut HashSet::new(), &mut refs);
                for path in refs {
                    hash.update(&assets[&path]);
                }
            }
            hash.update(version.as_bytes());
            let digest = format!("{:x}", hash.finalize());
            let path = EXTENSION
                .replace(logical, |captures: &regex::Captures<'_>| {
                    format!("-{}{}", &digest[..8], &captures[0])
                })
                .into_owned();
            (logical.clone(), path)
        })
        .collect()
}

fn compile(logical: &str, bytes: &[u8], paths: &BTreeMap<String, String>) -> Vec<u8> {
    let Some((regex, css)) = pattern(logical) else {
        return bytes.to_vec();
    };
    let Ok(text) = std::str::from_utf8(bytes) else {
        return bytes.to_vec();
    };
    let directory = Path::new(logical)
        .parent()
        .unwrap_or(Path::new(""))
        .to_string_lossy();
    let compiled = regex.replace_all(text, |captures: &regex::Captures<'_>| {
        let name = &captures[1];
        if ignored(name, css) {
            return captures[0].to_string();
        }
        let path = resolve(&directory, name).and_then(|path| paths.get(&path));
        let url = path
            .map(|path| {
                format!(
                    "/assets/{path}{}",
                    captures.get(2).map(|m| m.as_str()).unwrap_or("")
                )
            })
            .unwrap_or_else(|| name.to_string());
        if css {
            format!("url({url:?})")
        } else {
            format!("{url:?}")
        }
    });
    SOURCE_MAP
        .replace_all(&compiled, |captures: &regex::Captures<'_>| {
            let name = captures[2].split("/assets/").last().unwrap();
            let path = resolve(&directory, name).and_then(|path| paths.get(&path));
            let end = captures.get(3).map(|m| m.as_str()).unwrap_or("");
            let newline = &captures[4];
            match path {
                Some(path) => format!(
                    "{}# sourceMappingURL=/assets/{path}{end}{newline}",
                    &captures[1]
                ),
                None => format!("{}{end}{newline}", &captures[1]),
            }
        })
        .into_owned()
        .into_bytes()
}

fn app_assets<V: Vfs + ?Sized>(vfs: &V, dir: &Path) -> IngestResult<BTreeMap<String, Vec<u8>>> {
    let mut assets = BTreeMap::new();
    for base in ["app/assets", "lib/assets", "vendor/assets"] {
        let base = dir.join(base);
        if !vfs.is_dir(&base) {
            continue;
        }
        let mut roots = vfs.read_dir(&base)?;
        roots.sort();
        for root in roots {
            if vfs.is_dir(&root)
                && !root
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with('.'))
            {
                collect(vfs, &root, &root, &mut assets)?;
            }
        }
    }
    Ok(assets)
}

pub(super) fn version_is_consumed<V: Vfs + ?Sized>(vfs: &V, dir: &Path) -> bool {
    vfs.read_to_string(&dir.join("Gemfile.lock"))
        .ok()
        .is_some_and(|text| crate::gems::Lockfile::parse(&text).has("propshaft"))
        && read_version(vfs, dir).is_ok()
        && app_assets(vfs, dir).is_ok_and(|assets| !assets.is_empty())
}

fn ruby_string(value: &str) -> String {
    format!("{value:?}").replace('#', "\\#")
}

pub(super) fn ingest<V: Vfs + ?Sized>(
    vfs: &V,
    dir: &Path,
    app: &mut crate::App,
) -> IngestResult<()> {
    if !app
        .gem_lock
        .as_ref()
        .is_some_and(|lock| lock.has("propshaft"))
    {
        return Ok(());
    }
    let version = match read_version(vfs, dir) {
        Ok(version) => version,
        Err(error) => {
            super::survey::continue_or_fail(error)?;
            return Ok(());
        }
    };
    let assets = app_assets(vfs, dir)?;
    let paths = digested_paths(&assets, &version);
    if paths.is_empty() {
        return Ok(());
    }
    // tailwindcss-rails layouts use `stylesheet_link_tag :app`, which Propshaft
    // resolves to the compiled `builds/tailwind.css` logical name.
    let mut lookups = paths.clone();
    if paths.contains_key("tailwind.css") {
        lookups
            .entry("app.css".to_string())
            .or_insert_with(|| paths["tailwind.css"].clone());
    }
    let mut source = "def propshaft_asset_path(source)\ncase source\n".to_string();
    for (logical, path) in &lookups {
        source.push_str(&format!(
            "when {}\n{}\n",
            ruby_string(logical),
            ruby_string(&format!("/assets/{path}"))
        ));
    }
    for (logical, path) in &paths {
        app.binary_assets.push((
            format!("static/assets/{path}"),
            compile(logical, &assets[logical], &paths),
        ));
    }
    source.push_str("else\n\"/assets/#{source}\"\nend\nend\n");
    let mut methods =
        crate::runtime_src::parse_methods(&source).map_err(|message| IngestError::Parse {
            file: "<propshaft_asset_paths>".to_string(),
            message,
        })?;
    let rails = app
        .rails_application
        .get_or_insert_with(|| crate::dialect::LibraryClass {
            name: crate::ident::ClassId(crate::ident::Symbol::from("Rails::Application")),
            is_module: false,
            parent: None,
            parent_span: Default::default(),
            includes: Vec::new(),
            include_spans: Vec::new(),
            methods: Vec::new(),
            nullable_columns: Vec::new(),
            origin: None,
            constants: Vec::new(),
            unknown_calls: Vec::new(),
            class_ivar_initializers: Vec::new(),
        });
    rails.methods.append(&mut methods);
    Ok(())
}
