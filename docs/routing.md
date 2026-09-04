# Choosing a profile, model, and effort

Every dispatch lands on one account and one model, at one reasoning level. The
caller can name all three, none of them, or the account only — and Oga fills
in the rest from four inputs: what the caller declared about the work, what the
 project's `.oga.yaml` allows, what each connected account actually offers, and
how much of its usage window is left.

## Three paths through `delegate`

| What the caller names | What Oga does |
| --- | --- |
| Nothing | Sends the work to the loved model when the directory has one; otherwise routes: filters every model on every enabled account, scores the survivors, picks one. |
| A profile | Routes within that account only, so the project policy's own order decides which of its models runs. Always names a model, or refuses — it never quietly runs the account's static default. |
| A profile and a model | Runs exactly that. The same filters still execute — as advice, attached to `warnings`, never as a refusal. |

The third row is the important one. Naming an account is the caller's call, but
sending work to an account with revoked credentials should not be silent. Every
filter that would have excluded the pair becomes a warning on the response
instead, and the dispatch proceeds.

## The loved model: one answer instead of a policy

A directory can name one model as the place unnamed work goes. It is a single
value, not a list — loving a second model replaces the first — and it exists so
that changing where work goes is one declaration rather than an edit to every
route class.

```yaml
[models.opencode."opencode-go/ox-alpha-free"]
loved = true
```

The key sits on the same `[models.*]` tables as `enabled` and `preferred`. **A
file may mark at most one entry `loved`**; two is a config error naming both,
raised when the file is read rather than left for a dispatch to resolve:

```
invalid model config /path/.oga.yaml at models: only one model can be loved at
a time; claude/opus and opencode/opencode-go/ox-alpha-free are both marked
```

Scope follows the same two files as everything else: the project's `.oga.yaml`
answers for that project, and `~/.oga.yaml` answers everywhere else. A project
that loves nothing uses the global one. The two are never merged — with one in
each file, the project's is the answer.

A worker-scoped entry pins the account too. A flat entry —
`[models."opencode-go/ox-alpha-free"] loved = true` — matches that model wherever
it is offered, so a second account carrying it can take the work.

**The loved model is the default for every task class.** Mechanical, general,
build, context and reasoning all land on it, and the class no longer picks the
model — it still prices the effort, so hard work runs the same model at a higher
reasoning level. It also stands over the class's `allow` list and `min_quality`;
running under the tier the difficulty asks for is said in a warning rather than
refused.

**Naming a profile or a model still wins.** The loved model is a standing
default, not a lock, so a caller who names a destination gets it.

**A loved model never traps a dispatch.** It is skipped when its account is
disabled, the model is turned off, no connected account offers it, the account is
recorded `unavailable`, or its usage window is 98% spent. The dispatch then
routes exactly as it would with nothing loved, and a warning names what was
skipped and why:

```
opencode/opencode-go/ox-alpha-free is loved here but could not take this task:
the account is out of credits; this went to the usual choice for build work
instead
```

When it does take the work, the route's `reason` says so, and the `models` rows
carry `loved: true` on it.

### Setting it

```
oga love                      # what this directory sends unnamed work to
oga love <worker>/<model>     # send it there from now on
oga love --clear              # go back to choosing per task
oga love ... --global         # the same, for every project
```

The first `/` separates the worker from the model, since model ids carry slashes
of their own — `oga love opencode/opencode-go/ox-alpha-free`. Loving a model
that is switched off in that scope turns it back on, and says so: an inert
default is worse than no default. Loving a model the account does not list is
refused, unless that account's catalog could not be read at all.

## Difficulty is the one thing the caller declares

`difficulty` is optional on `delegate`. It is the only judgment the
caller is asked for, because the caller wrote the prompt.

| `difficulty` | Capability floor | Effort target on the model's ladder | Ranked by |
| --- | --- | --- | --- |
| `mechanical` | 2 of 5 | 10% | `balanced` |
| `standard` | 3 of 5 | 30% | `balanced` |
| `hard` | 4 of 5 | 60% | `balanced` |
| `critical` | 5 of 5 | 90% | `quality` |

Omitting it does not fall back to a constant. Oga reads the prompt, decides
which of the five task classes it is, and takes that class's difficulty:

