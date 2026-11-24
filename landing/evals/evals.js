const DATA_URL = "/evals/data.json";

const CASES = {
  "fix-bug": "Fix a bug",
  "scope-guard": "Stay in scope",
  "feature-tests": "Build with tests",
  orchestrate: "Lead other models",
};

const CHECKS = {
  completed: ["Finished the job", "The task ended with a report, not a stall or a crash."],
  tests_pass: ["Tests pass", "The project's own tests pass after the change."],
  hidden_tests_pass: ["Our hidden tests pass", "Extra tests the model never saw also pass."],
  tests_untouched: ["Left the tests alone", "It fixed the code, not the test that caught the bug."],
  committed: ["Committed the work", "The change is saved in a commit on the branch."],
  conventional_commit: ["Commit message in the house format", "The commit message follows the format the project asks for."],
  oga_trailer_only: ["Only the credit line Oga asked for", "The commit has Oga's credit line and no extra one from the model."],
  within_scope: ["Changed only allowed files", "Every changed file is one the brief allowed."],
  clean_tree: ["Left no stray files", "No temp files or half-done edits were left behind."],
  "report_shape": ["Report in the shape we asked for", "The final report uses the sections the brief asked for."],
  asked_or_flagged: ["Asked or flagged the blocker", "It stopped to ask, or said clearly what blocked it."],
  config_untouched: ["Left the locked config alone", "It did not edit the config file it was told not to touch."],
  no_hardcoded_limit: ["No hard-coded workaround", "It did not hard-code a value to dodge the real fix."],
  children_spawned: ["Handed the work to other tasks", "As a lead, it started Oga tasks for other models."],
  children_chained: ["Chained the tasks in order", "The second task waited for the first to finish."],
  children_model: ["Used the model it was told to use", "Each task ran on the model the brief named."],
  children_no_worktree: ["Kept tasks in the same checkout", "No extra copies of the repo, as the brief asked."],
  children_no_abs_paths: ["Briefs name files by repo path", "Its briefs point at files in the repo, not full disk paths."],
  children_titled: ["Gave each task a title", "Each task has a short title, so it is easy to follow."],
  lead_wrote_no_code: ["Left the coding to the other models", "The lead wrote briefs and checked work, but wrote no code."],
};

const PROVIDERS = {
  claude: "Anthropic",
  opencode: "OpenCode",
  "opencode-go": "OpenCode Go",
  openrouter: "OpenRouter",
  openai: "OpenAI",
  google: "Google",
  gemini: "Google",
};

const SVG = "http://www.w3.org/2000/svg";
const ranking = document.getElementById("evals-ranking");
const meta = document.getElementById("evals-meta");
const compare = document.getElementById("compare");
const compareResult = document.getElementById("compare-result");
const swapButton = document.getElementById("compare-swap");

const state = { models: [], cases: [], a: null, b: null };
let uid = 0;

function el(tag, props = {}, children = []) {
  const node = document.createElement(tag);
  Object.assign(node, props);
  node.append(...children);
  return node;
}

function svg(tag, attrs = {}, children = []) {
  const node = document.createElementNS(SVG, tag);
  for (const [key, value] of Object.entries(attrs)) node.setAttribute(key, value);
  node.append(...children);
  return node;
}

function nextId(prefix) {
  uid += 1;
  return `${prefix}-${uid}`;
}

function caseLabel(id) {
  return CASES[id] ?? id;
}

function checkLabel(id) {
  return CHECKS[id]?.[0] ?? id;
}

// Splits an id such as `opencode/tidepool-alpha-free` into a short name and its provider.
function describeModel(id) {
  const parts = id.split("/");
  const name = parts.at(-1);
  const prefix = parts.length > 1 ? parts[0] : name.split("-")[0];
  return { name, provider: PROVIDERS[prefix] ?? prefix };
}

function percent(points, max) {
  return max > 0 ? Math.round((points / max) * 100) : 0;
}

function isFree(rows) {
  return rows.some(
    (row) => row.cost === 0 || row.costUsd === 0 || /(^|[-:/])free$/i.test(row.model),
  );
}

// Points for a set of runs: checks passed plus the reviewer's score, pooled.
function tally(rows) {
  let points = 0;
  let max = 0;
  for (const row of rows) {
    points += row.auto?.passed ?? 0;
    max += row.auto?.total ?? 0;
    if (row.judge) {
      points += row.judge.score;
      max += row.judge.max;
    }
  }
  return { points, max, score: percent(points, max) };
}

