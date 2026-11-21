// Pane Sheets — ADDENDUM
// A · New Sheet button pair   B · Select choices editor
// Appended after the package locked. Uses only the existing kit primitives
// (.btn box, Pill, dot, .kanban/.kcol/.kcard) and the popover language from
// sheets-shared.jsx. Nothing already locked is touched.

// ══ A · the New Sheet button pair ══════════════════════════════════
const NewSheetBtn = ({ compact }) => (
  <button className="btn btn--outline">{G('plus', 14)}{!compact && <span>New Sheet</span>}</button>
);
const NewNoteBtn = ({ compact }) => (
  <button className="btn btn--primary">{G('plus', 14)}{!compact && <span>New Note</span>}</button>
);

// the host header at each of the three widths
function HostHeader({ title = 'Symbiotic', w, mode = 'full' }) {
  return (
    <div className="sh-hdr" style={{ width: w }}>
      <span className="sh-top__back">{I.chevL}</span>
      <span className="sh-hdr__title">{title}</span>
      <span className="sh-hdr__sp" />
      <span className="sh-hdr__pair">
        {mode === 'full' && <><NewSheetBtn /><NewNoteBtn /></>}
        {mode === 'icon' && <><NewSheetBtn compact /><NewNoteBtn /></>}
        {mode === 'menu' && <><NewNoteBtn /><button className="iconbtn">{I.chevD}</button></>}
      </span>
    </div>
  );
}

