//! ERB view translation for the Roda conversion (`--target roda`).
//!
//! Translates the Rails app's `app/views/**/*.html.erb` into ERB that
//! renders under Roda's `render` plugin (escape: true) with no Rails
//! helpers: `link_to`/`button_to` become anchors and method-override
//! forms, `form_with` blocks become hand-rolled `<form>` tags with
//! `_method` hidden inputs, `render` collections/partials become
//! `part(...)` calls — the same mechanics the reviewed exemplar
//! (fixtures/roda-blog) uses. ActiveModel error-API touches are
//! rewritten to their Sequel spellings (`errors.full_messages`).
//!
//! Deliberately scoped to the scaffold helper vocabulary; a `<%= %>`
//! line using an unrecognized Rails helper is carried as a
//! ROUNDHOUSE-TODO comment (never silently mistranslated), per the #67
//! conversion rule. `.json.jbuilder` views are skipped — the html-only
//! format asymmetry is part of the conversion ledger (see mod.rs).

use std::path::Path;

use crate::app::App;
use crate::dialect::HttpMethod;
use crate::lower::routes::FlatRoute;
use crate::naming;

use super::super::EmittedFile;

pub(super) struct ViewCx<'a> {
    pub app: &'a App,
    pub routes: &'a [FlatRoute],
}

/// Walk `<fixture>/app/views/`, translating each non-layout html.erb
/// into `views/<dir>/<name>.erb`.
pub(super) fn translate_views(
    app: &App,
    fixture: &Path,
    routes: &[FlatRoute],
) -> Vec<EmittedFile> {
    let cx = ViewCx { app, routes };
    let mut out = Vec::new();
    let views_root = fixture.join("app/views");
    let Ok(dirs) = std::fs::read_dir(&views_root) else { return out };
    let mut dir_names: Vec<String> = dirs
        .filter_map(|d| d.ok())
        .filter(|d| d.path().is_dir())
        .map(|d| d.file_name().to_string_lossy().to_string())
        .filter(|n| n != "layouts")
        .collect();
    dir_names.sort();
    for dir in &dir_names {
        let Ok(files) = std::fs::read_dir(views_root.join(dir)) else { continue };
        let mut names: Vec<String> = files
            .filter_map(|f| f.ok())
            .map(|f| f.file_name().to_string_lossy().to_string())
            .filter(|n| n.ends_with(".html.erb"))
            .collect();
        names.sort();
        for name in &names {
            let src = match std::fs::read_to_string(views_root.join(dir).join(name)) {
                Ok(s) => s,
                Err(_) => continue,
            };
            let stem = name.trim_end_matches(".html.erb");
            out.push(EmittedFile {
                path: format!("views/{dir}/{stem}.erb").into(),
                content: translate_erb(&src, &cx, dir),
            });
        }
    }
    out
}

// ── Line machinery ──────────────────────────────────────────────────

/// Join lines so every `<% … %>` tag is complete on one line (Rails
/// scaffold occasionally wraps a long `button_to` across lines).
fn join_tag_lines(src: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut pending: Option<String> = None;
    for line in src.lines() {
        match pending.take() {
            Some(mut p) => {
                p.push(' ');
                p.push_str(line.trim());
                if open_tag(&p) {
                    pending = Some(p);
                } else {
                    out.push(p);
                }
            }
            None => {
                if open_tag(line) {
                    pending = Some(line.to_string());
                } else {
                    out.push(line.to_string());
                }
            }
        }
    }
    if let Some(p) = pending {
        out.push(p);
    }
    out
}

/// Line has a `<%` tag that doesn't close on this line.
fn open_tag(line: &str) -> bool {
    match line.rfind("<%") {
        Some(i) => !line[i..].contains("%>"),
        None => false,
    }
}

