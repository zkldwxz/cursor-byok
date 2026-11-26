import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import { brandOf, LOGOS } from '../../../server/src/remote/client/brands.js';
import { leadOf } from '../../../server/src/remote/client/chat.js';
import { Computers, ticketOf } from '../../../server/src/remote/client/computers.js';
import { ICONS } from '../../../server/src/remote/client/dom.js';
import {
  answersOf,
  buildThreads,
  busyCount,
  changesOf,
  withOpen,
  chatState,
  dateBucket,
  delivered,
  deviceName,
  foldSteps,
  formatAge,
  groupRows,
  groupThreads,
  groupOpen,
  hostsOf,
  lineOf,
  placeName,
  projectsOf,
  resolveTheme,
  scoped,
  shownCount,
  shownRows,
  spliceRows,
  stepKind,
  toggleOption,
  waitingCount,
  workspaceOf,
} from '../../../server/src/remote/client/model.js';

const row = (index, over = {}) => ({ key: `k${index}`, index, kind: 'assistant', blocks: [], ...over });
const keys = (rows) => rows.map((item) => item.key);

test('a change to the conversation replaces its rows from there on and keeps the ones before', () => {
  const rows = [row(0), row(1), row(2)];
  assert.deepEqual(keys(spliceRows(rows, { from: 2, rows: [row(2, { text: 'new' }), row(3)] })), ['k0', 'k1', 'k2', 'k3']);
  assert.deepEqual(keys(spliceRows(rows, { from: 1, rows: [] })), ['k0']);
  assert.deepEqual(keys(spliceRows(rows, { from: 0, rows: [row(9)] })), ['k9']);
});

test('consecutive steps form one group and prose stands alone', () => {
  const tool = (index, over) => row(index, { kind: 'tool', tool: { title: 'x' }, ...over });
  const groups = groupRows([row(0, { kind: 'human' }), tool(1), tool(2, { kind: 'thinking' }), row(3), tool(4), row(5, { kind: 'other', todos: [] })]);
  assert.deepEqual(groups.map((group) => [group.kind, keys(group.rows)]), [
    ['human', ['k0']],
    ['steps', ['k1', 'k2']],
    ['assistant', ['k3']],
    ['steps', ['k4']],
    ['todos', ['k5']],
  ]);
});

test('an open chat waits for as long as something is asked, whatever the list last heard', () => {
  const chat = { asked: [], running: true };
  assert.equal(chatState(chat), 'running');
  assert.equal(chatState({ ...chat, running: false }), 'idle');
  assert.equal(chatState({ ...chat, asked: [{ id: 'a', kind: 'command' }] }), 'waiting');
});

const human = (index) => row(index, { kind: 'human', text: 'hi' });
const idle = { asked: [], running: false };
const busy = { asked: [], running: true };

test('a message shows at once, with Cursor thinking under it, before anything answers', () => {
  const shown = shownRows([human(0), row(1)], idle, { text: 'next', after: 1 });
  assert.deepEqual(shown.map((item) => [item.kind, item.text ?? null, item.status ?? null]), [
    ['human', 'hi', null],
    ['assistant', null, null],
    ['human', 'next', 'sending'],
    ['thinking', null, 'loading'],
  ]);
});

test('the message shown early gives way to the one Cursor reports', () => {
  const sending = { text: 'hi', after: 1 };
  assert.equal(delivered([human(0), row(1)], sending), false);
  assert.equal(delivered([human(0), row(1), human(2)], sending), true);
});

test('a running turn never looks stuck: it thinks until something is being written', () => {
  const kinds = (rows, chat, quiet) => shownRows(rows, chat, null, quiet).slice(rows.length).map((item) => item.kind);
  assert.deepEqual(kinds([human(0)], busy), ['thinking']);
  assert.deepEqual(kinds([human(0), row(1, { kind: 'thinking', status: 'loading' })], busy), []);
  assert.deepEqual(kinds([human(0), row(1, { kind: 'tool', tool: { kind: 'run' } })], busy), ['thinking']);
  assert.deepEqual(kinds([human(0), row(1)], busy), []);
  assert.deepEqual(kinds([human(0), row(1)], busy, true), ['thinking']);
  assert.deepEqual(kinds([human(0), row(1)], idle, true), []);
  assert.deepEqual(kinds([human(0)], { ...busy, asked: [{ id: 'a' }] }), []);
});