function ShNewSheetBtn() {
  return (
    <Frame stack>
      {/* the pair in the live board header, normal width */}
      <div>
        <div>
          <span className="sh-callout">board header · normal width · 1440</span>
          <Stage w={820} h={340} radius={0}>
            <div style={{ display: 'flex', flexDirection: 'column', height: '100%', background: 'var(--bg)' }}>
              <HostHeader />
              <div style={{ flex: 1, minHeight: 0, overflow: 'hidden' }}>
                <div className="kanban sh-kanban" style={{ gridTemplateColumns: 'repeat(3, 220px)' }}>
                  {BOARD.slice(0, 3).map((col) => (
                    <div className="kcol" key={col.id}>
                      <div className="kcol__head">
                        <div className="kcol__title"><span className={'dot dot--' + col.tone} />{col.name}<span className="kcol__count">{col.k}</span></div>
                        <button className="iconbtn">{G('plus', 14)}</button>
                      </div>
                      <div className="kcol__cards">
                        {col.id === 'todo'
                          ? <><SheetCard /><NoteCard title={col.cards[0].t} body={col.cards[0].b} /></>
                          : col.cards.slice(0, 2).map((c) => (
                            <article className={'kcard kcard--' + col.tone} key={c.t}>
                              <div className="kcard__head"><h4 className="kcard__title">{c.t}</h4><Pill tone="medium">MEDIUM</Pill></div>
                              {c.b && <p className="kcard__body">{c.b}</p>}
                            </article>
                          ))}
                      </div>
                    </div>
                  ))}
                </div>
              </div>
            </div>
          </Stage>
        </div>
        <Cap callout="siblings, not equals" title="Same box. Inverted ground.">
          <p style={{ maxWidth: 780 }}>New Sheet takes the kit's <code>.btn</code> box unchanged — same height, padding, radius, gap, font and icon size as New Note — and inverts only the ground: transparent fill, cyan border, cyan label, against New Note's filled cyan. Identical geometry is what makes them read as a pair; the fill is what ranks them.</p>
          <div style={{ display: 'flex', gap: 30, maxWidth: 860 }}>
            <div style={{ flex: 1 }}>
              <Anno><b>New Note stays primary and stays rightmost.</b> It is the far more frequent action and it already owns that position in every host view. New Sheet sits to its left, so the primary button never moves from where people reach for it.</Anno>
              <Anno><b>The same plus leads both labels</b>, in the same 14px slot. Ruled in review: New Sheet takes New Note's plus rather than the sheet grid glyph — both buttons create something, the plus says so, and the label is what distinguishes them. Nothing else in the pair differs but the fill.</Anno>
            </div>
            <div style={{ flex: 1 }}>
              <Anno tone="mute">We did not use the kit's <code>.btn--ghost</code>. Its <code>fg-3</code> label on a faint border reads as tertiary — a "more options" affordance — not as the second of two ways to create something.</Anno>
              <Anno tone="mute">Same pair, same order, in project view, folder view and All Tasks. Nothing about the host changes besides the added button.</Anno>
            </div>
          </div>
        </Cap>
      </div>

      {/* narrow behaviour */}
      <div>
        <div style={{ display: 'flex', flexDirection: 'column', gap: 18 }}>
          <div>
            <span className="sh-callout">≥ 900 · both labelled</span>
            <Stage w={820} h={60} radius={6}><HostHeader w={820} /></Stage>
          </div>
          <div>
            <span className="sh-callout">720–900 · New Sheet drops its label</span>
            <Stage w={720} h={60} radius={6}><HostHeader w={720} mode="icon" /></Stage>          </div>
          <div>
            <span className="sh-callout">&lt; 720 · New Sheet moves into the caret menu</span>
            <div style={{ position: 'relative', paddingBottom: 96 }}>
              <Stage w={560} h={60} radius={6}><HostHeader w={560} title="Symbiotic" mode="menu" /></Stage>
              <At x={352} y={70}>
                <div className="sh-pop" style={{ width: 186 }}>
                  <button className="sh-pop__i"><span className="sh-pop__ico" style={{ color: 'var(--accent)' }}>{G('plus', 14)}</span>New Sheet</button>
                  <button className="sh-pop__i"><span className="sh-pop__ico">{I.folder}</span>New Folder</button>
                </div>
              </At>
            </div>
          </div>
        </div>
        <Cap callout="what collapses, in order" title="The label goes before the button does.">
          <p style={{ maxWidth: 780 }}>Two steps, and the primary button is never the thing that gives way. First New Sheet sheds its label and holds its square box — the glyph alone is legible and the target stays full size. Below that it folds into a caret menu beside New Note, where it sits above New Folder as one of the create verbs.</p>
          <div style={{ display: 'flex', gap: 30, maxWidth: 860 }}>
            <div style={{ flex: 1 }}>
              <Anno><b>New Note keeps its label at every width.</b> If only one button can be read, it should be the one people press.</Anno>
              <Anno>The icon-only step keeps the kit's box, so the pair still reads as a pair — a filled rectangle and an outlined square, same height, same gap.</Anno>
            </div>
            <div style={{ flex: 1 }}>
              <Anno tone="mute">Breakpoints are on the header's own width, not the viewport, so a narrow project pane collapses the same way a narrow window does.</Anno>
              <Flag label="flagged · minor">The caret menu is a new affordance in the board header. If the header already has an overflow menu in a view we haven't seen, New Sheet should join that one rather than add a second caret.</Flag>
            </div>
          </div>
        </Cap>
      </div>
    </Frame>
  );
}

// ══ B · the select choices editor ══════════════════════════════════
const OPTS = ['Committed', 'In flight', 'Proposed', 'Cut'];

function OptRow({ v, editing, dragging, error }) {
  return (
    <div className={'sh-opt__i' + (dragging ? ' is-drag' : '')}>
      <span className="sh-opt__grip"><KitGlyph size={12}><path d="M9 6h.01M9 12h.01M9 18h.01M15 6h.01M15 12h.01M15 18h.01" strokeWidth="2.6" /></KitGlyph></span>
      <span className="sh-opt__val">
        {editing
          ? <input className={'sh-opt__in' + (error ? ' is-error' : '')} defaultValue={v} />
          : <span className="sh-chipfake">{v}</span>}
      </span>
      {!editing && <span className="sh-opt__x">{I.xS}</span>}
    </div>
  );
}

