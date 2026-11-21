// Pane Sheets — shared primitives
// Chrome, icons and card faces come from the Pane UI kit (ui_kits/pane):
// kit.css classes (.kanban/.kcol/.kcard/.dot/.pill/.ebtn/.esep/.iconbtn/.btn)
// and the kit's Icon set, loaded via pane-ui.jsx. Only genuinely new Sheets
// parts are authored here — the grid, the corner-wedge language, the formula
// bar, the styling panel — and every new glyph is drawn on KitGlyph, the
// kit's own 24px / 1.7-stroke base, so the two icon languages can't drift.

const COLREF = (i) => String.fromCharCode(65 + i);
const A1 = (r, c) => COLREF(c) + (r + 1);
const G = (name, size) => React.createElement(Icon[name], { size });

// ─── icons: kit glyphs where the kit has one, KitGlyph where it doesn't ───
const I = {
  // from the kit, at the sizes the kit's own components use
  home: G('home', 17), task: G('check', 17), cal: G('cal', 17), trash: G('trash', 16),
  grid: G('grid', 16), bell: G('bell', 17), gear: G('cog', 17),
  chevD: G('chevDown', 12), chevR: G('chev', 12), plus: G('plus', 14),
  folder: G('folder', 14), dots: G('more', 16), clock: G('clock', 15),
  hist: G('clock', 16), restore: G('refresh', 13), search: G('search', 15),
  chevL: <span style={{ display: 'flex', transform: 'rotate(180deg)' }}>{G('chev', 16)}</span>,

  // Sheets-only glyphs, on the kit's grid
  tick: <KitGlyph size={13}><polyline points="4 12.5 9.5 18 20 6" /></KitGlyph>,
  tickS: <KitGlyph size={11}><polyline points="4 12.5 9.5 18 20 6" /></KitGlyph>,
  x: <KitGlyph size={15}><path d="M6 6l12 12M18 6L6 18" /></KitGlyph>,
  xS: <KitGlyph size={11}><path d="M6.5 6.5l11 11M17.5 6.5l-11 11" /></KitGlyph>,
  sortA: <KitGlyph size={12}><path d="M12 20V4M6 10l6-6 6 6" /></KitGlyph>,
  sortD: <KitGlyph size={12}><path d="M12 4v16M6 14l6 6 6-6" /></KitGlyph>,
  funnel: <KitGlyph size={12}><path d="M3 4h18l-7 8v7l-4-2.5V12L3 4z" /></KitGlyph>,
  fx: <KitGlyph size={16}><path d="M7 21c2.6 0 3.3-1.7 3.6-4L13 6.5C13.3 4.4 14.3 3.5 16.4 3.5H19M7.3 12h7.5M17 14l5 7M22 14l-5 7" /></KitGlyph>,
  bucket: <KitGlyph size={16}><path d="M11 3l10 10-7.6 7.6a2.4 2.4 0 0 1-3.4 0L3 13.4a2.4 2.4 0 0 1 0-3.4L11 3z" /><path d="M20 16.5c1.3 1.8 1.3 3.4 0 4.4-1.3 1-3 .6-3.6-.8-.6-1.3.6-2.7 3.6-3.6z" /></KitGlyph>,
  textcolor: <KitGlyph size={16}><path d="M6 18L12 5l6 13M8.3 13.5h7.4" /></KitGlyph>,
  border: <KitGlyph size={16}><rect x="3" y="3" width="18" height="18" rx="2" /><path d="M3 12h18M12 3v18" strokeDasharray="2.6 2.6" opacity=".5" /></KitGlyph>,
  download: <KitGlyph size={15}><path d="M12 3v11M7.5 10L12 14.5 16.5 10M4 20h16" /></KitGlyph>,
  rowIns: <KitGlyph size={16}><rect x="3" y="4" width="18" height="6" rx="1.6" /><path d="M3 16h18M12 13.5v5" /></KitGlyph>,
  colIns: <KitGlyph size={16}><rect x="4" y="3" width="6" height="18" rx="1.6" /><path d="M16 3v18M13.5 12h5" /></KitGlyph>,
  pencil: <KitGlyph size={15}><path d="M17 3.5l3.5 3.5L8 19.5H4.5V16L17 3.5z" /></KitGlyph>,
  swap: <KitGlyph size={15}><path d="M3 8h15L14 4M21 16H6l4 4" /></KitGlyph>,
  warn: <KitGlyph size={13}><path d="M12 3.5l9.5 16.5H2.5L12 3.5zM12 9.5v4.5M12 17.4v.1" /></KitGlyph>,
};

