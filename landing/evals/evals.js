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
const slots = document.getElementById("compare-slots");

const SIDES = ["a", "b", "c", "d"];
const MIN_PICKS = 2;
const MAX_PICKS = 4;

const state = { models: [], cases: [], picks: [] };
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

// A model and the program that ran it make one entry, so its id carries both: `claude-sonnet-5-5@claude-code`.
function entryId(row) {
  if (!row.runner?.name) return row.model;
  return `${row.model}@${row.runner.name.toLowerCase().replace(/[^a-z0-9]+/g, "-")}`;
}

function describeRunner(rows) {
  const runner = rows.find((row) => row.runner?.name)?.runner;
  if (!runner) return null;
  const versions = [...new Set(rows.map((row) => row.runner?.version).filter(Boolean))];
  return { name: runner.name, version: versions.join(", ") };
}

function runnerText(runner) {
  return runner.version ? `${runner.name} ${runner.version}` : runner.name;
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
    const id = entryId(row);
    if (!models.has(id)) models.set(id, new Map());
    const cases = models.get(id);
    if (!cases.has(row.case)) cases.set(row.case, []);
    cases.get(row.case).push(row);
  }

  return [...models].map(([id, cases]) => {
    const all = [...cases.values()].flat();
    const { name, provider } = describeModel(all[0].model);
    const runner = describeRunner(all);
    const label = runner ? `${name} · ${runner.name}` : name;
    return { id, model: all[0].model, name, provider, runner, label, cases, free: isFree(all), ...tally(all) };
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

function bar(score) {
  const fill = el("span", { className: "bar-fill" });
  fill.style.transform = "scaleX(0)";
  animateTo(fill, "transform", `scaleX(${score / 100})`);
  const track = el("span", { className: "bar" }, [fill]);
  track.setAttribute("aria-hidden", "true");
  return track;
}

function modelTitle(model) {
  const title = el("span", { className: "model-title" }, [
    el("span", { className: "model-name", textContent: model.name }),
  ]);
  if (model.free) title.append(el("span", { className: "pill pill-free", textContent: "Free" }));
  const provider = el("span", { className: "model-provider", textContent: model.provider });
  // A provider such as OpenCode can also be the runner; its name shows once.
  if (model.runner?.name === model.provider) {
    provider.classList.add("model-runner");
    provider.title = `Ran in ${runnerText(model.runner)}`;
  } else if (model.runner) {
    const runner = el("span", { className: "model-runner", textContent: model.runner.name });
    runner.title = `Ran in ${runnerText(model.runner)}`;
    provider.append(" · ", runner);
  }
  return el("span", { className: "model" }, [title, provider]);
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
    button.setAttribute("aria-label", `${model.label}, ${model.score}% overall. Add to the comparison.`);
    button.addEventListener("click", () => {
      addPick(model.id);
      compare.scrollIntoView({ block: "start" });
    });
    return el("li", {}, [button]);
  });
  ranking.replaceChildren(el("ol", { className: "rank-list" }, items));
}

/* Picker: a button that opens a searchable list of models. */

let openPicker = null;

document.addEventListener("pointerdown", (event) => {
  if (openPicker && !openPicker.root.contains(event.target)) openPicker.close(false);
});

