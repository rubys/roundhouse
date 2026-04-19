// Roundhouse output browser. Loads one manifest per language from
// this directory and renders a tabbed file tree + code viewer.
(function () {
  const LANGS = ["ruby", "crystal", "elixir", "go", "python", "rust", "typescript"];
  const manifests = {};
  let currentLang = "ruby";
  let currentFile = null;

  const treeEl = document.getElementById("file-tree");
  const codeEl = document.getElementById("code-content");
  const headerEl = document.getElementById("code-header");
  const tabs = document.querySelectorAll(".tab");

  async function loadManifests() {
    treeEl.innerHTML = '<div class="loading">Loading...</div>';
    await Promise.all(
      LANGS.map(async (lang) => {
        try {
          const resp = await fetch(`${lang}.json`);
          manifests[lang] = await resp.json();
        } catch {
          manifests[lang] = { language: lang, files: [] };
        }
      })
    );
    renderTree();
  }

  function buildTree(files) {
    const root = { children: {}, files: [] };
    for (const file of files) {
      const parts = file.path.split("/");
      let node = root;
      for (let i = 0; i < parts.length - 1; i++) {
        if (!node.children[parts[i]]) {
          node.children[parts[i]] = { children: {}, files: [] };
        }
        node = node.children[parts[i]];
      }
      node.files.push(file);
    }
    return root;
  }

  function renderTree() {
    const manifest = manifests[currentLang];
    if (!manifest || manifest.files.length === 0) {
      treeEl.innerHTML = '<div class="loading">No files available</div>';
      return;
    }
    const tree = buildTree(manifest.files);
    treeEl.innerHTML = "";
    renderNode(tree, treeEl, 0);

    if (!currentFile && manifest.files.length > 0) {
      selectFile(manifest.files[0].path);
    } else if (currentFile) {
      const hit = manifest.files.find((f) => f.path === currentFile);
      if (hit) {
        selectFile(currentFile);
      } else {
        selectFile(manifest.files[0].path);
      }
    }
  }

  function renderNode(node, parentEl, depth) {
    const dirs = Object.keys(node.children).sort();
    for (const dirName of dirs) {
      const dirEl = document.createElement("div");

      const itemEl = document.createElement("div");
      itemEl.className = "tree-item dir";
      itemEl.style.paddingLeft = (0.75 + depth * 0.75) + "rem";
      itemEl.innerHTML = '<span class="tree-icon"></span>' + escapeHtml(dirName);

      const childrenEl = document.createElement("div");
      childrenEl.className = "tree-children";
      renderNode(node.children[dirName], childrenEl, depth + 1);

      if (depth < 2) {
        itemEl.classList.add("open");
        childrenEl.classList.add("open");
      }

      itemEl.addEventListener("click", () => {
        itemEl.classList.toggle("open");
        childrenEl.classList.toggle("open");
      });

      dirEl.appendChild(itemEl);
      dirEl.appendChild(childrenEl);
      parentEl.appendChild(dirEl);
    }

    const files = [...node.files].sort((a, b) => {
      const nameA = a.path.split("/").pop();
      const nameB = b.path.split("/").pop();
      return nameA.localeCompare(nameB);
    });

    for (const file of files) {
      const itemEl = document.createElement("div");
      itemEl.className = "tree-item file";
      itemEl.style.paddingLeft = (0.75 + depth * 0.75) + "rem";
      const fileName = file.path.split("/").pop();
      itemEl.innerHTML = '<span class="tree-icon"></span>' + escapeHtml(fileName);
      itemEl.dataset.path = file.path;

      itemEl.addEventListener("click", () => selectFile(file.path));
      parentEl.appendChild(itemEl);
    }
  }

  const EXT_TO_LANG = {
    ".rb": "ruby", ".cr": "crystal", ".py": "python", ".ts": "typescript",
    ".tsx": "typescript", ".ex": "elixir", ".exs": "elixir", ".eex": "elixir",
    ".rs": "rust", ".go": "go",
    ".js": "javascript", ".jsx": "javascript", ".json": "json", ".yml": "yaml",
    ".yaml": "yaml", ".toml": "toml", ".sql": "sql", ".html": "xml",
    ".htm": "xml", ".xml": "xml", ".erb": "erb", ".ecr": "erb",
    ".css": "css", ".scss": "scss", ".sh": "bash", ".bash": "bash",
    ".md": "markdown", ".txt": "plaintext", ".lock": "plaintext",
    ".cfg": "ini", ".ini": "ini", ".env": "bash", ".gemspec": "ruby",
    ".mod": "go", ".sum": "plaintext",
  };

  const NAME_TO_LANG = {
    "Gemfile": "ruby", "Rakefile": "ruby", "Dockerfile": "dockerfile",
    "Cargo.toml": "toml", "shard.yml": "yaml", "mix.exs": "elixir",
    "go.mod": "go", "package.json": "json", "tsconfig.json": "json",
  };

  function detectLanguage(filePath) {
    const fileName = filePath.split("/").pop();
    if (NAME_TO_LANG[fileName]) return NAME_TO_LANG[fileName];
    const ext = "." + fileName.split(".").pop().toLowerCase();
    return EXT_TO_LANG[ext] || null;
  }

  function selectFile(filePath) {
    currentFile = filePath;
    const manifest = manifests[currentLang];
    const file = manifest.files.find((f) => f.path === filePath);

    if (file) {
      headerEl.textContent = file.path;
      codeEl.textContent = file.content;
      codeEl.removeAttribute("data-highlighted");
      codeEl.className = "";

      const lang = detectLanguage(file.path);
      if (lang) {
        codeEl.classList.add(`language-${lang}`);
      }
      hljs.highlightElement(codeEl);
    }

    document.querySelectorAll(".tree-item.file").forEach((el) => {
      el.classList.toggle("active", el.dataset.path === filePath);
    });
  }

  function escapeHtml(text) {
    const div = document.createElement("div");
    div.textContent = text;
    return div.innerHTML;
  }

  tabs.forEach((tab) => {
    tab.addEventListener("click", () => {
      currentLang = tab.dataset.lang;

      document.documentElement.style.setProperty(
        "--accent",
        getComputedStyle(document.documentElement).getPropertyValue(
          `--accent-${currentLang}`
        )
      );

      tabs.forEach((t) => t.classList.remove("active"));
      tab.classList.add("active");

      document.querySelector("header h1 a").style.color =
        `var(--accent-${currentLang})`;

      renderTree();
    });
  });

  loadManifests();
})();
