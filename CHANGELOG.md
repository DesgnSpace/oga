# Changelog

All notable changes to Oga are documented here.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and Oga uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## Unreleased

### Added

- Added per-kind default model rules with `oga love --when`.
- Added `oga query --limit` and `oga query --code`.
- Added scheduled starts for delegated and resumed work.
- Added task-specific next-step guidance to MCP responses.
- Added queued follow-up instructions when a running worker cannot accept them.
- Added public documentation for installation, setup, delegation, task follow-up, worktrees, local data, and release notes.

### Changed

- Consolidated the maintained reference docs.

### Fixed

- Kept Codex and Pi account directories separate for each worker profile.
- Installed source builds beside released Oga without sharing an app identity.
- Let workers clear local, reversible obstacles before reporting a blocker.

## 0.0.5 - 2026-09-04

### Fixed

- Packaged the broker sidecar for supported macOS architectures.
- Located the bundled broker beside the app executable.
- Let Tauri handle app notarization during publishing.

## 0.0.4 - 2026-08-26

### Added

- Linked ignored project paths into task worktrees by default.
- Kept interrupted work on hold for recovery.

### Fixed

- Preserved task holds during handoff.
- Settled dependent tasks after direct completion.

## 0.0.3 - 2026-08-24

### Added

- Added the product landing page.
- Added Pages deployment for the landing site.

## 0.0.2 - 2026-08-24

### Fixed

- Loaded release environment values safely during publishing.
