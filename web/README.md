# oga-web

React frontend skeleton for Oga. Replaces `rust/crates/oga-ui` in a later step; that crate stays in place for now. The app runs inside the existing Tauri desktop shell and reaches the broker through `window.__TAURI__.core.invoke("broker_call", …)` — never by fetching the broker over HTTP (blocked by CSP, streaming lives in the shell).

## Stack

- Vite + React + TypeScript
- Tailwind CSS (v4 via `@tailwindcss/vite`)
- Import alias `@/` → `web/src/` (configured in both `tsconfig.app.json` and `vite.config.ts`)
- Global stylesheet `src/oga.css` — verbatim copy of `rust/crates/oga-ui/style.css` (2743 lines). Keep its `:root` tokens, `prefers-color-scheme: dark`, and `prefers-reduced-motion` blocks intact.
- Platform class `platform-macos` added to `<html>` at boot in `src/main.tsx` (UA sniff, same rule as the Rust app) for the window title-bar offset.

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
  main.tsx   — boot: platform class, loads oga.css + Tailwind, mounts App
  App.tsx    — placeholder only, proves styles load
  oga.css  — copied stylesheet (do not edit tokens by hand)
  index.css  — Tailwind import
```

No routing, screens, state, or broker client yet — later tasks own those.

## Notes

- Tailwind major: **4** (`4.3.3`, via `@tailwindcss/vite`).
- React `19.2.8`, Vite `7.x`, TypeScript `5.9.x`.
