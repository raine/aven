type Row = { status: 'todo' | 'inbox' | 'done'; ref: string; title: string; high?: boolean; due?: string; hit?: boolean; fresh?: boolean };
export type Scene = { prompt: string; seconds: number; expandable?: boolean; result: string; reply: string; apply: (rows: Row[]) => Row[] };

const glyphs = { todo: '□', inbox: '▣', done: '✓' };
export const base = (): Row[] => [
  { status: 'todo', ref: 'AVN-WV80', title: 'Fix flaky CI sync test', high: true, due: 'Thu' },
  { status: 'inbox', ref: 'AVN-H3QD', title: 'Show sync status in the TUI footer', due: 'Mon' },
  { status: 'inbox', ref: 'APP-P8N3', title: 'Document token rotation' },
  { status: 'inbox', ref: 'APP-C2MF', title: 'Drop the legacy auth middleware' },
];

const task = (title: string, meta: string) => `
  <div class="mcp-task">
    <i></i>
    <div><b>${title}</b><span>${meta}</span></div>
    <em>Complete</em>
  </div>`;
const changes = (heading: string, items: string) => `<p class="mcp-heading"><svg viewBox="0 0 24 24"><circle cx="12" cy="12" r="10" /><path d="m7.5 12.5 3 3 6-6.5" /></svg>${heading}</p>${items}`;
const change = (icon: string, title: string, lines: string[]) => `
  <div class="mcp-change">
    <svg viewBox="0 0 24 24">${icon}</svg>
    <div><b>${title}</b>${lines.map((line) => `<span>${line}</span>`).join('')}</div>
  </div>`;
const check = '<path d="m5 13 4 4 10-11" />';
const repeat = '<path d="M20 11a8 8 0 0 0-14-5l-2 2M4 4v4h4M4 13a8 8 0 0 0 14 5l2-2M20 20v-4h-4" />';

export const scenes: Scene[] = [
  {
    prompt: "What's due this week?",
    seconds: 8,
    expandable: true,
    result: `<p>You have <strong>2 tasks</strong> due this week.</p>${task('Show sync status in the TUI footer', 'Inbox · AVN-H3QD · Mon')}${task('Fix flaky CI sync test', 'To do · AVN-WV80 <u>High priority</u> · Thu')}`,
    reply: 'The footer work comes first, on Monday.',
    apply: (rows) => rows.map((row) => ({ ...row, hit: Boolean(row.due) })),
  },
  {
    prompt: 'File a high priority bug: login fails after token refresh. Due tomorrow.',
    seconds: 11,
    result: changes('Bug filed in Aven.', change(check, 'Login fails after token refresh', ['New task · High priority · Due tomorrow, Saturday', 'APP-R4TN'])),
    reply: 'It is now your nearest deadline.',
    apply: (rows) => [{ status: 'todo', ref: 'APP-R4TN', title: 'Login fails after token refresh', high: true, due: 'Sat', hit: true, fresh: true }, ...rows],
  },
  {
    prompt: 'The token rotation docs are merged. Mark that done and note the PR.',
    seconds: 9,
    result: changes('Both changes completed in Aven.', change(check, 'Document token rotation', ['Marked as done', 'Note added: "Merged in #482."', 'APP-P8N3'])),
    reply: 'You have 3 open tasks left.',
    apply: (rows) => rows.map((row) => (row.ref === 'APP-P8N3' ? { ...row, status: 'done' as const, hit: true } : row)),
  },
  {
    prompt: 'Add a weekly task to review dependency updates every Monday.',
    seconds: 12,
    result: changes('Recurring task created in Aven.', change(repeat, 'Review dependency updates', ['New recurring task · Every Monday', 'First occurrence: Monday', 'AVN-CC4T'])),
    reply: 'It will recur weekly on Mondays.',
    apply: (rows) => [...rows, { status: 'todo', ref: 'AVN-CC4T', title: 'Review dependency updates ↻', due: 'Mon', hit: true, fresh: true }],
  },
];

const actions = `<div class="mcp-actions"><svg viewBox="0 0 24 24"><rect x="9" y="9" width="11" height="11" rx="2" /><path d="M5 15H4V4h11v1" /></svg><svg viewBox="0 0 24 24"><path d="M4 9v6h3l5 4V5L7 9zM16 9a4 4 0 0 1 0 6M19 6a8 8 0 0 1 0 12" /></svg><svg viewBox="0 0 24 24"><path d="M12 15V4M8 8l4-4 4 4M5 13v7h14v-7" /></svg><svg viewBox="0 0 24 24">${repeat}</svg><svg viewBox="0 0 24 24"><path d="M6 12h.01M12 12h.01M18 12h.01" /></svg></div>`;

const escape = (text: string) => text.replace(/&/g, '&amp;').replace(/</g, '&lt;');

export const STATIC_SCENE = 0;

export const statusText = (scene: Scene) => `Worked for ${scene.seconds}s${scene.expandable ? ' ›' : ''}`;

export const userHTML = (scene: Scene) => escape(scene.prompt);

export const replyHTML = (scene: Scene) => `<p>${escape(scene.reply)}</p>${actions}`;

export const queueHTML = (rows: Row[]) => rows.map((row) => `
  <li class="${row.status === 'done' ? 'is-done' : ''} ${row.fresh ? 'mcp-enter' : ''}" ${row.hit ? 'data-hit' : ''}>
    <span class="status ${row.status}">${glyphs[row.status]}</span>
    <span class="ref">${row.ref}</span>
    <span class="title">${escape(row.title)}</span>
    <span class="priority ${row.high ? 'high' : ''}">${row.high ? '●' : '─'}</span>
    <span class="due">${row.due ?? ''}</span>
  </li>`).join('');

export const openCount = (rows: Row[]) => `${rows.filter((row) => row.status !== 'done').length} open`;