| Task class | Difficulty when the caller declares none |
| --- | --- |
| `mechanical` — renames, formatting, commit messages, probes, verification | `mechanical` |
| `general` — anything matching nothing else | `standard` |
| `build` — implementing, fixing, refactoring, writing tests | `standard` |
| `context` — reviewing, tracing, auditing, reading a codebase | `hard` |
| `reasoning` — architecture, root-cause, concurrency, security | `critical` |

The class is decided by weighted signals across the whole prompt, and the
heaviest class wins; a tie goes to the cheaper one. Declaring too high buys one
over-priced success, declaring too low buys a cheap retry.

The same class picks the `.oga.yaml` route. It never overrides a declaration.
When the prompt reads as needing a stronger tier than the declared difficulty
allows, `heuristicAgreed` goes false and a warning says so:

```
this reads like deep judgment or failure analysis but was sent as standard work;
raise difficulty if the result comes back thin
```

Nothing is silently upgraded, and a caller that declared nothing never sees that
warning — there is no declaration for the reading to contradict.

`docs/routing-defaults.md` has the reasoning behind these numbers, and what a
fresh install with one connected account gets for each class.

## Effort is read off the model, never invented

Providers publish different reasoning ladders: five rungs on Claude, six on pi,
none on some models. The difficulty's target is projected onto whatever the
chosen model actually publishes, so the level Oga asks for is always one that
model accepts.

Ladders whose rungs all fall inside Oga's own vocabulary — `minimal`, `low`,
`medium`, `high`, `xhigh`, `max` — are sorted into that order before the
projection, so a provider that publishes its levels in some other order projects
correctly anyway. A ladder using words outside the vocabulary keeps its
published order. **A model that publishes no ladder gets no effort flag** rather
than an invented one.

Passing `effort` explicitly overrides the projection. If the model's published
levels do not include it, the value is passed through as asked, with a warning
naming what the model does accept.

The `models` response exposes each model's `efforts` ladder and
`defaultEffort`. The shipped defaults are `claude`: `low, medium, high, xhigh,
max`; `pi`: `minimal, low, medium, high, xhigh, max`; Codex and OpenCode use
their published per-model levels; and Antigravity exposes no ladder because its
effort is part of the model id.

Effort selection is deterministic. An explicit `effort` wins. Otherwise Oga
classifies the prompt: renames, formatting, probes, and verification project to
the weakest useful rung; matching, review, tracing, and refactoring use the
higher context/build tiers; architecture, root-cause, concurrency, security,
and critical work use the deepest tier. Within a ladder, `medium` means the
middle vocabulary rung nearest the 30% standard-work target, not a provider's
global token budget.

## Project policy: `.oga.yaml`

A `.oga.yaml` in the task's `cwd` constrains which provider/model pairs may
run there. It is read from the task's directory, never from wherever the broker
was launched, and it names providers and model patterns — never local profile
IDs, so a committed file stays portable across clones.

```yaml
version: 1
routes:
  build:
    preference: quality
    min_quality: 5
    allow:
      - provider: claude
        model: opus
      - provider: opencode
        model: opencode-go/*
  reasoning:
    preference: quality
    allow:
      - provider: claude
        model: '*'
      - provider: codex
        model: '*'
```

