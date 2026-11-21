// Pane UI-kit primitives — verbatim from ui_kits/pane/ui.jsx, with the ES
// export swapped for a window assign so it can load as a Babel script tag.
// Do not restyle: this is the kit. Sheets-specific glyphs live in
// sheets-shared.jsx and are drawn on the same 24px / 1.7-stroke grid.

const KI = ({ d, c = [], r = [], extra, size = 18, fill = 'none' }) => (
  <svg width={size} height={size} viewBox="0 0 24 24" fill={fill}
    stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round">
    {d && <path d={d} />}
    {c.map((cc, i) => <circle key={i} {...cc} />)}
    {r.map((rr, i) => <rect key={i} {...rr} />)}
    {extra}
  </svg>
);

const Icon = {
  home: (p) => <KI {...p} d="M3 9l9-7 9 7v11a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z M9 22V12h6v10" />,
  check: (p) => <KI {...p} extra={<><polyline points="9 11 12 14 22 4"/><path d="M21 12v7a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h11"/></>} />,
  cal: (p) => <KI {...p} extra={<><rect x="3" y="4" width="18" height="18" rx="2"/><line x1="16" y1="2" x2="16" y2="6"/><line x1="8" y1="2" x2="8" y2="6"/><line x1="3" y1="10" x2="21" y2="10"/></>} />,
  trash: (p) => <KI {...p} extra={<><polyline points="3 6 5 6 21 6"/><path d="M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6"/></>} />,
  grid: (p) => <KI {...p} extra={<><rect x="3" y="3" width="7" height="7"/><rect x="14" y="3" width="7" height="7"/><rect x="14" y="14" width="7" height="7"/><rect x="3" y="14" width="7" height="7"/></>} />,
  bell: (p) => <KI {...p} d="M18 8A6 6 0 0 0 6 8c0 7-3 9-3 9h18s-3-2-3-9 M13.73 21a2 2 0 0 1-3.46 0" />,
  cog: (p) => <KI {...p} extra={<><circle cx="12" cy="12" r="3"/><path d="M19.4 15a2 2 0 0 0 .4 2.2l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a2 2 0 0 0-2.2-.4 2 2 0 0 0-1.2 1.8V22a2 2 0 1 1-4 0v-.1a2 2 0 0 0-1.3-1.8 2 2 0 0 0-2.2.4l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a2 2 0 0 0 .4-2.2 2 2 0 0 0-1.8-1.2H2a2 2 0 1 1 0-4h.1a2 2 0 0 0 1.8-1.3 2 2 0 0 0-.4-2.2l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a2 2 0 0 0 2.2.4H9a2 2 0 0 0 1.2-1.8V2a2 2 0 1 1 4 0v.1a2 2 0 0 0 1.2 1.8 2 2 0 0 0 2.2-.4l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a2 2 0 0 0-.4 2.2V9a2 2 0 0 0 1.8 1.2H22a2 2 0 1 1 0 4h-.1a2 2 0 0 0-1.8 1.2z"/></>} />,
  chev: (p) => <KI {...p} d="M9 18l6-6-6-6" />,
  chevDown: (p) => <KI {...p} d="M6 9l6 6 6-6" />,
  star: (p) => <KI {...p} fill="#F5B824" extra={<polygon points="12 2 15.09 8.26 22 9.27 17 14.14 18.18 21.02 12 17.77 5.82 21.02 7 14.14 2 9.27 8.91 8.26 12 2" stroke="#F5B824"/>} />,
  search: (p) => <KI {...p} extra={<><circle cx="11" cy="11" r="8"/><line x1="21" y1="21" x2="16.65" y2="16.65"/></>} />,
  refresh: (p) => <KI {...p} d="M23 4v6h-6 M1 20v-6h6 M3.5 9a9 9 0 0 1 14.85-3.36L23 10 M1 14l4.64 4.36A9 9 0 0 0 20.5 15" />,
  plus: (p) => <KI {...p} d="M12 5v14 M5 12h14" />,
  send: (p) => <KI {...p} d="M22 2L11 13 M22 2l-7 20-4-9-9-4z" />,
  paperclip: (p) => <KI {...p} d="M21.44 11.05l-9.19 9.19a6 6 0 0 1-8.49-8.49l9.19-9.19a4 4 0 0 1 5.66 5.66l-9.2 9.19a2 2 0 0 1-2.83-2.83l8.49-8.48" />,
  folder: (p) => <KI {...p} d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z" />,
  more: (p) => <KI {...p} extra={<><circle cx="12" cy="12" r="1"/><circle cx="19" cy="12" r="1"/><circle cx="5" cy="12" r="1"/></>} />,
  clock: (p) => <KI {...p} extra={<><circle cx="12" cy="12" r="10"/><polyline points="12 6 12 12 16 14"/></>} />,
};

const Button = ({ variant = 'primary', children, onClick, leading }) => (
  <button className={`btn btn--${variant}`} onClick={onClick}>{leading}<span>{children}</span></button>
);
const Pill = ({ tone = 'medium', children }) => <span className={`pill pill--${tone}`}>{children}</span>;
const Eyebrow = ({ children }) => <div className="eyebrow">{children}</div>;
const StatusDot = ({ tone = 'success' }) => <span className={`dot dot--${tone}`} />;

// Sheets draws its own glyphs on the kit's grid: same 24px viewBox, 1.7
// stroke, round caps. KitGlyph is that base, so a new glyph can't drift.
const KitGlyph = ({ size = 18, children, fill = 'none' }) => (
  <svg width={size} height={size} viewBox="0 0 24 24" fill={fill}
    stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round">{children}</svg>
);

Object.assign(window, { Icon, Button, Pill, Eyebrow, StatusDot, KitGlyph });
