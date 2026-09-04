# Context index

The code index used to be three tables telling three overlapping stories:
`context_maps` (one row per project), `context_files` (one row per file, with
every symbol packed into a `symbols_json` blob), and `context_learned_routes`
(a set of identifying words pinned to a `path` + `symbol` string pair). A fourth table,
`context_search_documents`, already flattened files and symbols into
one-row-per-thing for FTS5 — but only as a derived mirror, rebuilt from
`context_files` and kept in sync by a separate consistency checker
(`context_search_state`, `checkContextSearchConsistency`).

The failure mode this caused: a learned route's only handle on its target was
`(path, symbol)` as captured at learn time, plus a whole-file digest to sense
staleness. Move the symbol to another file, rename the file, and the route's
`(path, symbol)` no longer names anything — the route goes stale and stays
stale forever, even though the code it pointed at still exists.

## Identity

**A `context_entities` row's identity is a surrogate id
(`INTEGER PRIMARY KEY AUTOINCREMENT`), never reused for a different logical
thing.** Position (`path`, `line`, `end_line`, `name` for files) is a mutable
attribute of that row, not part of its identity.

Three alternatives were considered and rejected:

- **Content digest as identity.** Breaks on the very first edit — most edits
  are not moves, and treating every edit as "a new thing" would make identity
  useless for anything except a file that never changes.
- **Qualified name as identity.** Breaks on rename, and isn't even unique
  without the path (two files can both define `formatDate`), which defeats
  the purpose of an identity that's supposed to survive relocation.
- **Kind + name as identity.** Same ambiguity as qualified name — common
  names collide across a real codebase — and still breaks on rename.

Instead, identity is a plain surrogate id, and a **content digest is the
matching signal** used only at re-index time to decide whether a fresh
extraction is the same thing relocated (reuse the id, update position) or a
genuinely new thing (mint an id). This gets move-survival for the cases that
matter (a file renamed, a symbol relocated to another file) without ever
guessing across ambiguous candidates — see "Ambiguity" below.

## Position

`context_entities.path` (+ `line`/`end_line` for symbol rows, `name` for
both — a symbol's own name, or a file's basename without extension) is
position: where to point a reader right now. It has no bearing on identity
and is free to change on every re-index.

A symbol row's `path` is a denormalized copy of its parent file row's `path`
(see `parent_id` below), kept in sync by an `AFTER UPDATE OF path` trigger so
every existing `(cwd, path)` query pattern — prefix scans, `ORDER BY path`,
`sweepContextFiles`'s stat loop — keeps working with zero joins.

## Schema

One table replaces `context_files` (incl. its `symbols_json` blob) and
`context_search_documents`: **`context_entities`**, one row per file *or*
per symbol.

```
id            surrogate identity, never reused
cwd, kind     'file' | 'fn' | 'class' | 'type' | 'const' | 'struct' | 'enum' | 'ext' | 'view'
parent_id     symbol rows -> their file's id; file rows -> NULL; ON DELETE CASCADE
path, line, end_line, name     position
digest        file: whole-file content hash (unchanged from context_files.digest)
              symbol: hash of the symbol's own declaration text (line..end_line) — new
purpose, confirmed, comments_json         symbol rationale (file rows: purpose only)
params, returns, exported                 symbol-only
lang, status, lines, size, mtime_ms,
touch_count, touched_at, mapped_at,
header_comment, refs_json, importance     file-only
path_text, comments, signature, refs,
identifier_tokens, symbol_kind            derived search text — same columns,
                                           same weights, same computation
                                           (buildFileDocumentText /
                                           buildSymbolDocumentText, unchanged)
created_at, updated_at
```

File-only and symbol-only columns are nullable on the row where they don't
apply. This mirrors a pattern the codebase already used —
`context_search_documents.symbol_name IS NULL` distinguished a file row from
a symbol row — just carrying the source-of-truth columns into the same row
instead of a second table keyed by `(cwd, path)`/`row_key`.

