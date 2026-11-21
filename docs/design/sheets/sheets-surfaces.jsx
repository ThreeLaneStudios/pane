// Pane Sheets — 4 · Board card face  ·  5 · Version history  ·  6 · CSV export

// ── 4 · the sheet card ─────────────────────────────────────────────
// Built on the kit's .kcard (KanbanBoard.jsx markup + kit.css) with the
// kit's Pill and .dot. New: the glyph, the micro-grid, the footer metric.
function SheetCard({ title = 'Q3 Content Budget', tone = 'todo', status = 'To Do', cols = 8, rows = 24, formulas = true, crumb = 'Symbiotic › Articles', due = '9/25/2026', compact }) {
  const tmpl = '1.6fr 1fr 1fr 0.7fr';
  return (
    <article className={'kcard kcard--' + tone + ' sh-card'}>
      <div className="kcard__head">
        <span className="sh-card__glyph">{G('grid', 13)}</span>
        <h4 className="kcard__title">{title}</h4>
      </div>
      <div className="sh-mini">
        <div className="sh-mini__h" style={{ gridTemplateColumns: tmpl }}>
          {['text', 'select', 'currency', 'checkbox'].map((t) => <span key={t} className="sh-mini__hc"><TypeGlyph t={t} /></span>)}
        </div>
        {!compact && [0, 1].map((r) => (
          <div className="sh-mini__r" key={r} style={{ gridTemplateColumns: tmpl }}>
            <span className="sh-mini__c"><span className="sh-mini__bar" style={{ width: ['82%', '64%'][r] }} /></span>
            <span className="sh-mini__c"><span className="sh-mini__bar" style={{ width: '58%' }} /></span>
            <span className="sh-mini__c" style={{ justifyContent: 'flex-end' }}><span className={'sh-mini__bar' + (formulas ? ' sh-mini__bar--calc' : '')} style={{ width: '52%' }} /></span>
            <span className="sh-mini__c" style={{ justifyContent: 'center' }}><span className="sh-mini__box" style={{ background: r === 0 ? 'rgba(0,228,228,0.3)' : 'transparent', borderColor: r === 0 ? 'var(--accent)' : 'var(--fg-4)' }} /></span>
          </div>
        ))}
      </div>
      <div className="kcard__head">
        <span className="sh-card__status"><span className={'dot dot--' + tone} style={{ width: 6, height: 6 }} />{status}</span>
        <Pill tone="medium">MEDIUM</Pill>
        {due && <span className="pill pill--tag sh-due">{due}</span>}
        <Pill tone="tag">budget</Pill>
      </div>
      <div className="sh-card__foot">
        <b>{cols} × {rows}</b>{formulas && <span className="sh-card__calc">· 8 computed</span>}
        <span className="sh-card__crumb">{crumb}</span>
      </div>
    </article>
  );
}

// the real note card, for the side-by-side comparison
function NoteCard({ title, body, tone = 'todo' }) {
  return (
    <article className={'kcard kcard--' + tone}>
      <div className="kcard__head">
        <h4 className="kcard__title">{title}</h4>
        <Pill tone="medium">MEDIUM</Pill>
      </div>
      {body && <p className="kcard__body">{body}</p>}
      <div className="sh-card__foot">
        <b>454 words</b>
        <span className="sh-card__crumb">Symbiotic › Articles</span>
      </div>
    </article>
  );
}