test('questions can be submitted only once each has an option or words of its own', () => {
  const questions = [{ id: 'a' }, { id: 'b' }];
  assert.equal(answersOf(questions, {}), null);
  assert.equal(answersOf(questions, { a: { options: ['1'], text: '' }, b: { options: [], text: '  ' } }), null);
  assert.deepEqual(answersOf(questions, { a: { options: ['1'], text: '' }, b: { options: [], text: ' other ' } }), [
    { question: 'a', options: ['1'], text: '' },
    { question: 'b', options: [], text: 'other' },
  ]);
});

test('a single choice replaces the one before it and a multiple choice adds to it', () => {
  assert.deepEqual(toggleOption(['1'], '2', false), ['2']);
  assert.deepEqual(toggleOption(['1'], '2', true), ['1', '2']);
  assert.deepEqual(toggleOption(['1', '2'], '1', true), ['2']);
});

const step = (index, tool, over) => row(index, { kind: 'tool', status: 'completed', tool, ...over });

test('finished steps in a row fold into one line that counts them', () => {
  const folded = foldSteps([
    step(1, { kind: 'read' }),
    step(2, { kind: 'search' }),
    step(3, { kind: 'run' }),
    step(4, { kind: 'run' }),
    row(5, { kind: 'thinking', seconds: 65 }),
    step(6, { kind: 'tool' }),
  ]);
  assert.equal(folded.length, 1);
  assert.deepEqual(folded[0].tally, { read: 1, searched: 1, ran: 2, used: 1, seconds: 65 });
  assert.equal(folded[0].rows.length, 6);
});

test('what a person may look for is never folded away', () => {
  const folded = foldSteps([
    step(1, { kind: 'run' }),
    step(2, { kind: 'run' }, { status: 'error' }),
    step(3, { kind: 'run' }, { status: 'loading' }),
    step(4, { kind: 'edit' }),
    step(5, { kind: 'run' }),
  ]);
  assert.ok(folded.every((item) => !item.tally));
  assert.deepEqual(folded.flatMap((item) => keys(item.rows)), ['k1', 'k2', 'k3', 'k4', 'k5']);
});

test('a step is what the server says it is, and thinking is thinking', () => {
  assert.equal(stepKind(step(1, { kind: 'edit' })), 'edit');
  assert.equal(stepKind(row(2, { kind: 'thinking' })), 'thinking');
  assert.equal(stepKind(row(3, { kind: 'tool' })), 'tool');
});

test('a phone is named by its own system, even where its browser also mentions another', () => {
  assert.equal(deviceName('Mozilla/5.0 (Linux; U; Android 16; zh-cn; RMX5010) AppleWebKit/537.36'), 'Android');
  assert.equal(deviceName('Mozilla/5.0 (iPhone; CPU iPhone OS 19_0 like Mac OS X)'), 'iPhone');
  assert.equal(deviceName('Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)'), 'Mac');
  assert.equal(deviceName('Mozilla/5.0 (X11; Linux x86_64)'), 'Linux');
  assert.equal(deviceName('something else'), 'Browser');
});

test('theme preference resolves against OS preference', () => {
  assert.equal(resolveTheme('auto', true), 'light');
  assert.equal(resolveTheme('auto', false), 'dark');
  assert.equal(resolveTheme('light', false), 'light');
  assert.equal(resolveTheme('dark', true), 'dark');
});

