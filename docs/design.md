# Design rules

These rules set the spacing, layout, type, and radii for the Oga desktop app
(`web/`). Settings is built on them, and every new screen follows them. They
live in code as well as here: the tokens are in the `:root` block of
`web/src/oga.css`, and the layout pieces are `PageHeader`, `Section`, `Card`,
and `CardRow` in `web/src/components/primitives/Page.tsx`.

Use a token or a shared class. If a value you need is missing, add a token
here and in `:root` first. Do not type a raw pixel value into a rule.

## Spacing scale

All spacing sits on a 4px base. The small steps (1 to 3) set the gaps inside
a control or row and stay tight. The larger steps (4 to 8) set the room
between and around things, and they are generous so screens breathe.

| Token | Value | Typical use |
| --- | --- | --- |
| `--space-1` | 4px | Title to description in a row or section header; between rail items |
| `--space-2` | 8px | Tight groups: icon to text in small controls, page title to description, section label to its card |
| `--space-3` | 12px | Icon to label in the rail; pill inner padding; row vertical padding; between inline controls |
| `--space-4` | 20px | Row side padding; rail padding; the list gutter |
| `--space-5` | 28px | Between rail groups; text to control; below the page header; around the home card |
| `--space-6` | 40px | Between sections; narrow-window page top padding |
| `--space-7` | 56px | Above the page title; content side padding; dialog margin |
| `--space-8` | 80px | Below the last section |

Spacing tokens set gaps, never the size of a thing. A field, track, or
skeleton block that needs a height uses a control token
(`--control-height`, `--control-height-small`, `--nav-item-height`) or its
own `rem` size, so a change to the scale never resizes a control.

## Motion

Motion shows what changed. It is never decoration.

- 150 to 250ms, `--ease-out`, and only on `opacity`, `scale`, or
  `translate`, so layout never moves. Use the `scale` and `translate`
  properties, not `transform`, so motion never clobbers a positioned element.
- Interruptible: CSS transitions with `@starting-style` for entrances. A
  keyframe only where the element already exists, like a row that moves.
- Nothing moves on load, on scroll, or on a keystroke. Keyboard actions such
  as task search open at once.
- Reduced motion cuts every duration to almost zero.

| What | Motion |
| --- | --- |
| Pressing a button (`.icon-button`, `.text-button`, `.settings-button`) | Scales to 0.97, 160ms |
| A menu opening | Fades in from 0.97 scale, from its top edge, 150ms |
| A dialog opening | Fades in from 0.97 scale over its fading backdrop, 200ms |
| A task status mark changing | Grows in from 0.4 scale, 200ms, only when it changed while shown |
| A task arriving or changing status in the sidebar or on home | Fades in and drops 4px into place, 200ms |

## Radii

| Token | Value | Use |
| --- | --- | --- |
| `--radius-chip` | 4px | Inline code, key caps, small tags, checkboxes |
| `--radius-control` | 8px | Buttons, icon buttons, inputs, selects, pickers |
| `--radius-row` | 8px | List rows and their hover and selection fill, inset blocks |
| `--radius-panel` | 12px | Cards, the transcript card, the changes panel, the reply box, menus, popovers, toasts |
| `--radius-window` | 20px | Dialogs, the home card |
| `--radius-full` | 999px | Capsules: scrollbars, switch tracks, bars |

Nested corners stay concentric: an element inside a rounded surface takes
the outer radius minus the gap between them, never more than the outer
radius. A menu (12px) with 4px padding gives its rows 8px; a popover with
8px padding gives them `calc(var(--radius-panel) - var(--space-2))`. Once the
gap reaches the outer radius, the inner element keeps its own token.

## Type

| Token | Value | Use |
| --- | --- | --- |
| `--text-title` | 24px | Page title, weight 600 |
| `--text-lg` | 16px | Dialog title in the rail |
| `--text-base` | 13px | Body text, row titles, row descriptions (muted), section labels (600, muted) |
| `--text-md` | 12px | Rail and task list group labels, small captions, and every muted meta line in the task view: title bar facts, `+`/`−` counts, the changed files summary, the line under the composer |
| `--text-reading` | 14px | What the worker says and what the user sends, in the task view |

Descriptions use `--color-text-muted` and stop at `65ch`.

Reading text uses `--leading-reading` (1.5) and stops at `--reading-max`
(`72ch`, about 70 characters a line).

## Controls