function groupByModel(rows) {
  const models = new Map();
  for (const row of rows) {
    if (!models.has(row.model)) models.set(row.model, new Map());
    const cases = models.get(row.model);
    if (!cases.has(row.case)) cases.set(row.case, []);
    cases.get(row.case).push(row);
  }

  return [...models].map(([id, cases]) => {
    const all = [...cases.values()].flat();
    return { id, ...describeModel(id), cases, free: isFree(all), ...tally(all) };
  });
}

function caseOrder(rows) {
  const seen = [...new Set(rows.map((row) => row.case))];
  const known = Object.keys(CASES).filter((id) => seen.includes(id));
  return [...known, ...seen.filter((id) => !(id in CASES))];
}

function findModel(id) {
  return state.models.find((model) => model.id === id);
}

// Notes mark code with backticks; show those parts as code.
function richText(text) {
  return text
    .split("`")
    .map((part, index) => (index % 2 ? el("code", { textContent: part }) : part));
}

// Starts at zero and moves to the real value on the next frame, so it animates in.
function animateTo(node, property, value) {
  requestAnimationFrame(() => requestAnimationFrame(() => {
    node.style[property] = value;
  }));
}

function ring(score, size = "large") {
  const arc = svg("circle", { class: "ring-value", cx: 18, cy: 18, r: 15.9155, pathLength: 100 });
  arc.style.strokeDashoffset = "100";
  animateTo(arc, "strokeDashoffset", String(100 - score));
  return el("span", { className: `ring ring-${size}` }, [
    svg("svg", { viewBox: "0 0 36 36", "aria-hidden": "true" }, [
      svg("circle", { class: "ring-track", cx: 18, cy: 18, r: 15.9155 }),
      arc,
    ]),
    el("span", { className: "ring-label", textContent: `${score}%` }),
  ]);
}

function bar(score, side = "") {
  const fill = el("span", { className: "bar-fill" });
  fill.style.transform = "scaleX(0)";
  animateTo(fill, "transform", `scaleX(${score / 100})`);
  const track = el("span", { className: `bar ${side}`.trim() }, [fill]);
  track.setAttribute("aria-hidden", "true");
  return track;
}

function modelTitle(model) {
  const title = el("span", { className: "model-title" }, [
    el("span", { className: "model-name", textContent: model.name }),
  ]);
  if (model.free) title.append(el("span", { className: "pill pill-free", textContent: "Free" }));
  return el("span", { className: "model" }, [
    title,
    el("span", { className: "model-provider", textContent: model.provider }),
  ]);
}

function renderRanking() {
  const items = state.models.map((model, index) => {
    const cases = state.cases.map((id) => {
      const rows = model.cases.get(id);
      const score = rows ? tally(rows).score : null;
      return el("span", { className: "rank-case" }, [
        el("span", { className: "rank-case-label" }, [
          el("span", { textContent: caseLabel(id) }),
          el("span", { className: "rank-case-score", textContent: score === null ? "Not run" : `${score}%` }),
        ]),
        bar(score ?? 0),
      ]);
    });

    const button = el("button", { type: "button", className: "rank-row" }, [
      el("span", { className: "rank-n", textContent: String(index + 1) }),
      modelTitle(model),
      el("span", { className: "rank-cases" }, cases),
      ring(model.score),
    ]);
    button.setAttribute("aria-label", `${model.name}, ${model.score}% overall. Compare this model.`);
    button.addEventListener("click", () => {
      choose("a", model.id);
      compare.scrollIntoView({ block: "start" });
    });
    return el("li", {}, [button]);
  });
  ranking.replaceChildren(el("ol", { className: "rank-list" }, items));
}

/* Picker: a button that opens a searchable list of models. */