function OptPanel({ state = 'list' }) {
  return (
    <div className="sh-opt">
      <div className="sh-opt__head">
        <span className="sh-opt__back">{<span style={{ display: 'flex', transform: 'rotate(180deg)' }}>{I.chevR}</span>}</span>
        <span className="sh-opt__t">Options</span>
        <span className="sh-opt__n">{state === 'empty' ? '0' : state === 'add' ? '4' : '4'}</span>
      </div>
      {state === 'empty' ? (
        <>
          <div className="sh-opt__empty">
            <b>No choices yet.</b><br />Cells in this column are empty until you add one. Existing values stay as they are and render as mismatches.
          </div>
          <button className="sh-opt__add"><span className="sh-pop__ico">{I.plus}</span>Add the first choice</button>
        </>
      ) : (
        <>
          <div className="sh-opt__list">
            {state === 'reorder' ? (
              <>
                <OptRow v="Committed" />
                <div className="sh-opt__drop" />
                <OptRow v="Cut" dragging />
                <OptRow v="In flight" />
                <OptRow v="Proposed" />
              </>
            ) : state === 'dupe' ? (
              <>
                {OPTS.map((o) => <OptRow key={o} v={o} />)}
                <OptRow v="Committed" editing error />
                <div className="sh-opt__err">{I.warn}<span>“Committed” is already a choice.</span></div>
              </>
            ) : (
              OPTS.map((o, i) => <OptRow key={o} v={o} editing={state === 'rename' && i === 1} />)
            )}
          </div>
          {state === 'add'
            ? <div className="sh-opt__i"><span className="sh-opt__grip" /><span className="sh-opt__val"><input className="sh-opt__in" placeholder="Choice name" /></span></div>
            : state === 'dupe' ? null
            : <button className="sh-opt__add"><span className="sh-pop__ico">{I.plus}</span>Add choice</button>}
          <div className="sh-opt__foot">Removing a choice leaves cell values untouched. A cell holding a removed choice renders as a mismatch.</div>
        </>
      )}
    </div>
  );
}

