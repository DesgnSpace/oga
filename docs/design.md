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
| `--radius-sm` | 4px | Key caps |
| `--radius` | 6px | Buttons, inputs, selects |
| `--radius-md` | 8px | Rail pills, the rail search field |
| `--radius-lg` | 12px | Cards, messages, the settings dialog |

## Type

| Token | Value | Use |
| --- | --- | --- |
| `--text-title` | 28px | Page title, weight 600 |
| `--text-heading` | 17px | Section heading, weight 600 |
| `--text-lg` | 16px | Dialog title in the rail |
| `--text-base` | 13px | Body text, row titles (600), row descriptions (muted) |
| `--text-md` | 12px | Rail group labels, small captions |

Descriptions use `--color-text-muted` and stop at `65ch`.

## Layout

### Sidebar (rail)

- Its own full-height panel on `--color-sidebar`, with a hairline to its
  right. It is 16rem wide.
- `--space-5` (24px) padding on every side.
- Search field at the top, 32px tall, `--radius-md`.
- Items sit in groups under a small muted label (`--text-md`, 600). Groups are
  `--space-5` apart, and items in a group are `--space-1` apart.
- Each item is an icon and a label, `--nav-item-height` (40px) tall, with
  `--space-3` inside padding and `--space-3` between icon and label. Icons
  come from `web/src/ui/icons.tsx`.
- The selected item is a pill: `--color-selection` background, `--radius-md`.
- Two shared edges: every box (search field, pills) starts at the rail
  padding, and every piece of text or icon (title, group labels, search icon,
  item icons) starts `--space-3` inside it.
- Below 800px wide the rail stacks over the page. Items flow into three
  columns (two below 560px), and the group labels hide.

### Content column

- The content area scrolls on its own and sits on `--color-bg`.
- One centred column, `.page-column`: at most `--content-max` (1000px) of
  content, with `--space-7` side padding, `--space-8` above the title, and
  `--space-8` below the last section.
- Below 800px: `--space-5` sides, `--space-6` top, `--space-7` bottom.

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

- `--radius-lg`, a 1px `--color-border` border, `--color-surface` background.
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

## Dialogs

The settings dialog keeps `--space-7` clear on every side of the window
(`--space-4` below 800px), uses `--radius-lg`, and places its close button
`--space-5` from the corner.