// type glyph — monospace letter for the scalar types, KitGlyph mark for the rest
function TypeGlyph({ t }) {
  const ch = { text: 'T', number: '#', currency: '$' }[t];
  if (ch) return <span style={{ fontFamily: 'var(--font-mono)', fontSize: 11, fontWeight: 500 }}>{ch}</span>;
  if (t === 'date') return G('cal', 13);
  if (t === 'checkbox') return <KitGlyph size={13}><rect x="3.5" y="3.5" width="17" height="17" rx="3.5" /><polyline points="8 12.5 11 15.5 16.5 9" /></KitGlyph>;
  return <KitGlyph size={13}><path d="M4 6h16M4 12h11M4 18h6" /><path d="M17.5 15.5l2.5 2.5 2.5-2.5" /></KitGlyph>;
}
const TYPES = [
  ['text', 'Text'], ['number', 'Number'], ['currency', 'Currency'],
  ['date', 'Date'], ['checkbox', 'Checkbox'], ['select', 'Select'],
];

// ─── formula tokenizer (display only — colors refs / fns / numbers) ───
function Formula({ src, caret }) {
  const parts = String(src).split(/(\b[A-Z]+\d+(?::[A-Z]+\d+)?\b|\b[A-Z]{2,}(?=\()|"[^"]*"|\d+\.?\d*)/g);
  return (
    <span>
      {parts.map((p, i) => {
        if (!p) return null;
        let cls = 'sh-t-op';
        if (/^[A-Z]+\d+(:[A-Z]+\d+)?$/.test(p)) cls = 'sh-t-ref';
        else if (/^[A-Z]{2,}$/.test(p)) cls = 'sh-t-fn';
        else if (/^"/.test(p)) cls = 'sh-t-str';
        else if (/^\d/.test(p)) cls = 'sh-t-num';
        else if (/[a-z]/.test(p)) cls = 'sh-t-num';
        return <span key={i} className={cls}>{p}</span>;
      })}
      {caret && <span className="sh-caret" />}
    </span>
  );
}

// ─── app chrome ──────────────────────────────────────────────────────
// The board beneath the overlay is the kit's KanbanBoard markup (.kanban /
// .kcol / .kcard / .dot / .pill / .iconbtn), not a stand-in.
const BOARD = [
  { id: 'cabinet', name: 'Cabinet', tone: 'cabinet', k: 20, cards: [
    { t: 'Social Media Assets', p: 'medium', b: 'Profile pictures — square logo lockup optimized for small circular crops.' },
    { t: 'Editorial Workflow', p: 'medium', b: 'End-to-end pipeline from topic discovery to published article.' },
    { t: 'Daily Tasks Template', p: 'medium', b: 'Build to $5M net worth through equity in philosophically-aligned ventures.' },
    { t: 'Success Metrics Template', p: 'medium', b: 'A living scoreboard, not a form you duplicate.' },
  ] },
  { id: 'todo', name: 'To Do', tone: 'todo', k: 5, cards: [
    { t: 'Content Categories Drafting', p: 'medium', b: 'These are the beats Symbiotic covers. Every article falls into one.' },
    { t: 'Editorial Calendar', p: 'medium', b: 'The live tracker for all articles — from assigned to published.' },
    { t: 'Newsletter Strategy', p: 'medium' },
    { t: 'Daily Tasks: 20260820', p: 'medium', b: 'Helo' },
  ] },
  { id: 'progress', name: 'In Progress', tone: 'progress', k: 10, cards: [
    { t: 'Twitter/X Setup', p: 'medium', b: 'Secure handle @symbiotic or closest available variant.' },
    { t: 'YouTube Channel Setup', p: 'medium', b: 'Create channel under Symbiotic brand account.' },
    { t: 'Publishing Pipeline Setup', p: 'medium', b: 'Technical infrastructure to publish articles and schedule social posts.' },
    { t: 'Launch Promo Plan', p: 'medium' },
  ] },
  { id: 'review', name: 'Review', tone: 'review', k: 8, cards: [
    { t: 'Social Media Assets', p: 'medium', b: 'Export at platform-specific sizes.' },
    { t: 'Logo Direction', p: 'medium', b: 'Wordmark exploration for Symbiotic Intelligence.' },
    { t: 'Topic Scout Pipeline', p: 'medium', b: 'Sources, format and scheduling for the nightly run.' },
  ] },
];
function AppShell({ children, overlayWidth = 1000, project = 'Symbiotic' }) {
  const railDots = ['#e879f9', '#f59e0b', '#00E4E4', '#a855f7', '#6b6b6b'];
  return (
    <div className="sh-app">
      {/* the sidebar in its collapsed state — kit Icon glyphs at Sidebar's own 17px */}
      <div className="sh-rail">
        <div className="sh-rail__ico">{I.home}</div>
        <div className="sh-rail__ico is-on">{I.task}</div>
        <div className="sh-rail__ico">{I.cal}</div>
        <div className="sh-rail__ico">{I.trash}</div>
        <div className="sh-rail__ico">{I.grid}</div>
        <div className="sh-rail__dots">{railDots.map((c, i) => <span key={i} className="sh-rail__dot" style={{ background: c }} />)}</div>
        <div className="sh-rail__sp" />
        <div className="sh-rail__ico">{I.bell}</div>
        <div className="sh-rail__ico">{I.gear}</div>
      </div>
      <div className="sh-main">
        <div className="sh-top">
          <span className="sh-top__back">{I.chevL}</span>
          <span className="sh-top__title">{project}</span>
          <span className="sh-top__sp" />
          <button className="btn btn--primary">{I.plus}<span>New Note</span></button>
        </div>
        <div className="sh-boardwrap">
          <div className="kanban sh-kanban">
            {BOARD.map((col) => (
              <div className="kcol" key={col.id}>
                <div className="kcol__head">
                  <div className="kcol__title"><span className={'dot dot--' + col.tone} />{col.name}<span className="kcol__count">{col.k}</span></div>
                  <button className="iconbtn">{G('plus', 14)}</button>
                </div>
                <div className="kcol__cards">
                  {col.cards.map((c) => (
                    <article className={'kcard kcard--' + col.tone} key={c.t}>
                      <div className="kcard__head">
                        <h4 className="kcard__title">{c.t}</h4>
                        {c.p && <Pill tone={c.p}>{c.p.toUpperCase()}</Pill>}
                      </div>
                      {c.b && <p className="kcard__body">{c.b}</p>}
                    </article>
                  ))}
                  <button className="kcol__add">{G('plus', 13)} Add task</button>
                </div>
              </div>
            ))}
          </div>
        </div>
      </div>
      <div className="sh-overlay" style={{ width: overlayWidth }}>{children}</div>
    </div>
  );
}

// overlay header — title + the same metadata chips the note editor shows
function SheetHead({ title = 'Q3 Content Budget', folder = 'Articles' }) {
  return (
    <div className="sh-ohead">
      <span className="sh-ohead__title">{title}</span>
      <span className="sh-chip">{I.folder} {folder} {I.chevD}</span>
      <span className="sh-chip">Cabinet</span>
      <span className="sh-chip sh-chip--accent">Medium</span>
      <span className="sh-chip sh-chip--date">9/25/2026</span>
    </div>
  );
}

// ─── toolbar ─────────────────────────────────────────────────────────
// The note editor's own .etool row (kit.css): .ebtn / .ebtn--active / .esep.
// structure | style | formula | view | overflow. Explicit insert buttons
// exist but are never the required path — ghost typing is.
function Toolbar({ open, styleSwatch = 'transparent', children }) {
  const B = ({ id, ico, label }) => (
    <button className={'ebtn' + (open === id ? ' ebtn--active' : '')}>{ico}{label && <span>{label}</span>}</button>
  );
  return (
    <div className="sh-tb" style={{ position: 'relative' }}>
      <B id="colins" ico={I.colIns} />
      <B id="rowins" ico={I.rowIns} />
      <span className="esep" />
      <button className={'ebtn sh-tb__fill' + (open === 'fill' ? ' ebtn--active' : '')}>{I.bucket}<span className="sh-tb__swatch" style={{ background: styleSwatch }} /></button>
      <B id="color" ico={I.textcolor} />
      <B id="border" ico={I.border} />
      <span className="esep" />
      <B id="fx" ico={I.fx} />
      <span className="esep" />
      <B id="sort" ico={I.sortD} />
      <B id="filter" ico={I.funnel} />
      <span className="esep" />
      <B id="hist" ico={I.hist} />
      <span className="sh-tb__sp" />
      <B id="more" ico={I.dots} />
      {children}
    </div>
  );
}

function FormulaBar({ cref = 'A1', src, editing, empty }) {
  return (
    <div className={'sh-fbar' + (editing ? ' is-editing' : '')}>
      <div className="sh-fbar__ref">{cref}</div>
      <div className="sh-fbar__fx">fx</div>
      <div className="sh-fbar__in">
        {src ? <Formula src={src} caret={editing} /> : <span className="sh-fbar__empty">{empty || 'Empty cell'}</span>}
      </div>
      <div className="sh-fbar__x">{I.chevD}</div>
    </div>
  );
}

function StatusBar({ cols, rows, formulas, saved = 'just now', paused }) {
  return (
    <div className="sh-status">
      <b>{cols}</b> columns<span>·</span><b>{rows}</b> rows
      {formulas != null && <><span>·</span><span className="sh-status__calc"><span className="sh-status__wedge" /><b>{formulas}</b> computed</span></>}
      {paused && <><span>·</span><span style={{ color: 'var(--fg-3)' }}>computation paused</span></>}
      <span className="sh-status__sp" />
      <span>{saved}</span>
    </div>
  );
}

// ─── the grid ────────────────────────────────────────────────────────
// cols: [{n, t, w, code, sort:'asc'|'desc', filter:bool}]
// rows: [{h, cells:[{v, k:'calc'|'err'|'mis'|'raw', st:{bg,fg,bd}}]}]
// sel/range/refRange are 0-indexed into real rows+cols.
function SheetGrid({
  cols, rows, ghostCols = 3, ghostRows = 4, rowH = 34, headH = 38, gutter = 40,
  sel, range, refRange, colSel, rowSel, readOnly, openHeader, typingGhost,
  gripCol, gripRow, editor, children, style,
}) {
  const widths = cols.map((c) => c.w || 148);
  const ghostW = 120;
  // Read-only renders no ghost band, so the template must not reserve one —
  // otherwise each row starts a column late and the whole grid shears.
  const gc = readOnly ? 0 : ghostCols;
  const gr = readOnly ? 0 : ghostRows;
  const template = [gutter, ...widths, ...Array(gc).fill(ghostW)].map((w) => w + 'px').join(' ');
  const inR = (r, c, R) => R && r >= R.r0 && r <= R.r1 && c >= R.c0 && c <= R.c1;
  const edge = (r, c, R, p) => !R ? '' : [
    r === R.r0 ? ` is-${p}edge-t` : '', r === R.r1 ? ` is-${p}edge-b` : '',
    c === R.c0 ? ` is-${p}edge-l` : '', c === R.c1 ? ` is-${p}edge-r` : '',
  ].join('');

  const cellStyle = (st) => {
    if (!st) return undefined;
    const s = {};
    if (st.bg) s.background = st.bg;
    if (st.fg) s.color = st.fg;
    if (st.bd) {
      const { w = 1, c = 'var(--border-strong)', s: ls = 'solid', side = 'all' } = st.bd;
      if (side === 'all') s.boxShadow = `inset 0 0 0 ${w}px ${c}`;
      else if (side === 'bottom') s.borderBottom = `${w}px ${ls} ${c}`;
      else if (side === 'top') s.borderTop = `${w}px ${ls} ${c}`;
      else if (side === 'left') s.borderLeft = `${w}px ${ls} ${c}`;
      else if (side === 'right') s.borderRight = `${w}px ${ls} ${c}`;
    }
    return s;
  };

  const Cell = ({ cell, col, r, c }) => {
    const k = cell && cell.k;
    let cls = 'sh-c';
    if (k === 'err') cls += ' sh-c--err';
    else if (k === 'raw') cls += ' sh-c--raw';
    else if (k === 'mis') cls += ' sh-c--mis';
    else {
      if (k === 'calc') cls += ' sh-c--calc';
      cls += ' sh-c--' + (col.t === 'number' ? 'num' : col.t === 'currency' ? 'currency' : col.t === 'date' ? 'date' : col.t === 'checkbox' ? 'check' : 'text');
    }
    if (!readOnly) {
      if (inR(r, c, range)) cls += ' is-range' + edge(r, c, range, 'r');
      if (inR(r, c, refRange)) cls += ' is-ref' + edge(r, c, refRange, 'ref');
      if (sel && sel.r === r && sel.c === c) cls += ' is-anchor';
    }
    let body = null;
    const v = cell ? cell.v : '';
    if (k === 'err' || k === 'raw' || k === 'mis') body = <span className="sh-c__v">{v}</span>;
    else if (col.t === 'checkbox') body = <span className={'sh-box' + (v ? ' is-on' : '')}>{v ? I.tickS : null}</span>;
    else if (col.t === 'select') body = v ? <span className="sh-pill">{v}</span> : null;
    else if (col.t === 'currency') body = v !== '' && v != null ? <><span className="sh-c__v">{v}</span><span className="sh-cur">{col.code || 'USD'}</span></> : null;
    else body = <span className="sh-c__v">{v}</span>;
    return (
      <div className={cls} style={cellStyle(cell && cell.st)}>
        {body}
        {k === 'calc' && <span className="sh-w sh-w--calc" />}
        {k === 'err' && <span className="sh-w sh-w--err" />}
        {k === 'mis' && <span className="sh-w sh-w--mis" />}
        {editor && editor.r === r && editor.c === c && editor.node}
      </div>
    );
  };

  return (
    <div className="sh-gridwrap" style={style}>
      <div className={'sh-grid' + (readOnly ? ' sh-ro' : '')} style={{ gridTemplateColumns: template }}>
        {/* header band */}
        <div className="sh-corner" style={{ height: headH }}>{!readOnly && G('grid', 14)}</div>
        {cols.map((c, i) => (
          <div key={i} className={'sh-h' + (openHeader === i ? ' is-open' : '') + (colSel === i ? ' is-sel' : '')} style={{ height: headH }}>
            <span className="sh-h__ref">{COLREF(i)}</span>
            <span className="sh-h__type"><TypeGlyph t={c.t} /></span>
            <span className="sh-h__name">{c.n}</span>
            {c.t === 'currency' && <span className="sh-h__code">{c.code || 'USD'}</span>}
            {c.sort && <span className="sh-h__flag">{c.sort === 'asc' ? I.sortA : I.sortD}</span>}
            {c.filter && <span className="sh-h__flag">{I.funnel}</span>}
            {!readOnly && <span className={'sh-h__grip' + (gripCol === i ? ' is-forced' : '')} />}
          </div>
        ))}
        {!readOnly && Array.from({ length: gc }).map((_, i) => (
          <div key={'gh' + i} className="sh-h sh-h--ghost" style={{ height: headH }}>
            <span className="sh-h__ref">{COLREF(cols.length + i)}</span>
          </div>
        ))}

        {/* real rows */}
        {rows.map((row, r) => {
          const h = row.h || rowH;
          return (
            <React.Fragment key={r}>
              <div className={'sh-g' + (rowSel === r ? ' is-sel' : '')} style={{ height: h }}>
                {r + 1}
                {!readOnly && <span className={'sh-g__grip' + (gripRow === r ? ' is-forced' : '')} />}
              </div>
              {cols.map((c, ci) => <Cell key={ci} cell={row.cells[ci]} col={c} r={r} c={ci} />)}
              {!readOnly && Array.from({ length: gc }).map((_, i) => <div key={'gc' + i} className="sh-c sh-c--ghost" />)}
            </React.Fragment>
          );
        })}

        {/* ghost rows */}
        {!readOnly && Array.from({ length: gr }).map((_, gi) => (
          <React.Fragment key={'gr' + gi}>
            <div className="sh-g sh-g--ghost" style={{ height: rowH }}>{rows.length + gi + 1}</div>
            {cols.map((c, ci) => (
              <div key={ci} className={'sh-c sh-c--ghost' + (typingGhost && typingGhost.r === rows.length + gi && typingGhost.c === ci ? ' is-typing' : '')}>
                {typingGhost && typingGhost.r === rows.length + gi && typingGhost.c === ci && <span className="sh-c__v">{typingGhost.v}</span>}
              </div>
            ))}
            {Array.from({ length: gc }).map((_, i) => (
              <div key={'gg' + i} className={'sh-c sh-c--ghost' + (typingGhost && typingGhost.r === rows.length + gi && typingGhost.c === cols.length + i ? ' is-typing' : '')}>
                {typingGhost && typingGhost.r === rows.length + gi && typingGhost.c === cols.length + i && <span className="sh-c__v">{typingGhost.v}</span>}
              </div>
            ))}
          </React.Fragment>
        ))}
        {children}
      </div>
    </div>
  );
}

// absolute overlay inside a stage (popovers, guides, editors)
const At = ({ x, y, children, z = 30 }) => (
  <div style={{ position: 'absolute', left: x, top: y, zIndex: z }}>{children}</div>
);

// ─── annotation helpers (house style) ────────────────────────────────
function Frame({ children, stack, gap, pad }) {
  return <div className={'sh-frame' + (stack ? ' sh-frame--stack' : '')} style={{ gap, padding: pad }}>{children}</div>;
}
function Stage({ w, h, children, radius = 10, border = true }) {
  return (
    <div className="sh-stage" style={{ width: w, height: h, borderRadius: radius, overflow: 'hidden', border: border ? '1px solid var(--border)' : 0, background: 'var(--bg)' }}>{children}</div>
  );
}
function Cap({ callout, title, children }) {
  return (
    <div className="sh-cap">
      {callout && <span className="sh-callout">{callout}</span>}
      {title && <h4>{title}</h4>}
      {children}
    </div>
  );
}
const Anno = ({ tone, children }) => (
  <div className="sh-anno"><span className={'sh-anno__dot' + (tone ? ' sh-anno__dot--' + tone : '')} /><span className="sh-anno__t">{children}</span></div>
);
const Flag = ({ children, label = 'flagged for review' }) => (
  <div className="sh-flag"><span className="sh-flag__l">{label}</span><span className="sh-flag__t">{children}</span></div>
);

Object.assign(window, {
  I, G, TypeGlyph, TYPES, COLREF, A1, Formula, AppShell, SheetHead, Toolbar,
  FormulaBar, StatusBar, SheetGrid, At, Frame, Stage, Cap, Anno, Flag,
});