function ShCard() {
  return (
    <Frame stack>
      <div style={{ display: 'flex', gap: 30, alignItems: 'flex-start' }}>
        <div>
          <span className="sh-callout">equal board width · sheet vs note, side by side</span>
          <Stage w={520} h={286} radius={8}>
            <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 12, padding: 12 }}>
              <SheetCard />
              <NoteCard title="Content Categories Drafting" body="These are the beats Symbiotic covers. Every article falls into one of these categories, which is how the editor picks a writer." />
            </div>
          </Stage>
        </div>
        <div>
          <span className="sh-callout">in column · revised height</span>
          <Stage w={252} h={520} radius={8}>
            <div className="kcol" style={{ border: 0, borderRadius: 0, height: '100%', boxSizing: 'border-box' }}>
              <div className="kcol__head">
                <div className="kcol__title"><span className="dot dot--todo" />To Do<span className="kcol__count">5</span></div>
                <button className="iconbtn">{G('plus', 14)}</button>
              </div>
              <div className="kcol__cards">
                <NoteCard title="Content Categories Drafting" body="These are the beats Symbiotic covers. Every article falls into one of these categories." />
                <SheetCard />
                <SheetCard title="Ad Spend Tracker" cols={5} rows={0} formulas={false} due="8/14/2026" crumb="Symbiotic › Marketing" />
              </div>
            </div>
          </Stage>
        </div>
        <div style={{ display: 'flex', flexDirection: 'column', gap: 22 }}>
          <div>
            <span className="sh-callout">narrow board · glyph header only</span>
            <div style={{ display: 'flex', gap: 14 }}>
              <div style={{ width: 176 }}><SheetCard title="Topic Scout Scores" tone="review" status="Review" cols={12} rows={318} crumb="Symbiotic › Articles" due={null} compact /></div>
              <div style={{ width: 176 }}><SheetCard title="Ad Spend Tracker" tone="progress" status="In Progress" cols={5} rows={0} formulas={false} due="8/14/2026" crumb="Symbiotic › Marketing" compact /></div>
            </div>
          </div>
          <div>
            <span className="sh-callout">all tasks · list density</span>
            <Stage w={476} h={132} radius={8}>
              <div style={{ padding: 8 }}>
                {[['Q3 Content Budget', true, '8 × 24'], ['Editorial Workflow', false, '454 words'], ['Ad Spend Tracker', true, '5 × 0']].map(([t, isSheet, meta]) => (
                  <div key={t} style={{ display: 'flex', alignItems: 'center', gap: 10, padding: '9px 10px', borderRadius: 6, background: isSheet ? 'var(--overlay-hover)' : 'transparent' }}>
                    <span className="sh-card__glyph" style={{ width: 20, height: 20, borderRadius: 5, background: isSheet ? 'var(--accent-soft)' : 'var(--surface-2)', color: isSheet ? 'var(--accent)' : 'var(--fg-4)' }}>
                      {isSheet ? G('grid', 12) : <KitGlyph size={12}><path d="M5 6h14M5 12h14M5 18h9" /></KitGlyph>}
                    </span>
                    <span style={{ flex: 1, fontSize: 13, fontWeight: 500, color: 'var(--fg-1)' }}>{t}</span>
                    <span style={{ fontFamily: 'var(--font-mono)', fontSize: 10.5, color: 'var(--fg-4)' }}>{meta}</span>
                    <Pill tone="medium">MEDIUM</Pill>
                  </div>
                ))}
              </div>
            </Stage>
          </div>
        </div>
        <Cap callout="not a text preview" title="A sheet card shows you a sheet.">
          <p>The face is the kit's own <code>kcard</code> — same padding, radius, status rail, hover and <code>Pill</code> set as a note card — with three sheet-specific parts inside it: a cyan grid glyph beside the title, a micro-grid, and a footer metric. Revised down to <b>two data rows</b>: at equal board width the sheet card is now within a few pixels of a three-line note card, so a column of sheets costs no more scroll than a column of notes.</p>
          <Anno><b>The type-glyph header row is the informative part and it always stays.</b> It comes from the sheet's actual column config, so the preview says text / select / currency / checkbox — the most useful thing a preview of a table can say.</Anno>
          <Anno><b>Board column and card widths are unchanged from the live app.</b> The kit's <code>.kanban</code> track — <code>repeat(5, minmax(220px, 1fr))</code> at <code>gap: 16px</code> — and the card's own width, padding and radius are all untouched. This revision is <b>height only</b>: two micro-grid rows instead of three. Nothing here specifies a new column or card width, and none should be built.</Anno>
          <Anno tone="mute">The narrow-card frame above is drawn at 176px to show the compact treatment's threshold. It is a demonstration of the breakpoint, not a board width — the live board's columns never go below the kit's 220px minimum.</Anno>
          <Anno>Computed columns show their bars in 35% cyan, the same wedge color. Cyan in a preview means "there are formulas in here".</Anno>
          <Anno><b>Standard task metadata is unchanged</b> — the kit's <code>dot dot--tone</code> plus <code>Pill</code> for priority and tags, in the same order note cards use. The rail color still comes from the column. Only the due-date chip adds a tone the kit's pill set doesn't carry.</Anno>
          <Anno>The footer carries the micro-summary <code>8 × 24</code> (columns × rows) and the computed count, then the breadcrumb, matching the note card's footer position.</Anno>
          <Anno tone="mute">An empty sheet reads <code>5 × 0</code> with an unfilled micro-grid rather than a special empty state — it is a sheet with no rows yet, not a broken card.</Anno>
          <Flag label="ratified in review">All Tasks keeps both the glyph swap and the <code>cols × rows</code> metric — the differentiation is the point.</Flag>
        </Cap>
      </div>
    </Frame>
  );
}