/// Count of ERB block openers minus closers on a line — for tracking
/// where a dropped `if` block or a `form_with` block ends.
fn nesting_delta(line: &str) -> i32 {
    let mut d = 0;
    let mut rest = line;
    while let Some(i) = rest.find("<%") {
        let Some(j) = rest[i..].find("%>") else { break };
        let tag = &rest[i + 2..i + j];
        let t = tag.trim_start_matches(['=', '#', '-']).trim();
        if t.starts_with("if ") || t.starts_with("unless ") || t.starts_with("case ") {
            d += 1;
        } else if t.ends_with(" do") || t.contains(" do |") || t.ends_with("do") && t.contains(".each") {
            d += 1;
        } else if t == "end" || t.starts_with("end ") {
            d -= 1;
        }
        rest = &rest[i + j + 2..];
    }
    d
}

fn indent_of(line: &str) -> String {
    line[..line.len() - line.trim_start().len()].to_string()
}

struct FormState {
    /// Params key: `article` → `name="article[title]"`.
    name: String,
    /// Ruby expr the field values read from (`article`); None for a
    /// new-record form (no value attributes).
    value_expr: Option<String>,
    /// ERB nesting depth inside the form when it opened; the `<% end %>`
    /// that brings us back here closes the form.
    depth: i32,
}

pub(super) fn translate_erb(src: &str, cx: &ViewCx, dir: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut drop_depth: Option<i32> = None;
    let mut depth: i32 = 0;
    let mut forms: Vec<FormState> = Vec::new();

    for raw in join_tag_lines(src) {
        let line = rewrite_error_api(&raw);
        let line = rewrite_dom_id(&line);
        let trimmed = line.trim();
        let pad = indent_of(&line);
        let delta = nesting_delta(&line);

        // Inside a dropped block (`<% if notice.present? %> … <% end %>`):
        // swallow lines until the matching end.
        if let Some(d0) = drop_depth {
            depth += delta;
            if depth <= d0 {
                drop_depth = None;
            }
            continue;
        }

        // Flash paragraphs render once in the layout (exemplar shape);
        // the per-view Rails blocks would double-render them.
        if trimmed.starts_with("<% if notice.present?") || trimmed.starts_with("<% if alert.present?")
        {
            drop_depth = Some(depth);
            depth += delta;
            continue;
        }

        // Form-block close: the `end` that returns to the opening depth.
        if let Some(f) = forms.last() {
            if trimmed == "<% end %>" && depth + delta == f.depth {
                out.push(format!("{pad}</form>"));
                forms.pop();
                depth += delta;
                continue;
            }
        }
        depth += delta;

        // Per-construct transforms.
        if let Some(tag) = erb_output_tag(trimmed) {
            if tag.starts_with("turbo_stream_from") {
                out.push(format!("{pad}<%# ROUNDHOUSE-TODO: {tag} (no live-update equivalent) %>"));
                continue;
            }
            if let Some(rest) = tag.strip_prefix("render ") {
                if let Some(lines) = translate_render(rest, cx, dir, &pad) {
                    out.extend(lines);
                    continue;
                }
            }
            if let Some(rest) = tag.strip_prefix("link_to ") {
                if let Some(l) = translate_link_to(rest, cx) {
                    out.push(format!("{pad}{l}"));
                    continue;
                }
            }
            if let Some(rest) = tag.strip_prefix("button_to ") {
                if let Some(lines) = translate_button_to(rest, cx, &pad) {
                    out.extend(lines);
                    continue;
                }
            }
            if tag.starts_with("form_with") {
                // Record the depth BEFORE this line's own `do` opened
                // (depth - delta): the `<% end %>` that brings nesting
                // back to that level is the form's close, not an inner
                // if/each close.
                if let Some((lines, st)) = translate_form_with(&tag, cx, &pad, depth - delta) {
                    out.extend(lines);
                    forms.push(st);
                    continue;
                }
            }
            if let Some(rest) = tag.strip_prefix("form.") {
                if let Some(f) = forms.last() {
                    if let Some(lines) = translate_form_field(rest, f, &pad) {
                        out.extend(lines);
                        continue;
                    }
                }
            }
            // Unrecognized Rails helper output tags become TODO comments
            // rather than broken Ruby at render time.
            if is_rails_helper_tag(&tag) {
                out.push(format!("{pad}<%# ROUNDHOUSE-TODO: untranslated Rails helper: {tag} %>"));
                continue;
            }
        }
        // `<% content_for … %>` statement form (titles): layout title is
        // static in the conversion.
        if trimmed.starts_with("<% content_for") {
            let inner = trimmed.trim_start_matches("<%").trim_end_matches("%>").trim();
            out.push(format!("{pad}<%# ROUNDHOUSE-TODO: {inner} (layout title is static) %>"));
            continue;
        }
        out.push(line.trim_end().to_string());
    }
    // Collapse runs of blank lines the dropped blocks leave behind.
    let mut cleaned: Vec<String> = Vec::new();
    for l in out {
        if l.trim().is_empty() && cleaned.last().is_some_and(|p: &String| p.trim().is_empty()) {
            continue;
        }
        cleaned.push(l);
    }
    let mut s = cleaned.join("\n");
    if !s.ends_with('\n') {
        s.push('\n');
    }
    s
}

