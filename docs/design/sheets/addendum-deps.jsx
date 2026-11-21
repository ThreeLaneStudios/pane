// Pane Sheets Addendum — the two pieces the addendum frames borrow from the
// full package: the sample sheet's columns/rows, and the Kanban card faces.
// Verbatim from sheets-grid.jsx and sheets-surfaces.jsx so the frames render
// identically to the full document.

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

// Built on the kit's .kcard. Locked in review: widths match the live app.
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

Object.assign(window, { BCOLS, BROWS, SheetCard, NoteCard });
