# oga-web

React frontend for Oga. The app runs inside the Tauri desktop shell (`rust/apps/oga-desktop`) and reaches the broker through `window.__TAURI__.core.invoke("broker_call", …)` — never by fetching the broker over HTTP (blocked by CSP, streaming lives in the shell).

## Stack

- Vite + React + TypeScript
- Tailwind CSS (v4 via `@tailwindcss/vite`)
- Import alias `@/` → `web/src/` (configured in both `tsconfig.app.json` and `vite.config.ts`)
- Global stylesheet `src/oga.css`. Keep its `:root` tokens, `prefers-color-scheme: dark`, and `prefers-reduced-motion` blocks intact.
- Platform class `platform-macos` added to `<html>` at boot in `src/main.tsx` (UA sniff) for the window title-bar offset.

## Run

```sh
cd web
bun install
bun run dev      # Vite dev server
bun run build    # typecheck + production build
bun run typecheck
bun test
bun run preview  # serve the production build
```

## Structure

```
src/
  main.tsx     — boot: platform class, font, loads oga.css + Tailwind, mounts App
  App.tsx      — mounts the app shell
  router.tsx   — client-side routes
  bridge/      — broker client and pushed events over the Tauri bridge
  state/       — sidebar and task detail state
  domain/      — pure projections: activity, changes, trace, review, markdown
  screens/     — task, settings, usage, sidebar
  shell/       — app chrome, menus, shortcuts, updates
  components/  — shared components
  ui/          — icons and small UI helpers
  oga.css      — global stylesheet and design tokens
  index.css    — Tailwind import, fonts, syntax colours
```

## Notes

- Tailwind major: **4** (`4.3.3`, via `@tailwindcss/vite`).
- React `19.2.8`, Vite `7.x`, TypeScript `5.9.x`.
