// Pane Sheets — 1 · Grid editor
// Sample sheet used by every section: "Q3 Content Budget".

const BCOLS = [
  { n: 'Line item', t: 'text', w: 196 },
  { n: 'Owner', t: 'text', w: 116 },
  { n: 'Status', t: 'select', w: 124 },
  { n: 'Units', t: 'number', w: 82 },
  { n: 'Rate', t: 'currency', w: 112, code: 'USD' },
  { n: 'Total', t: 'currency', w: 118, code: 'USD' },
  { n: 'Due', t: 'date', w: 120 },
  { n: 'Signed off', t: 'checkbox', w: 94 },
];
const R = (item, owner, status, units, rate, total, due, done) => ({
  cells: [{ v: item }, { v: owner }, { v: status }, { v: units }, { v: rate }, { v: total, k: 'calc' }, { v: due }, { v: done }],
});
const BROWS = [
  R('Longform essays', 'Dana', 'Committed', '6', '1,400.00', '8,400.00', 'Jul 21, 2026', true),
  R('Newsletter design', 'Priya', 'Committed', '1', '2,600.00', '2,600.00', 'Jul 28, 2026', true),
  R('Illustration set', 'Marco', 'In flight', '12', '320.00', '3,840.00', 'Aug 04, 2026', false),
  R('Video edit pass', 'Dana', 'In flight', '3', '900.00', '2,700.00', 'Aug 11, 2026', false),
  R('Paid amplification', 'Sam', 'Proposed', '1', '5,000.00', '5,000.00', 'Aug 18, 2026', false),
  R('Podcast pilot', 'Priya', 'Proposed', '2', '1,150.00', '2,300.00', 'Sep 01, 2026', false),
  R('Community AMA', 'Marco', 'Proposed', '4', '275.00', '1,100.00', 'Sep 08, 2026', false),
  R('Contingency', '', 'Proposed', '1', '2,000.00', '2,000.00', '', false),
];

// the whole editor stack, reused across artboards
function SheetPanel({
  w = 1000, formula, fref = 'F3', editing, view, paused, toolbarOpen, grid = {}, children, rows = BROWS, cols = BCOLS,
}) {
  return (
    <>
      <SheetHead />
      <Toolbar open={toolbarOpen} styleSwatch={grid.swatch} />
      {paused && paused}
      <FormulaBar cref={fref} src={formula} editing={editing} />
      {view && view}
      <SheetGrid cols={cols} rows={rows} {...grid} />
      <StatusBar cols={cols.length} rows={rows.length} formulas={grid.formulas != null ? grid.formulas : 8} paused={!!paused} />
      {children}
    </>
  );
}

// ── 1a · in context ─────────────────────────────────────────────────
function ShInContext() {
  return (
    <Stage w={1440} h={900} radius={0} border={false}>
      <AppShell overlayWidth={1000}>
        <SheetPanel formula="=D3*E3" fref="F3" grid={{ sel: { r: 2, c: 5 } }} />
      </AppShell>
    </Stage>
  );
}

// ── 1b · overflow + sticky bands ────────────────────────────────────
function ShOverflow() {
  const ref = React.useRef(null);
  React.useEffect(() => { const el = ref.current && ref.current.querySelector('.sh-gridwrap'); if (el) { el.scrollLeft = 300; el.scrollTop = 96; } }, []);
  return (
    <Frame>
      <div ref={ref}>
        <Stage w={860} h={400}>
          <div style={{ display: 'flex', flexDirection: 'column', height: '100%' }}>
            <FormulaBar cref="E6" src="1,150.00" />
            <SheetGrid cols={BCOLS} rows={BROWS} sel={{ r: 5, c: 4 }} />
            <StatusBar cols={8} rows={8} formulas={8} />
          </div>
        </Stage>
      </div>
      <Cap callout="scrolled 300px right · 96px down" title="Nothing squishes. It scrolls.">
        <p>Columns keep the width they were given, so a sheet wider than the overlay overflows on both axes rather than compressing. The header band and the row-number gutter are sticky, and the corner cell is sticky on both — so the reference frame never leaves the screen.</p>
        <Anno><b>Sticky bands sit on <code>surface-1</code></b>, one step up from the canvas, and use the stronger border token. Scrolled cells pass underneath, never through.</Anno>
        <Anno>Column letters live in the header at <code>10px</code> mono next to the type glyph, so A1 references in the formula bar are always readable off the grid itself.</Anno>
        <Anno tone="mute">Row numbers are mono and tabular — a 4-digit row is the same width as a 1-digit one, so the gutter never reflows mid-scroll.</Anno>
      </Cap>
    </Frame>
  );
}