| Token | Value | Use |
| --- | --- | --- |
| `--control-height` | 28px | Text buttons, icon buttons, selects, and source pickers in the same row |
| `--control-height-small` | 24px | Compact menu, toast, and inline controls |
| `--control-padding-block` | `--space-1` (4px) | Top and bottom of text controls |
| `--control-padding-inline` | `--space-3` (12px) | Left and right of standard text controls |
| `--control-padding-inline-small` | `--space-2` (8px) | Left and right of compact controls |
| `--control-line-height` | 1.3 | Single-line control labels |

Standard controls use `--text-base` (13px), a 1px border, and
`--radius-control`. Their height includes the padding and border. Icon
buttons are square and centre their icons. Menu items use the compact height;
navigation rows, file rows, and image previews keep their content-driven
heights. On coarse pointers, text buttons and selects use 8px block padding
and compact controls grow to `--control-height`, so touch targets stay large.

## Layout

### Sidebar (rail)

- Its own full-height panel on `--color-sidebar`, with a hairline to its
  right. It is 15rem wide.
- `--space-4` (20px) padding on the sides and bottom, `--space-5` on top.
- Search field at the top, `--nav-item-height` (32px) tall, `--radius-control`.
- Items sit in groups under a small muted label (`--text-md`, 600). Groups are
  `--space-5` apart, and items in a group touch.
- Each item is an icon and a label, `--nav-item-height` (32px) tall, with
  `--space-3` inside padding and `--space-3` between icon and label. Icons
  come from `web/src/ui/icons.tsx`.
- The selected item is a pill: `--color-selection` background, `--radius-row`.
- Two shared edges: every box (search field, pills) starts at the rail
  padding, and every piece of text or icon (title, group labels, search icon,
  item icons) starts `--space-3` inside it.
- Below 800px wide the rail stacks over the page. Items flow into three
  columns (two below 560px), and the group labels hide.

### Content column

- The content area scrolls on its own and sits on `--color-bg`.
- One centred column, `.page-column`: at most `--settings-max` (672px) of
  content, with `--space-7` side padding, `--space-7` above the title, and
  `--space-8` below the last section.
- Below 800px: `--space-5` sides, `--space-6` top, `--space-7` bottom.
- Home is the one wider screen: its card is at most `--home-max` (72rem)
  wide, with `--space-5` around it (`--space-3` below 800px).
- `--content-max` is the one reading column every main-pane screen shares:
  empty states and the task screen's transcript, messages, and composer all
  size to it, so their left and right edges line up. Settings reads as a
  form, so it uses the narrower `--settings-max`.

### Task list

- Rows are the title alone; worker, project, time and cost show on hover.
- A row's status mark shows only what its group doesn't already say: running,
  waiting, needs a reply, failed, or a new update. A finished task already
  opened has no mark, and its title stays on the same edge as the others.
- Grouped by status, Completed and Cancelled list their first 20 tasks, then
  a "Show more" row. A task open past the fold is listed under those 20.
  Every other group lists in full, and a search lists every match.

### Task title bar

- The task's status mark and title (`--text-reading`, 500) lead the bar.
- On the right, one line of muted `--text-md` text: project folder ·
  worker · model · effort, with the model's provider prefix dropped. The
  full project path, full model name, cost, time and tokens show on hover. No chip or pill behind it.
- The `+`/`−` counts keep their colours at the same size and normal weight,
  then the icon buttons.

### Task feed

- What the user sent (the brief, follow-ups, replies, instructions) sits in
  a right-aligned bubble: a light tint, no border, `--radius-panel`, at most
  `--reading-max` wide.
- What the worker says is the main text: `--text-reading`, full text colour,
  left-aligned, at most `--reading-max` wide.
