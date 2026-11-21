// Pane Sheets — 2 · Formula experience  ·  3 · Cell styling

// ── 2a · formula bar states ─────────────────────────────────────────
function ShFormulaBar() {
  const cols = BCOLS.slice(0, 6);
  const rows = BROWS.slice(0, 5).map((r) => ({ cells: r.cells.slice(0, 6) }));
  const row = (callout, node) => <div><span className="sh-callout">{callout}</span>{node}</div>;
  return (
    <Frame stack>
      <div style={{ display: 'flex', gap: 26, alignItems: 'flex-start', flexWrap: 'wrap' }}>
        {row('selected: a computed cell · F3', (
          <Stage w={620} h={44} radius={6}><FormulaBar cref="F3" src="=D3*E3" /></Stage>
        ))}
        {row('selected: a typed value · E3', (
          <Stage w={620} h={44} radius={6}><FormulaBar cref="E3" src="320.00" /></Stage>
        ))}
        {row('selected: empty · G9', (
          <Stage w={620} h={44} radius={6}><FormulaBar cref="G9" /></Stage>
        ))}
        {row('editing · caret live, refs colored', (
          <Stage w={620} h={44} radius={6}><FormulaBar cref="F9" src='=IF(SUM(F1:F8)>20000, "over", AVERAGE(F1:F8))' editing /></Stage>
        ))}
      </div>
      <div style={{ display: 'flex', gap: 34, alignItems: 'flex-start' }}>
        <div>
          <span className="sh-callout">editing F9 · referenced ranges outline in the grid</span>
          <Stage w={760} h={286}>
            <div style={{ display: 'flex', flexDirection: 'column', height: '100%' }}>
              <FormulaBar cref="F9" src="=SUM(F1:F5)" editing />
              <SheetGrid cols={cols} rows={rows} ghostCols={1} ghostRows={2} sel={{ r: 5, c: 5 }}
                refRange={{ r0: 0, c0: 5, r1: 4, c1: 5 }}
                editor={{ r: 5, c: 5, node: null }} />
            </div>
          </Stage>
        </div>
        <Cap callout="one formula, two places to edit it" title="The bar and the cell hold the same string.">
          <p>The bar shows the raw formula of the anchor cell — never the result. Typed values appear as themselves, so the bar answers "is this computed?" before you look at the cell. Editing in the bar and editing in the cell are the same edit, with the same tokenizing and the same caret.</p>
          <Anno><b>Refs are cyan, functions are full-strength white, numbers and strings recede.</b> The same three colors are used in the bar, in-cell, and in the paused-mode raw strings.</Anno>
          <Anno>While a formula is being edited, every range it references gets a <b>dashed</b> cyan outline in the grid. Dashed, because the solid outline already means "this is your selection" — the two must never be confusable.</Anno>
          <Anno tone="mute">The <code>fx</code> slot is a label, not a button — function insertion happens by typing. The chevron on the right expands the bar to three lines for long formulas.</Anno>
        </Cap>
      </div>
    </Frame>
  );
}