function ShOptions() {
  const cols = BCOLS.slice(0, 4);
  const rows = BROWS.slice(0, 5).map((r) => ({ cells: r.cells.slice(0, 4) }));
  const misRows = rows.map((r, i) => {
    if (i !== 2) return r;
    const cells = r.cells.map((c) => ({ ...c }));
    cells[2] = { v: 'In flight', k: 'mis' };
    return { cells };
  });
  return (
    <Frame stack>
      {/* entry: header menu → Edit options… */}
      <div>
        <div style={{ position: 'relative', paddingRight: 176 }}>
          <span className="sh-callout">column header menu · type is select</span>
          <Stage w={620} h={300}>
            <div style={{ display: 'flex', flexDirection: 'column', height: '100%' }}>
              <Toolbar />
              <FormulaBar cref="C3" src="In flight" />
              <SheetGrid cols={cols} rows={rows} ghostCols={1} ghostRows={1} openHeader={2} colSel={2} />
            </div>
          </Stage>
          <At x={314} y={90}>
            <div className="sh-pop" style={{ width: 208 }}>
              <button className="sh-pop__i"><span className="sh-pop__ico">{I.pencil}</span>Rename<span className="sh-pop__kb">⏎</span></button>
              <button className="sh-pop__i"><span className="sh-pop__ico">{I.swap}</span>Type<span className="sh-pop__kb">Select</span><span style={{ marginLeft: 6, color: 'var(--fg-4)', display: 'flex' }}>{I.chevR}</span></button>
              <button className="sh-pop__i is-on"><span className="sh-pop__ico" style={{ color: 'var(--accent)' }}>{I.dots}</span>Edit options…<span className="sh-opt__n" style={{ marginLeft: 'auto' }}>4</span><span style={{ marginLeft: 6, color: 'var(--accent)', display: 'flex' }}>{I.chevR}</span></button>
              <div className="sh-pop__hr" />
              <button className="sh-pop__i"><span className="sh-pop__ico">{I.sortA}</span>Sort ascending</button>
              <button className="sh-pop__i"><span className="sh-pop__ico">{I.sortD}</span>Sort descending</button>
              <button className="sh-pop__i"><span className="sh-pop__ico">{I.funnel}</span>Filter…</button>
              <div className="sh-pop__hr" />
              <button className="sh-pop__i sh-pop__danger"><span className="sh-pop__ico">{I.trash}</span>Delete column</button>
            </div>
          </At>
          <At x={528} y={148}><OptPanel /></At>
        </div>
        <Cap callout="a sixth row, conditional" title="One nested panel, reached two ways.">
          <p style={{ maxWidth: 780 }}>The five-verb menu gains an <b>Edit options…</b> row — present only when the column's type is select, sitting directly under Type because it is the continuation of that decision. It carries the current choice count and opens a nested panel to its right. The cell dropdown's existing "Edit options…" row opens the same panel.</p>
          <div style={{ display: 'flex', gap: 30, maxWidth: 860 }}>
            <div style={{ flex: 1 }}>
              <Anno><b>It stays inside the popover language.</b> Same surface, same border, same row metrics as the menu it came from — a modal for four strings would be heavier than the thing it edits.</Anno>
              <Anno>The count on the row is the useful glance: "Select · 4" answers what the column is and whether it has been configured, without opening anything.</Anno>
            </div>
            <div style={{ flex: 1 }}>
              <Anno><b>Choices render as the chips they will become.</b> Each row shows the value in the same <code>sh-pill</code> the cell uses, so the panel is a preview of the column, not a text list.</Anno>
              <Anno tone="mute">Order in the panel is the order in <code>options_json</code>, which is the order the cell dropdown shows. There is no alphabetical mode — a status list is meaningful in sequence.</Anno>
            </div>
          </div>
        </Cap>
      </div>

      {/* the four states */}
      <div>
        <div style={{ display: 'flex', gap: 18, flexWrap: 'wrap' }}>
          <div><span className="sh-callout">add</span><OptPanel state="add" /></div>
          <div><span className="sh-callout">rename in place</span><OptPanel state="rename" /></div>
          <div><span className="sh-callout">duplicate · rejected with signal</span><OptPanel state="dupe" /></div>
          <div><span className="sh-callout">reorder · drag</span><OptPanel state="reorder" /></div>
          <div><span className="sh-callout">empty · what type-change produces</span><OptPanel state="empty" /></div>
        </div>
      </div>
      <div>
        <div>
          <span className="sh-callout">after removing "In flight" · C3 renders as a mismatch</span>
          <Stage w={620} h={230}>
            <SheetGrid cols={cols} rows={misRows} ghostCols={1} ghostRows={1} sel={{ r: 2, c: 2 }} />
          </Stage>
        </div>
        <Cap callout="four states" title="Every verb is one gesture, and removal never destroys.">
          <p style={{ maxWidth: 780 }}>Add appends an empty field at the end of the list, focused, committing on Enter — so adding three choices is three keystroke cycles with no trip to a button. Rename swaps the chip for a field in place. Remove is an ✕ that appears on row hover, in the danger tone, with no confirm.</p>
          <div style={{ display: 'flex', gap: 30, maxWidth: 860 }}>
            <div style={{ flex: 1 }}>
              <Anno><b>Removal does not touch stored values.</b> A cell holding a removed choice keeps its text and renders as a mismatch — amber top-right wedge, dotted underline, left-aligned — exactly as a type change behaves. The panel's footer says this in one line, permanently, which is why removal needs no dialog.</Anno>
              <Anno>Reorder is a drag on the six-dot grip with a cyan drop line, the same gesture the Kanban cards use. It is cheap here because the rows are already a list — worth shipping.</Anno>
            </div>
            <div style={{ flex: 1 }}>
              <Anno><b>The empty state is the one that matters most</b>, because changing a column to select is what produces it. It says what the column does now ("cells are empty until you add one"), what happened to what was there, and offers one button: <b>Add the first choice</b>.</Anno>
              <Anno tone="mute">No colour-per-choice in v1. Cells render every choice in the neutral chip, and per-cell styling already covers anyone who wants "Cut" to look red.</Anno>
              <Anno tone="danger"><b>Duplicate names are rejected with a signal, not silently.</b> The field keeps focus and switches to the danger ring, shakes once, and a one-line reason appears beneath it naming the collision. Nothing is lost — the typed text stays, so the fix is to keep typing.</Anno>
            </div>
          </div>
        </Cap>
      </div>
    </Frame>
  );
}

Object.assign(window, { NewSheetBtn, NewNoteBtn, HostHeader, ShNewSheetBtn, OptPanel, OptRow, ShOptions, OPTS });