- Tool calls between two worker messages fold into one muted line with a
  short label: what is running now, or at most two counts ("Edited 4 files,
  ran 11 commands"). Opened, each call is one quiet line: a verb, then its
  file, command, or pattern in monospace. Its result shows once that line is
  opened.
- A turn's header is a short status ("Working · 24 min · step 1 of 5"); the
  worker's words stay in the feed.

### Composer

- One box at `--text-reading`. Its placeholder says what to type and which
  keys send ("Send a message · ⌘↵ to send now, ⌥↵ to queue"); the send
  button sits inside the box.
- One quiet `--text-md` line under it: what the run may change and its
  branch on the left ("Can edit · feat/x"), and while a run is active the
  Send now | Queue switch on the right: a tinted track, the chosen part
  raised on the box's colour.

### Page header — `PageHeader`

- The page title (`--text-title`) names the page and matches its rail label.
- An optional description sits `--space-2` under it.
- Page actions (refresh, add, a scope picker, a "checked" time) sit on the
  right, vertically centred on the header, `--space-3` apart. They wrap under
  the title when there is no room.
- `--space-5` below the header.
- No eyebrow labels above titles. A heading says what the page or section is
  once.

### Section — `Section`

- A quiet label (`--text-base`, 600, muted) with an optional muted
  description `--space-1` under it, and optional actions on the right.
- `--space-2` between the header and the card.
- `--space-6` between sections: wrap them in `.page-sections`.
- Sections never clip. The content area scrolls instead.

### Card — `Card`

- `--radius-panel`, a 1px `--color-border` border, `--color-surface` background.
- Holds rows. Anything else in a card (a status line, an empty state) takes
  the same padding as a row.

### Row — `CardRow`

- `--space-3` vertical and `--space-4` horizontal padding.
- Title on top (normal weight), muted description `--space-1` under it.
- The control (switch, checkbox, select, input, button) sits on the right,
  vertically centred, `--space-5` from the text. The text wraps before it
  reaches the control. When the row is too narrow, the control drops under
  the text.
- Hairlines between rows run the full card width.
- Pass `as="label"` when the whole row should reach its one control.
- A row that opens something (a worker) stretches its open button over the
  row and keeps the control above it; a chevron at the far right shows it
  opens.

### Home

- One card fills the pane: `--color-surface`, a 1px `--color-border`
  edge, `--radius-window`, no shadow. It holds up to three columns, split
  by hairlines, and each scrolls on its own.
- Views column, `--home-side-width` (12rem), on `--color-bg`: the
  workspace (an ink square with "O", "Oga", the worker count), a 32px
  search box with a `⌘K` key cap that opens the task search, then rail-style
  rows under small labels: Tasks (Recent, Needs you, In progress, Finished)
  and, with more than one project, Projects. The current row sits on
  `--color-surface` with a hairline ring. Needs you carries a count badge,
  the column's one ink tint.
- List column: a `--text-lg` title, a muted `--text-md` subtitle, and a
  count pill on the right. Below it, the tasks as bordered cards
  `--space-2` apart, tasks that need the user first, then a muted hint.
- A task card: a `--control-height` tile tinted with the ink accent (danger
  for a problem) holding the status icon, the title (600) over a muted meta
  line (worker · project · time), a status tag, and an Open button that
  shows on hover, focus, or when the card is picked. Hover and the picked
  card take `--color-border-strong`; the picked card also takes a 2% ink
  fill.
- Preview column, `--home-preview-width` (20rem): the picked task's request
  in a tinted bubble, the worker's latest words as plain text, and a facts
  card with uppercase mono labels. Other tasks that need the user show as
  outlined pills under it. A box at the bottom with a grey square send
  button opens the task with its reply box focused.
- Below a 880px card the views column folds into tabs over the list. Below
  640px the preview hides and picking a task opens it. When the list column
  is under 480px the cards drop their Open button.

| Token | Light | Dark | Use |
| --- | --- | --- | --- |
| `--color-border-strong` | `#dcdcdc` | `#424242` | A hovered or picked card's edge, one step darker than a hairline |
| `--home-max` | 72rem | 72rem | Widest the home card grows |
| `--home-side-width` | 12rem | 12rem | The home views column |
| `--home-preview-width` | 20rem | 20rem | The home preview column |

### Changed files panel

- The top bar is one line: what is being compared on the left (a quiet
  label that opens the source menu) with refresh beside it for git-backed
  sources, then icon buttons on the right: go to a file, more options, full
  screen, close. The unified or split choice, order, grouping, open or close
  all, and the review keys live in the more options menu.
- Counts and review progress sit in one muted line above the first file.
- A file's header is one line: the file name (600), its folder in muted
  text, then the `+`/`−` counts. Per-file actions sit on the right.
- Changed lines take a soft tint and a thin coloured edge in the line
  number gutter. Changed words inside them take a slightly stronger tint.
- Skipped lines between two hunks show as a row that says how many lines
  are hidden ("26 unmodified lines").
- A long line wraps with its continuation indented under the code.

| Token | Light | Dark | Use |
| --- | --- | --- | --- |
| `--diff-line-tint` | 6% | 9% | How much of the status colour tints an added or removed line |
| `--diff-gutter-tint` | 4% | 7% | The same, for that line's number |
| `--diff-word-tint` | 14% | 20% | The changed words inside a changed line |
| `--diff-marker` | 2px | 2px | Width of the coloured edge on a changed line's number |

## Dialogs

The settings dialog keeps `--space-7` clear on every side of the window
(`--space-4` below 800px), uses `--radius-window`, and places its close button
`--space-5` from the corner.