// ── 2b · computed vs typed ─────────────────────────────────────────
function ShComputed() {
  const cols = [{ n: 'Line item', t: 'text', w: 168 }, { n: 'Units', t: 'number', w: 80 }, { n: 'Rate', t: 'currency', w: 108, code: 'USD' }, { n: 'Total', t: 'currency', w: 112, code: 'USD' }];
  const rows = [
    { cells: [{ v: 'Longform essays' }, { v: '6' }, { v: '1,400.00' }, { v: '8,400.00', k: 'calc' }] },
    { cells: [{ v: 'Illustration set' }, { v: '12' }, { v: '320.00' }, { v: '3,840.00', k: 'calc' }] },
    { cells: [{ v: 'Video edit pass' }, { v: '3' }, { v: '900.00' }, { v: '2,700.00', k: 'calc' }] },
    { cells: [{ v: 'Retainer (fixed)' }, { v: '1' }, { v: '4,500.00' }, { v: '4,500.00' }] },
    { cells: [{ v: 'Subtotal' }, { v: '' }, { v: '' }, { v: '19,440.00', k: 'calc' }] },
  ];
  return (
    <Frame>
      <div style={{ display: 'flex', flexDirection: 'column', gap: 20 }}>
        <Stage w={520} h={250}><SheetGrid cols={cols} rows={rows} ghostCols={1} ghostRows={1} /></Stage>
        <div style={{ display: 'flex', gap: 18, alignItems: 'center', paddingLeft: 2 }}>
          {[['calc', 'computed', 'var(--accent)'], ['err', 'error', 'var(--status-danger)'], ['mis', 'mismatch', 'var(--status-warning)']].map(([k, label, c]) => (
            <span key={k} className="sh-legend__i">
              <span style={{ position: 'relative', width: 26, height: 20, border: '1px solid var(--border)', borderRadius: 2, background: k === 'calc' ? 'rgba(0,228,228,0.035)' : k === 'err' ? 'rgba(239,68,68,0.09)' : 'transparent' }}>
                <span className={'sh-w sh-w--' + k} style={{ borderLeftColor: k === 'mis' ? undefined : c, borderRightColor: k === 'mis' ? c : undefined, position: 'absolute' }} />
              </span>
              {label}
            </span>
          ))}
        </div>
      </div>
      <Cap callout="row 4 is typed, the rest are computed" title="A 5px wedge, not a badge.">
        <p>Computed cells carry a cyan wedge in the bottom-left corner and a 3.5% cyan wash. That is the entire distinction. It survives at any column width, it doesn't compete with the value for horizontal room, and it costs nothing on a sheet where every cell is a formula.</p>
        <Anno><b>Position is the category, color is the meaning.</b> Bottom-left is always "this cell came from a formula" — cyan when it worked, red when it didn't. Top-right is always about the data itself.</Anno>
        <Anno>Row 4 shows the case that matters: a hand-typed value sitting in a column of formulas. Without the wedge, a total someone overwrote is indistinguishable from one that recalculates.</Anno>
        <Anno tone="mute">The wash is deliberately below the threshold of a color: it reads as slightly-lit rather than tinted, which is what lets a fully computed column still look like a grid instead of a highlight.</Anno>
        <Anno tone="mute">The status bar carries the same wedge next to the sheet's formula count, which is where the language is taught.</Anno>
      </Cap>
    </Frame>
  );
}

