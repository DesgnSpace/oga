# oga-context

The project code index behind `oga query`, `oga relearn`, and the MCP `query`
and `map` tools. It parses source with real tree-sitter grammars, stores one
row per symbol in SQLite, and answers a plain-language question with a
`path:line#symbol` anchor.

## Shape

| Module | What it owns |
| --- | --- |
| `lang/` | One adapter per language, plus the trait they share. |
| `symbols.rs` | The engine: run an adapter's capture query over a parsed tree and turn matches into symbols. |
| `walk.rs` | Which files are candidates: extension, `.gitignore`, lockfiles, excluded directories. |
| `store.rs` | Every read and write against SQLite, including the FTS5 index. |
| `routes.rs` | Learned routes — the hint phrases people attach to a place. |
| `query.rs` | Ranking a question into ordered candidates. |
| `index.rs` | `ContextIndex`: build, reconcile, list, question, learn. |

Nothing outside `lang/` knows a language exists.

## The pipeline

A **build** walks the tree, parses every candidate file in parallel, and writes
the whole index in one transaction. A **reconcile** stats each file first: a
file whose size and modification time still match is never opened, and a file
whose content hash still matches is never re-parsed. That is why an unchanged
project reconciles in milliseconds.

Every file row carries the file's content hash. Every symbol row carries a hash
of its own declaration with the name masked out, so a saved route can follow
its target through a rename or a move to another file.

## Ranking

Signals, strongest first:

1. **A learned route**, matched exactly on the question's hint key. Decisive.
2. **A learned route** matched on overlapping hint words.
3. **An exact symbol name**: the question folded to a lookup key equals the
   symbol's. `extractSymbols`, `extract_symbols`, and "extract symbols" all
   fold to the same key. Decisive.
4. **Identifier tokens** shared between the question and the symbol's name,
   then its enclosing symbol's name.
5. **The search index** over doc comment and signature.
6. **Path tokens**.

Each of the question's words is weighted by how much of the project it
reaches: a word that names three symbols is worth far more than a word that
names three thousand. Fields and enum variants rank below the type that holds
them, and files under `tests/`, `fixtures/`, or `examples/` rank below real
code.

One anchor comes back when a decisive signal fired, or when the question
landed whole on a candidate that is well clear of the runner-up. Otherwise the
answer lists up to `--limit` candidates with the words each one matched. When
nothing matched, the answer says so and names the words that found nothing.

## Adding a language

Four steps, all inside `lang/`:

1. Add the grammar crate to `Cargo.toml`.
2. Write `lang/queries/<language>.scm`. Every pattern captures the declaration
   as `@def.<kind>` and its identifier as `@name`. `<kind>` is one of the
   `SymbolKind` values: `fn`, `method`, `struct`, `class`, `enum`, `variant`,
   `trait`, `impl`, `type`, `const`, `static`, `module`, `field`, `macro`,
   `heading`.
3. Write `lang/<language>.rs` implementing `LanguageAdapter`: the extensions it
   owns, the grammar, the query, how the language joins a nested name, what
   counts as public, and — where it matters — which comments document the
   declaration below them and which files or declarations are noise. When the
   tree alone gets a symbol wrong, `refine` corrects it: it is how a Go method
   picks up its receiver, a Python docstring comes out of the body, and a
   config key inside a list gets its position.
4. Register it in `lang::adapters()`.

The engine handles the rest. Nesting comes from byte containment, so a function
inside a type becomes a method, a value outside one becomes a constant, and
anything declared inside a function body is dropped as local. Signatures stop
at the node's `body` field, or at the end of the first line when there is none.

Shipping today: Rust, TypeScript/TSX/JavaScript, Swift, Python, Go, PHP,
Markdown, JSON, TOML, YAML.

## Kinds

`SymbolKind` is deliberately small and shared across languages. An interface
and a protocol are both `trait`; an `impl` block and a Swift `extension` are
both `impl`; a namespace and a module are both `module`. Markdown headings are
`heading`, and a heading's line range covers its whole section, so an answer
points at the section that holds the prose.

## Config files

JSON, TOML and YAML have no declarations, so every key is a symbol and the
qualified name is its dotted path: `updater.endpoints[0].url`,
`workspace.package.version`. A key that holds a container is a `module`, a key
that holds a scalar is a `field`, and an entry in a list is known by its
position. Values are not indexed; a value shorter than 64 characters rides
along in the signature so a question can be asked with the value and land on
the key that holds it. Lockfiles are skipped whatever their format.