Route keys are the five task classes: `mechanical`, `context`, `build`,
`reasoning`, `general`. Each takes a non-empty `allow` list plus optional
`preference` (`balanced`, `quality`, `cost`, `speed`) and `min_quality` (an
integer 1–5, applied as a floor alongside the difficulty's).

`allow` entries are matched against normalized lowercase IDs. `provider` is
exact; `model` accepts `*` as a wildcard and nothing else — no regex. **Order
matters**: entries are written best-first, and when the caller has already named
the account, that order is what picks the model.

Omitting a class from `[routes]` leaves it unconstrained. Omitting the file
entirely leaves routing exactly as it is without one. A file containing only
`worker` is complete and valid — it carries prompt rules, not routing, and
needs no `version`.

Anything else fails loudly, naming the file and the exact field:

```
invalid routing policy /path/.oga.yaml at routes.build.min_quality: must be an
integer from 1 to 5
```

An `allow` entry that matches no model any connected account offers is reported
per entry, and selection continues with the remaining entries:

```
this project allows claude model opus for build work, but no connected account
offers it; remove the entry or connect that account
```

`oga config <dir>` reports the same finding for **every** class the file
defines, under `warnings`, so a stale entry surfaces when the config is read
rather than months later on the one dispatch that falls into that class. A
provider whose model discovery failed is left alone — its one configured entry
is not evidence about the rest of the account.

Policy is the authority on the automatic path — it cannot select outside
`allow`. An explicit profile-and-model pair outside it still runs, and carries a
warning saying it overrode the project's policy.

When **every** model a class allows is unusable, the allow list is set aside
rather than leaving the class unroutable. The route reports `relaxed: ["policy"]`
and warns:

```
no model this project allows for build work can run right now, so this went to
one outside those rules; update the allow list in .oga.yaml, or connect an
account offering a model it already names
```

A `modelHint` turns that off. The caller narrowed the choice to one model, so
failing on it says more than quietly dodging the policy would.

## Model choices: `.oga.yaml`

Model settings use the user file as a default and the project file as an
override. A project can enable a model disabled globally, and can choose its
own preferred set. An entry binds to one worker — the same model name under two
workers is two independent entries, so toggling it for one never touches the
other:

```yaml
models:
  opencode:
    openai/gpt-5.6-luna:
      enabled: true
      preferred: true
      loved: true
      capabilities: [reasoning, long-context]
```

`preferred` and `loved` answer different questions. `preferred` is a shortlist —
any number of models, and it only narrows what `models` shows. `loved` is a
single destination, and it decides where work actually goes.

A flat entry with no worker segment — `[models."openai/gpt-5.6-luna"]` — is the
older shape and still read: it binds the model under every worker that offers
it, and a worker-scoped entry for the same model wins field by field. New
switches are always written worker-scoped.

An unlisted model is enabled, not preferred, and not loved. If no preferred model
is declared, the `models` tool returns every enabled model instead of an empty
list; when one is declared, the loved model rides that narrow view too, since it
is where work goes. Derived capabilities are `reasoning`, `long-context`, `tool-use`, and `free`; a
model's `capabilities` entry replaces derived tags.

The Settings screen still exposes per-directory switches for local convenience.
Those switches follow the same precedence: project values override global
values, and they do not form a global gate.

Silence is consent: a worker or model no scope has ruled on stays available. An
install where nobody has opened Settings routes exactly as it did before.

Projects appear in Settings on their own, from the directories Oga has already
worked in — tasks, memories, and context maps. Nothing is registered by hand.

Routes:

| Route | What it does |
| --- | --- |
| `GET /api/projects` | The global directory plus every known project. |
| `GET /api/model-settings?cwd=` | Every worker and model for one scope, with its state. The catalog is unfiltered — a model turned off still has to be visible to turn back on. |
| `PUT /api/model-settings` | `{cwd, profileId, enabled?, models?}`. `null` clears a switch back to inherited. |
| `DELETE /api/model-settings?cwd=` | Drops every switch that scope sets. |

## Account availability

Availability is recorded from what actually happened, not probed. Every profile
carries one of three states. Status evidence appears in routing and task
warnings rather than the capacity list:

| State | Set by | Effect on routing |
| --- | --- | --- |
| `unavailable` | An observed `auth` or `billing` failure, or a `network`/`rate_limit` failure whose `retryAt` has not passed | Excluded on the automatic path, with the reason and retry time in a warning |
| `available` | An observed successful generation | Scored normally |
| `unknown` | No observed outcome yet, or a retry time that has passed without a recheck | Stays eligible, with a warning |

Auth and billing failures have no retry time: they stay unavailable until a
successful run clears them. Catalog access is not evidence — an account can list
its models and still be out of credits, so a refresh that only reads a catalog
reports `unknown` rather than `available`. No availability check ever sends a
prompt or spends inference credit.

## Quota

Selection reads session and weekly windows where the provider exposes them. It
uses the worst window on each account:

- **At 98% or more**, the account is filtered out on the automatic path — a run
  that dies part-way through is worse than a slower model. A caller that named
  the account keeps the dispatch and gets the warning.
- **At 90% or more**, a caller-named account is warned that the run may stop
  part-way through.
- **From 75%**, the account is deprioritized by a scoring penalty rather than
  excluded, so a cheap model on a busy account can still win.

A provider that reports no usage at all — opencode and pi report none — is
**unknown headroom**: never filtered, never credited. `quotaUsedPercent` is
`null` for those. Reading silence as spent would exclude the accounts carrying
most of the work; reading it as free would make silence the cheapest thing to
buy.

## The order things are filtered

1. **Capability** — accounts that are disabled, models that cannot call tools,
   and image/video/audio/embedding/TTS models.
2. **Settings** — workers and models turned off for this project, or for every
   project, in the app's Settings › Models.
3. **Loved** — a directory's loved model takes the work here, before anything
   below runs, and only when the caller named no profile and no model. Steps 1
   and 2 still bind it; availability and quota can still send it back to this
   list.
4. **Model hint** — exact ID, then bare name, then substring. A hint matching
   nothing is an error, not a fallback.
5. **Policy** — the `allow` list for this task class.
6. **Availability** — recorded `unavailable` accounts.
7. **Quota** — the 98% cutoff, automatic path only.
8. **Floor** — models below the capability tier the difficulty demands.

Three of these give way rather than leave a task with nowhere to go, in this
order, each named in `relaxed` on the route and on the task record:

1. **Floor** — drops a tier at a time until something clears. Oga's own
   inference about the work, so it goes before anything the user wrote.
2. **Policy** — the whole `allow` list, automatic path only, no model hint.
3. **Quota** — the 98% cutoff. Last, because a run that dies part-way through is
   the worst of the three outcomes.

Each is tried on its own before both go together, so neither is dropped when it
was not the thing in the way. Settings, tool-calling capability, and recorded
unavailability never give way: the first is the person's own instruction and the
other two make the run fail on arrival.

Survivors are scored on quality, cost, and speed, weighted by `preference`, less
the usage penalty. Ties break on the policy's `allow` order (only when the
caller named the profile), then score, then which account's weekly headroom is
closest to being lost, then profile ID, then model ID.

