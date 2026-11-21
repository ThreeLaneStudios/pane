# Card Design Options

Design explorations for Pane's card list component, solving inconsistent metadata positioning.

## Problem
- Status badge and folder name fight for the same top-right spot
- Priority floats randomly
- Card heights vary wildly
- Date wraps on narrow cards

## Files
- `card-options-wide.html` — 3 design options (A, B, C) for wide cards (~480px, editor closed)
- `card-options-narrow.html` — 3 design options (A, B, C) for narrow cards (~250px, editor open)
- `editor-open.png` — Screenshot of current narrow state (reference)
- `editor-closed.png` — Screenshot of current wide state (reference)

## Design Options

| Option | Layout | Wide Height | Narrow Height | Best For |
|--------|--------|-------------|---------------|----------|
| **A** | Two-Row (title+status / pill strip) | 68px | 54px | Density, power users |
| **B** | Three-Row (title / preview / metadata strip) | 88px | 54px | Content-heavy notes |
| **C** | Grid (title+status / preview / labeled 2-col grid) | 96px | 66px | Metadata-heavy workflows |

## Key Design Decisions
- **Fixed positions**: Empty metadata uses `visibility: hidden` — space is reserved, nothing shifts
- **Consistent heights**: Fixed per option, regardless of metadata density
- **Narrow drops**: Preview text + assignee dropped in all narrow variants. Date shortened to "Apr 02"
- **Dark Tron aesthetic**: #0a0a0f bg, rgba(255,255,255,0.05) cards, #00E4E4 teal accent, no emojis

## How to Review
Open the HTML files in a browser. Each shows 4 example cards side-by-side in 3 design variants:
1. Full metadata (all fields populated)
2. Minimal (title + folder only)
3. Partial (status + priority + date)
4. Empty (title only)