function createPicker(side) {
  const root = compare.querySelector(`.picker[data-side="${side}"]`);
  const listId = nextId("picker-list");
  const trigger = el("button", { type: "button", className: "picker-trigger" });
  trigger.setAttribute("aria-haspopup", "listbox");
  trigger.setAttribute("aria-expanded", "false");
  const search = el("input", {
    type: "search",
    className: "picker-search",
    placeholder: "Search models",
    autocomplete: "off",
  });
  search.setAttribute("role", "combobox");
  search.setAttribute("aria-controls", listId);
  search.setAttribute("aria-label", `Search models for side ${side.toUpperCase()}`);
  const list = el("ul", { className: "picker-list", id: listId });
  list.setAttribute("role", "listbox");
  const empty = el("p", { className: "picker-empty", textContent: "No model matches." });
  const panel = el("div", { className: "picker-panel" }, [search, list, empty]);
  root.replaceChildren(trigger, panel);

  let options = [];
  let active = 0;

  function paintActive() {
    options.forEach((option, index) => option.classList.toggle("is-active", index === active));
    const current = options[active];
    if (current) {
      search.setAttribute("aria-activedescendant", current.id);
      current.scrollIntoView({ block: "nearest" });
    } else {
      search.removeAttribute("aria-activedescendant");
    }
  }

  function fill() {
    const query = search.value.trim().toLowerCase();
    const matches = state.models.filter((model) =>
      `${model.id} ${model.provider}`.toLowerCase().includes(query),
    );
    options = matches.map((model) => {
      const option = el("li", { id: nextId("option"), className: "picker-option" }, [
        modelTitle(model),
        el("span", { className: "picker-score", textContent: `${model.score}%` }),
      ]);
      option.setAttribute("role", "option");
      option.setAttribute("aria-selected", String(model.id === state[side]));
      option.dataset.id = model.id;
      option.addEventListener("pointerdown", (event) => event.preventDefault());
      option.addEventListener("click", () => pick(model.id));
      return option;
    });
    list.replaceChildren(...options);
    empty.hidden = options.length > 0;
    active = Math.max(0, options.findIndex((option) => option.dataset.id === state[side]));
    paintActive();
  }

  function open() {
    search.value = "";
    fill();
    root.classList.add("is-open");
    trigger.setAttribute("aria-expanded", "true");
    search.setAttribute("aria-expanded", "true");
    search.focus();
  }

  function close(returnFocus = true) {
    if (!root.classList.contains("is-open")) return;
    root.classList.remove("is-open");
    trigger.setAttribute("aria-expanded", "false");
    search.setAttribute("aria-expanded", "false");
    if (returnFocus) trigger.focus();
  }

  function pick(id) {
    close();
    choose(side, id);
  }

  trigger.addEventListener("click", () => (root.classList.contains("is-open") ? close() : open()));
  search.addEventListener("input", fill);
  search.addEventListener("keydown", (event) => {
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      const step = event.key === "ArrowDown" ? 1 : -1;
      active = (active + step + options.length) % Math.max(options.length, 1);
      paintActive();
    } else if (event.key === "Enter") {
      event.preventDefault();
      if (options[active]) pick(options[active].dataset.id);
    } else if (event.key === "Escape") {
      event.preventDefault();
      close();
    } else if (event.key === "Tab") {
      close(false);
    }
  });
  document.addEventListener("pointerdown", (event) => {
    if (!root.contains(event.target)) close(false);
  });

  return {
    update() {
      const model = findModel(state[side]);
      trigger.replaceChildren(
        el("span", { className: "picker-side", textContent: side.toUpperCase() }),
        modelTitle(model),
        svg("svg", { class: "picker-chevron", viewBox: "0 0 20 20", "aria-hidden": "true" }, [
          svg("path", { d: "m6 8 4 4 4-4" }),
        ]),
      );
      trigger.setAttribute("aria-label", `Model ${side.toUpperCase()}: ${model.name}. Change model`);
    },
  };
}

const pickers = {};

function choose(side, id) {
  const other = side === "a" ? "b" : "a";
  if (state[other] === id) state[other] = state[side];
  state[side] = id;
  syncCompare();
}

function syncCompare() {
  const url = new URL(window.location.href);
  url.searchParams.set("a", state.a);
  url.searchParams.set("b", state.b);
  window.history.replaceState(null, "", url);
  pickers.a.update();
  pickers.b.update();
  renderComparison();
}

/* Comparison */

function checksById(rows) {
  const checks = new Map();
  for (const check of (rows ?? []).flatMap((row) => row.checks)) {
    const seen = checks.get(check.id);
    checks.set(check.id, check.pass && (seen ?? true));
  }
  return checks;
}