// ── 1c · ghost space ────────────────────────────────────────────────
function ShGhost() {
  const cols = BCOLS.slice(0, 4);
  const rows = BROWS.slice(0, 4).map((r) => ({ cells: r.cells.slice(0, 4) }));
  return (
    <Frame stack>
      <div style={{ display: 'flex', gap: 40, alignItems: 'flex-start' }}>
        <Stage w={720} h={330}>
          <SheetGrid cols={cols} rows={rows} ghostCols={3} ghostRows={4} />
        </Stage>
        <Cap callout="at rest" title="The grid always has room past the edge.">
          <p>Three ghost columns and four ghost rows render beyond the last real one. Their letters and numbers are present but at 55% opacity, and their hairlines are a third the weight of real grid lines — the grid reads as continuing rather than ending.</p>
          <Anno>Ghost headers carry a letter and <b>no name</b>. There is nothing to click and nothing to configure until the column exists.</Anno>
          <Anno tone="mute">Hovering a ghost cell tints it with <code>overlay-hover</code> — the same hover the real cells use, which is what tells you it's typeable.</Anno>
        </Cap>
      </div>
      <div style={{ display: 'flex', gap: 40, alignItems: 'flex-start' }}>
        <Stage w={720} h={330}>
          <SheetGrid cols={cols} rows={rows} ghostCols={3} ghostRows={4} typingGhost={{ r: 4, c: 1, v: 'Priya' }} />
        </Stage>
        <Cap callout="typing into E1 of the ghost band" title="Typing is what creates it.">
          <p>The first keystroke in a ghost cell commits its row (and column, if the cell is past the last one) and a fresh ghost band appears beyond. No dialog, no confirm, no "add column" step in the required path.</p>
          <Anno>The typed ghost cell adopts the full selected treatment immediately — <b>surface-1 fill and the cyan ring</b> — so the commit is legible before you look anywhere else.</Anno>
          <Anno>Committing a ghost <b>column</b> gives it the default type, <code>text</code>, and an empty name. The header goes into rename the moment the column exists, so naming is offered but never blocking.</Anno>
          <Anno tone="mute">The toolbar's insert-column and insert-row buttons still exist for people who look for them, and for inserting <i>between</i> existing columns — which ghost typing can't express.</Anno>
        </Cap>
      </div>
    </Frame>
  );
}

// ── 1d · new sheet ──────────────────────────────────────────────────
function ShNewSheet() {
  const cols = Array.from({ length: 5 }).map(() => ({ n: '', t: 'text', w: 148 }));
  const rows = Array.from({ length: 6 }).map(() => ({ cells: cols.map(() => ({ v: '' })) }));
  return (
    <Frame>
      <Stage w={800} h={382}>
        <div style={{ display: 'flex', flexDirection: 'column', height: '100%' }}>
          <div className="sh-ohead" style={{ padding: '14px 18px 10px' }}>
            <span className="sh-ohead__title" style={{ fontSize: 19, color: 'var(--fg-4)' }}>Untitled sheet</span>
            <span className="sh-chip">{I.folder} Articles {I.chevD}</span>
          </div>
          <Toolbar />
          <FormulaBar cref="A1" />
          <SheetGrid cols={cols} rows={rows} ghostCols={2} ghostRows={3} sel={{ r: 0, c: 0 }} />
          <StatusBar cols={5} rows={6} formulas={0} saved="new" />
        </div>
      </Stage>
      <Cap callout="a sheet at birth" title="Five text columns, unnamed.">
        <p>The provisional seed holds: five columns, all <code>text</code>, six rows, selection parked on A1. Five columns at the default 148px width fill the overlay without overflowing, so a new sheet opens with no scrollbars in either axis.</p>
        <Anno>Headers seed <b>empty</b>, not "Column 1". A placeholder name is a lie that survives to CSV export; an empty header is an obvious invitation.</Anno>
        <Anno>Every seeded column is <code>text</code> because text is the only type that can hold any value the user pastes. Types are a narrowing decision, made after the data arrives.</Anno>
        <Flag>If seeding changes, our argument is for <b>fewer</b> columns, not more: three named-on-first-use columns read as a starting point, five read as a form to fill in. Five is fine — we'd only revisit it if new sheets are commonly created from a paste.</Flag>
      </Cap>
    </Frame>
  );
}