// ── 2c · error cells ───────────────────────────────────────────────
const ERRS = [
  ['#REF!', 'A reference points at a column or row that no longer exists.', '=SUM(H1:H8)'],
  ['#VALUE!', 'A value in the formula is the wrong kind — usually text where a number is needed.', '=D3*E3'],
  ['#DIV/0!', 'Something divides by zero, or by an empty cell.', '=F5/D5'],
  ['#NAME?', "That function name isn't recognized.", '=TOTAL(F1:F8)'],
  ['#PARSE!', "The formula can't be read — most often an unclosed bracket or quote.", '=IF(F1>100, "over'],
  ['#CYCLE!', 'The formula depends on its own result, directly or through another cell.', '=F9+1'],
];
function ShErrors() {
  const cols = [{ n: 'Line item', t: 'text', w: 168 }, { n: 'Units', t: 'number', w: 80 }, { n: 'Rate', t: 'currency', w: 104, code: 'USD' }, { n: 'Total', t: 'currency', w: 116, code: 'USD' }];
  const rows = [
    { cells: [{ v: 'Longform essays' }, { v: '6' }, { v: '1,400.00' }, { v: '8,400.00', k: 'calc' }] },
    { cells: [{ v: 'Illustration set' }, { v: 'about 12', k: 'mis' }, { v: '320.00' }, { v: '#VALUE!', k: 'err' }] },
    { cells: [{ v: 'Video edit pass' }, { v: '0' }, { v: '900.00' }, { v: '#DIV/0!', k: 'err' }] },
    { cells: [{ v: 'Paid amplification' }, { v: '1' }, { v: '5,000.00' }, { v: '#REF!', k: 'err' }] },
    { cells: [{ v: 'Subtotal' }, { v: '' }, { v: '' }, { v: '#CYCLE!', k: 'err' }] },
  ];
  return (
    <Frame stack>
      <div style={{ display: 'flex', gap: 30, alignItems: 'flex-start' }}>
        <div style={{ position: 'relative' }}>
          <span className="sh-callout">#VALUE! selected · detail on selection</span>
          <Stage w={520} h={250}><SheetGrid cols={cols} rows={rows} ghostCols={1} ghostRows={1} sel={{ r: 1, c: 3 }} /></Stage>
          <At x={266} y={116}>
            <div className="sh-errpop">
              <div className="sh-errpop__code">#VALUE!</div>
              <div className="sh-errpop__msg">A value in the formula is the wrong kind — usually text where a number is needed. <b style={{ color: 'var(--fg-1)', fontWeight: 500 }}>B2</b> holds text.</div>
              <div className="sh-errpop__f">=D2*E2</div>
            </div>
          </At>
        </div>
        <div style={{ display: 'flex', flexDirection: 'column', gap: 0, width: 420 }}>
          <span className="sh-callout">the six codes · one treatment</span>
          {ERRS.map(([code, msg, f]) => (
            <div key={code} style={{ display: 'flex', gap: 14, alignItems: 'flex-start', padding: '9px 0', borderTop: '1px solid var(--border-faint)' }}>
              <span style={{ position: 'relative', width: 92, flexShrink: 0, height: 26, display: 'flex', alignItems: 'center', padding: '0 8px', boxSizing: 'border-box', background: 'rgba(239,68,68,0.09)', border: '1px solid var(--border-faint)', borderRadius: 2, fontFamily: 'var(--font-mono)', fontSize: 11.5, fontWeight: 500, color: 'var(--status-danger)' }}>
                {code}<span className="sh-w sh-w--err" style={{ position: 'absolute' }} />
              </span>
              <span style={{ fontSize: 12, color: 'var(--fg-3)', lineHeight: 1.45 }}>{msg}<br /><code style={{ fontFamily: 'var(--font-mono)', fontSize: 10.5, color: 'var(--fg-4)', background: 'transparent', padding: 0 }}>{f}</code></span>
            </div>
          ))}
        </div>
      </div>
      <Cap callout="one error treatment" title="Errors look like errors, and stay in their cell.">
        <p style={{ maxWidth: 940 }}>Every error renders identically: the code in mono red on a 9% red wash, left-aligned regardless of the column's type, with a red wedge in the bottom-left corner. Nothing pops, nothing toasts, nothing blocks the sheet. The code alone is enough to scan a column and see that four cells broke the same way.</p>
        <div style={{ display: 'flex', gap: 34, maxWidth: 1000 }}>
          <div style={{ flex: 1 }}>
            <Anno tone="danger"><b>Red wedge, bottom-left</b> — same corner as the cyan computed wedge, because an error <i>is</i> a formula result. The color is the only thing that changes.</Anno>
            <Anno>Selecting an error cell opens a detail card under it: the code, one plain sentence, and the raw formula. No error numbers, no docs links.</Anno>
          </div>
          <div style={{ flex: 1 }}>
            <Anno>The sentence names the culprit cell where we know it (<code>B2 holds text</code>), which turns a diagnosis into a destination.</Anno>
            <Anno tone="mute">The formula stays visible and editable in the formula bar the whole time — an error never hides its own cause.</Anno>
          </div>
        </div>
      </Cap>
    </Frame>
  );
}

