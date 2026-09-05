const root = new URL("..", import.meta.url);
const changelog = await Bun.file(new URL("CHANGELOG.md", root)).text();

function escapeHtml(value) {
  return value
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}

function inlineMarkdown(value) {
  return escapeHtml(value)
    .replace(/`([^`]+)`/g, "<code>$1</code>")
    .replace(/\[([^\]]+)\]\(([^)]+)\)/g, '<a href="$2">$1</a>');
}

function renderChangelog(markdown) {
  const lines = markdown.trim().split("\n");
  const output = [];
  let listOpen = false;

  for (const line of lines) {
    if (line.startsWith("# ")) continue;
    if (line.startsWith("## ")) {
      if (listOpen) output.push("</ul>");
      listOpen = false;
      output.push(`<h2>${inlineMarkdown(line.slice(3))}</h2>`);
      continue;
    }
    if (line.startsWith("### ")) {
      if (listOpen) output.push("</ul>");
      listOpen = false;
      output.push(`<h3>${inlineMarkdown(line.slice(4))}</h3>`);
      continue;
    }
    if (line.startsWith("- ")) {
      if (!listOpen) output.push("<ul>");
      listOpen = true;
      output.push(`<li>${inlineMarkdown(line.slice(2))}</li>`);
      continue;
    }
    if (!line && listOpen) {
      output.push("</ul>");
      listOpen = false;
      continue;
    }
    if (line) output.push(`<p>${inlineMarkdown(line)}</p>`);
  }

  if (listOpen) output.push("</ul>");
  return output.join("\n          ");
}

const content = renderChangelog(changelog);
const nav = `            <li><a href="/docs/">Overview</a></li>
            <li><a href="/docs/install.html">Install</a></li>
            <li><a href="/docs/setup.html">Set up accounts</a></li>
            <li><a href="/docs/delegate.html">Hand off work</a></li>
            <li><a href="/docs/follow.html">Check on the work</a></li>
            <li><a href="/docs/worktrees.html">Branches &amp; pull requests</a></li>
            <li><a href="/docs/data.html">Your data</a></li>
            <li><a href="/docs/changelog.html" aria-current="page">Changelog</a></li>`;

const page = `<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <meta name="description" content="Changes in Oga releases and the next release.">
  <meta name="color-scheme" content="light dark">
  <title>Oga docs - Changelog</title>
  <link rel="icon" href="/logo.svg" type="image/svg+xml">
  <link rel="stylesheet" href="/styles.css">
  <link rel="stylesheet" href="/docs.css">
</head>
<body>
  <a class="skip-link" href="#main">Skip to content</a>
  <header class="site-header">
    <a class="wordmark" href="/">
      <img src="/logo.svg" alt="Oga logo" width="32" height="32">
      <span aria-hidden="true">Oga</span>
    </a>
    <a class="header-link" href="/#install">Install</a>
  </header>
  <main id="main">
    <div class="docs-shell">
      <details class="docs-sidebar-mobile">
        <summary>Docs menu</summary>
        <nav aria-label="Docs pages">
          <ul class="docs-nav-list">
${nav}
          </ul>
        </nav>
      </details>
      <nav class="docs-sidebar-desktop" aria-label="Docs pages">
        <p class="docs-nav-label">Docs</p>
        <ul class="docs-nav-list">
${nav}
        </ul>
      </nav>
      <article class="docs-content">
        <p class="docs-eyebrow">Docs / Changelog</p>
        <h1>What changed</h1>
        <p class="docs-intro">Release notes for Oga.</p>
          ${content}
      </article>
    </div>
  </main>
  <footer>
    <div class="footer-brand">
      <strong>Oga</strong>
      <span>&copy; 2026 Oga.</span>
    </div>
    <div class="footer-meta">
      <a href="/docs/">Docs</a>
      <a href="https://desgn.space">Desgn</a>
    </div>
  </footer>
</body>
</html>
`;

await Bun.write(new URL("landing/docs/changelog.html", root), page);
