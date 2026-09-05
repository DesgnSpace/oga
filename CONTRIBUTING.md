# Contributing

## Dev setup

```bash
bun install
bun test
bunx tsc --noEmit
```

- `make dev` launches the desktop app for local development.
- `make install` builds the full app bundle and installs it to `~/Applications`
  as **Oga (local)** — a separate app from a released Oga.app, with its own
  bundle identifier, so both can sit in Applications. They share port 7331 and
  `~/.oga`, so run one at a time. `make install` also repoints `oga` on your
  PATH at the build it just installed. `INSTALL_DIR` overrides where it lands.

## Tests and types

`bun test` must pass. `bunx tsc --noEmit` must be clean.

The test suite exercises task lifecycle, event tracing, scope enforcement, the
event socket protocol, watch exit codes, and the public task view contract.

## Code style

Comments are sparse and speak to *why*, not *what*. The code itself carries the
what. A comment exists only when the reason would not survive the next reader.

Prose in the codebase is plain and concrete — reasons over rules, active voice,
short sentences. Doc strings and tool descriptions follow the same voice.

## License

Contributions are accepted under the project license ([AGPL-3.0](LICENSE.md)).
By submitting a contribution you agree to license it under those terms.