// ── 1e · selection & navigation ─────────────────────────────────────
function ShSelection() {
  const cols = BCOLS.slice(0, 6);
  const rows = BROWS.slice(0, 6).map((r) => ({ cells: r.cells.slice(0, 6) }));
  const K = ({ k, children }) => (
    <div style={{ display: 'flex', alignItems: 'center', gap: 10, marginTop: 8 }}>
      <span style={{ fontFamily: 'var(--font-mono)', fontSize: 10.5, color: 'var(--fg-2)', background: 'var(--surface-2)', border: '1px solid var(--border)', borderRadius: 4, padding: '3px 6px', minWidth: 62, textAlign: 'center', flexShrink: 0 }}>{k}</span>
      <span style={{ fontSize: 12, color: 'var(--fg-3)', lineHeight: 1.4 }}>{children}</span>
    </div>
  );
  return (
    <Frame stack>
      <div style={{ display: 'flex', gap: 34, alignItems: 'flex-start' }}>
        <div style={{ display: 'flex', flexDirection: 'column', gap: 22 }}>
          <div>
            <span className="sh-callout">single cell · anchor</span>
            <Stage w={700} h={190}><SheetGrid cols={cols} rows={rows.slice(0, 4)} ghostCols={1} ghostRows={1} sel={{ r: 1, c: 2 }} /></Stage>
          </div>
          <div>
            <span className="sh-callout">click-drag range · C2:E5</span>
            <Stage w={700} h={230}><SheetGrid cols={cols} rows={rows} ghostCols={1} ghostRows={1} sel={{ r: 1, c: 2 }} range={{ r0: 1, c0: 2, r1: 4, c1: 4 }} /></Stage>
          </div>
        </div>
        <Cap callout="two states, one language" title="Ring is where you are. Fill is what you picked.">
          <p>The anchor cell always carries a 1.5px inset cyan ring, whether it is alone or the corner of a range. A range fills every cell with <code>accent-soft</code> and draws one solid cyan outline around the whole block — the anchor keeps its ring inside that fill, so you never lose the cursor in a selection.</p>
          <Anno>A range never changes cell contents' alignment or type rendering. Currency stays right-aligned and keeps its code inside a selection.</Anno>
          <Anno>Computed cells inside a range deepen to a 18% cyan fill instead of the 3.5% resting tint, so the corner wedges stay visible against the selection.</Anno>
          <div className="sh-legend">
            <span className="sh-legend__i"><span style={{ width: 14, height: 14, borderRadius: 2, boxShadow: 'inset 0 0 0 1.5px var(--accent)' }} /> anchor</span>
            <span className="sh-legend__i"><span style={{ width: 14, height: 14, borderRadius: 2, background: 'var(--accent-soft)', border: '1px solid var(--accent)' }} /> range</span>
          </div>
        </Cap>
      </div>
      <div style={{ display: 'flex', gap: 34, alignItems: 'flex-start' }}>
        <div style={{ display: 'flex', gap: 22 }}>
          <div>
            <span className="sh-callout">column header click · whole column</span>
            <Stage w={430} h={200}><SheetGrid cols={cols.slice(0, 4)} rows={rows.slice(0, 4)} ghostCols={1} ghostRows={1} colSel={2} sel={{ r: 0, c: 2 }} range={{ r0: 0, c0: 2, r1: 3, c1: 2 }} /></Stage>
          </div>
          <div>
            <span className="sh-callout">row number click · whole row</span>
            <Stage w={430} h={200}><SheetGrid cols={cols.slice(0, 4)} rows={rows.slice(0, 4)} ghostCols={1} ghostRows={1} rowSel={2} sel={{ r: 2, c: 0 }} range={{ r0: 2, c0: 0, r1: 2, c1: 3 }} /></Stage>
          </div>
        </div>
        <Cap callout="keyboard" title="Navigation">
          <p>The header or gutter cell tints to <code>accent-soft</code> to show which band owns the selection.</p>
          <K k="← ↑ → ↓">Move the anchor one cell. Stops at the last real cell; one more press steps into ghost space and commits on type.</K>
          <K k="⇧ + arrows">Grow or shrink the range from the anchor.</K>
          <K k="Tab / ⇧Tab">Next / previous cell in the row, wrapping to the next row.</K>
          <K k="Enter">Commit and move down. In a range, moves within the range.</K>
          <K k="⌘ + arrows">Jump to the last real cell in that direction.</K>
          <K k="Esc">Cancel the in-cell edit, keep the selection.</K>
        </Cap>
      </div>
    </Frame>
  );
}

