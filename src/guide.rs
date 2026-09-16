//! The user guide, rendered for the Pages site.
//!
//! `docs/guide/*.md` and `RELEASES.md` are the guide as the repository
//! reads it; `roundhouse --site` renders the same files to
//! `_site/docs/guide/*.html` and `_site/docs/releases.html` in the
//! site's own page shell, so the live site and the tree never say two
//! different things. Links are rewritten by where they resolve: to
//! another guide page → that page's `.html`; to anything else in the
//! repository (architecture docs, DEVELOPMENT.md, the VS Code client)
//! → the file on GitHub, since the site does not carry it.

use std::fs;
use std::path::{Path, PathBuf};

use pulldown_cmark::{CowStr, Event, Options, Parser, Tag, html};

const GITHUB_BLOB: &str = "https://github.com/rubys/roundhouse/blob/main/";
const GITHUB_TREE: &str = "https://github.com/rubys/roundhouse/tree/main/";

/// Every page: (repository path, site path). README.md is the guide's
/// index page; RELEASES.md rides along because every page links to it.
fn pages() -> Result<Vec<(PathBuf, PathBuf)>, String> {
    let dir = Path::new("docs/guide");
    let mut out = Vec::new();
    let mut names: Vec<_> = fs::read_dir(dir)
        .map_err(|e| format!("read {}: {e}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
        .filter(|n| n.ends_with(".md"))
        .collect();
    names.sort();
    for name in names {
        let site = if name == "README.md" { "index.html".to_string() } else { name.replace(".md", ".html") };
        out.push((dir.join(&name), Path::new("docs/guide").join(site)));
    }
    out.push((PathBuf::from("RELEASES.md"), PathBuf::from("docs/releases.html")));
    Ok(out)
}

/// Render every page under `out`. Called by `project::build_site`.
pub fn render_site(out: &Path) -> Result<(), String> {
    let pages = pages()?;
    for (src, site) in &pages {
        let md = fs::read_to_string(src).map_err(|e| format!("read {}: {e}", src.display()))?;
        let page = render_page(&md, src, site, &pages);
        let dst = out.join(site);
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
        }
        fs::write(&dst, page).map_err(|e| format!("write {}: {e}", dst.display()))?;
        eprintln!("wrote {}", dst.display());
    }
    Ok(())
}

/// One page: the Markdown's first `# ` heading becomes the header
/// tagline and is dropped from the body; everything else renders into
/// the site's `doc-main` shell.
fn render_page(md: &str, src: &Path, site: &Path, pages: &[(PathBuf, PathBuf)]) -> String {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    let src_dir = src.parent().unwrap_or(Path::new(""));
    let site_dir = site.parent().unwrap_or(Path::new(""));

    let mut title = String::new();
    let mut in_title = false;
    let mut events = Vec::new();
    for ev in Parser::new_ext(md, options) {
        match ev {
            Event::Start(Tag::Heading { level: pulldown_cmark::HeadingLevel::H1, .. }) if title.is_empty() => {
                in_title = true;
            }
            Event::End(pulldown_cmark::TagEnd::Heading(pulldown_cmark::HeadingLevel::H1)) if in_title => {
                in_title = false;
            }
            Event::Text(t) | Event::Code(t) if in_title => title.push_str(&t),
            Event::Start(Tag::Link { link_type, dest_url, title: t, id }) => {
                let dest = rewrite_link(&dest_url, src_dir, site_dir, pages);
                events.push(Event::Start(Tag::Link { link_type, dest_url: CowStr::from(dest), title: t, id }));
            }
            other => events.push(other),
        }
    }
    let mut body = String::new();
    html::push_html(&mut body, events.into_iter());

    let depth = site.components().count().saturating_sub(1);
    let root = "../".repeat(depth);
    let github = format!("{GITHUB_BLOB}{}", src.display());
    let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;");
    format!(
        "<!DOCTYPE html>
<html lang=\"en\">
<head>
  <meta charset=\"UTF-8\">
  <meta name=\"viewport\" content=\"width=device-width, initial-scale=1.0\">
  <title>Roundhouse — {title}</title>
  <link rel=\"stylesheet\" href=\"{root}style.css\">
</head>
<body class=\"doc guide\">
  <header>
    <h1><a href=\"{root}\">Roundhouse</a></h1>
    <p class=\"tagline\"><a href=\"{root}docs/guide/\">User guide</a> · {title}</p>
  </header>

  <main class=\"doc-main\">
{body}
    <p class=\"footnote\">This page is <a href=\"{github}\">{path}</a> in the repository; edits are welcome there.</p>
  </main>
</body>
</html>
",
        title = esc(&title),
        path = esc(&src.display().to_string()),
    )
}

/// Where a Markdown link lands on the site. Absolute URLs and
/// in-page anchors pass through; a relative path is resolved against
/// the source file's directory in the repository, then mapped to the
/// rendered page when it names one and to GitHub otherwise.
fn rewrite_link(dest: &str, src_dir: &Path, site_dir: &Path, pages: &[(PathBuf, PathBuf)]) -> String {
    if dest.starts_with('#') || dest.contains("://") || dest.starts_with("mailto:") {
        return dest.to_string();
    }
    let (path, fragment) = match dest.split_once('#') {
        Some((p, f)) => (p, Some(f)),
        None => (dest, None),
    };
    let repo_path = normalize(&src_dir.join(path));
    let target = pages
        .iter()
        .find(|(repo, _)| *repo == repo_path)
        .map(|(_, site)| relative(site_dir, site))
        .unwrap_or_else(|| {
            let base = if path.ends_with('/') || repo_path.is_dir() { GITHUB_TREE } else { GITHUB_BLOB };
            format!("{base}{}", repo_path.display())
        });
    match fragment {
        Some(f) => format!("{target}#{f}"),
        None => target,
    }
}

/// Lexically resolve `.` and `..` (the paths are repository-relative
/// and never escape it: a `..` past the root is dropped).
fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

/// `to` relative to the directory `from`, both repository-relative.
fn relative(from: &Path, to: &Path) -> String {
    let from: Vec<_> = from.components().collect();
    let to: Vec<_> = to.components().collect();
    let common = from.iter().zip(to.iter()).take_while(|(a, b)| a == b).count();
    let mut s = "../".repeat(from.len() - common);
    let rest: Vec<String> = to[common..].iter().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    s.push_str(&rest.join("/"));
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pages() -> Vec<(PathBuf, PathBuf)> {
        vec![
            (PathBuf::from("docs/guide/README.md"), PathBuf::from("docs/guide/index.html")),
            (PathBuf::from("docs/guide/check.md"), PathBuf::from("docs/guide/check.html")),
            (PathBuf::from("RELEASES.md"), PathBuf::from("docs/releases.html")),
        ]
    }

    #[test]
    fn guide_links_become_site_pages_and_the_rest_go_to_github() {
        let src = Path::new("docs/guide");
        let site = Path::new("docs/guide");
        let p = pages();
        assert_eq!(rewrite_link("check.md", src, site, &p), "check.html");
        assert_eq!(rewrite_link("README.md", src, site, &p), "index.html");
        assert_eq!(rewrite_link("../../RELEASES.md", src, site, &p), "../releases.html");
        assert_eq!(
            rewrite_link("../pipeline/runtime.md#deliberate-divergences-from-rails", src, site, &p),
            "https://github.com/rubys/roundhouse/blob/main/docs/pipeline/runtime.md#deliberate-divergences-from-rails"
        );
        assert_eq!(
            rewrite_link("../../editors/vscode/", src, site, &p),
            "https://github.com/rubys/roundhouse/tree/main/editors/vscode"
        );
        assert_eq!(rewrite_link("#two-modes", src, site, &p), "#two-modes");
        assert_eq!(rewrite_link("https://x.test/a", src, site, &p), "https://x.test/a");
    }

    #[test]
    fn releases_links_back_into_the_guide() {
        let p = pages();
        assert_eq!(
            rewrite_link("docs/guide/README.md", Path::new(""), Path::new("docs"), &p),
            "guide/index.html"
        );
    }

    #[test]
    fn the_title_moves_to_the_header_and_tables_render() {
        let md = "# Install\n\nText [here](check.md).\n\n| a | b |\n|---|---|\n| 1 | 2 |\n";
        let out = render_page(md, Path::new("docs/guide/install.md"), Path::new("docs/guide/install.html"), &pages());
        assert!(out.contains("<title>Roundhouse — Install</title>"), "{out}");
        assert!(!out.contains("<h1>Install</h1>"), "{out}");
        assert!(out.contains("<a href=\"check.html\">here</a>"), "{out}");
        assert!(out.contains("<table>"), "{out}");
        assert!(out.contains("href=\"../../style.css\""), "{out}");
    }

    #[test]
    fn every_checked_in_page_renders() {
        // The real pages, so a link that resolves nowhere is caught here
        // rather than on the live site.
        let p = super::pages().expect("docs/guide exists");
        assert!(p.len() >= 12, "{p:?}");
        for (src, site) in &p {
            let md = fs::read_to_string(src).unwrap();
            let out = render_page(&md, src, site, &p);
            assert!(out.contains("<title>Roundhouse — "), "{}", src.display());
            // Exactly one GitHub link into docs/guide/: the footnote's
            // "this page in the repository". A second one is a guide
            // link that failed to resolve to its rendered page.
            let github_guide = out.matches("href=\"https://github.com/rubys/roundhouse/blob/main/docs/guide/").count();
            let own_footnote = usize::from(src.starts_with("docs/guide"));
            assert_eq!(github_guide, own_footnote, "{} links to a guide page by GitHub URL", src.display());
        }
    }
}
