# Design rules

These rules set the spacing, layout, type, and radii for the Oga desktop app
(`web/`). Settings is built on them, and every new screen follows them. They
live in code as well as here: the tokens are in the `:root` block of
`web/src/oga.css`, and the layout pieces are `PageHeader`, `Section`, `Card`,
and `CardRow` in `web/src/components/primitives/Page.tsx`.

Use a token or a shared class. If a value you need is missing, add a token
here and in `:root` first. Do not type a raw pixel value into a rule.

## Spacing scale

All spacing sits on a 4px base.

| Token | Value | Typical use |
| --- | --- | --- |
| `--space-1` | 4px | Title to description in a row or section header; between rail items |
| `--space-2` | 8px | Tight groups: icon to text in small controls, page title to description |
| `--space-3` | 12px | Icon to label in the rail; pill inner padding; between inline controls |
| `--space-4` | 16px | Section heading to its card; row vertical padding |
| `--space-5` | 24px | Row horizontal padding; rail padding; between rail groups; text to control |
| `--space-6` | 32px | Rail search field height; narrow-window page top padding |
| `--space-7` | 48px | Between sections; below the page header; content side padding; dialog margin |
| `--space-8` | 64px | Above the page title |

There is no 20px step. The existing tokens already name 24px as `--space-5`,
and renumbering them would move every screen. Where a reference calls for
20–24px, use 24px; where it calls for 16–20px, use 16px.

## Radii

| Token | Value | Use |
| --- | --- | --- |
| `--radius-chip` | 4px | Inline code, key caps, small tags, checkboxes |
| `--radius-control` | 8px | Buttons, icon buttons, inputs, selects, pickers |
| `--radius-row` | 8px | List rows and their hover and selection fill, inset blocks |
| `--radius-panel` | 12px | Cards, the transcript card, the changes panel, the reply box, menus, popovers, toasts |
| `--radius-window` | 20px | Dialogs |
| `--radius-full` | 999px | Capsules: scrollbars, switch tracks, bars |

Nested corners stay concentric: an element inside a rounded surface takes
the outer radius minus the gap between them, never more than the outer
radius. A menu (12px) with 4px padding gives its rows 8px; a popover with
8px padding gives them `calc(var(--radius-panel) - var(--space-2))`. Once the
gap reaches the outer radius, the inner element keeps its own token.

## Type

| Token | Value | Use |
| --- | --- | --- |
| `--text-title` | 28px | Page title, weight 600 |
| `--text-heading` | 17px | Section heading, weight 600 |
| `--text-lg` | 16px | Dialog title in the rail |
| `--text-base` | 13px | Body text, row titles (600), row descriptions (muted) |
| `--text-md` | 12px | Rail group labels, small captions |
| `--text-reading` | 14px | What the worker says and what the user sends, in the task view |

Descriptions use `--color-text-muted` and stop at `65ch`.

Reading text uses `--leading-reading` (1.65) and stops at `--reading-max`
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
  right. It is 16rem wide.
- `--space-5` (24px) padding on every side.
- Search field at the top, 32px tall, `--radius-control`.
- Items sit in groups under a small muted label (`--text-md`, 600). Groups are
  `--space-5` apart, and items in a group are `--space-1` apart.
- Each item is an icon and a label, `--nav-item-height` (40px) tall, with
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
- One centred column, `.page-column`: at most `--content-max` (800px) of
  content, with `--space-7` side padding, `--space-8` above the title, and
  `--space-8` below the last section.
- Below 800px: `--space-5` sides, `--space-6` top, `--space-7` bottom.
- `--content-max` is the one reading column every main-pane screen shares:
  settings, empty states, and the task screen's transcript, messages, and
  composer all size to it, so their left and right edges line up.

### Task list

- Rows are the title alone; worker, project, time and cost show on hover.
- A row's status mark shows only what its group doesn't already say: running,
  waiting, needs a reply, failed, or a new update. A finished task already
  opened has no mark, and its title stays on the same edge as the others.
- Grouped by status, Completed and Cancelled list their first 20 tasks, then
  a "Show more" row. Every other group lists in full, and a search lists
  every match.

### Task title bar

- The task's status mark and title (`--text-reading`, 500) lead the bar.
- On the right, one line of muted `--text-md` text: worker · model ·
  effort, with the model's provider prefix dropped. The project, full model
  name, cost, time and tokens show on hover. No chip or pill behind it.
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

### Page header — `PageHeader`

- The page title (`--text-title`) names the page and matches its rail label.
- An optional description sits `--space-2` under it.
- Page actions (refresh, add, a scope picker, a "checked" time) sit on the
  right, vertically centred on the header, `--space-3` apart. They wrap under
  the title when there is no room.
- `--space-7` below the header.
- No eyebrow labels above titles. A heading says what the page or section is
  once.

### Section — `Section`

- A heading (`--text-heading`) with an optional muted description
  `--space-1` under it, and optional actions on the right.
- `--space-4` between the header and the card.
- `--space-7` between sections: wrap them in `.page-sections`.
- Sections never clip. The content area scrolls instead.

### Card — `Card`

- `--radius-panel`, a 1px `--color-border` border, `--color-surface` background.
- Holds rows. Anything else in a card (a status line, an empty state) takes
  the same padding as a row.

### Row — `CardRow`

- `--space-4` vertical and `--space-5` horizontal padding.
- Title on top (600), muted description `--space-1` under it.
- The control (switch, checkbox, select, input, button) sits on the right,
  vertically centred, `--space-5` from the text. The text wraps before it
  reaches the control. When the row is too narrow, the control drops under
  the text.
- Hairlines between rows are inset by the row padding, not full width.
- Pass `as="label"` when the whole row should reach its one control.
- A row that opens something (a worker) stretches its open button over the
  row and keeps the control above it; a chevron at the far right shows it
  opens.

### Changed files panel

- The top bar is one line: what is being compared on the left (a quiet
  label that opens the source menu), then icon buttons on the right: go to
  a file, more options, full screen, close. The unified or split choice,
  order, grouping, open or close all, refresh, and the review keys live in
  the more options menu.
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