// `trigger` is the button that opens the list. `choices` lists the models to offer.
function createPicker({ trigger, label, selected, choices, onPick, alignEnd = false }) {
  const listId = nextId("picker-list");
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
  search.setAttribute("aria-label", label);
  const list = el("ul", { className: "picker-list", id: listId });
  list.setAttribute("role", "listbox");
  const empty = el("p", { className: "picker-empty", textContent: "No model matches." });
  const panel = el("div", { className: "picker-panel" }, [search, list, empty]);
  const root = el("div", { className: `picker${alignEnd ? " align-end" : ""}` }, [trigger, panel]);

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
    const matches = choices().filter((model) =>
      `${model.id} ${model.provider} ${model.runner?.name ?? ""}`.toLowerCase().includes(query),
    );
    options = matches.map((model) => {
      const option = el("li", { id: nextId("option"), className: "picker-option" }, [
        modelTitle(model),
        el("span", { className: "picker-score", textContent: `${model.score}%` }),
      ]);
      option.setAttribute("role", "option");
      option.setAttribute("aria-selected", String(model.id === selected));
      option.dataset.id = model.id;
      option.addEventListener("pointerdown", (event) => event.preventDefault());
      option.addEventListener("click", () => pick(model.id));
      return option;
    });
    list.replaceChildren(...options);
    empty.hidden = options.length > 0;
    active = Math.max(0, options.findIndex((option) => option.dataset.id === selected));
    paintActive();
  }

  function open() {
    if (openPicker) openPicker.close(false);
    search.value = "";
    fill();
    root.classList.add("is-open");
    trigger.setAttribute("aria-expanded", "true");
    search.setAttribute("aria-expanded", "true");
    search.focus();
    openPicker = api;
  }

  function close(returnFocus = true) {
    if (!root.classList.contains("is-open")) return;
    root.classList.remove("is-open");
    trigger.setAttribute("aria-expanded", "false");
    search.setAttribute("aria-expanded", "false");
    if (openPicker === api) openPicker = null;
    if (returnFocus) trigger.focus();
  }

  function pick(id) {
    close(false);
    onPick(id);
  }

  const api = { root, close };
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
  return root;
}

/* Picked models: one card each, plus a card to add another. */

function sideBadge(index) {
  return el("span", { className: "picker-side", textContent: SIDES[index].toUpperCase() });
}

function sideClass(index) {
  return `side-${SIDES[index]}`;
}

function spareModels() {
  return state.models.filter((model) => !state.picks.includes(model.id));
}

function overallDelta(model, best, allEven) {
  const gap = best - model.score;
  if (allEven) return el("span", { className: "slot-delta", textContent: "Even" });
  if (gap === 0) return el("span", { className: "slot-delta is-best", textContent: "Best overall" });
  return el("span", { className: "slot-delta", textContent: `${gap} ${gap === 1 ? "point" : "points"} behind` });
}

function slotCard(id, index, columns) {
  const model = findModel(id);
  const scores = state.picks.map((pick) => findModel(pick).score);
  const best = Math.max(...scores);
  const allEven = scores.every((score) => score === best);

  const trigger = el("button", { type: "button", className: "picker-trigger" }, [
    sideBadge(index),
    modelTitle(model),
    svg("svg", { class: "picker-chevron", viewBox: "0 0 20 20", "aria-hidden": "true" }, [
      svg("path", { d: "m6 8 4 4 4-4" }),
    ]),
  ]);
  trigger.setAttribute("aria-label", `Model ${SIDES[index].toUpperCase()}: ${model.label}. Change model`);
  trigger.dataset.focus = `slot-${index}`;
  const picker = createPicker({
    trigger,
    label: `Search models for ${SIDES[index].toUpperCase()}`,
    selected: id,
    choices: () => state.models,
    onPick: (next) => replacePick(index, next),
    alignEnd: index > 0 && index >= columns / 2,
  });

  const head = el("div", { className: "slot-head" }, [picker]);
  if (state.picks.length > MIN_PICKS) {
    const remove = el("button", { type: "button", className: "slot-remove" }, [
      svg("svg", { viewBox: "0 0 20 20", "aria-hidden": "true" }, [svg("path", { d: "m6 6 8 8M14 6l-8 8" })]),
    ]);
    remove.setAttribute("aria-label", `Remove ${model.label} from the comparison`);
    remove.addEventListener("click", () => removePick(index));
    head.append(remove);
  }

  return el("li", { className: `slot ${sideClass(index)}` }, [
    head,
    el("div", { className: "slot-score" }, [
      ring(model.score),
      el("span", { className: "slot-score-text" }, [
        el("span", { className: "slot-label", textContent: "Overall" }),
        overallDelta(model, best, allEven),
      ]),
    ]),
    ...(model.runner ? [el("p", { className: "slot-runner", textContent: `Ran in ${runnerText(model.runner)}` })] : []),
  ]);
}

