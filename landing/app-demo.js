// The home page app panel: tasks change state on a loop, and a pick shows
// that task in the preview. The markup holds the first state, so the panel
// reads fine without this script.

const app = document.querySelector("[data-app]");

if (app) {
  const TASKS = {
    build: {
      title: "Build the feature",
      worker: "Codex",
      project: "release",
      prompt: "Add CSV import to the contacts page. Only touch src/import/.",
      time: "4m",
      cost: "$0.42",
      replies: {
        running: "The parser works. Now adding the upload form.",
        done: "Done. Contacts import from CSV files up to 10 MB.",
      },
    },
    tests: {
      title: "Write the tests",
      worker: "Claude Code",
      project: "release",
      prompt: "Write tests for the CSV importer.",
      time: "2m",
      cost: "$0.18",
      replies: {
        needs: "Should a row with no email be skipped or rejected?",
      },
    },
    check: {
      title: "Check the build",
      worker: "Antigravity",
      project: "release",
      prompt: "Run the build and the tests on this branch.",
      time: "5m",
      cost: "$0.09",
      replies: {
        running: "Running 214 tests.",
        done: "The build passes. All 214 tests pass.",
      },
    },
    review: {
      title: "Review the change",
      worker: "Pi",
      project: "release",
      prompt: "Review the CSV importer before it merges.",
      time: "1m",
      cost: "$0.06",
      replies: {
        queued: "Starts when Build the feature is done.",
        running: "Reading the 6 changed files.",
        needs: "2 files changed outside src/import/. Keep them?",
      },
    },
    docs: {
      title: "Update the docs",
      worker: "OpenCode",
      project: "docs",
      prompt: "Add a guide for CSV import to the help pages.",
      time: "3m",
      cost: "$0.11",
      replies: {
        done: "Added the import guide with 2 screenshots.",
      },
    },
  };

  const LABELS = { running: "Running", needs: "Needs you", queued: "Waiting", done: "Done" };

  // Each step names the task that changed, so the preview can follow it.
  const STEPS = [
    { focus: "tests", looks: { tests: "needs", build: "running", check: "running", review: "queued", docs: "done" } },
    { focus: "check", looks: { tests: "needs", build: "running", check: "done", review: "queued", docs: "done" } },
    { focus: "build", looks: { tests: "needs", build: "done", check: "done", review: "running", docs: "done" } },
    { focus: "review", looks: { tests: "needs", build: "done", check: "done", review: "needs", docs: "done" } },
  ];

  const STEP_MS = 3400;
  const PAUSE_AFTER_PICK_MS = 12000;

  const list = app.querySelector("[data-app-list]");
  const preview = app.querySelector("[data-app-preview]");
  const thread = app.querySelector("[data-app-thread]");
  const badge = app.querySelector("[data-app-badge]");
  const field = (name) => preview.querySelector(`[data-field="${name}"]`);
  const rows = new Map([...list.children].map((row) => [row.dataset.task, row]));
  const reduceMotion = window.matchMedia("(prefers-reduced-motion: reduce)");

  let step = 0;
  let picked = "tests";
  let looks = { ...STEPS[0].looks };
  let timer;
  let pausedUntil = 0;
  let visible = false;

  const replay = (element, className) => {
    element.classList.remove(className);
    void element.offsetWidth;
    element.classList.add(className);
  };

  function renderPreview(id) {
    const task = TASKS[id];
    const look = looks[id];
    preview.dataset.look = look;
    field("title").textContent = task.title;
    field("prompt").textContent = task.prompt;
    field("reply").textContent = task.replies[look] ?? task.replies.running;
    field("status").textContent = LABELS[look];
    field("worker").textContent = task.worker;
    field("project").textContent = task.project;
    field("time").textContent = task.time;
    field("cost").textContent = task.cost;
    field("compose").textContent = look === "needs" ? "Answer in the task" : "Follow up in the task";
    replay(thread, "app-fade");
  }

  function pick(id) {
    picked = id;
    for (const [taskId, row] of rows) {
      row.querySelector(".app-task").setAttribute("aria-pressed", String(taskId === id));
    }
    renderPreview(id);
  }

  // Tasks that need you lead; the rest keep their order. Rows that move
  // slide from where they were (first, last, invert, play).
  function sortRows() {
    const order = [...rows.keys()].sort((a, b) => Number(looks[b] === "needs") - Number(looks[a] === "needs"));
    const before = new Map([...rows].map(([id, row]) => [id, row.getBoundingClientRect().top]));
    for (const id of order) list.append(rows.get(id));
    if (reduceMotion.matches) return;
    for (const [id, row] of rows) {
      const shift = before.get(id) - row.getBoundingClientRect().top;
      if (shift === 0) continue;
      row.animate([{ transform: `translateY(${shift}px)` }, { transform: "none" }], {
        duration: 300,
        easing: "cubic-bezier(0.23, 1, 0.32, 1)",
      });
    }
  }

  function apply(next) {
    for (const [id, row] of rows) {
      if (looks[id] === next.looks[id]) continue;
      const button = row.querySelector(".app-task");
      button.dataset.look = next.looks[id];
      button.querySelector(".app-tag").textContent = LABELS[next.looks[id]];
      replay(button.querySelector(".app-task-icon"), "app-changed");
    }
    looks = { ...next.looks };
    badge.textContent = String(Object.values(looks).filter((look) => look === "needs").length);
    sortRows();
    pick(next.focus);
  }

  function tick() {
    if (visible && !document.hidden && Date.now() >= pausedUntil) {
      step = (step + 1) % STEPS.length;
      apply(STEPS[step]);
    }
    timer = setTimeout(tick, STEP_MS);
  }

  function start() {
    clearTimeout(timer);
    if (!reduceMotion.matches) timer = setTimeout(tick, STEP_MS);
  }

  for (const [id, row] of rows) {
    row.querySelector(".app-task").addEventListener("click", () => {
      pausedUntil = Date.now() + PAUSE_AFTER_PICK_MS;
      if (id !== picked) pick(id);
    });
  }

  if ("IntersectionObserver" in window) {
    new IntersectionObserver(([entry]) => {
      visible = entry.isIntersecting;
    }, { threshold: 0.4 }).observe(app);
  } else {
    visible = true;
  }

  reduceMotion.addEventListener("change", start);
  start();
}