// ── 5 · version history, sheet-native right pane ───────────────────
const SNAPS = [
  { t: 'Aug 20, 12:31 PM', src: 'sheet', n: '12 cells', on: true },
  { t: 'Aug 20, 11:59 AM', src: 'agent', n: 'Brit Freed' },
  { t: 'Aug 19, 4:11 PM', src: 'sheet', n: '3 cells' },
  { t: 'Aug 19, 9:02 AM', src: 'restore', n: '' },
  { t: 'Aug 18, 5:40 PM', src: 'sheet', n: 'structure' },
  { t: 'Aug 18, 2:15 PM', src: 'manual', n: '' },
];
function SrcLabel({ src, n }) {
  const map = {
    sheet: { cls: 'sh-src--sheet', ico: G('grid', 11), label: 'Sheet edit' },
    agent: { cls: 'sh-src--agent', ico: <KitGlyph size={11}><circle cx="12" cy="12" r="9" /><circle cx="12" cy="12" r="3" fill="currentColor" /></KitGlyph>, label: 'Agent' },
    restore: { cls: 'sh-src--restore', ico: G('refresh', 11), label: 'Before restore' },
    manual: { cls: '', ico: <KitGlyph size={11}><path d="M17 3.5l3.5 3.5L8 19.5H4.5V16L17 3.5z" /></KitGlyph>, label: 'Manual edit' },
  }[src];
  return (
    <span className={'sh-src ' + map.cls}>
      <span style={{ display: 'flex' }}>{map.ico}</span>
      {map.label}{n && <span className="sh-src__n">· {n}</span>}
    </span>
  );
}
function ShHistory() {
  // The preview renders the snapshot's own columns and values, in order, with
  // its cell styling intact — nothing schematic, nothing re-derived.
  const cols = [
    { n: 'Line item', t: 'text', w: 164 }, { n: 'Owner', t: 'text', w: 92 },
    { n: 'Status', t: 'select', w: 108 }, { n: 'Units', t: 'number', w: 66 },
    { n: 'Rate', t: 'currency', w: 112, code: 'USD' }, { n: 'Total', t: 'currency', w: 118, code: 'USD' },
  ];
  const RED = { bg: 'rgba(239,68,68,0.13)' };
  const SUM = { bg: '#232327', bd: { side: 'top', w: 2, c: 'var(--border-strong)' } };
  const rows = [
    { cells: [{ v: 'Longform essays' }, { v: 'Dana' }, { v: 'Committed' }, { v: '6' }, { v: '1,400.00' }, { v: '8,400.00', k: 'calc' }] },
    { cells: [{ v: 'Newsletter design' }, { v: 'Priya' }, { v: 'Committed' }, { v: '1' }, { v: '2,600.00' }, { v: '2,600.00', k: 'calc' }] },
    { cells: [{ v: 'Illustration set' }, { v: 'Marco' }, { v: 'In flight' }, { v: '12' }, { v: '320.00' }, { v: '3,840.00', k: 'calc' }] },
    { cells: [{ v: 'Video edit pass' }, { v: 'Dana' }, { v: 'In flight' }, { v: '3' }, { v: '900.00' }, { v: '2,700.00', k: 'calc' }] },
    { cells: [{ v: 'Paid amplification', st: RED }, { v: 'Sam' }, { v: 'Proposed' }, { v: '1' }, { v: '5,000.00' }, { v: '5,000.00', k: 'calc', st: { fg: '#EF4444' } }] },
    { cells: [{ v: 'Podcast pilot' }, { v: 'Priya' }, { v: 'Proposed' }, { v: '2' }, { v: '1,150.00' }, { v: '2,300.00', k: 'calc' }] },
    { cells: [{ v: 'Subtotal', st: { ...SUM, fg: '#F0EDE8' } }, { v: '', st: SUM }, { v: '', st: SUM }, { v: '25', st: SUM }, { v: '', st: SUM }, { v: '24,840.00', k: 'calc', st: { bg: 'rgba(0,228,228,0.14)', fg: '#00E4E4', bd: { side: 'top', w: 2, c: 'var(--accent)' } } }] },
  ];
  return (
    <Frame>
      <Stage w={900} h={520} radius={12} border={false}>
        <div style={{ padding: 0, height: '100%' }}>
          <div className="sh-vh">
            <div className="sh-vh__top">
              {I.hist}<span className="sh-vh__title">Version History</span>
              <span className="sh-vh__x">{I.x}</span>
            </div>
            <div className="sh-vh__body">
              <div className="sh-vh__list">
                {SNAPS.map((s) => (
                  <button key={s.t} className={'sh-snap' + (s.on ? ' is-on' : '')}>
                    <div className="sh-snap__t">{s.t}</div>
                    <SrcLabel src={s.src} n={s.n} />
                  </button>
                ))}
              </div>
              <div className="sh-vh__pane">
                <div className="sh-vh__phead">
                  <span className="sh-vh__pt"><b>Aug 20, 12:31 PM</b></span>
                  <span className="sh-vh__pmeta">6 columns · 7 rows · 7 computed</span>
                  <span className="sh-vh__sp" />
                  <span className="sh-vh__undo">Restoring takes a snapshot first</span>
                  <button className="btn btn--primary">{I.restore}<span>Restore</span></button>
                </div>
                <SheetGrid cols={cols} rows={rows} readOnly gutter={34} rowH={32} headH={34} />
              </div>
            </div>
          </div>
        </div>
      </Stage>
      <Cap callout="existing shell · new right pane" title="A snapshot of a sheet looks like the sheet.">
        <p>The two-pane overlay, the snapshot list, the timestamps, the source labels and the Restore button are all unchanged. Only the preview is new: the snapshot's own column headers with their type glyphs, its row numbers, its values in their own columns and alignments, and its cell styling — at the same 32px row height the editor uses.</p>
        <Anno><b>Headers are part of the preview, not chrome.</b> Without them a restore preview can't tell you which column a value belongs to, which is the only question the preview exists to answer.</Anno>
        <Anno>Every cell keeps its editor rendering: currency right-aligned with its code, select as a chip, numbers tabular. A value that reads one way in the editor and another way in the preview is a preview you can't trust.</Anno>
        <Anno><b>Read-only is expressed by removal, not by a badge.</b> No ghost columns or rows, no selection ring, no hover on headers or the gutter, values one step down at <code>fg-2</code>. There is nothing to click, so nothing invites a click.</Anno>
        <Anno>Cell styling is preserved exactly — the red flag on row 5 and the cyan subtotal are the snapshot's, not the preview's. A restore preview that dropped formatting would misrepresent what restoring gives back.</Anno>
        <Anno>Computed cells keep their wedges at 45% opacity: you can see which values were formulas without the wash competing with the styling.</Anno>
        <Anno><b>Restore is confident.</b> Cyan filled button, no confirm dialog, with one quiet line beside it — "Restoring takes a snapshot first" — because that pre-restore snapshot is what makes the action safe.</Anno>
      </Cap>
    </Frame>
  );
}