function addCard(columns) {
  const trigger = el("button", { type: "button", className: "slot-add" }, [
    svg("svg", { viewBox: "0 0 20 20", "aria-hidden": "true" }, [svg("path", { d: "M10 4v12M4 10h12" })]),
    el("span", { textContent: "Add a model" }),
  ]);
  trigger.dataset.focus = "add";
  const picker = createPicker({
    trigger,
    label: "Search models to add",
    selected: null,
    choices: spareModels,
    onPick: (id) => addPick(id, { fromAddCard: true }),
    alignEnd: columns > 1,
  });
  return el("li", { className: "slot slot-empty" }, [picker]);
}

function canAdd() {
  return state.picks.length < MAX_PICKS && spareModels().length > 0;
}

function renderSlots(focus) {
  const columns = state.picks.length + (canAdd() ? 1 : 0);
  const cards = state.picks.map((id, index) => slotCard(id, index, columns));
  if (canAdd()) cards.push(addCard(columns));
  const list = el("ul", { className: "slot-list" }, cards);
  list.style.setProperty("--columns", String(columns));
  list.dataset.columns = String(columns);
  slots.replaceChildren(list);
  if (focus) slots.querySelector(`[data-focus="${focus}"]`)?.focus();
}

// Picking a model that sits in another slot swaps the 2 slots.
function replacePick(index, id) {
  const from = state.picks.indexOf(id);
  if (from !== -1) state.picks[from] = state.picks[index];
  state.picks[index] = id;
  syncCompare(`slot-${index}`);
}

// A full comparison swaps its last model for the new one.
function addPick(id, { fromAddCard = false } = {}) {
  if (state.picks.includes(id)) return;
  if (state.picks.length >= MAX_PICKS) state.picks[MAX_PICKS - 1] = id;
  else state.picks.push(id);
  const focus = canAdd() ? "add" : `slot-${state.picks.indexOf(id)}`;
  syncCompare(fromAddCard ? focus : undefined);
}

function removePick(index) {
  state.picks.splice(index, 1);
  syncCompare(canAdd() ? "add" : `slot-${Math.min(index, state.picks.length - 1)}`);
}

// Keeps the picks in the address as `?m=a,b,c` so a comparison can be shared.
function writeUrl() {
  const url = new URL(window.location.href);
  url.searchParams.delete("a");
  url.searchParams.delete("b");
  url.searchParams.delete("m");
  const rest = url.searchParams.toString();
  const picks = state.picks
    .map((id) => encodeURIComponent(id).replaceAll("%2F", "/").replaceAll("%40", "@"))
    .join(",");
  url.search = `?m=${picks}${rest ? `&${rest}` : ""}`;
  window.history.replaceState(null, "", url);
}

