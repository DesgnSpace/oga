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
- Added public documentation for installation, setup, delegation, task follow-up, worktrees, and local data.

### Changed

- Consolidated the maintained reference docs and added generated landing release notes.

### Fixed

- Kept Codex and Pi account directories separate for each worker profile.
- Installed source builds beside released Oga without sharing an app identity.
- Let workers clear local, reversible obstacles before reporting a blocker.

## 1.0.0 - 2026-09-04

### Added

- First Oga 1.0.0 release.