// ── 1f · per-type cell editors ──────────────────────────────────────
function ShEditors() {
  const one = (col, cell, editor, h = 108, pop) => (
    <div style={{ position: 'relative' }}>
      <Stage w={318} h={h}>
        <SheetGrid cols={[col]} rows={[{ cells: [cell] }, { cells: [{ v: '' }] }]} ghostCols={1} ghostRows={1} gutter={34} sel={{ r: 0, c: 0 }} editor={editor} />
      </Stage>
      {pop}
    </div>
  );
  const ed = (cls, body, w = 180, hint) => ({
    r: 0, c: 0,
    node: <span className={'sh-ed ' + cls} style={{ width: w }}>{body}{hint && <span className="sh-ed__hint">{hint}</span>}</span>,
  });
  return (
    <Frame stack>
      <div style={{ display: 'grid', gridTemplateColumns: 'repeat(3, 318px)', gap: 26 }}>
        <div>
          <span className="sh-callout">text · freeform</span>
          {one({ n: 'Line item', t: 'text', w: 180 }, { v: 'Longform essays' }, ed('', <>Longform essa<span className="sh-caret" /></>))}
        </div>
        <div>
          <span className="sh-callout">number · numeric entry</span>
          {one({ n: 'Units', t: 'number', w: 180 }, { v: '12' }, ed('sh-ed--num', <>12<span className="sh-caret" /></>, 180, 'Numbers only · ↑↓ to step'))}
        </div>
        <div>
          <span className="sh-callout">currency · code from column config</span>
          {one({ n: 'Rate', t: 'currency', w: 180, code: 'USD' }, { v: '1,400.00' }, ed('sh-ed--num', <>1400<span className="sh-caret" /><span className="sh-cur">USD</span></>, 180, 'Formats to 1,400.00 on commit'))}
        </div>
        <div>
          <span className="sh-callout">checkbox · toggle, no editor</span>
          {one({ n: 'Signed off', t: 'checkbox', w: 180 }, { v: true }, null, 108,
            <div style={{ position: 'absolute', left: 44, top: 78, fontSize: 11.5, color: 'var(--fg-4)', width: 264, lineHeight: 1.45 }}>Space or click toggles in place. There is no edit mode — a checkbox cell has no intermediate state to hold.</div>)}
        </div>
        <div>
          <span className="sh-callout">date · masked entry + picker</span>
          {one({ n: 'Due', t: 'date', w: 180 }, { v: 'Aug 04, 2026' }, ed('', <>08/04/2026<span className="sh-caret" /></>, 180), 108,
            <At x={34} y={70}>
              <div className="sh-pop" style={{ width: 214, padding: 10 }}>
                <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginBottom: 9 }}>
                  <span style={{ color: 'var(--fg-3)', transform: 'rotate(180deg)', display: 'flex' }}>{I.chevR}</span>
                  <span style={{ flex: 1, textAlign: 'center', fontSize: 12.5, fontWeight: 600, color: 'var(--fg-1)' }}>August 2026</span>
                  <span style={{ color: 'var(--fg-3)', display: 'flex' }}>{I.chevR}</span>
                </div>
                <div style={{ display: 'grid', gridTemplateColumns: 'repeat(7,1fr)', gap: 2 }}>
                  {['S', 'M', 'T', 'W', 'T', 'F', 'S'].map((d, i) => <span key={i} style={{ fontSize: 9.5, fontWeight: 600, color: 'var(--fg-4)', textAlign: 'center', padding: '2px 0' }}>{d}</span>)}
                  {Array.from({ length: 31 }).map((_, i) => {
                    const on = i + 1 === 4;
                    return <span key={i} style={{ aspectRatio: '1', display: 'flex', alignItems: 'center', justifyContent: 'center', fontSize: 11, borderRadius: 4, fontVariantNumeric: 'tabular-nums', background: on ? 'var(--accent)' : 'transparent', color: on ? 'var(--fg-on-accent)' : 'var(--fg-2)', fontWeight: on ? 600 : 400 }}>{i + 1}</span>;
                  })}
                </div>
              </div>
            </At>)}
        </div>
        <div>
          <span className="sh-callout">select · dropdown</span>
          {one({ n: 'Status', t: 'select', w: 180 }, { v: 'In flight' }, null, 108,
            <At x={34} y={40}>
              <div className="sh-pop" style={{ width: 180 }}>
                {['Committed', 'In flight', 'Proposed', 'Cut'].map((o) => (
                  <button key={o} className={'sh-pop__i' + (o === 'In flight' ? ' is-on' : '')}><span className="sh-pill">{o}</span><span className="sh-pop__tick">{I.tick}</span></button>
                ))}
                <div className="sh-pop__hr" />
                <button className="sh-pop__i"><span className="sh-pop__ico">{I.pencil}</span>Edit options…</button>
              </div>
            </At>)}
        </div>
      </div>
      <Cap callout="one editor per type" title="The editor is the affordance.">
        <p style={{ maxWidth: 900 }}>Every editor opens flush over its cell at the cell's own width, one pixel out on each side, carrying the same cyan ring the selection uses — so entering edit mode never moves the value you were looking at. Editors that constrain input say so in a hint line beneath the cell rather than rejecting keystrokes silently.</p>
        <div style={{ display: 'flex', gap: 34, maxWidth: 1000, marginTop: 2 }}>
          <div style={{ flex: 1 }}>
            <Anno><b>Numeric types keep their alignment while editing.</b> The caret sits at the right edge where the digits are, not at the left where a text caret would go.</Anno>
            <Anno><b>Currency edits raw.</b> You type <code>1400</code>, the cell commits <code>1,400.00</code>. The code comes from column config and is never typed.</Anno>
          </div>
          <div style={{ flex: 1 }}>
            <Anno><b>Date accepts both.</b> The masked field takes <code>08/04/2026</code> or <code>8/4/26</code>; the picker below it is for pointing. Both write the same value.</Anno>
            <Anno><b>Select is a dropdown, not a combobox.</b> Options come from column config; adding one is an explicit trip through "Edit options…" so a typo can't become a fifth status.</Anno>
          </div>
        </div>
      </Cap>
    </Frame>
  );
}