// ── 2d · computation paused ────────────────────────────────────────
function ShPaused() {
  const cols = [{ n: 'Line item', t: 'text', w: 164 }, { n: 'Units', t: 'number', w: 78 }, { n: 'Rate', t: 'currency', w: 104, code: 'USD' }, { n: 'Total', t: 'currency', w: 132, code: 'USD' }, { n: 'Share', t: 'number', w: 128 }];
  const raw = (a, b, c, d, e) => ({ cells: [{ v: a }, { v: b }, { v: c }, { v: d, k: 'raw' }, { v: e, k: 'raw' }] });
  const rows = [
    raw('Longform essays', '6', '1,400.00', '=B1*C1', '=D1/SUM(D$1:D$8)'),
    raw('Newsletter design', '1', '2,600.00', '=B2*C2', '=D2/SUM(D$1:D$8)'),
    raw('Illustration set', '12', '320.00', '=B3*C3', '=D3/SUM(D$1:D$8)'),
    raw('Video edit pass', '3', '900.00', '=B4*C4', '=D4/SUM(D$1:D$8)'),
    raw('Paid amplification', '1', '5,000.00', '=B5*C5', '=D5/SUM(D$1:D$8)'),
    raw('Podcast pilot', '2', '1,150.00', '=B6*C6', '=D6/SUM(D$1:D$8)'),
  ];
  const notice = (
    <div className="sh-paused">
      {I.clock}
      <span className="sh-paused__t"><b>512 formulas</b> — computation paused for size.</span>
      <span className="sh-paused__sub">Cells show what you wrote.</span>
      <span className="sh-paused__sp" />
    </div>
  );
  return (
    <Frame>
      <Stage w={720} h={330}>
        <div style={{ display: 'flex', flexDirection: 'column', height: '100%' }}>
          <Toolbar />
          {notice}
          <FormulaBar cref="D3" src="=B3*C3" />
          <SheetGrid cols={cols} rows={rows} ghostCols={1} ghostRows={2} sel={{ r: 2, c: 3 }} />
          <StatusBar cols={5} rows={512} formulas={512} paused />
        </div>
      </Stage>
      <Cap callout="500+ formula cells" title="Paused, not broken.">
        <p>Above the threshold the sheet stops evaluating and shows every formula as the string you typed — mono, muted, left-aligned, with no wedges, because there is no result to mark. The notice sits between the toolbar and the formula bar: in the path of anyone about to type a formula, out of the path of anyone scrolling data.</p>
        <Anno><b>Neutral surface, no warning color.</b> This is a size fact, not a failure. It uses <code>surface-1</code> and the same clock glyph as version history.</Anno>
        <Anno>The count leads — <b>512 formulas</b> — because the number is the explanation. "Cells show what you wrote" is the second half of the same sentence, in muted text.</Anno>
        <Anno>Raw strings use the same token colors as the formula bar, so a paused sheet is still readable as formulas rather than as text.</Anno>
        <Anno tone="mute">The status bar repeats "computation paused" so the state is discoverable after the notice scrolls out of attention.</Anno>
        <Flag label="ratified in review">No action button. The notice stands alone — v1 isn't aimed at users who reach 500 formulas, so a "compute once" escape hatch would be UI built for an edge we're not serving yet.</Flag>
      </Cap>
    </Frame>
  );
}

// ══ 3 · CELL STYLING ═══════════════════════════════════════════════
const FILLS = [
  ['none', 'transparent'], ['slate', '#232327'], ['cyan', 'rgba(0,228,228,0.14)'],
  ['green', 'rgba(34,197,94,0.14)'], ['amber', 'rgba(245,158,11,0.14)'],
  ['red', 'rgba(239,68,68,0.13)'], ['violet', 'rgba(139,92,246,0.15)'],
];
const INKS = [
  ['default', '#F0EDE8'], ['muted', '#A0A0A0'], ['cyan', '#00E4E4'],
  ['green', '#22C55E'], ['amber', '#F59E0B'], ['red', '#EF4444'], ['violet', '#a78bfa'],
];