function ShSourceLabels() {
  return (
    <Frame>
      <Stage w={286} h={330} radius={10}>
        <div className="sh-vh__list" style={{ width: '100%', height: '100%', borderRight: 0, boxSizing: 'border-box' }}>
          {SNAPS.map((s) => (
            <button key={s.t} className={'sh-snap' + (s.on ? ' is-on' : '')}>
              <div className="sh-snap__t">{s.t}</div>
              <SrcLabel src={s.src} n={s.n} />
            </button>
          ))}
        </div>
      </Stage>
      <Cap callout="left pane · source labels" title="Four sources, told apart at a glance.">
        <p>Sheet snapshots needed a label that doesn't read as a manual note edit. Each source gets a glyph, a word, and a color from the existing status palette — and sheet edits add a change count, which is the one piece of context a table snapshot can offer that prose can't.</p>
        <div style={{ display: 'flex', flexDirection: 'column', gap: 10, marginTop: 4, marginBottom: 12 }}>
          {[['sheet', '12 cells', 'Cyan, grid glyph, with the number of cells that changed. Structural edits read "· structure" instead of a count.'],
            ['agent', 'Brit Freed', 'Violet, unchanged from today. The name is the agent, not the source type.'],
            ['restore', '', 'Amber. Written automatically the moment before a restore, so the thing you undo a restore with is visibly different from what you restored.'],
            ['manual', '', 'Neutral surface-2, unchanged from today — a hand edit to a note body.']].map(([src, n, desc]) => (
            <div key={src} style={{ display: 'flex', gap: 12, alignItems: 'flex-start' }}>
              <span style={{ flexShrink: 0, width: 118 }}><SrcLabel src={src} n={n} /></span>
              <span style={{ fontSize: 12, color: 'var(--fg-3)', lineHeight: 1.45 }}>{desc}</span>
            </div>
          ))}
        </div>
        <Anno tone="mute">All four use the same chip shape and 10.5px weight, so the list still scans as one column of timestamps with a qualifier underneath.</Anno>
      </Cap>
    </Frame>
  );
}