// ── 1g · type mismatch ─────────────────────────────────────────────
function ShMismatch() {
  const cols = [{ n: 'Line item', t: 'text', w: 176 }, { n: 'Units', t: 'number', w: 116 }, { n: 'Rate', t: 'currency', w: 128, code: 'USD' }, { n: 'Due', t: 'date', w: 132 }];
  const rows = [
    { cells: [{ v: 'Longform essays' }, { v: '6' }, { v: '1,400.00' }, { v: 'Jul 21, 2026' }] },
    { cells: [{ v: 'Illustration set' }, { v: 'about 12', k: 'mis' }, { v: '320.00' }, { v: 'Aug 04, 2026' }] },
    { cells: [{ v: 'Paid amplification' }, { v: '1' }, { v: 'TBD', k: 'mis' }, { v: 'end of Q3', k: 'mis' }] },
    { cells: [{ v: 'Podcast pilot' }, { v: '2' }, { v: '1,150.00' }, { v: 'Sep 01, 2026' }] },
  ];
  return (
    <Frame>
      <div style={{ position: 'relative' }}>
        <Stage w={640} h={250}>
          <SheetGrid cols={cols} rows={rows} ghostCols={1} ghostRows={2} sel={{ r: 1, c: 1 }} />
        </Stage>
        <At x={182} y={124}>
          <div className="sh-pop" style={{ width: 236, padding: '10px 12px' }}>
            <div style={{ display: 'flex', alignItems: 'center', gap: 7, color: 'var(--status-warning)', fontSize: 12, fontWeight: 500, marginBottom: 5 }}>{I.warn} Not a number</div>
            <div style={{ fontSize: 12.5, color: 'var(--fg-2)', lineHeight: 1.45 }}>Kept as text, exactly as typed. It won't be counted by SUM or AVERAGE.</div>
          </div>
        </At>
      </div>
      <Cap callout="hover or select a mismatched cell" title="The value survives. The column keeps its type.">
        <p>A value that doesn't parse as its column's type is never rewritten, never blanked, never rejected on entry. It renders as text — left-aligned, so the break from the column's own alignment is the first thing you notice — with a dotted amber underline and an amber wedge in the top-right corner.</p>
        <Anno tone="warn"><b>Top-right, amber</b> = something about the data. Bottom-left wedges are always about formulas. Corner position tells you which kind of problem you have before you read the color.</Anno>
        <Anno>Alignment does most of the work: a text value in a number column sits on the wrong side of the cell, which reads as wrong at a glance across a hundred rows.</Anno>
        <Anno tone="mute">Mismatch is a rendering state, not an error state. Nothing is blocked, nothing needs dismissing, and changing the column's type back to <code>text</code> makes it an ordinary cell again.</Anno>
      </Cap>
    </Frame>
  );
}