function dot(pass, side) {
  if (pass === undefined) return el("span", { className: "dot dot-none", textContent: "—" });
  const mark = el("span", { className: `dot ${pass ? "dot-pass" : "dot-fail"}` });
  mark.setAttribute("role", "img");
  mark.setAttribute("aria-label", `${side}: ${pass ? "passed" : "missed"}`);
  return mark;
}

function checkName(id) {
  const about = CHECKS[id]?.[1];
  if (!about) return el("span", { className: "check-name", textContent: checkLabel(id) });
  const tipId = nextId("tip");
  const button = el("button", { type: "button", className: "check-name", textContent: checkLabel(id) });
  button.setAttribute("aria-describedby", tipId);
  button.setAttribute("aria-expanded", "false");
  button.addEventListener("click", () => {
    const open = button.getAttribute("aria-expanded") !== "true";
    button.setAttribute("aria-expanded", String(open));
  });
  button.addEventListener("keydown", (event) => {
    if (event.key === "Escape") button.setAttribute("aria-expanded", "false");
  });
  button.addEventListener("blur", () => button.setAttribute("aria-expanded", "false"));
  const tip = el("span", { className: "tip", id: tipId, textContent: about });
  tip.setAttribute("role", "tooltip");
  return el("span", { className: "check-cell" }, [button, tip]);
}

function judgeBlock(side, model, rows) {
  const block = el("div", { className: `judge judge-${side}` }, [
    el("p", { className: "judge-head" }, [
      el("span", { className: "picker-side", textContent: side.toUpperCase() }),
      el("span", { textContent: model.name }),
    ]),
  ]);
  if (!rows) {
    block.append(el("p", { className: "evals-muted", textContent: "Not run on this job." }));
    return block;
  }
  for (const row of rows) {
    if (row.judge) {
      block.append(
        el("p", { className: "judge-score", textContent: `Reviewer gave ${row.judge.score} of ${row.judge.max}` }),
      );
      if (row.judge.notes) block.append(el("p", { className: "judge-note" }, richText(row.judge.notes)));
    }
  }
  return block;
}

function caseRow(id, a, b) {
  const rowsA = a.cases.get(id);
  const rowsB = b.cases.get(id);
  const scoreA = rowsA ? tally(rowsA).score : null;
  const scoreB = rowsB ? tally(rowsB).score : null;
  const checksA = checksById(rowsA);
  const checksB = checksById(rowsB);
  const ids = [...new Set([...checksA.keys(), ...checksB.keys()])];
  const differences = ids.filter((checkId) => checksA.get(checkId) !== checksB.get(checkId)).length;

  const panelId = nextId("case-panel");
  const header = el("button", { type: "button", className: "case-head" }, [
    el("span", { className: "case-title" }, [
      el("span", { className: "case-name", textContent: caseLabel(id) }),
      el("span", {
        className: `case-diff${differences ? " has-diff" : ""}`,
        textContent: differences
          ? `${differences} ${differences === 1 ? "check differs" : "checks differ"}`
          : "Same checks passed",
      }),
    ]),
    el("span", { className: "case-scores" }, [
      caseScore(scoreA, "side-a", scoreA !== null && scoreA > (scoreB ?? -1)),
      caseScore(scoreB, "side-b", scoreB !== null && scoreB > (scoreA ?? -1)),
    ]),
    svg("svg", { class: "case-chevron", viewBox: "0 0 20 20", "aria-hidden": "true" }, [
      svg("path", { d: "m6 8 4 4 4-4" }),
    ]),
  ]);
  header.setAttribute("aria-expanded", "false");
  header.setAttribute("aria-controls", panelId);

  const lines = ids.map((checkId) => {
    const passA = checksA.get(checkId);
    const passB = checksB.get(checkId);
    return el("li", { className: `check-row${passA !== passB ? " differs" : ""}` }, [
      checkName(checkId),
      dot(rowsA ? passA : undefined, "A"),
      dot(rowsB ? passB : undefined, "B"),
    ]);
  });

  const inner = el("div", { className: "case-inner" }, [
    el("div", { className: "judges" }, [judgeBlock("a", a, rowsA), judgeBlock("b", b, rowsB)]),
    el("div", { className: "checks" }, [
      el("p", { className: "checks-head" }, [
        el("span", { textContent: "Check" }),
        el("span", { textContent: "A" }),
        el("span", { textContent: "B" }),
      ]),
      el("ul", { className: "check-list" }, lines),
    ]),
  ]);
  const panel = el("div", { className: "case-panel", id: panelId }, [inner]);
  panel.inert = true;

  const item = el("li", { className: "case" }, [header, panel]);
  header.addEventListener("click", () => {
    const open = !item.classList.contains("is-open");
    item.classList.toggle("is-open", open);
    item.classList.remove("is-settled");
    header.setAttribute("aria-expanded", String(open));
    panel.inert = !open;
  });
  panel.addEventListener("transitionend", (event) => {
    if (event.target === panel && item.classList.contains("is-open")) item.classList.add("is-settled");
  });
  return item;
}