test('dateBucket and formatAge classify calendar boundaries and relative durations', () => {
  const now = new Date(2026, 10, 24, 14, 0, 0).getTime(); // 14:00 local time
  assert.equal(dateBucket(30, now), 'today');
  assert.equal(dateBucket(13 * 60, now), 'today'); // 01:00 today
  assert.equal(dateBucket(16 * 60, now), 'yesterday'); // 22:00 yesterday
  assert.equal(dateBucket(40 * 60, now), 'week'); // 2 days ago
  assert.equal(dateBucket(9 * 24 * 60, now), 'earlier'); // 9 days ago

  assert.equal(formatAge(0, true), '刚刚');
  assert.equal(formatAge(0, false), 'now');
  assert.equal(formatAge(14, true), '14分钟');
  assert.equal(formatAge(14, false), '14m');
  assert.equal(formatAge(11 * 60, true), '11小时');
  assert.equal(formatAge(11 * 60, false), '11h');
  assert.equal(formatAge(3 * 24 * 60, true), '3天');
  assert.equal(formatAge(3 * 24 * 60, false), '3d');
  assert.equal(formatAge(14 * 24 * 60, true), '2周');
  assert.equal(formatAge(14 * 24 * 60, false), '2w');
});

const now = new Date(2026, 10, 24, 14, 0, 0).getTime();
const minutes = (count) => now - count * 60_000;
const workspaces = [
  {
    id: '/p/shop',
    title: 'shop',
    chats: [
      { id: 'a', label: 'fix cart', state: 'idle', updatedAt: minutes(1) },
      { id: 'b', label: 'old idea', state: 'idle', updatedAt: minutes(2 * 24 * 60) },
    ],
  },
  { id: '/p/blog', title: 'blog', chats: [{ id: 'c', label: 'draft post', state: 'waiting', updatedAt: minutes(30) }] },
  { id: '/p/empty', title: 'empty', chats: [] },
];
const names = (groups) => groups.map((group) => [group.title ?? group.bucket ?? group.id, group.items.map((item) => item.label)]);

test('conversations waiting anywhere are counted, and each knows its project', () => {
  assert.equal(waitingCount(workspaces), 1);
  assert.equal(workspaceOf(workspaces, 'c').title, 'blog');
  assert.equal(workspaceOf(workspaces, 'gone'), null);
});

test('the open conversation shows the state it reports itself', () => {
  const threads = buildThreads(workspaces, { current: 'a', open: { asked: [], running: true }, now });
  assert.deepEqual(threads.map((item) => [item.id, item.state, item.current, item.ageMinutes]), [
    ['a', 'running', true, 1],
    ['b', 'idle', false, 2 * 24 * 60],
    ['c', 'waiting', false, 30],
  ]);
});

test('conversations group by project, date or status, with pinned ones first and only there', () => {
  const threads = buildThreads(workspaces, { pinned: new Set(['a']), now });
  assert.deepEqual(names(groupThreads(workspaces, threads, 'project', now)), [
    ['pinned', ['fix cart']],
    ['shop', ['old idea']],
    ['blog', ['draft post']],
    ['empty', []],
  ]);
  assert.deepEqual(names(groupThreads(workspaces, threads, 'date', now)), [
    ['pinned', ['fix cart']],
    ['today', ['draft post']],
    ['week', ['old idea']],
  ]);
  assert.deepEqual(names(groupThreads(workspaces, threads, 'status', now)), [
    ['pinned', ['fix cart']],
    ['waiting', ['draft post']],
    ['idle', ['old idea']],
  ]);
});