// ── 1h · column header menu ────────────────────────────────────────
function ShHeaderMenu() {
  const cols = BCOLS.slice(0, 5);
  const rows = BROWS.slice(0, 5).map((r) => ({ cells: r.cells.slice(0, 5) }));
  const Menu = ({ children, w = 208 }) => <div className="sh-pop" style={{ width: w }}>{children}</div>;
  return (
    <Frame stack>
      <div style={{ display: 'flex', gap: 26, alignItems: 'flex-start' }}>
        <div style={{ position: 'relative' }}>
          <span className="sh-callout">click the header</span>
          <Stage w={640} h={260}><SheetGrid cols={cols} rows={rows} ghostCols={1} ghostRows={1} openHeader={4} colSel={4} /></Stage>
          <At x={452} y={57}>
            <Menu>
              <button className="sh-pop__i"><span className="sh-pop__ico">{I.pencil}</span>Rename<span className="sh-pop__kb">⏎</span></button>
              <button className="sh-pop__i"><span className="sh-pop__ico">{I.swap}</span>Type<span className="sh-pop__kb">Currency</span><span style={{ marginLeft: 6, color: 'var(--fg-4)', display: 'flex' }}>{I.chevR}</span></button>
              <div className="sh-pop__hr" />
              <button className="sh-pop__i"><span className="sh-pop__ico">{I.sortA}</span>Sort ascending</button>
              <button className="sh-pop__i"><span className="sh-pop__ico">{I.sortD}</span>Sort descending</button>
              <button className="sh-pop__i"><span className="sh-pop__ico">{I.funnel}</span>Filter…</button>
              <div className="sh-pop__hr" />
              <button className="sh-pop__i sh-pop__danger"><span className="sh-pop__ico">{I.trash}</span>Delete column</button>
            </Menu>
          </At>
        </div>
        <div style={{ display: 'flex', gap: 26 }}>
          <div>
            <span className="sh-callout">rename · inline in the menu</span>
            <div className="sh-pop" style={{ width: 208 }}>
              <div className="sh-pop__lab">Rename column</div>
              <input className="sh-pop__in" defaultValue="Rate" />
              <div className="sh-pop__sub">Enter commits, Esc reverts. The A1 reference is positional — renaming never breaks a formula.</div>
            </div>
          </div>
          <div>
            <span className="sh-callout">type · submenu</span>
            <div className="sh-pop" style={{ width: 190 }}>
              <div className="sh-pop__lab">Column type</div>
              {TYPES.map(([t, label]) => (
                <button key={t} className={'sh-pop__i' + (t === 'currency' ? ' is-on' : '')}>
                  <span className="sh-pop__ico"><TypeGlyph t={t} /></span>{label}<span className="sh-pop__tick">{I.tick}</span>
                </button>
              ))}
              <div className="sh-pop__hr" />
              <div className="sh-pop__sub">Values that don't parse as the new type stay as they are and render mismatched. Nothing is lost on a type change.</div>
            </div>
          </div>
        </div>
      </div>
      <Cap callout="one menu, five verbs" title="Header interactions">
        <p style={{ maxWidth: 900 }}>The whole header cell is the trigger — there is no separate caret to hit. An open header keeps a cyan inset ring so it stays findable behind the popover, and its column tints to show what the menu will act on.</p>
        <div style={{ display: 'flex', gap: 34, maxWidth: 1000 }}>
          <div style={{ flex: 1 }}>
            <Anno><b>Rename is inline.</b> A field in the menu, not a modal — renaming a column is the most frequent of these five actions by a wide margin.</Anno>
            <Anno><b>Type change never destroys.</b> Non-parsing values fall back to the mismatch rendering, which means a wrong type guess is always reversible.</Anno>
          </div>
          <div style={{ flex: 1 }}>
            <Anno><b>Delete is the only red row</b> and the only one below the second divider. It is a destructive structural change and sits away from the view-only verbs above it.</Anno>
            <Anno tone="mute">The type submenu shows the current type checked and echoes it in the parent row, so a glance at the closed menu answers "what is this column?".</Anno>
          </div>
        </div>
      </Cap>
    </Frame>
  );
}

