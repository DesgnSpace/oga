# Button and control sizing audit

CSS-derived sizes at the app's 16px root size and the body's 13px type with
1.35 line height. These are box heights, including border and padding: for a
single-line control, use the greater of its declared height or minimum and its
line box plus vertical padding and borders. Browser default padding on buttons
without an explicit padding rule is not specified by the app. No browser
measurement was available for this pass.

`V × H` means vertical and horizontal padding in pixels. `≥` means the box
also depends on its contents. The values before the arrow are from the prior
CSS; those after it are the chosen desktop scale.

| Variant | Before: height; padding | After: height; padding |
| --- | --- | --- |
| Settings buttons, including primary and danger | 35.55; 8 × 16 | 28; 4 × 12 |
| Plain text buttons | 35.55; 8 × 16 | 28; 4 × 12 |
| Changed-files source picker | 35.55; 8 × 16 | 28; 4 × 12 |
| Shared icon buttons, including task header, changed-files header, sidebar, and modal close | 28 square; browser padding | 28 square; 0 |
| Recovery action | 35.55; 8 × 16 | 28; 4 × 12 |
| Task actions | 28; 4 × 10 | 28; 4 × 12 |
| Load-more action | 28; 4 × 8 | 28; 4 × 12 |
| Menu items, including source and task menus | 24; 3 × 6 | 24; 2 × 8 |
| Composer send icon | 28 square; browser padding | 28 square; 0 |
| Composer send-now action | 28; 4 × 8 | 24; 2 × 8 |
| Copy icon over messages and code | 32 square; 4 | 28 square; 4 |
| Search clear icon | 28 high × 24 wide; browser padding | 24 square; 0 |
| Toast close icon | 28 high × 24 wide; browser padding | 24 square; 0 |
| Toast action | 28 minimum; 0 | 24 minimum; 0 |
| Toast more and clear-all actions | 28 minimum; 4 × 8 | 24 minimum; 0 × 8 |
| Trace card toggle | 28; 4 × 4 | 28; 4 × 12 |
| Trace scroll-to-bottom icon | 28 high × 24 wide; browser padding | 24 square; 0 |
| Usage tabs | 28 minimum; 4 × 12 | 28; 4 × 12 |
| Usage period buttons | 28 minimum; 4 × 8 | 28; 4 × 12 |
| Diff view toggle | 24 minimum; 2 × 8 | 24 minimum; 2 × 8 |
| Scope selects | 35.55; 8 × 12 | 28; 4 × 12, with room for the arrow |
| Handoff selects | 35.55; 8 × 12 | 28; 4 × 12, with room for the arrow |
| Worker-form selects and inputs | 28 fixed; 4 × 8 | 28 fixed; 4 × 12 |
| Usage sort select | 28 minimum; 2 × 4 | 28; 4 × 12, with room for the arrow |

Full-row and content controls keep their own dimensions. Sidebar group and task
rows have a 32px minimum with 4 × 12 and 8 × 8 padding respectively; settings
navigation rows have a 40px minimum with 0 × 12 padding. Changed-file headings
have a 32px minimum with 0 × 8 padding, while tree rows and turn headings have
a 28px minimum. File attachment buttons are 28px minimum with 4 × 8 padding;
image previews are 48px square. Metadata chips and diff-view buttons have a
24px minimum. Inline underlined actions in the offline banner and transcript
have a 28px minimum. Worker rows and file previews cover their content and do
not have a single button height. None of these row or content controls changed.

On coarse pointers, changed controls retain at least their prior height. The
copy icon remains 48px square. Text buttons and selects whose desktop height
shrunk recover their previous line height and padding; compact controls with a
prior 28px minimum regain that minimum.