/// The inner expression of a single `<%= … %>` line, when the line is
/// exactly one output tag.
fn erb_output_tag(trimmed: &str) -> Option<String> {
    let rest = trimmed.strip_prefix("<%=")?;
    let inner = rest.strip_suffix("%>")?;
    if inner.contains("<%") {
        return None;
    }
    Some(inner.trim().to_string())
}

fn is_rails_helper_tag(tag: &str) -> bool {
    [
        "link_to", "button_to", "form_with", "render ", "turbo_", "csrf_", "javascript_",
        "stylesheet_", "image_tag", "form_for",
    ]
    .iter()
    .any(|h| tag.starts_with(h))
}

// ── Small rewrites ──────────────────────────────────────────────────

/// ActiveModel error API → Sequel spellings.
fn rewrite_error_api(line: &str) -> String {
    line.replace(".errors.count", ".errors.full_messages.size")
        .replace(".errors.each do |error|", ".errors.full_messages.each do |message|")
        .replace("error.full_message", "message")
}

/// `<%= dom_id(article) %>` → `article_<%= article.id %>`;
/// `<%= dom_id(article, :comments_count) %>` →
/// `comments_count_article_<%= article.id %>`. (Rails' dom_id shape,
/// without ActionView.)
fn rewrite_dom_id(line: &str) -> String {
    let mut out = String::new();
    let mut rest = line;
    while let Some(i) = rest.find("<%= dom_id(") {
        let after = &rest[i + "<%= dom_id(".len()..];
        let Some(close) = after.find(") %>") else { break };
        let args = &after[..close];
        out.push_str(&rest[..i]);
        let parts = split_args(args);
        let record = parts[0].trim();
        let base = record.rsplit('.').next().unwrap_or(record).trim_start_matches('@');
        match parts.get(1) {
            Some(prefix) => {
                let prefix = prefix.trim().trim_start_matches(':');
                out.push_str(&format!("{prefix}_{base}_<%= {record}.id %>"));
            }
            None => out.push_str(&format!("{base}_<%= {record}.id %>")),
        }
        rest = &after[close + ") %>".len()..];
    }
    out.push_str(rest);
    out
}

// ── Argument parsing ────────────────────────────────────────────────

/// Split a Ruby argument list on top-level commas (tracks (), [], {},
/// and both quote kinds — enough for scaffold-shaped view code).
fn split_args(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut in_dq = false;
    let mut in_sq = false;
    let mut cur = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if !in_sq => in_dq = !in_dq,
            '\'' if !in_dq => in_sq = !in_sq,
            '(' | '[' | '{' if !in_dq && !in_sq => depth += 1,
            ')' | ']' | '}' if !in_dq && !in_sq => depth -= 1,
            ',' if depth == 0 && !in_dq && !in_sq => {
                out.push(cur.trim().to_string());
                cur.clear();
                continue;
            }
            _ => {}
        }
        cur.push(c);
        let _ = chars.peek();
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

