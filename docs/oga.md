# Working with `oga`

`oga oga` is a terminal for one conversation with a capable assistant that
hands its legwork to cheaper workers. You talk to it, it plans, and it delegates
through Oga. When a worker finishes, gets stuck, or asks a question, oga
carries that news back into the same conversation on its own — nobody watches,
nobody polls, and the terminal is never blocked waiting.

The shape it is built for: an expensive model doing the thinking, a cheap model
doing the reading and the mechanical edits.

## Run it

The broker must be running first.

```sh
oga serve &
oga oga --worker <profile> [options]
```

`oga oga` opens the screen with the prompt focused. It does not start the
assistant or call a model until you send your first message, so startup is
instant (only a broker health check runs first).

The command accepts these options:

```text
usage: oga oga [options]
  --worker <profile>  Choose the assistant you talk to. Any provider profile works.
  --model <id>        Choose its model.
  --cwd <path>        Set the working directory.
  --goal <text>       Send this as the first message. Without it, --print reads the message from stdin.
  --confirm[=all]     Tell the assistant to ask you before work is stopped, archived, or removed. Add =all to be asked before any change.
  --fresh             Start a new session, discarding any saved one.
  --print             Run without the full screen and write each step as a line.
  --json              Run like --print and write each step as JSON.
  --help              Show this help.
```

`--worker`, `--model`, `--cwd`, and `--goal` accept a separate value or
`=value`. A passed `--goal` is treated as the first message.

## An example run

```sh
oga oga --worker claude-work --model fable \
  --goal "I am adding billing to the API. Plan it, and send the mechanical parts out."
```

The assistant plans in prose, then calls Oga's `delegate` tool with a full
brief and a cheap model. You keep typing to it while that worker runs. When the
worker finishes, its report lands in the conversation and the assistant judges
it — no keypress, no `oga watch`, no second terminal.

## Running it without the screen

`--print` runs the same session with no full-screen terminal. It takes its first
message from `--goal` or from standard input, writes one line per step to
standard output, and ends when the assistant has finished and no work is still
running:

```sh
oga oga --worker <profile> --print --goal "get the release branch green"
echo "get the release branch green" | oga oga --worker <profile> --print
```

Each line names who it came from or what happened:

```text
you: get the release branch green
oga: Sending the test sweep to a cheaper worker.
task 4f21ac8e running — Run the suite
update: Run the suite is completed
oga: Two specs fail for the same reason. Here is the fix.
end: the assistant finished and no work is still running
```

`--json` writes the same steps as one JSON object per line, so a script can read
the session without parsing prose. Every object carries a `type` of `line`,
`relay`, `answer`, `task`, `status`, or `end`.

Nobody is at the keyboard in a headless run. If a worker asks a question and the
assistant answers it in prose rather than with its own tool, oga forwards that
reply to the worker so the run is not left waiting on a person who is not there.

### Exit codes

| Code | Meaning |
| --- | --- |
| `0` | The session ended cleanly. |
| `1` | The session ended in an error, or could not be reached. |
| `2` | The options could not be read, or `--print` had no message to start from. |

## The screen

- **State line:** The session state (`Ready`, `Starting`, `Running`, `Stopped`)
  and, when the link drops, a reconnection notice.
- **Working in:** The current directory, the assistant's profile, and its model.
- **Work panel:** Each task's state mark, title, short id, and model, with its
  latest activity as an indented line.
- **Delegated work:** One dim row per recent piece of work, carrying its status
  and the last thing that worker said. Work stays listed after it finishes.
- **Conversation:** Your messages, the assistant's replies rendered as formatted
  prose (bold, inline code, wrapped at word boundaries), and one line per piece
  of news that oga carried in. Assistant output streams in as it arrives;
  internal protocol markers never appear here.
- **Tool rows:** What the assistant itself is doing right now, one labelled row
  per call.
- **Composer:** Up to five lines of the message you are preparing. Longer
  messages stay intact while the rows nearest the cursor remain visible.
- **Status line:** What oga is doing. While the assistant is answering it reads
  `Thinking…`; once its reply has landed it names what is left — `Waiting on 1
  piece of work.` while handed-out work is still running, or `Nothing running.
  Type a message to continue.` when none is.
- **Keys line:** The available keys.

## Keys

