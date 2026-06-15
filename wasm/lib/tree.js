// Generic, dependency-free file-tree widget shared by both surfaces. The
// playground uses it twice (the source sidebar + the output-file picker
// popover); studio uses it for its source sidebar. Pure DOM — no Monaco, no
// framework — so it loads anywhere the editor abstraction does.
//
// renderTree(container, paths, opts) draws a collapsible tree from a flat list
// of slash-delimited paths. opts: { isOpen(dir)->bool, toggleDir(dir)->void,
// isActive(path)->bool, onPick(path)->void }. A folder toggle re-renders the
// same container in place (the caller's isOpen/toggleDir own the open state, so
// the widget itself is stateless across redraws).

// Build a nested {dirs: Map<name,node>, files: [{name,path}]} tree from the
// flat, slash-delimited paths.
export function buildTree(paths) {
  const root = { dirs: new Map(), files: [] };
  for (const path of paths) {
    const parts = path.split("/");
    let node = root;
    for (let i = 0; i < parts.length - 1; i++) {
      if (!node.dirs.has(parts[i])) node.dirs.set(parts[i], { dirs: new Map(), files: [] });
      node = node.dirs.get(parts[i]);
    }
    node.files.push({ name: parts[parts.length - 1], path });
  }
  return root;
}

// Every interior directory path (e.g. "app", "app/views", "app/views/articles").
export function allDirPaths(paths) {
  const dirs = new Set();
  for (const path of paths) {
    const parts = path.split("/");
    for (let i = 1; i < parts.length; i++) dirs.add(parts.slice(0, i).join("/"));
  }
  return dirs;
}

export function renderTree(container, paths, opts) {
  container.innerHTML = "";
  container.appendChild(treeLevel(buildTree(paths), "", opts, () => renderTree(container, paths, opts)));
}

function treeLevel(node, prefix, opts, redraw) {
  const ul = document.createElement("ul");
  ul.className = "tree";
  for (const [name, child] of [...node.dirs.entries()].sort((a, b) => a[0].localeCompare(b[0]))) {
    const dirPath = prefix ? `${prefix}/${name}` : name;
    const open = opts.isOpen(dirPath);
    const li = document.createElement("li");
    const btn = document.createElement("button");
    btn.className = "folder";
    btn.innerHTML = `<span class="tw">${open ? "▾" : "▸"}</span>`;
    btn.append(`${name}/`);
    btn.onclick = () => { opts.toggleDir(dirPath); redraw(); };
    li.appendChild(btn);
    if (open) li.appendChild(treeLevel(child, dirPath, opts, redraw));
    ul.appendChild(li);
  }
  for (const f of node.files.sort((a, b) => a.name.localeCompare(b.name))) {
    const li = document.createElement("li");
    const btn = document.createElement("button");
    btn.className = "file";
    btn.textContent = f.name;
    btn.title = f.path;
    btn.dataset.path = f.path;
    btn.classList.toggle("active", opts.isActive(f.path));
    btn.onclick = () => opts.onPick(f.path);
    li.appendChild(btn);
    ul.appendChild(li);
  }
  return ul;
}