test('every icon referenced in remote client modules is defined in dom.js', () => {
  const clientDir = path.resolve(import.meta.dirname, '../../../server/src/remote/client');
  const referenced = new Set();
  for (const file of ['app.js', 'chat.js', 'dock.js', 'conversations.js', 'waiting.js', 'tune.js', 'settings.js', 'sidebar.js']) {
    const source = fs.readFileSync(path.join(clientDir, file), 'utf8');
    for (const match of source.matchAll(/icon\(\s*["']([a-zA-Z0-9_-]+)["']\s*\)/g)) {
      referenced.add(match[1]);
    }
  }
  for (const name of ['sun', 'moon', 'monitor', 'compose', 'sidebar', 'infinity', 'branch', 'plus', 'pin', 'remote', 'status', 'sortGroup', 'file', 'edit', 'terminal', 'search', 'wrench', 'layers', 'alert', 'clock', 'question', 'diff', 'cpu', 'folder', 'chat', 'link', 'wifiOff', 'copy', 'home', 'gear', 'context']) {
    assert.ok(name in ICONS, `missing expected SVG icon '${name}' in dom.js`);
  }
  for (const name of referenced) {
    assert.ok(name in ICONS, `referenced icon '${name}' is missing in dom.js`);
  }
});

test('a group shows three conversations, then two more, then all of them', () => {
  assert.deepEqual([0, 1, 2].map((step) => shownCount(19, step)), [3, 5, 19]);
  assert.deepEqual([0, 1, 2].map((step) => shownCount(4, step)), [3, 4, 4]);
  assert.equal(shownCount(2, 0), 2);
  assert.equal(shownCount(0, 0), 0);
});

const home = [
  { id: 'a', title: 'A', chats: [{ id: 'a1', label: 'old', state: 'idle', updatedAt: 1 }] },
  { id: 'b', title: 'B', chats: [{ id: 'b1', label: 'new', state: 'idle', updatedAt: 9 }, { id: 'b2', label: 'asks', state: 'waiting', updatedAt: 2 }] },
  { id: 'c', title: 'C', chats: [] },
];

const far = { id: 'ssh://box/srv/a', title: 'a', host: 'box', chats: [{ id: 'f1', label: 'deploy', state: 'idle', updatedAt: 5, readOnly: true }] };

test('a conversation can begin in any project of the computer, and on no SSH host', () => {
  assert.deepEqual(projectsOf([...home, far]).map(({ id }) => id), ['b', 'a', 'c']);
  assert.deepEqual(projectsOf(null), []);
});

test('an SSH conversation is listed with its host, apart from a project of the same name', () => {
  const all = [...home, far];
  assert.equal(placeName(far), 'a (box)');
  const threads = buildThreads(all, { now: 600_000 });
  assert.equal(threads.find(({ id }) => id === 'f1').project, 'a (box)');
  const groups = groupThreads(all, threads, 'project');
  assert.deepEqual(groups.map(({ title, host }) => [title, host]), [['B', null], ['a (box)', 'box'], ['A', null], ['C', null]]);
});

const well = { online: true, known: true, slow: false, reachable: true, chat: { ...busy, retry: null } };

test('the light is green only while every stretch of the way holds', () => {
  assert.equal(lineOf(well).now, 'ok');
  assert.equal(lineOf({ ...well, chat: null }).now, 'ok');
  assert.equal(lineOf({ ...well, online: false }).now, 'lost');
  assert.equal(lineOf({ ...well, online: false, known: false, reachable: undefined }).now, 'connecting');
  assert.equal(lineOf({ ...well, known: false, reachable: undefined }).now, 'connecting');
  assert.equal(lineOf({ ...well, slow: true }).now, 'slow');
  assert.equal(lineOf({ ...well, reachable: false }).now, 'unreachable');
});

test('a model that is tried again is told apart from a phone that reconnects', () => {
  const retrying = { ...well, chat: { ...busy, retry: 3 } };
  const line = lineOf(retrying);
  assert.deepEqual([line.phone, line.cursor, line.model, line.now, line.attempt], ['ok', 'ok', 'retrying', 'retrying', 3]);
  const lost = lineOf({ ...retrying, online: false });
  assert.deepEqual([lost.phone, lost.cursor, lost.model, lost.now, lost.attempt], ['lost', 'unknown', 'unknown', 'lost', null]);
  // A computer of an earlier version does not say.
  assert.equal(lineOf({ ...well, chat: busy }).model, 'unknown');
});

test('work that stands still says why, and only while there is work', () => {
  const waits = (chat, over) => shownRows([row(0)], chat, null, false, lineOf({ ...well, chat, ...over })).slice(1).map((item) => item.wait?.now ?? null);
  assert.deepEqual(waits({ ...busy, retry: 2 }), ['retrying']);
  assert.deepEqual(waits({ ...busy, retry: null }, { online: false }), ['lost']);
  assert.deepEqual(waits({ ...busy, retry: null }, { reachable: false }), ['unreachable']);
  assert.deepEqual(waits({ ...idle, retry: null }, { online: false }), []);
  assert.deepEqual(waits({ ...busy, retry: null }), []);
});

test('thinking has no icon, and every other step keeps its own', () => {
  assert.equal(leadOf(row(1, { kind: 'thinking', status: 'loading' })), null);
  assert.equal(leadOf(step(2, { kind: 'run' })), 'terminal');
  assert.equal(leadOf(step(3, { kind: 'run' }, { status: 'error' })), 'alert');
});

test('a model shows the logo of its maker, by its own name before its label', () => {
  assert.equal(brandOf('claude-opus-5-5', 'My model'), 'claude');
  assert.equal(brandOf('mynewapi/deepseek-flash'), 'deepseek');
  assert.equal(brandOf(undefined, 'GLM 5.3 Flash'), 'zhipu');
  assert.equal(brandOf('claude-opus', 'gpt look-alike'), 'claude');
  assert.equal(brandOf('gpt-4o3'), 'openai');
  assert.equal(brandOf('house blend', null), null);
  for (const [brand, shapes] of Object.entries(LOGOS)) assert.ok(shapes.length, brand);
});

test('the phone and the desktop know the same makers', () => {
  const desktop = fs.readFileSync(path.resolve(import.meta.dirname, '../src/shared/utils/modelBrand.ts'), 'utf8');
  const phone = fs.readFileSync(path.resolve(import.meta.dirname, '../../../server/src/remote/client/brands.js'), 'utf8');
  const makers = (source) => [...source.matchAll(/\[word\("(.+)"\), "(\w+)"\]/g)].map((match) => match.slice(1));
  assert.ok(makers(desktop).length > 20);
  assert.deepEqual(makers(phone), makers(desktop));
  for (const [, brand] of makers(phone)) assert.ok(brand in LOGOS, brand);
});

test('a touch is felt unless that was turned off, and sounds only when that was turned on', async () => {
  const felt = [];
  globalThis.Feel = { play: (kind) => felt.push(kind) };
  const { feel, feels, setFeel } = await import('../../../server/src/remote/client/feel.js');
  assert.deepEqual([feels('haptics'), feels('sound')], [true, false]);
  feel('send');
  setFeel('haptics', false);
  feel('send');
  assert.deepEqual(felt, ['send']);
  delete globalThis.Feel;
});

test('projects come in the order they were last worked in, and empty ones last', () => {
  const groups = groupThreads(home, buildThreads(home, { now: 600_000 }), 'project');
  assert.deepEqual(groups.map((group) => group.title), ['B', 'A', 'C']);
});

test('of the projects only the latest is open until one is opened or folded', () => {
  const projects = groupThreads(home, buildThreads(home, { now: 600_000 }), 'project');
  const open = (chosen) => projects.map((group) => groupOpen(projects, group, chosen));
  assert.deepEqual(open(new Map()), [true, false, false]);
  assert.deepEqual(open(new Map([['project:b', false], ['project:a', true]])), [false, true, false]);
  const pinned = groupThreads(home, buildThreads(home, { now: 600_000, pinned: new Set(['a1']) }), 'project');
  assert.deepEqual(pinned.map((group) => groupOpen(pinned, group, new Map())), [true, true, false, false]);
  const states = groupThreads(home, buildThreads(home, { now: 600_000 }), 'status');
  assert.ok(states.length > 1 && states.every((group) => groupOpen(states, group, new Map())));
});

test('every file of the page is one the server gives to browsers', () => {
  const clientDir = path.resolve(import.meta.dirname, '../../../server/src/remote/client');
  const served = fs.readFileSync(path.resolve(clientDir, '../web.rs'), 'utf8');
  for (const file of fs.readdirSync(clientDir)) assert.ok(served.includes(`include_str!("client/${file}")`), file);
});

const pairingLink = (ticket) => `cursor-byok://pair?d=${Buffer.from(JSON.stringify(ticket)).toString('base64url')}`;

test('only a pairing link says which computer, and holds its code', () => {
  assert.deepEqual(ticketOf(` ${pairingLink({ k: 'key', c: 'once?>~', a: [] })}\n`), { key: 'key', code: 'once?>~', relay: false });
  assert.deepEqual(ticketOf(pairingLink({ k: 'key', c: '', a: [], r: { u: 'https://relay' } })), { key: 'key', code: null, relay: true });
  for (const other of [null, '', 'https://example.com', pairingLink({ c: 'code' }), 'cursor-byok://pair?d=%%']) {
    assert.equal(ticketOf(other), null);
  }
});

const memory = () => {
  const stored = new Map();
  return { stored, getItem: (key) => stored.get(key) ?? null, setItem: (key, value) => stored.set(key, value) };
};

test('computers stay known, each with its own token, and one of them is on screen', () => {
  const storage = memory();
  const computers = new Computers(storage);
  assert.equal(computers.current, null);
  computers.add({ id: 'home', link: 'l1', token: 't1' });
  computers.told({ name: 'Home Mac', link: 'l2' });
  computers.add({ id: 'work', link: 'l3', token: 't3' });
  assert.equal(computers.current.id, 'work');
  computers.choose('home');
  const later = new Computers(storage);
  assert.deepEqual(later.current, { id: 'home', name: 'Home Mac', link: 'l2', token: 't1' });
  // A browser's computer tells no link.
  later.told({ name: 'Mac', link: null });
  assert.equal(later.current.link, 'l2');
  later.add({ id: 'home', link: 'l4', token: 't4' });
  assert.deepEqual(later.list.map(({ id, name, token }) => [id, name, token]), [['work', null, 't3'], ['home', 'Mac', 't4']]);
  later.forget('home');
  assert.equal(later.current.id, 'work');
  later.forget('work');
  assert.equal(new Computers(storage).current, null);
});

test('only the conversations of this computer, or of one SSH host, are shown when asked for', () => {
  const other = { id: 'ssh://linux/srv', title: 'srv', host: 'linux', chats: [] };
  const all = [...home, far, other];
  assert.deepEqual(hostsOf(all), ['box', 'linux']);
  const ids = (scope) => scoped(all, scope).map(({ id }) => id);
  assert.deepEqual(ids('local'), ['a', 'b', 'c']);
  assert.deepEqual(ids('linux'), ['ssh://linux/srv']);
  assert.equal(ids('all').length, 5);
  assert.equal(ids('gone').length, 5);
  assert.deepEqual(scoped(null, 'local'), []);
});

test('the app pairs with the link it was given, then comes back with its token', async (t) => {
  const calls = [];
  const channels = [];
  let dialled = async () => calls.length;
  const stored = new Map();
  Object.assign(globalThis, {
    WebSocket: { CONNECTING: 0, OPEN: 1, CLOSED: 3 },
    localStorage: { getItem: (key) => stored.get(key) ?? null, setItem: (key, value) => stored.set(key, value), removeItem: (key) => stored.delete(key) },
    document: { addEventListener() {}, hidden: false },
    addEventListener() {},
    __TAURI__: {
      core: {
        Channel: class { constructor() { channels.push(this); } },
        invoke: async (name, args) => (calls.push([name, args]), name === 'dial' ? dialled() : null),
      },
    },
  });
  const { Link } = await import('../../../server/src/remote/client/link.js');
  const heard = [];
  const on = Object.fromEntries(['open', 'closed', 'away', 'unpaired', 'expired', 'slow', 'message'].map((name) => [name, (value) => heard.push([name, value])]));
  const computers = new Computers(globalThis.localStorage);
  const link = new Link(computers, on, { awayMs: 60 });
  // Otherwise a failure leaves it trying for ever.
  t.after(() => link.leave());
  const settle = () => new Promise((resolve) => setTimeout(resolve, 5));
  const said = () => JSON.parse(calls.findLast(([name]) => name === 'say')[1].text);
  const says = (message) => channels.at(-1).onmessage({ message: JSON.stringify(message) });

  assert.equal(link.pair('https://example.com'), false);
  assert.equal(calls.length, 0);

  const ticket = pairingLink({ k: 'key', c: 'once', a: [] });
  const moved = pairingLink({ k: 'key', c: '', a: ['10.0.0.9:1'] });
  assert.equal(link.pair(ticket), true);
  await settle();
  assert.equal(calls[0][1].ticket, ticket);
  assert.equal(said().code, 'once');
  says({ type: 'paired', token: 'token' });

  link.connect();
  await settle();
  says({ type: 'computer', name: 'Mac', link: moved });
  link.connect();
  await settle();
  // It is dialled where it said it is.
  assert.equal(calls.filter(([name]) => name === 'dial').at(-1)[1].ticket, moved);
  assert.deepEqual(said(), { token: 'token' });

  // A spent link leaves the pairing there was.
  link.pair(pairingLink({ k: 'other', c: 'spent', a: [] }));
  await settle();
  says({ type: 'unpaired' });
  assert.equal(heard.at(-1)[0], 'expired');
  link.connect();
  await settle();
  assert.deepEqual(said(), { token: 'token' });
  says({ type: 'unpaired' });
  assert.equal(heard.at(-1)[0], 'unpaired');
  assert.equal(computers.current, null);
  computers.add({ id: 'key', link: ticket, token: 'token' });

  dialled = async () => { throw new Error('no answer'); };
  link.connect();
  await settle();
  assert.equal(heard.at(-1)[0], 'closed');
  // A network that changes loses a few dials at once: that is not being away.
  dialled = async () => 98;
  link.connect();
  await settle();
  says({ type: 'overview' });
  await new Promise((resolve) => setTimeout(resolve, 90));
  assert.ok(!heard.some(([name]) => name === 'away'));
  dialled = async () => { throw new Error('no answer'); };
  link.connect();
  await new Promise((resolve) => setTimeout(resolve, 90));
  assert.equal(heard.filter(([name]) => name === 'away').length, 1);
  dialled = async () => 99;
  link.connect();
  await settle();
  link.leave();
  assert.deepEqual(computers.list, []);
  assert.equal(calls.at(-1)[0], 'hang_up');
});

test('a conversation that stops or starts to wait is told once, and nothing is told of a first look', () => {
  const at = (...chats) => [{ id: 'w', title: 'W', chats: chats.map(([id, state]) => ({ id, label: id.toUpperCase(), state, updatedAt: 1 })) }];
  const before = at(['a', 'running'], ['b', 'running'], ['c', 'idle'], ['d', 'waiting']);
  assert.deepEqual(changesOf(null, before), []);
  const after = at(['a', 'idle'], ['b', 'waiting'], ['c', 'idle'], ['d', 'waiting'], ['e', 'waiting']);
  assert.deepEqual(changesOf(before, after), [
    { id: 'a', label: 'A', kind: 'done' },
    { id: 'b', label: 'B', kind: 'waiting' },
    { id: 'e', label: 'E', kind: 'waiting' },
  ]);
  assert.deepEqual(changesOf(after, after), []);
  assert.equal(busyCount(before), 3);
  assert.equal(busyCount(after), 3);
  assert.equal(busyCount(null), 0);
});

test('the open conversation is told by its own stream, which hears a short turn the overview never shows running', () => {
  const at = (state) => [{ id: 'w', title: 'W', chats: [{ id: 'a', label: 'A', state, updatedAt: 1 }, { id: 'b', label: 'B', state: 'running', updatedAt: 1 }] }];
  const sent = withOpen(at('idle'), { id: 'a', state: 'running' });
  assert.equal(busyCount(sent), 2);
  const answered = withOpen(at('idle'), { id: 'a', state: 'idle' });
  assert.deepEqual(changesOf(sent, answered), [{ id: 'a', label: 'A', kind: 'done' }]);
  assert.deepEqual(withOpen(at('idle'), null), at('idle'));
});