### Perishable headroom breaks a tie

A usage window resets on a schedule, and whatever share of it went unused is
gone at that point — it does not carry over. When two survivors are otherwise
equal, Oga prefers the one whose weekly window has the most to lose the
soonest: a large remaining share close to its reset date, over an account that
still has days of runway. An account already past `LOW_HEADROOM_PERCENT` (90%)
of its week is not a target either way — there is nothing left there worth
racing the clock for.

This reads the **weekly** window, not the session one: a session recycles every
few hours, so unused headroom in it comes back before it matters, but a week
resets once and a share left unspent there is actually wasted. It only ever
breaks a tie the score already produced — it cannot lower the model tier a
difficulty demands, cannot override a caller-named profile or model, and stays
silent for a provider that reports no usage. When it decides the winner, the
route's `warnings` name the account, how much of its week is left, and when
that window resets.

## When nothing survives

Selection fails with a structured error rather than a guess:

```
code: "no_eligible_model"
rejected: [{ profileId, model, stage, reason, retryAt? }]
earliestRetryAt: "2026-08-06T09:40:00.000Z"
```

`stage` is one of `floor`, `quota`, `availability`, `catalog`, `policy`,
`settings`, `capability`, `profile`. Rejections are ordered most-informative first and
capped at 12 — a full list of every model a policy was never going to allow is
noise. `earliestRetryAt` is there so a caller who learns an account frees up in
forty minutes can wait instead of guessing at another one.

## Reading back

Use `models` for the one capacity read before an explicit destination. Its
default view is preferred, enabled models, plus the loved one; use
`onlyPreferred: false` to widen that view, or `onlyEnabled: false` to inspect
disabled models. Each row can be passed directly to `delegate`.

`delegate` without a destination keeps automatic selection. It uses the default
profile, the configured preference, and low effort for the regular path; an
explicit profile, model, difficulty, or effort still wins.

`delegate` returns its decision as `selection` on its response, and stores
it on the task row. It records `decidedBy` — `router`, `caller-profile`, or
`caller-explicit` — so a good routing call and a lucky caller guess stay
tellable apart when the outcome is read later.