/// `class: "x"` style keyword args from a split arg list. Returns
/// (positional, kwargs).
fn partition_kwargs(args: Vec<String>) -> (Vec<String>, Vec<(String, String)>) {
    let mut pos = Vec::new();
    let mut kw = Vec::new();
    for a in args {
        if let Some(i) = a.find(':') {
            let key = &a[..i];
            if !key.is_empty()
                && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                && a[i + 1..].starts_with(' ')
            {
                kw.push((key.to_string(), a[i + 1..].trim().to_string()));
                continue;
            }
        }
        pos.push(a);
    }
    (pos, kw)
}

fn kwarg<'a>(kw: &'a [(String, String)], key: &str) -> Option<&'a str> {
    kw.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

fn unquote(s: &str) -> Option<&str> {
    s.strip_prefix('"')?.strip_suffix('"')
}

/// A `class:` value as an HTML attribute string. Array-with-conditional
/// classes (scaffold's error-highlight idiom) reduce to the first
/// static string.
fn class_attr(kw: &[(String, String)]) -> String {
    match kwarg(kw, "class") {
        Some(v) => {
            if let Some(q) = unquote(v) {
                format!(" class=\"{q}\"")
            } else if v.starts_with('[') {
                // ["static classes", { conditional => bool }] — keep the
                // static part.
                match v.find('"').and_then(|i| v[i + 1..].find('"').map(|j| &v[i + 1..i + 1 + j]))
                {
                    Some(stat) => format!(" class=\"{stat}\""),
                    None => String::new(),
                }
            } else {
                String::new()
            }
        }
        None => String::new(),
    }
}

// ── Path resolution ─────────────────────────────────────────────────

fn named_route<'a>(cx: &ViewCx<'a>, as_name: &str) -> Option<&'a FlatRoute> {
    cx.routes
        .iter()
        .find(|r| r.named && r.as_name == as_name && r.method == HttpMethod::Get)
        .or_else(|| cx.routes.iter().find(|r| r.named && r.as_name == as_name))
}

