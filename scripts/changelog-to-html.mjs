import { readFile, writeFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
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

function headingId(value) {
  return value
    .toLowerCase()
    .replaceAll("&", " and ")
    .normalize("NFKD")
    .replace(/[\u0300-\u036f]/g, "")
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-|-$/g, "");
}

function headingMarkup(tag, value, id) {
  return `<${tag} id="${id}">${inlineMarkdown(value)}<a class="heading-anchor" href="#${id}" aria-label="Copy link to this section"><svg viewBox="0 0 24 24" aria-hidden="true" focusable="false"><path d="M8 7h3V5H8a4 4 0 1 0 0 8h3v-2H8a2 2 0 1 1 0-4Zm5 0h3a2 2 0 1 1 0 4h-3v2h3a4 4 0 1 0 0-8h-3v2Zm-5 1v2h8V8H8Z"></path></svg></a></${tag}>`;
}

function renderChangelog(markdown) {
  const lines = markdown.trim().split("\n");
  const output = [];
  let listOpen = false;
  let releaseId;

  for (const line of lines) {
    if (line.startsWith("# ")) continue;
    if (line.startsWith("## ")) {
      if (listOpen) output.push("</ul>");
      listOpen = false;
      releaseId = headingId(line.slice(3));
      output.push(headingMarkup("h2", line.slice(3), releaseId));
      continue;
    }
    if (line.startsWith("### ")) {
      if (listOpen) output.push("</ul>");
      listOpen = false;
      output.push(
        headingMarkup(
          "h3",
          line.slice(4),
          `${releaseId ?? "section"}-${headingId(line.slice(4))}`,
        ),
      );
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

function renderReleaseNav(markdown) {
  return markdown
    .trim()
    .split("\n")
    .filter((line) => line.startsWith("## "))
    .map((line) => {
      const title = line.slice(3);
      const id = headingId(title);
      return `<li><a href="/docs/changelog.html#${id}">${inlineMarkdown(title)}</a></li>`;
    })
    .join("\n             ");
}

function latestVersion(markdown) {
  return markdown.match(/^## (?!Unreleased\b)(\S+)/m)?.[1];
}

const out = process.argv[2] ?? "landing/docs/changelog.html";
const page = template.replace(
  "<!-- CHANGELOG_BODY -->",
  renderChangelog(changelog),
);
const pageWithNav = page.replaceAll("<!-- CHANGELOG_NAV -->", renderReleaseNav(changelog));
await writeFile(join(root, out), pageWithNav + (pageWithNav.endsWith("\n") ? "" : "\n"));

const landingDocsSuffix = /\/docs\/changelog\.html$/;
if (landingDocsSuffix.test(out)) {
  const version = latestVersion(changelog);
  if (version) {
    const indexPath = join(root, out.replace(landingDocsSuffix, "/index.html"));
    const index = await readFile(indexPath, "utf8");
    await writeFile(
      indexPath,
      index.replace("<!-- LATEST_VERSION -->", `v${escapeHtml(version)}`),
    );
  }
}