function StylePanel({ fill = 2, ink = 0, side = 'bottom', weight = 2 }) {
  return (
    <div className="sh-sty">
      <div className="sh-sty__sec">
        <div className="sh-sty__lab">Cell background</div>
        <div className="sh-sw">
          {FILLS.map(([n, c], i) => (
            <button key={n} title={n} className={'sh-sw__b' + (c === 'transparent' ? ' sh-sw__b--none' : '') + (i === fill ? ' is-on' : '')} style={{ background: c }}>{c === 'transparent' ? I.xS : null}</button>
          ))}
        </div>
      </div>
      <div className="sh-sty__sec">
        <div className="sh-sty__lab">Text</div>
        <div className="sh-sw">
          {INKS.map(([n, c], i) => (
            <button key={n} title={n} className={'sh-sw__b sh-sw__b--txt' + (i === ink ? ' is-on' : '')} style={{ color: c }}>A</button>
          ))}
        </div>
      </div>
      <div className="sh-sty__sec">
        <div className="sh-sty__lab">Border</div>
        <div className="sh-seg" style={{ marginBottom: 6 }}>
          {[['none', 'None'], ['all', 'All'], ['top', 'Top'], ['bottom', 'Btm'], ['left', 'Left'], ['right', 'Rgt']].map(([k, l]) => (
            <button key={k} className={'sh-seg__b' + (k === side ? ' is-on' : '')}>{l}</button>
          ))}
        </div>
        <div className="sh-seg">
          {[1, 2, 3].map((w) => (
            <button key={w} className={'sh-seg__b' + (w === weight ? ' is-on' : '')}>
              <span style={{ width: 18, height: w, background: 'currentColor', borderRadius: 1 }} />
            </button>
          ))}
          <button className="sh-seg__b" style={{ flex: 1.4 }}>Dashed</button>
        </div>
      </div>
      <button className="sh-sty__clear">Clear styling</button>
    </div>
  );
}

function ShStyling() {
  const cols = [{ n: 'Line item', t: 'text', w: 168 }, { n: 'Status', t: 'select', w: 116 }, { n: 'Units', t: 'number', w: 78 }, { n: 'Total', t: 'currency', w: 116, code: 'USD' }];
  const HEAD = { bg: '#232327', fg: '#F0EDE8' };
  const rows = [
    { cells: [{ v: 'Longform essays' }, { v: 'Committed' }, { v: '6' }, { v: '8,400.00', k: 'calc' }] },
    { cells: [{ v: 'Illustration set' }, { v: 'In flight' }, { v: '12' }, { v: '3,840.00', k: 'calc' }] },
    { cells: [{ v: 'Paid amplification', st: { bg: 'rgba(239,68,68,0.13)' } }, { v: 'Proposed' }, { v: '1' }, { v: '5,000.00', k: 'calc', st: { fg: '#EF4444' } }] },
    { cells: [{ v: 'Podcast pilot' }, { v: 'Proposed' }, { v: '2' }, { v: '2,300.00', k: 'calc' }] },
    { cells: [{ v: 'Subtotal', st: { ...HEAD, bd: { side: 'top', w: 2, c: 'var(--border-strong)' } } }, { v: '', st: { bg: '#232327', bd: { side: 'top', w: 2, c: 'var(--border-strong)' } } }, { v: '', st: { bg: '#232327', bd: { side: 'top', w: 2, c: 'var(--border-strong)' } } }, { v: '19,540.00', k: 'calc', st: { bg: 'rgba(0,228,228,0.14)', fg: '#00E4E4', bd: { side: 'top', w: 2, c: 'var(--accent)' } } }] },
  ];
  return (
    <Frame stack>
      <div style={{ display: 'flex', gap: 30, alignItems: 'flex-start' }}>
        <div style={{ position: 'relative' }}>
          <span className="sh-callout">toolbar → fill · range selected</span>
          <Stage w={560} h={296}>
            <div style={{ display: 'flex', flexDirection: 'column', height: '100%' }}>
              <Toolbar open="fill" styleSwatch="rgba(0,228,228,0.14)" />
              <FormulaBar cref="D5" src="=SUM(D1:D4)" />
              <SheetGrid cols={cols} rows={rows} ghostCols={1} ghostRows={1} sel={{ r: 4, c: 3 }} range={{ r0: 4, c0: 0, r1: 4, c1: 3 }} />
            </div>
          </Stage>
          <At x={96} y={44}><StylePanel /></At>
        </div>
        <div style={{ position: 'relative' }}>
          <span className="sh-callout">right-click a cell or range</span>
          <Stage w={430} h={296}>
            <SheetGrid cols={cols.slice(0, 3)} rows={rows.slice(0, 4)} ghostCols={1} ghostRows={2} sel={{ r: 1, c: 0 }} range={{ r0: 1, c0: 0, r1: 2, c1: 2 }} />
          </Stage>
          <At x={128} y={104}>
            <div className="sh-pop" style={{ width: 206 }}>
              <button className="sh-pop__i"><span className="sh-pop__ico">{I.bucket}</span>Style cells…<span className="sh-pop__kb">⌘⇧S</span></button>
              <button className="sh-pop__i"><span className="sh-pop__ico" style={{ color: 'var(--fg-4)' }}>{I.xS}</span>Clear styling</button>
              <div className="sh-pop__hr" />
              <button className="sh-pop__i"><span className="sh-pop__ico">{I.rowIns}</span>Insert row above</button>
              <button className="sh-pop__i"><span className="sh-pop__ico">{I.colIns}</span>Insert column left</button>
              <div className="sh-pop__hr" />
              <button className="sh-pop__i"><span className="sh-pop__ico">{I.download}</span>Export sheet as CSV…</button>
              <div className="sh-pop__hr" />
              <button className="sh-pop__i sh-pop__danger"><span className="sh-pop__ico">{I.trash}</span>Delete 6 cells' contents</button>
            </div>
          </At>
        </div>
      </div>
      <Cap callout="two doors, one panel" title="Styling reads as Pane, not as Excel.">
        <p style={{ maxWidth: 940 }}>The toolbar carries three persistent controls — fill, text, border — with the fill button showing the current swatch, so the last-used color is one click away. Right-click on any cell or range opens the context menu, whose first item leads to the same panel. There is no separate formatting mode and no ribbon.</p>
        <div style={{ display: 'flex', gap: 30, maxWidth: 1060 }}>
          <div style={{ flex: 1 }}>
            <Anno><b>Seven fills, seven inks, both drawn from tokens.</b> Six status/accent hues at 13–15% alpha over the near-black canvas, plus <code>surface-2</code> as a neutral and a "none". Nothing saturated: a fill has to survive being read through, not shout.</Anno>
            <Anno><b>Ink colors are the tokens at full strength</b> — the same six hues, so a red fill and red text are visibly the same red.</Anno>
          </div>
          <div style={{ flex: 1 }}>
            <Anno>Borders are side + weight + dashed, in two rows of segments. Weight renders as an actual 1/2/3px rule rather than a number, which is the only labeling that survives at this size.</Anno>
            <Anno>"Clear styling" is a full-width row at the bottom of the panel and repeats in the context menu — styling accumulates, so the exit has to be as easy as the entrance.</Anno>
            <Anno tone="mute">Cell styling and the corner-wedge language never collide: wedges sit in the corners, fills sit behind the value, and a styled computed cell keeps its cyan wedge.</Anno>
          </div>
        </div>
      </Cap>
    </Frame>
  );
}