/// Substitute `:params` in a route path with per-argument `<%= arg.id %>`
/// interpolations, in order.
fn href_with_args(path: &str, args: &[&str]) -> String {
    let mut idx = 0usize;
    path.split('/')
        .map(|seg| {
            if seg.starts_with(':') {
                let a = args.get(idx).copied().unwrap_or("");
                idx += 1;
                if a.ends_with(".id") {
                    format!("<%= {a} %>")
                } else {
                    format!("<%= {a}.id %>")
                }
            } else {
                seg.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// Resolve a link/button target expression to an href (with embedded
/// ERB interpolations). `verb` selects among routes when the target is
/// a model/array (GET show vs DELETE destroy).
fn target_href(target: &str, cx: &ViewCx, verb: HttpMethod) -> Option<String> {
    let t = target.trim();
    // `[comment.article, comment]` — nested resource route by verb +
    // param count.
    if let Some(inner) = t.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
        let elems = split_args(inner);
        let refs: Vec<&str> = elems.iter().map(|e| e.as_str()).collect();
        let matches: Vec<&FlatRoute> = cx
            .routes
            .iter()
            .filter(|r| r.method == verb && r.path_params.len() == refs.len())
            .collect();
        let route = match matches.as_slice() {
            [one] => one,
            _ => return None,
        };
        return Some(href_with_args(&route.path, &refs));
    }
    // `edit_article_path(article)` / `articles_path`
    if let Some(i) = t.find("_path") {
        let stem = &t[..i];
        let rest = t[i + "_path".len()..].trim();
        let args: Vec<String> = if rest.is_empty() {
            vec![]
        } else {
            let inner = rest.strip_prefix('(')?.strip_suffix(')')?;
            split_args(inner)
        };
        let route = named_route(cx, stem)?;
        let refs: Vec<&str> = args.iter().map(|a| a.as_str()).collect();
        return Some(href_with_args(&route.path, &refs));
    }
    // Bare model reference (`article` / `@article`): the resource's
    // member route for the verb (`show` for GET, `destroy` for DELETE).
    if t.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '@') {
        let name = t.trim_start_matches('@');
        let action = match verb {
            HttpMethod::Delete => "destroy",
            _ => "show",
        };
        let controller = format!("{}Controller", naming::camelize(&naming::pluralize_snake(name)));
        let route = cx.routes.iter().find(|r| {
            r.method == verb
                && r.action.as_str() == action
                && r.controller.0.as_str() == controller
        })?;
        return Some(href_with_args(&route.path, &[t]));
    }
    None
}

// ── Construct translations ──────────────────────────────────────────

/// `link_to TEXT, TARGET[, class: …]` → `<a href …>`.
fn translate_link_to(rest: &str, cx: &ViewCx) -> Option<String> {
    let (pos, kw) = partition_kwargs(split_args(rest));
    let [text, target] = pos.as_slice() else { return None };
    let href = target_href(target, cx, HttpMethod::Get)?;
    let text_html = match unquote(text) {
        Some(t) => t.to_string(),
        None => format!("<%= {text} %>"),
    };
    Some(format!("<a href=\"{href}\"{}>{text_html}</a>", class_attr(&kw)))
}

/// `button_to TEXT, TARGET, method: :delete[, class:, form_class:,
/// data: …]` → a method-override form (the exemplar's destroy shape).
fn translate_button_to(rest: &str, cx: &ViewCx, pad: &str) -> Option<Vec<String>> {
    let (pos, kw) = partition_kwargs(split_args(rest));
    let [text, target] = pos.as_slice() else { return None };
    let method = kwarg(&kw, "method").map(|m| m.trim_start_matches(':')).unwrap_or("post");
    let verb = match method {
        "delete" => HttpMethod::Delete,
        "patch" => HttpMethod::Patch,
        "put" => HttpMethod::Put,
        _ => HttpMethod::Post,
    };
    let href = target_href(target, cx, verb)?;
    let text_html = match unquote(text) {
        Some(t) => t.to_string(),
        None => format!("<%= {text} %>"),
    };
    let form_class = match kwarg(&kw, "form_class").and_then(unquote) {
        Some(fc) => format!("inline {fc}"),
        None => "inline".to_string(),
    };
    let mut lines = vec![format!(
        "{pad}<form class=\"{form_class}\" method=\"post\" action=\"{href}\">"
    )];
    if method != "post" {
        lines.push(format!(
            "{pad}  <input type=\"hidden\" name=\"_method\" value=\"{method}\">"
        ));
    }
    lines.push(format!(
        "{pad}  <button type=\"submit\"{}>{text_html}</button>",
        class_attr(&kw)
    ));
    lines.push(format!("{pad}</form>"));
    if let Some(d) = kwarg(&kw, "data") {
        lines.push(format!(
            "{pad}<%# ROUNDHOUSE-TODO: button_to data: {d} dropped (Turbo attribute) %>"
        ));
    }
    Some(lines)
}

/// `render "form", article: @article` → `part(...)`;
/// `render @articles` / `render @article.comments` → each + part loop.
fn translate_render(rest: &str, cx: &ViewCx, dir: &str, pad: &str) -> Option<Vec<String>> {
    let (pos, kw) = partition_kwargs(split_args(rest));
    let first = pos.first()?;
    if let Some(name) = unquote(first) {
        // Partial in the current view dir, locals passed through.
        let locals = kw
            .iter()
            .map(|(k, v)| format!("{k}: {v}"))
            .collect::<Vec<_>>()
            .join(", ");
        let sep = if locals.is_empty() { String::new() } else { format!(", {locals}") };
        return Some(vec![format!("{pad}<%== part(\"{dir}/_{name}\"{sep}) %>")]);
    }
    // Collection render: the partial comes from the collection's
    // (plural) name, each element bound to the singular.
    let coll = first.trim();
    let plural = coll.rsplit('.').next()?.trim_start_matches('@');
    let singular = naming::singularize(plural);
    if !cx.app.models.iter().any(|m| naming::snake_case(m.name.0.as_str()) == singular) {
        return None;
    }
    Some(vec![
        format!("{pad}<% {coll}.each do |{singular}| %>"),
        format!("{pad}  <%== part(\"{plural}/_{singular}\", {singular}: {singular}) %>"),
        format!("{pad}<% end %>"),
    ])
}

/// `form_with(model: article, class: "contents") do |form|` → a
/// hand-rolled `<form>` with persisted-conditional action/method
/// (exemplar mechanics). Returns the opening lines + the state the
/// field translations need.
fn translate_form_with(
    tag: &str,
    cx: &ViewCx,
    pad: &str,
    depth: i32,
) -> Option<(Vec<String>, FormState)> {
    // Strip `form_with`, optional parens, and the trailing `do |form|`.
    let rest = tag.strip_prefix("form_with")?.trim();
    let rest = rest.strip_suffix("|")?.trim_end();
    let (rest, _blockvar) = rest.rsplit_once("do |")?;
    let rest = rest.trim().trim_start_matches('(');
    let rest = rest.strip_suffix(')').unwrap_or(rest).trim_end_matches(',').trim();
    let (_, kw) = partition_kwargs(split_args(rest));
    let model = kwarg(&kw, "model")?;
    let class = class_attr(&kw);

    // Nested `[@article, Comment.new]` → the child resource's create
    // route under the parent.
    if let Some(inner) = model.trim().strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
        let elems = split_args(inner);
        let [parent, child] = elems.as_slice() else { return None };
        let child_const = child.trim().strip_suffix(".new")?;
        let child_snake = naming::snake_case(child_const);
        let controller = format!(
            "{}Controller",
            naming::camelize(&naming::pluralize_snake(&child_snake))
        );
        let route = cx.routes.iter().find(|r| {
            r.method == HttpMethod::Post
                && r.action.as_str() == "create"
                && r.controller.0.as_str() == controller
        })?;
        let action = href_with_args(&route.path, &[parent.as_str()]);
        let lines = vec![format!(
            "{pad}<form method=\"post\" action=\"{action}\"{class}>"
        )];
        return Some((
            lines,
            FormState { name: child_snake, value_expr: None, depth },
        ));
    }

    // Single model (`article`): POST to the collection for new records,
    // method-override PATCH to the member for persisted ones.
    let expr = model.trim();
    let name = expr.rsplit('.').next()?.trim_start_matches('@').to_string();
    let plural = naming::pluralize_snake(&name);
    let controller = format!("{}Controller", naming::camelize(&plural));
    let create = cx.routes.iter().find(|r| {
        r.method == HttpMethod::Post
            && r.action.as_str() == "create"
            && r.controller.0.as_str() == controller
    })?;
    let update_path = format!("{}/#{{{expr}.id}}", create.path);
    let lines = vec![
        format!(
            "{pad}<form method=\"post\" action=\"<%= {expr}.id ? \"{update_path}\" : \"{}\" %>\"{class}>",
            create.path
        ),
        format!("{pad}  <% if {expr}.id %>"),
        format!("{pad}    <input type=\"hidden\" name=\"_method\" value=\"patch\">"),
        format!("{pad}  <% end %>"),
    ];
    Some((
        lines,
        FormState { name, value_expr: Some(expr.to_string()), depth },
    ))
}

/// `form.label/:text_field/:textarea/:submit` → plain HTML controls.
fn translate_form_field(rest: &str, f: &FormState, pad: &str) -> Option<Vec<String>> {
    let (method, args) = match rest.find(|c: char| c == ' ' || c == '(') {
        Some(i) => (&rest[..i], rest[i..].trim().trim_start_matches('(').trim_end_matches(')')),
        None => (rest, ""),
    };
    let (pos, kw) = partition_kwargs(split_args(args));
    let name = &f.name;
    match method {
        "label" => {
            let attr = pos.first()?.trim_start_matches(':');
            let text = pos.get(1).and_then(|t| unquote(t)).map(|t| t.to_string())
                .unwrap_or_else(|| humanize(attr));
            Some(vec![format!(
                "{pad}<label for=\"{name}_{attr}\"{}>{text}</label>",
                class_attr(&kw)
            )])
        }
        "text_field" => {
            let attr = pos.first()?.trim_start_matches(':');
            let value = match &f.value_expr {
                Some(e) => format!(" value=\"<%= {e}.{attr} %>\""),
                None => String::new(),
            };
            Some(vec![format!(
                "{pad}<input type=\"text\" name=\"{name}[{attr}]\" id=\"{name}_{attr}\"{value}{}>",
                class_attr(&kw)
            )])
        }
        "textarea" | "text_area" => {
            let attr = pos.first()?.trim_start_matches(':');
            let rows = kwarg(&kw, "rows").map(|r| format!(" rows=\"{r}\"")).unwrap_or_default();
            let value = match &f.value_expr {
                Some(e) => format!("<%= {e}.{attr} %>"),
                None => String::new(),
            };
            Some(vec![format!(
                "{pad}<textarea name=\"{name}[{attr}]\" id=\"{name}_{attr}\"{rows}{}>{value}</textarea>",
                class_attr(&kw)
            )])
        }
        "submit" => {
            let text = pos.first().and_then(|t| unquote(t)).map(|t| t.to_string())
                .unwrap_or_else(|| format!("Save {}", humanize(name)));
            Some(vec![format!(
                "{pad}<button type=\"submit\"{}>{text}</button>",
                class_attr(&kw)
            )])
        }
        _ => None,
    }
}

/// `comments_count` → `Comments count`; `title` → `Title`.
fn humanize(attr: &str) -> String {
    let mut s = attr.replace('_', " ");
    if let Some(first) = s.get(..1) {
        let up = first.to_uppercase();
        s.replace_range(..1, &up);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_args_respects_nesting() {
        assert_eq!(
            split_args(r#""Destroy", article, method: :delete, data: { turbo_confirm: "Sure?" }"#),
            vec![
                r#""Destroy""#,
                "article",
                "method: :delete",
                r#"data: { turbo_confirm: "Sure?" }"#
            ]
        );
        assert_eq!(
            split_args(r#"article.title, article, class: "a, b""#),
            vec!["article.title", "article", r#"class: "a, b""#]
        );
    }

    #[test]
    fn dom_id_rewrites() {
        assert_eq!(
            rewrite_dom_id(r#"<div id="<%= dom_id(article) %>">"#),
            r#"<div id="article_<%= article.id %>">"#
        );
        assert_eq!(
            rewrite_dom_id(r#"<span id="<%= dom_id(article, :comments_count) %>">"#),
            r#"<span id="comments_count_article_<%= article.id %>">"#
        );
    }

    #[test]
    fn error_api_rewrites() {
        assert_eq!(
            rewrite_error_api("<h2><%= pluralize(article.errors.count, \"error\") %></h2>"),
            "<h2><%= pluralize(article.errors.full_messages.size, \"error\") %></h2>"
        );
        assert_eq!(
            rewrite_error_api("<% article.errors.each do |error| %>"),
            "<% article.errors.full_messages.each do |message| %>"
        );
        assert_eq!(rewrite_error_api("<li><%= error.full_message %></li>"), "<li><%= message %></li>");
    }
}