| Key | Use |
| --- | --- |
| `Enter` | Send your text. |
| `Shift-Enter` / `Alt-Enter` | Start a new line. |
| `Backspace` | Delete the previous character. |
| `Left` / `Right` | Move the cursor within your text. |
| `Home` / `End` | Jump to the start or end of your text. |
| `Up` / `Down` | Recall earlier messages, then return to your draft. |
| `Esc` | Cancel the current response, or close an open task. |
| `Ctrl-S` | Cancel the current response. |
| `Ctrl-C` | Cancel the current response; press it again to quit. |
| `Tab` / `Shift-Tab` | Pick the next or previous piece of delegated work. |
| `Ctrl-O` | Open the picked task's own output. |
| `Page Up` / `Page Down` | Scroll the transcript five lines. |

Cancelling stops the assistant's current response and keeps your history. A new
message resumes the same session. `Ctrl-C` with no response in progress detaches:
the assistant is stopped but its row and conversation are kept in
`~/.oga/oga/<hash>.json`. A new `oga oga` in the same directory reattaches
automatically; use `--fresh` to discard the saved session and start over.
Reattaching replays the recent conversation onto the screen and tells the
assistant about any work that finished while nothing was attached, once per
piece of work.

## Commands

Commands run in oga instead of being sent to the assistant:

| Command | Use |
| --- | --- |
| `/help` | Show the available commands. |
| `/clear` | Clear the conversation from this screen. |
| `/cancel` | Cancel the current response. |
| `/quit` | Leave oga. |

## What the assistant can do

The assistant is a normal agent session. It thinks and writes freely, and it
reaches Oga through the MCP tools every Oga caller has — `delegate`,
`tasks`, `inspect`, `reply`, `steer`, `resume`, `cancel`, `handoff`, `complete`,
`archive`, `models`, `query`, `memory`. It also has read-only file tools so it
can judge what it is handing out. Editing files is the workers' job.

Work it hands out is stamped with its session id, so oga sees every task it
started without being told about each one.

## What oga carries back

oga watches the broker's event stream for the tasks this session started and
continues the conversation whenever one of them moves:

- **Finished:** which task, that it finished, and the end of its report.
- **Failed or stopped:** which task and why.
- **Blocked:** which task, what it said, and that a decision is needed.
- **Asked a question:** which task and the question itself.

Several changes that land close together arrive as one message rather than one
each, so a batch of workers finishing at once costs a single turn.

Your own messages and this news share one queue in arrival order, so neither can
starve the other and nothing is delivered twice or out of sequence. An idle
session sends nothing at all: no timers, no polling, no tokens spent waiting.

## Confirmation

Without `--confirm`, the assistant works without checking in.

`--confirm` tells it to ask you before it stops, archives, or removes work, and
to wait for your answer. `--confirm=all` asks it to check in before any change
at all. Because the assistant reaches the broker directly, this is an
instruction it follows rather than a lock oga holds — the honest way to say it
is that `--confirm` sets the assistant's operating rule.

## Recovery

- **A message that will not land:** oga retries three times, waiting one, three,
  then nine seconds, showing a line each try. Once the budget is spent the run
  ends in an error and `oga oga` exits `1`.
- **A worker left waiting:** a question the assistant answered with its own
  `reply` tool is already handled. One it only talked about is forwarded to the
  worker at the end of that turn.
- **Broker offline:** the event stream reconnects with backoff and the state
  line says so; the conversation is not lost.

## Harnesses

`--worker` takes any provider profile — claude, codex, opencode, antigravity, or
pi. The header shows which one is driving and how:

- **live** (claude, antigravity): the session holds an open channel, so a
  message reaches it while it works.
- **turn-based** (codex, opencode, pi): each message resumes the same provider
  session once the previous run lands. Honest about being between-turns;
  nothing pretends to be live.

Both carry the Oga MCP server whatever the provider, so the same tools are
available either way.

## The agent lane

The assistant runs in Oga's separate agent lane. It does not appear in the
default task list and cannot be addressed through task routes. The terminal
keeps its row on detach so a later `oga oga` can reattach; `--fresh` removes
it. Tasks it creates retain its id, so their origin remains visible after the
run ends. Each formatted worker detail is capped at 8 KiB, and each relayed
report is trimmed to its last 2,000 characters. The broker remains the single
writer; oga never touches the store.