// ── 3b · styling + wedges coexisting ───────────────────────────────
function ShCoexist() {
  const cols = [{ n: 'Line item', t: 'text', w: 158 }, { n: 'Units', t: 'number', w: 74 }, { n: 'Rate', t: 'currency', w: 100, code: 'USD' }, { n: 'Total', t: 'currency', w: 122, code: 'USD' }];
  const RED = { bg: 'rgba(239,68,68,0.13)' };
  const AMB = { bg: 'rgba(245,158,11,0.14)' };
  const CYA = { bg: 'rgba(0,228,228,0.14)', bd: { side: 'top', w: 2, c: 'var(--accent)' } };
  const rows = [
    { cells: [{ v: 'Longform essays' }, { v: '6' }, { v: '1,400.00' }, { v: '8,400.00', k: 'calc' }] },
    { cells: [{ v: 'Flagged line', st: RED }, { v: '3' }, { v: '900.00' }, { v: '2,700.00', k: 'calc', st: RED }] },
    { cells: [{ v: 'Illustration set', st: AMB }, { v: 'about 12', k: 'mis', st: AMB }, { v: '320.00' }, { v: '#VALUE!', k: 'err', st: AMB }] },
    { cells: [{ v: 'Paid amplification' }, { v: '0' }, { v: '5,000.00', st: RED }, { v: '#DIV/0!', k: 'err', st: RED }] },
    { cells: [{ v: 'Subtotal', st: { bg: '#232327', fg: '#F0EDE8', bd: { side: 'top', w: 2, c: 'var(--border-strong)' } } }, { v: '', st: { bg: '#232327', bd: { side: 'top', w: 2, c: 'var(--border-strong)' } } }, { v: '', st: { bg: '#232327', bd: { side: 'top', w: 2, c: 'var(--border-strong)' } } }, { v: '11,100.00', k: 'calc', st: CYA }] },
  ];
  const chip = (label, st, k) => (
    <div style={{ display: 'flex', alignItems: 'center', gap: 11, padding: '8px 0', borderTop: '1px solid var(--border-faint)' }}>
      <span className={'sh-c' + (k === 'err' ? ' sh-c--err' : k === 'calc' ? ' sh-c--calc' : k === 'mis' ? ' sh-c--mis' : '')}
        style={{ position: 'relative', width: 78, height: 26, flexShrink: 0, justifyContent: k === 'calc' ? 'flex-end' : 'flex-start', border: '1px solid var(--border-faint)', borderRadius: 2, ...st }}>
        <span className="sh-c__v">{k === 'err' ? '#VALUE!' : k === 'mis' ? 'about 12' : '8,400'}</span>
        {k === 'calc' && <span className="sh-w sh-w--calc" />}
        {k === 'err' && <span className="sh-w sh-w--err" />}
        {k === 'mis' && <span className="sh-w sh-w--mis" />}
      </span>
      <span style={{ fontSize: 12, color: 'var(--fg-3)', lineHeight: 1.45 }}>{label}</span>
    </div>
  );
  return (
    <Frame>
      <div style={{ position: 'relative' }}>
        <span className="sh-callout">user fills + formula state, on the same cells</span>
        <Stage w={540} h={252}>
          <SheetGrid cols={cols} rows={rows} ghostCols={1} ghostRows={1} sel={{ r: 2, c: 3 }} />
        </Stage>
        <At x={58} y={186}>
          <div className="sh-errpop">
            <div className="sh-errpop__code">#VALUE!</div>
            <div className="sh-errpop__msg">A value in the formula is the wrong kind — usually text where a number is needed. <b style={{ color: 'var(--fg-1)', fontWeight: 500 }}>B3</b> holds text.</div>
            <div className="sh-errpop__f">=B3*C3</div>
          </div>
        </At>
      </div>
      <Cap callout="the claim, shown" title="Fill sits behind the value. Wedges sit in the corners.">
        <p>Row 3 is the hard case: the user has put an amber fill on the whole row, column B holds an unparseable number, and column D's formula failed as a result. Three signals, one cell each, none of them hidden — because they occupy different real estate. A fill paints the cell box; a wedge occupies 5px of a corner the value never reaches.</p>
        {chip('Computed, no styling. Cyan wedge on the 3.5% wash.', null, 'calc')}
        {chip('Computed inside a user fill. The wash is overpainted; the wedge is not.', { background: 'rgba(239,68,68,0.13)' }, 'calc')}
        {chip('Error inside a user fill. The red wedge and mono red code survive any fill in the palette.', { background: 'rgba(245,158,11,0.14)' }, 'err')}
        {chip('Mismatch inside a user fill. Amber wedge, top-right, plus the dotted underline.', { background: 'rgba(245,158,11,0.14)' }, 'mis')}
        <Anno><b>The error text always wins over the fill.</b> An error cell keeps its mono red type and its own red wash if the user hasn't set one; if they have, the fill shows and the code stays red on top of it. No palette entry is dark enough or saturated enough to take that contrast below legible.</Anno>
        <Anno>Selecting the cell is what produces detail. The popover reads the same whether or not the cell is styled — it describes the formula, not the formatting.</Anno>
        <Anno tone="mute">This is why the wedges are wedges. A badge, a pill, or an icon inside the cell would compete with both the value and the fill; a corner triangle competes with nothing.</Anno>
      </Cap>
    </Frame>
  );
}

Object.assign(window, { ShFormulaBar, ShComputed, ShErrors, ShPaused, StylePanel, ShStyling, ShCoexist, FILLS, INKS });