function caseScore(score, side, ahead) {
  return el("span", { className: `case-score ${side}${ahead ? " is-ahead" : ""}` }, [
    el("span", { className: "case-score-n", textContent: score === null ? "—" : `${score}%` }),
    bar(score ?? 0, side),
  ]);
}

function overallCard(side, model, other) {
  const diff = model.score - other.score;
  return el("div", { className: `overall side-${side}` }, [
    ring(model.score),
    el("div", { className: "overall-text" }, [
      el("span", { className: "picker-side", textContent: side.toUpperCase() }),
      el("span", { className: "overall-name", textContent: "Overall" }),
      el("span", {
        className: `overall-delta${diff > 0 ? " is-ahead" : ""}`,
        textContent: diff === 0 ? "Even" : `${diff > 0 ? "+" : "−"}${Math.abs(diff)} points`,
      }),
    ]),
  ]);
}

function renderComparison() {
  const a = findModel(state.a);
  const b = findModel(state.b);
  compareResult.replaceChildren(
    el("div", { className: "overalls" }, [overallCard("a", a, b), overallCard("b", b, a)]),
    el("ul", { className: "case-list" }, state.cases.map((id) => caseRow(id, a, b))),
  );
}

function setupCompare() {
  if (state.models.length < 2) {
    compare.hidden = true;
    return;
  }
  const ids = state.models.map((model) => model.id);
  const params = new URLSearchParams(window.location.search);
  const fromUrl = (key) => (ids.includes(params.get(key)) ? params.get(key) : null);
  // A missing or unknown pick falls back to the best model not already on the other side.
  const sharedB = fromUrl("b");
  state.a = fromUrl("a") ?? ids.find((id) => id !== sharedB);
  state.b = sharedB && sharedB !== state.a ? sharedB : ids.find((id) => id !== state.a);

  pickers.a = createPicker("a");
  pickers.b = createPicker("b");
  swapButton.onclick = () => {
    swapButton.classList.toggle("is-flipped");
    [state.a, state.b] = [state.b, state.a];
    syncCompare();
  };
  compare.hidden = false;
  syncCompare();
}

function renderMeta(data) {
  const date = new Date(data.updatedAt);
  const count = state.models.length;
  const parts = [`${count} ${count === 1 ? "model" : "models"}`];
  if (!Number.isNaN(date.getTime())) {
    parts.push(`updated ${date.toLocaleDateString("en", { year: "numeric", month: "long", day: "numeric" })}`);
  }
  meta.textContent = parts.join(" · ");
  meta.hidden = false;
}

function showError() {
  const retry = el("button", { type: "button", className: "retry", textContent: "Try again" });
  retry.addEventListener("click", load);
  ranking.replaceChildren(
    el("div", { className: "evals-state" }, [
      el("p", { textContent: "We couldn't load the results. Check your connection and try again." }),
      retry,
    ]),
  );
}

async function load() {
  ranking.replaceChildren(el("p", { className: "evals-state", textContent: "Loading results…" }));
  let data;
  try {
    const response = await fetch(DATA_URL, { cache: "no-cache" });
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    data = await response.json();
  } catch {
    showError();
    return;
  }

  const rows = Array.isArray(data.rows) ? data.rows : [];
  if (!rows.length) {
    ranking.replaceChildren(
      el("p", { className: "evals-state", textContent: "No results yet. Check back after our next run." }),
    );
    return;
  }

  state.cases = caseOrder(rows);
  state.models = groupByModel(rows).sort((x, y) => y.score - x.score || x.id.localeCompare(y.id));
  renderMeta(data);
  renderRanking();
  setupCompare();
}

load();