// ── 6 · CSV export ─────────────────────────────────────────────────
function ShExport() {
  const cols = BCOLS.slice(0, 5);
  const rows = BROWS.slice(0, 5).map((r) => ({ cells: r.cells.slice(0, 5) }));
  return (
    <Frame>
      <div style={{ position: 'relative' }}>
        <span className="sh-callout">toolbar → ⋯</span>
        <Stage w={700} h={300}>
          <div style={{ display: 'flex', flexDirection: 'column', height: '100%' }}>
            <SheetHead />
            <Toolbar open="more" />
            <FormulaBar cref="E3" src="320.00" />
            <SheetGrid cols={cols} rows={rows} ghostCols={1} ghostRows={2} sel={{ r: 2, c: 4 }} />
            <StatusBar cols={8} rows={8} formulas={8} />
          </div>
        </Stage>
        <At x={472} y={122}>
          <div className="sh-pop" style={{ width: 214 }}>
            <button className="sh-pop__i"><span className="sh-pop__ico">{I.download}</span>Export as CSV…</button>
            <button className="sh-pop__i"><span className="sh-pop__ico">{I.hist}</span>Version history</button>
            <div className="sh-pop__hr" />
            <button className="sh-pop__i"><span className="sh-pop__ico">{I.pencil}</span>Rename sheet</button>
            <button className="sh-pop__i"><span className="sh-pop__ico">{I.folder}</span>Move to…</button>
            <div className="sh-pop__hr" />
            <button className="sh-pop__i sh-pop__danger"><span className="sh-pop__ico">{I.trash}</span>Delete sheet</button>
          </div>
        </At>
      </div>
      <Cap callout="one sheet, one file" title="Export lives in the sheet's own overflow menu.">
        <p>The trigger sits at the top of the toolbar's <code>⋯</code> menu — the same menu the note editor uses for note-level actions — and repeats as one row in the cell context menu. Both open the OS save dialog directly; there is no export panel, no options step, no format picker.</p>
        <Anno><b>The trailing ellipsis is load-bearing.</b> "Export as CSV…" promises a dialog. Without it the label reads like a file appears somewhere on its own.</Anno>
        <Anno>It is the first item because it is the only one of these that produces something the user takes away. Version history sits under it; the destructive row is alone at the bottom.</Anno>
        <Anno>The menu is per-sheet by construction — it hangs off this sheet's toolbar — which is what keeps "one sheet to one CSV" from needing to be explained anywhere.</Anno>
        <Anno tone="mute">Computed cells export their <b>values</b>, not their formulas; a paused sheet has no values, so exporting one exports the formula strings. Worth a line in the dialog's default filename convention rather than UI.</Anno>
      </Cap>
    </Frame>
  );
}

Object.assign(window, { SheetCard, NoteCard, ShCard, SrcLabel, SNAPS, ShHistory, ShSourceLabels, ShExport });