function syncCompare(focus) {
  writeUrl();
  renderSlots(focus);
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

function dot(pass, name, best) {
  if (pass === undefined) return el("span", { className: "dot dot-none", textContent: "—" });
  const mark = el("span", { className: `dot ${pass ? "dot-pass" : "dot-fail"}${best ? " is-best" : ""}` });
  mark.setAttribute("role", "img");
  mark.setAttribute("aria-label", `${name}: ${pass ? "passed" : "missed"}`);
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

function judgeBlock(index, model, rows) {
  const block = el("div", { className: `judge ${sideClass(index)}` }, [
    el("p", { className: "judge-head" }, [sideBadge(index), el("span", { textContent: model.label })]),
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

// Marks the top score, unless every picked model ran the job and tied.
function bestOf(scores) {
  const known = scores.filter((score) => score !== null);
  const top = Math.max(...known);
  const tied = known.length === scores.length && known.every((score) => score === top);
  return (score) => !tied && score !== null && score === top;
}

function caseRow(id, models) {
  const runs = models.map((model) => model.cases.get(id));
  const scores = runs.map((rows) => (rows ? tally(rows).score : null));
  const checks = runs.map(checksById);
  const ids = [...new Set(checks.flatMap((map) => [...map.keys()]))];
  const results = (checkId) => runs.map((rows, index) => (rows ? checks[index].get(checkId) : undefined));
  const differences = ids.filter((checkId) => new Set(results(checkId)).size > 1).length;
  const isBestScore = bestOf(scores);

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
    el(
      "span",
      { className: "case-scores" },
      scores.map((score, index) => caseScore(score, index, models[index], isBestScore(score))),
    ),
    svg("svg", { class: "case-chevron", viewBox: "0 0 20 20", "aria-hidden": "true" }, [
      svg("path", { d: "m6 8 4 4 4-4" }),
    ]),
  ]);
  header.setAttribute("aria-expanded", "false");
  header.setAttribute("aria-controls", panelId);

  const lines = ids.map((checkId) => {
    const passes = results(checkId);
    const differs = new Set(passes).size > 1;
    return el("li", { className: `check-row${differs ? " differs" : ""}` }, [
      checkName(checkId),
      ...passes.map((pass, index) => dot(pass, models[index].label, differs && pass === true)),
    ]);
  });

  const inner = el("div", { className: "case-inner" }, [
    el("div", { className: "judges" }, models.map((model, index) => judgeBlock(index, model, runs[index]))),
    el("div", { className: "checks" }, [
      el("p", { className: "checks-head" }, [
        el("span", { textContent: "Check" }),
        ...models.map((model, index) => {
          const badge = sideBadge(index);
          badge.title = model.label;
          return el("span", { className: sideClass(index) }, [badge]);
        }),
      ]),
      el("ul", { className: "check-list" }, lines),
    ]),
  ]);
  const panel = el("div", { className: "case-panel", id: panelId }, [inner]);
  panel.inert = true;

  const item = el("li", { className: "case" }, [header, panel]);
  header.addEventListener("click", () => setCaseOpen(item, !item.classList.contains("is-open")));
  panel.addEventListener("transitionend", (event) => {
    if (event.target === panel && item.classList.contains("is-open")) item.classList.add("is-settled");
  });
  return item;
}

const phone = window.matchMedia("(max-width: 760px)");

// On a phone one job stays open at a time, so the page stays short.
function setCaseOpen(item, open) {
  if (open && phone.matches) {
    for (const other of item.parentElement.querySelectorAll(".case.is-open")) {
      if (other !== item) setCaseOpen(other, false);
    }
  }
  const header = item.querySelector(".case-head");
  item.classList.toggle("is-open", open);
  item.classList.remove("is-settled");
  header.setAttribute("aria-expanded", String(open));
  item.querySelector(".case-panel").inert = !open;
}

function caseScore(score, index, model, best) {
  const node = el("span", { className: `case-score ${sideClass(index)}${best ? " is-best" : ""}` }, [
    sideBadge(index),
    el("span", { className: "case-score-n", textContent: score === null ? "—" : `${score}%` }),
    bar(score ?? 0),
  ]);
  node.title = model.label;
  return node;
}

function renderComparison() {
  const models = state.picks.map(findModel);
  const list = el("ul", { className: "case-list" }, state.cases.map((id) => caseRow(id, models)));
  list.style.setProperty("--picks", String(models.length));
  compareResult.replaceChildren(list);
}

function initialPicks() {
  const ids = state.models.map((model) => model.id);
  const params = new URLSearchParams(window.location.search);
  // `?a=&b=` is the older 2-model address; it still opens the same pair.
  const asked = params.has("m")
    ? params.get("m").split(",")
    : [params.get("a"), params.get("b")];
  // A link from before runners were shown names only the model; it opens that model's best entry.
  const resolved = asked.map((id) => (ids.includes(id) ? id : state.models.find((model) => model.model === id)?.id));
  const picks = [...new Set(resolved.filter(Boolean))].slice(0, MAX_PICKS);
  // Fill up to 2 with the best models not already picked.
  for (const id of ids) {
    if (picks.length >= MIN_PICKS) break;
    if (!picks.includes(id)) picks.push(id);
  }
  return picks;
}

function setupCompare() {
  if (state.models.length < MIN_PICKS) {
    compare.hidden = true;
    return;
  }
  state.picks = initialPicks();
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