// ── 1i · sort / filter is view-only ────────────────────────────────
function ShView() {
  const cols = BCOLS.slice(0, 6).map((c, i) => i === 5 ? { ...c, sort: 'desc' } : i === 2 ? { ...c, filter: true } : c);
  const order = [0, 4, 2, 3];
  const rows = order.map((i) => ({ cells: BROWS[i].cells.slice(0, 6) }));
  const strip = (
    <div className="sh-view">
      <span className="sh-vchip">{I.sortD}<b>Total</b> descending<span className="sh-vchip__x">{I.xS}</span></span>
      <span className="sh-vchip">{I.funnel}<b>Status</b> is not Cut<span className="sh-vchip__x">{I.xS}</span></span>
      <span className="sh-view__note">View only — row order and stored data are unchanged.</span>
    </div>
  );
  return (
    <Frame>
      <Stage w={780} h={300}>
        <div style={{ display: 'flex', flexDirection: 'column', height: '100%' }}>
          <FormulaBar cref="F1" src="=D1*E1" />
          {strip}
          <SheetGrid cols={cols} rows={rows} ghostCols={1} ghostRows={2} />
          <StatusBar cols={8} rows={8} formulas={8} />
        </div>
      </Stage>
      <Cap callout="sorted and filtered" title="A view you can see the edges of.">
        <p>Because sort and filter change nothing underneath, the design's whole job is to make the view visible and trivially removable. Two indicators, always both present: a glyph in the affected header, and a removable chip in a strip that only exists while a view is active.</p>
        <Anno><b>The strip appears and disappears with the view.</b> An empty sheet-wide view bar sitting there permanently would train people to ignore it.</Anno>
        <Anno>Header glyphs are cyan — the same accent as every other "this is active" state in Pane. Sort shows its direction; filter shows a funnel.</Anno>
        <Anno>Row numbers keep their <b>stored</b> positions under a sort, so the gutter reading 1, 5, 3, 4 is itself the tell that you're looking at a reordered view.</Anno>
        <Anno tone="mute">The sentence at the end of the strip is deliberately flat. It is the one place we can say plainly that nothing was moved.</Anno>
      </Cap>
    </Frame>
  );
}

