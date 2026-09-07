import { mkdir, readFile, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const changelog = await readFile(join(root, "CHANGELOG.md"), "utf8");
const template = await readFile(
  join(root, "landing/docs/changelog.template.html"),
  "utf8",
);

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

function latestVersion(markdown) {
  return markdown.match(/^## (?!Unreleased\b)(\S+)/m)?.[1];
}

const out = process.argv[2] ?? "dist/changelog.html";
const outputPath = resolve(root, out);
await mkdir(dirname(outputPath), { recursive: true });
const page = template.replace(
  "<!-- CHANGELOG_BODY -->",
  renderChangelog(changelog),
);
await writeFile(outputPath, page + (page.endsWith("\n") ? "" : "\n"));

const landingDocsSuffix = /\/docs\/changelog\.html$/;
if (landingDocsSuffix.test(outputPath)) {
  const version = latestVersion(changelog);
  if (version) {
    const indexPath = outputPath.replace(landingDocsSuffix, "/index.html");
    const index = await readFile(indexPath, "utf8");
    await writeFile(
      indexPath,
      index.replace("<!-- LATEST_VERSION -->", `v${escapeHtml(version)}`),
    );
  }
}