`context_entities_fts` (FTS5, external content = `context_entities`) replaces
`context_search_fts`. Same columns, same `bm25()` weight list, same three
triggers (`_ai`/`_ad`/`_au`), just pointed at the merged table. Because
source columns and derived search-text columns now live in the same row,
written in the same statement, the two can no longer drift out of sync
during normal operation — the entire class of bug `context_search_state` /
`checkContextSearchConsistency` existed to catch (file row exists, mirror
row doesn't, or vice versa) is gone by construction, not by more checking.

**`context_maps` keeps its own table** — a project's build state (`scheme`,
`state`, `built_at`, counts) isn't "a thing in the codebase," it's
bookkeeping about the index itself, and forcing it into `context_entities`
as a fake `kind='project'` row would force genuinely different things into
one row for no reason beyond a lower table count. It does absorb
`context_search_state`'s columns, though (`search_state`, `search_indexed_at`,
`search_indexed_map_updated_at`, `search_indexed_file_count`,
`search_indexed_symbol_count`, `search_last_error`) — that table was always a
1:1 extension of `context_maps` (`cwd` FK, `ON DELETE CASCADE`), so folding
its columns in is a pure storage relocation with the exact same
`getContextSearchState`/`setContextSearchState` contract, not a new merge
decision.

`checkContextSearchConsistency` stays, adapted: it no longer compares two
tables (nothing to compare — there's one row now), it compares each entity's
stored derived-text columns against a fresh recomputation from that same
row's source columns (catches raw tampering or a write bug), plus the FTS5
shadow index's row count against `context_entities` (catches FTS5-level
corruption, which is orthogonal to the merge and can't be designed away).
The `rankedCandidatesForQuestion` degrade path in `context-map.ts`
(building → FTS → rebuild-on-failure → legacy fallback scorer) is untouched
code, calling the same store methods with the same signatures.

**A trap this design has to actively avoid:** `context_entities.digest`
means two different things depending on `kind` — a file's whole-file hash,
or a symbol's own declaration-text hash. `ContextSearchDocumentRow.digest`
(what `searchContextDocuments`/`exactContextSearchDocuments`/
`getContextSearchFileDocument` return to ranking code) means only the
former, for every row, file or symbol — `verifyQuestionCandidates`
(context-map.ts) compares it against a freshly-read file's digest to decide
whether a candidate went stale, and a symbol's own digest would never equal
that, reading every symbol-level candidate as "changed on disk" on every
query and forcing a redundant re-rank. `contextSearchDocumentFromEntityRow`
therefore takes the file digest as an explicit second argument rather than
ever reading `row.digest` for this shape; the three store methods that
build `ContextSearchDocumentRow`s resolve it via `FILE_DIGEST_EXPRESSION`
(the row's own digest for a file, its parent's for a symbol).
`listContextEntityDigests` (move-detection's own read path) is unaffected —
it always wants the row's true own digest.

**`context_learned_routes`** keeps its own table — a route maps the words
that identify a place to that place, a different grain of thing than a code
entity. A route is stored as **hints**: the useful words only, stemmed,
deduped and sorted into one `hints` string, so "config twitter", "twitter
config" and "where is the twitter config" all land on the same row. The
write path takes the words directly (`hints: ["config", "twitter"]`) — no
sentence required:

```
entity_id           REFERENCES context_entities(id) ON DELETE SET NULL
hints               the identifying words, canonicalized (sorted, deduped,
                    stemmed); FTS-indexed; part of the UNIQUE key
learned_path, learned_symbol     snapshot at learn/last-confirm time (audit trail,
                                  survives entity deletion; NOT the live answer)
source_digest        the target FILE's digest (always file-level, whether the
                      route names a symbol or not)
UNIQUE(cwd, hints, learned_path, learned_symbol)
```

`entity_id` is resolved at learn time when a matching entity already exists
(the common case — `learnContextRoutes` heals the target file immediately
before storing, so it always does for a non-worktree task). It stays `NULL`
when nothing matches yet (a worktree task learning against content that
hasn't reached the origin project) — see "Edge cases."

`route.path`/`route.symbol`, as returned by `listLearnedContextRoutes` /
`exactLearnedContextRoutes` / `searchLearnedContextRoutes`, are resolved
**live** from the attached entity's *current* position when `entity_id`
resolves to a row, falling back to the `learned_path`/`learned_symbol`
snapshot only when orphaned. This is the actual fix: the answer a route
produces tracks the code, not the moment it was learned.

Staleness stays exactly what it was: `source_digest` compared against the
target's containing file's current digest, file-level, unchanged in
granularity or meaning. What's new is that a confirmed move (see below)
refreshes `source_digest` to the new container's current digest as part of
applying the move — so relocating the code doesn't itself read as an edit
that invalidates the route. An *unrelated* edit to the file still invalidates
every route into it, exactly as before — the merge changes the shape of the
index, not the ranking or staleness heuristics built on top of it.

## Position updates

Move-detection runs in `reconcileContextMap` — the function whose own job
was already "bring the map back in line with the tree it describes" by
walking the whole tree in one pass, which is what makes a moved file or
symbol detectable at all: both its old and new position are visible
together, in the same pass, before either side commits. `buildContextMap`
and `healContextFile` (and the task-fold path built on it) still compute
and store a real per-symbol digest on every write — the signal move-
detection matches on — but only `reconcileContextMap` runs the matching
itself; see "A task's fold" below for why that boundary was drawn there.

The pass ends with two pools — entities that didn't reappear where they
used to be ("vanished") and fresh extractions that didn't match anything
locally by name ("arrived") — matched by exact `(kind, digest)` equality via
one shared primitive (`matchMoves`). A **unique** 1:1 match applies as an
`UPDATE` (position columns only; content-derived columns are left alone,
since a digest match proves the content is byte-identical) and preserves the
row's `id`, so every route attached to it keeps resolving. A symbol-level
move refreshes attached routes' `source_digest` once every touched file's
own content is written (`refreshRouteDigestsForEntity`, called after the
ordinary upserts below it, never from inside the move itself — the
destination file's digest isn't final until its own write lands). A
non-match, on either side, is the pre-existing behavior: delete the vanished
row (cascades to its own children; orphans, never deletes, any attached
route), insert the arrived one fresh.

Matching is scoped to destinations that already have a tracked entity: a
symbol landing in a file that is *also* brand new in the same pass isn't
matched (there is no id to move it into yet without inserting a placeholder
row first, purely to get an id, and then overwriting it — more moving parts
for a narrower case than the common one). It reads as a fresh symbol, same
as before this change. File-level moves have no such restriction — a moved
file's destination is by definition a new path, and the whole point is
recognising that the "new" path is actually the old file relocated.

A within-file move (a symbol's line shifted because something above it grew
or shrank, name unchanged) never touches this machinery — it's the existing
per-file name-based `reconcile()`, which now also carries the entity's full
prior content (not just its id) into a matched cross-file destination, so a
moved symbol keeps its purpose, comments and confirmed state exactly as
`reconcile()` already preserves them for an ordinary same-file edit.

## Edge cases

- **Deleted symbol.** No candidate matches it anywhere in the pass. Its
  entity row is deleted. Any route attached to it is **not** deleted —
  `ON DELETE SET NULL` orphans it (`entity_id` becomes `NULL`, the
  `learned_path`/`learned_symbol` snapshot survives for audit/history via
  `listLearnedContextRoutes`). A confirmed route is a record of a human
  decision; deleting it silently the moment its target code disappears would
  lose that decision. An orphaned-but-visible row keeps it without also
  claiming a stale row still answers correctly.
- **Split symbol** (one function's body becomes two). At most one of the two
  new candidates can digest-match the original (a split necessarily changes
  the text), so this reduces to "deleted symbol, plus two arrivals" — no
  special-casing needed.
- **Ambiguous match** (a vanished entity's digest matches more than one
  arrived candidate, or an arrived entity's digest matches more than one
  vanished candidate — e.g. a function copy-pasted to two places in the same
  edit). `matchMoves` requires a match to be unique on **both** sides; any
  key with more than one candidate on either side is excluded from `moved`
  entirely and every entity involved falls through to plain delete/insert.
  The rule is explicit and conservative by design: never guess which of
  several equally-good candidates a route should follow.
- **Pure rename, body unchanged** (`sweepHolds` → `releaseHolds`, same file,
  same position). Out of scope. Code motion means a file renamed, a symbol
  relocated to another file, or a symbol shifting position — not a symbol's
  own name changing. A content digest can't catch this by construction (the
  declaration text, and therefore the digest, includes the identifier), and
  a heuristic that ignores the name token specifically would be accuracy
  work layered on top of a shape change. A pure rename reads as
  delete+insert, same as before the merge.
- **A task's fold.** `foldContextMap` heals each file a task's settle wrote
  to independently, one `healContextFile` call per path, exactly as it did
  before this change — it does not batch them to run move-detection across
  a task's own diff. A task that moves a function by editing both the
  source and destination file in the same settle therefore has each side
  healed on its own: the source's heal sees the symbol gone and deletes it
  (orphaning any attached route), the destination's heal sees a new symbol
  and inserts it fresh. `queueContextReconcile` is queued right after every
  fold, but by the time it runs, both heals have already committed — there
  is no more "before" state left for it to match the two sides against, so
  it correctly finds nothing further to change. A move is caught reliably
  only when a whole-tree `reconcileContextMap` is the *first* thing to see
  both sides together: a manual edit outside any task, or a task whose
  settle only touched one side while the other changed earlier. Batching
  fold's heals so a single task's own settle could catch its own move
  immediately is future work — see "Scope boundaries."

## Migration

Schema version 35. One pass, guarded the same way every versioned migration
in `schema.ts` is (checked against `schema_migrations`, run inside
`BEGIN IMMEDIATE`/`COMMIT`/`ROLLBACK`):

1. Create `context_entities` (+ indexes, the path-cascade trigger, the FTS5
   table and its three triggers), and add the `search_*` columns to
   `context_maps`.
2. Populate `context_entities` from `context_files`: one `kind='file'` row
   per file (carrying its real digest forward unchanged), then one row per
   entry in `symbols_json` per file, `parent_id` set to that file's freshly
   minted id. Derived search-text columns are computed the same way
   `contextSearchDocumentsForFile` already did, so a database that had
   already run the search backfill and one that hadn't both converge on the
   same result.
3. Set `context_maps`' new `search_*` columns to **ready**, with real counts
   off the just-populated `context_entities`, for every cwd that had ever
   been through the search machinery before (the only ones
   `context_search_state` had a row for) — not a copy of whatever that row
   said. `context_search_state` tracked whether a *separate* backfill had
   caught a project's search mirror up to its files; after the merge there
   is no separate mirror to catch up — this same migration populated full
   search text in the same write as everything else about each entity, so
   carrying over a stale `'building'` (or `'failed'`) flag would leave a
   freshly- and fully-indexed install reading as not-ready, permanently,
   until something on disk happened to change (see "Position updates" for
   why an unrelated read alone never flips it). Drop the standalone table.
4. Rewrite `context_learned_routes`: `path`/`symbol` become
   `learned_path`/`learned_symbol` (verbatim), `source_digest` carries over
   **unchanged** (still file-level — nothing about its meaning changed, so
   there's nothing to recompute), and `entity_id` is resolved by looking up
   the just-populated `context_entities` for `(cwd, learned_path[,
   learned_symbol])`. Every existing route row survives the migration; none
   are dropped.
5. Drop `context_files`, `context_search_documents`, `context_search_fts`
   (+ its triggers).

**A pre-existing symbol row's `digest` cannot be computed during migration.**
`context_files.symbols_json` never stored the symbol's own declaration text
(only `line`/`endLine` pointing into a file the migration — pure SQL, no
filesystem access, matching every other migration in this file — cannot
read). Migrated symbol rows get `digest = ''`. `matchMoves` treats an empty
digest as never-matchable on either side, so this can't produce a false
positive move right after an upgrade; it just means a migrated symbol
doesn't participate in move-detection until the next time it's actually
re-extracted (at which point a real digest is computed and it behaves
normally from then on). File-level digests need no such placeholder — they
were already real content hashes in `context_files.digest` and carry
forward exactly.

One consequence worth naming: a **symbol-scoped** learned route's
`source_digest` was always file-level (see above), so it stays directly
comparable to its (correctly migrated, real) file digest after the upgrade
— no regression for the common case. It only reads as freshly-stale if the
container file already didn't match at the moment of migration, which is
the exact same staleness a plain file edit would have produced before this
change.

## Scope boundaries

Ranking, scoring, term-matching, and the FTS `bm25()` weight list stand
apart from identity and position: every ranking function in `context-map.ts`
operates on `ContextFile[]` / `ContextSymbol[]` / `RankedCandidate[]` /
`ContextSearchDocumentRow[]`, and the new identity/position machinery lives
entirely behind those shapes rather than inside ranking itself. `ContextFile`
and `ContextSymbol` in `types.ts` carry no entity id or move-detection
state — a deliberate boundary, not an oversight.

Two things are explicitly out of scope for this design, left for future
work rather than folded in:

- **Accuracy work** — finer-grained staleness, fuzzy rename detection.
- **Batching `foldContextMap`'s per-file heals** so a single task's own
  settle can catch a move it made in one diff, rather than relying on the
  next whole-tree reconcile to be the first thing that sees both sides (see
  "A task's fold" above) — and matching a symbol move into a file that's
  also brand new in the same reconcile pass (see "Position updates" above).