// ── 1j · resize ────────────────────────────────────────────────────
function ShResize() {
  const cols = BCOLS.slice(0, 4);
  const rows = BROWS.slice(0, 4).map((r) => ({ cells: r.cells.slice(0, 4) }));
  const tallRows = rows.map((r, i) => i === 1 ? { ...r, h: 62 } : r);
  return (
    <Frame stack>
      <div style={{ display: 'flex', gap: 26, alignItems: 'flex-start' }}>
        <div style={{ position: 'relative' }}>
          <span className="sh-callout">dragging the C/D boundary</span>
          <Stage w={560} h={230}><SheetGrid cols={cols} rows={rows} ghostCols={1} ghostRows={1} gripCol={2} /></Stage>
          <div className="sh-guide" style={{ left: 476, top: 20, bottom: 0 }} />
          <div className="sh-readout" style={{ left: 484, top: 30 }}>124 px</div>
        </div>
        <div style={{ position: 'relative' }}>
          <span className="sh-callout">dragging row 2's lower edge</span>
          <Stage w={560} h={230}><SheetGrid cols={cols} rows={tallRows} ghostCols={1} ghostRows={1} gripRow={1} /></Stage>
          <div className="sh-guide sh-guide--row" style={{ top: 154, left: 0, right: 0 }} />
          <div className="sh-readout" style={{ left: 46, top: 162 }}>62 px</div>
        </div>
      </div>
      <Cap callout="both axes drag" title="Resize">
        <p style={{ maxWidth: 920 }}>A 6px hit zone straddles the header's right edge for columns and the <i>gutter</i> cell's lower edge for rows. Hovering it shows a 2px cyan bar on the boundary; dragging extends that bar the full length of the grid and pins a mono pixel readout to the cursor. Nothing else in the grid moves until the drag ends.</p>
        <div style={{ display: 'flex', gap: 34, maxWidth: 1000 }}>
          <div style={{ flex: 1 }}>
            <Anno><b>Row resize lives in the gutter only.</b> A hit zone on the bottom edge of a data cell would fight with click-drag range selection, which is by far the more common gesture in the cell area.</Anno>
            <Anno>Both readouts are mono and tabular so the number doesn't jitter as it changes width mid-drag.</Anno>
          </div>
          <div style={{ flex: 1 }}>
            <Anno tone="mute">Cell content does not reflow at any width. A too-narrow column ellipsizes; it never wraps, because wrapping would change row height as a side effect of a column drag.</Anno>
            <Flag label="row resize · ship it, minus one thing">Per-row height should ship in v1 — a sheet with pasted paragraphs is unusable without it. What we'd hold back is <b>double-click-to-autofit</b>: with no wrapping, autofit has no height to compute, and it is the affordance people will try first.</Flag>
          </div>
        </div>
      </Cap>
    </Frame>
  );
}

Object.assign(window, {
  BCOLS, BROWS, SheetPanel, ShInContext, ShOverflow, ShGhost, ShNewSheet,
  ShSelection, ShEditors, ShMismatch, ShHeaderMenu, ShView, ShResize,
});
