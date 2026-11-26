//! Holds what is known about the computer and shows it: the conversations, one of them, or why none.
import { logo } from "./brands.js";
import { Transcript } from "./chat.js";
import { Computers, ticketOf } from "./computers.js";
import { Conversations, notice } from "./conversations.js";
import { chinese, copy } from "./copy.js";
import { Composer, drawAsked, drawChips, drawPanel, drawReadOnly } from "./dock.js";
import { h, icon, redraw } from "./dom.js";
import { feel, feelTouches, setFeel } from "./feel.js";
import { drawLine, drawStretches } from "./line.js";
import { Link, pair } from "./link.js";
import { buildThreads, chatState, delivered, hostsOf, lineOf, placeName, projectsOf, resolveTheme, scoped, sendingAfter, shownRows, spliceRows, waitingCount, workspaceOf } from "./model.js";
import { alert, alertOpened, native, paintBars, scan } from "./native.js";
import { Settings } from "./settings.js";
import { Sidebar } from "./sidebar.js";
import { drawTune } from "./tune.js";
import { Views } from "./views.js";
import { drawWaiting } from "./waiting.js";

const $ = (id) => document.getElementById(id);
const NEAR_END = 80;
/** Text that stops for this long is no longer being written. */
const QUIET_MS = 1_000;
const THEME_KEY = "cursor-byok-remote-theme";
const GROUP_KEY = "cursor-byok-remote-group";
const PINNED_KEY = "cursor-byok-remote-pinned";
/** Whose conversations are shown, by computer. */
const SCOPES_KEY = "cursor-byok-remote-scopes";
const FOLDERS_KEY = "cursor-byok-remote-folders";
const wide = matchMedia("(min-width: 900px)");
const lightMedia = matchMedia("(prefers-color-scheme: light)");
const themeColor = $("theme-color");

function readStorage(key, fallback) {
  try {
    const raw = localStorage.getItem(key);
    return raw ?? fallback;
  } catch {
    return fallback;
  }
}

function writeStorage(key, value) {
  try {
    if (value === null || value === undefined) localStorage.removeItem(key);
    else localStorage.setItem(key, value);
  } catch {
    // Ignore storage failures in private browsing.
  }
}

function readJson(key, fallback) {
  try {
    const raw = localStorage.getItem(key);
    return raw ? JSON.parse(raw) : fallback;
  } catch {
    return fallback;
  }
}

function savedTheme() {
  const value = readStorage(THEME_KEY, "auto");
  return value === "dark" || value === "light" ? value : "auto";
}

function savedGroupBy() {
  const value = readStorage(GROUP_KEY, "project");
  return value === "date" || value === "status" ? value : "project";
}

const state = {
  overview: null,
  online: false,
  /** The computer was asked and has not answered yet. */
  slow: false,
  /** The conversation on screen: `{ session }`, or `{ draft }` for one not begun in that workspace. */
  place: null,
  chat: null,
  rows: [],
  /** The message on its way to Cursor. */
  sending: null,
  /** When the rows last changed. */
  changed: 0,
  follow: true,
  sidebarCollapsed: false,
  theme: savedTheme(),
  groupBy: savedGroupBy(),
  pinned: new Set(readJson(PINNED_KEY, [])),
  /** Folders opened or folded by hand. */
  scopes: new Map(readJson(SCOPES_KEY, [])),
  folders: new Map(readJson(FOLDERS_KEY, [])),
};

document.documentElement.lang = chinese ? "zh-CN" : "en";

/** Modes and models chosen here; the server forgets them with the connection. */
const picks = new WeakMap();
/** In the order they take: a model brings what can be set of it. */
const PICKS = ["mode", "model", "context", "effort"];
/** Conversations seen before show at once, until Cursor has sent them again. */
const seen = new Map();
/** Menus seen before open at once, and are read again for the next time. */
const menus = {};
let quietTimer;

function applyTheme() {
  if (state.theme === "auto") delete document.documentElement.dataset.theme;
  else document.documentElement.dataset.theme = state.theme;
  const resolved = resolveTheme(state.theme, lightMedia.matches);
  const ground = getComputedStyle(document.documentElement).getPropertyValue("--ground").trim();
  themeColor?.setAttribute("content", ground);
  paintBars(ground, resolved === "light");
}

function setTheme(theme) {
  state.theme = theme;
  writeStorage(THEME_KEY, theme === "auto" ? null : theme);
  applyTheme();
  draw();
}

const computers = new Computers(localStorage);
const link = new Link(computers, {
  open() {
    state.online = true;
    if (state.place) enter();
    draw();
  },
  closed() {
    Object.assign(state, { online: false, slow: false });
    draw();
  },
  slow(slow) {
    state.slow = slow;
    draw();
  },
  // What was read stays in sight; its light says that the computer is away. A browser's page came from the computer, so only its address could be wrong.
  away: () => native && !state.overview && gate(copy.away, "wifiOff"),
  unpaired: () => gate(native ? copy.scan : copy.gate, "link"),
  expired: () => gate(copy.expired, "link"),
  message(message) {
    if (views.now === "gate") views.reset("home");
    if (message.type === "overview") {
      state.overview = message;
      alert(message.workspaces);
      if (!document.hidden) showOpened();
      const newest = message.workspaces.flatMap((workspace) => workspace.chats)[0];
      if (wide.matches && !state.place && newest) open({ session: newest.id });
    } else if (message.type === "chat" && shown(message)) {
      hear(message);
    }
    draw();
  },
});

/** A message about the conversation left a moment ago is not about this one. */
const shown = ({ session, draft }) => (state.place?.session ? session === state.place.session : session === null && draft !== null && draft === state.place?.draft);

function hear(message) {
  const ran = state.chat?.running;
  // While Cursor is still reading the conversation, what is on screen stays.
  // Only what happens in sight is felt, not what an opened conversation brings along.
  if (message.open && state.chat?.open) {
    if (ran && !message.running) feel(message.error === null ? "done" : "warn");
    else if (message.asked.length > state.chat.asked.length) feel("warn");
  }
  if (message.open) state.rows = spliceRows(state.rows, message);
  state.chat = { ...message, open: message.open || Boolean(state.chat?.open) };
  if (state.sending && (delivered(state.rows, state.sending) || (ran && !message.running))) state.sending = null;
  if (message.session && message.open) seen.set(message.session, { chat: state.chat, rows: state.rows });
  state.changed = Date.now();
  clearTimeout(quietTimer);
  if (message.running) quietTimer = setTimeout(draw, QUIET_MS);
}

/** Shows the message before Cursor has it; gives it back if Cursor does not take it. */
async function send(text) {
  const sending = sendingAfter(state.rows, text);
  Object.assign(state, { sending, follow: true });
  draw();
  const reply = await run({ type: "send", text });
  if (reply.type !== "refused") return { taken: true };
  // Elsewhere by now: its text waits in the conversation it was for.
  const here = state.sending === sending;
  if (here) state.sending = null;
  draw();
  return { taken: false, here };
}

function enter() {
  const { place } = state;
  link.tell(place.session ? { attach: place.session } : { draft: place.draft });
  const picked = picks.get(place) ?? {};
  for (const kind of PICKS) if (picked[kind]) link.command({ type: "pick", kind, id: picked[kind] });
}

const run = async (command) => {
  const { place } = state;
  const reply = await link.command(command);
  if (reply.type === "refused") {
    feel("warn");
    toast(copy.refused[reply.code] ?? copy.refused.failed);
  }
  // The first message made a conversation of the draft.
  if (place?.draft && reply.session && state.place === place) {
    state.place = { session: reply.session };
    picks.set(state.place, picks.get(place) ?? {});
    draw();
  }
  return reply;
};

/** Kept up to date while its sheet is open. */
const stretches = h("div", { class: "stretches" });
const tuning = h("div", { class: "tune" });
const scroller = $("scroll");
const transcript = new Transcript($("transcript"));
const composer = new Composer($("composer"), send, () => run({ type: "stop" }));

function open(place) {
  const known = seen.get(place.session);
  Object.assign(state, { place, chat: known?.chat ?? null, rows: known?.rows ?? [], sending: null, follow: true });
  transcript.forget();
  enter();
}

function select({ id }) {
  if (id !== state.place?.session) open({ session: id });
  views.go("chat");
}

const here = () => state.place?.draft ?? workspaceOf(state.overview?.workspaces, state.place?.session)?.id;

/** Where it begins is asked for only when that is not plain. */
function begin(workspace) {
  const projects = projectsOf(state.overview?.workspaces);
  const project = projects.find(({ id }) => id === workspace) ?? (projects.length === 1 ? projects[0] : null);
  if (project) {
    if (state.place?.draft !== project.id) open({ draft: project.id });
    views.go("chat");
    draw();
  } else if (projects.length) sheet(copy.newChatIn, projects.map(({ id, title }) => ({ label: title, pick: () => begin(id) })));
}

const views = new Views((view) => {
  if (view === "settings") settings.opened();
  draw();
});

function chatNotice(listed) {
  const { overview, place, chat, rows, sending } = state;
  if (!place) return [copy.choose, "folder"];
  if (rows.length || sending) return null;
  if (overview && !overview.reachable) return [copy.unreachable, "wifiOff"];
  if (!chat?.open) return place.session && overview && !listed ? [copy.lost, "wifiOff"] : [[copy.opening], null];
  return [copy.emptyChat, "chat"];
}

function drawStatusBar(workspace, told) {
  const bar = $("statusbar");
  bar.hidden = !workspace;
  if (!workspace) return;
  redraw($("status-left"), placeName(workspace), () => [h("span", { class: "status-project" }, icon("folder"), h("span", {}, placeName(workspace)))]);
  redraw($("status-right"), told, () => [
    h("span", { class: "status-pill", "data-state": told }, h("span", { class: "state", "data-state": told }), h("span", {}, copy.state[told] ?? copy.offline)),
  ]);
}

const actions = {
  onSelectThread: select,
  onNewChat: (workspace = here()) => begin(workspace),
  onSettings: () => views.go("settings"),
  onGrouping: () =>
    sheet(copy.grouping, Object.entries(copy.groupOption).map(([mode, label]) => ({
      label,
      current: mode === state.groupBy,
      pick: () => {
        state.groupBy = mode;
        writeStorage(GROUP_KEY, mode);
        draw();
      },
    }))),
  onFold: (folder, open) => {
    state.folders.set(folder, open);
    writeStorage(FOLDERS_KEY, JSON.stringify([...state.folders]));
    draw();
  },
  onTogglePin: (session) => {
    if (!state.pinned.delete(session)) state.pinned.add(session);
    writeStorage(PINNED_KEY, JSON.stringify([...state.pinned]));
    draw();
  },
  onThreadMenu: ({ id, label, pinned }) => {
    feel("hold");
    sheet(label || copy.untitled, [{ label: pinned ? copy.unpin : copy.pin, pick: () => actions.onTogglePin(id) }]);
  },
};
const sidebar = new Sidebar($("sidebar"), actions);
const conversations = new Conversations(actions);
const host = () => (wide.matches ? sidebar.body : $("home-lists")).append(conversations.root);
wide.addEventListener("change", host);
host();
const settings = new Settings($("settings-body"), {
  onTheme: setTheme,
  onFeel: (name, on) => {
    setFeel(name, on);
    draw();
  },
  onComputer: turn,
  onPair: look,
  onLeave: () => {
    link.leave();
    clear();
    if (computers.current) turn(computers.current.id, true);
    else gate(native ? copy.scan : copy.gate, "link");
  },
});

/** Nothing of one computer is shown as another's. */
function clear() {
  Object.assign(state, { overview: null, online: false, slow: false, place: null, chat: null, rows: [], sending: null });
  alert(null);
  seen.clear();
  for (const kind of Object.keys(menus)) delete menus[kind];
}

function turn(id, anew = false) {
  if (id === computers.current?.id && !anew) return;
  clear();
  views.reset("home");
  link.choose(id);
  draw();
}

function setScope(scope) {
  state.scopes.set(computers.current.id, scope);
  writeStorage(SCOPES_KEY, JSON.stringify([...state.scopes]));
  draw();
}

/** Whose conversations, and of which computer. */
function chooseScope() {
  const now = scopeNow();
  const scopes = ["all", "local", ...hostsOf(state.overview?.workspaces)];
  const others = native ? [{ title: copy.computer, items: [...computers.list.map(({ id, name }) => ({ label: name ?? copy.computer, mark: icon("monitor"), current: id === computers.current?.id, pick: () => turn(id) })), { label: copy.addComputer, mark: icon("plus"), pick: look }] }] : [];
  sheets(copy.scope, [{ items: scopes.map((scope) => ({ label: copy.scopes[scope] ?? scope, mark: icon({ all: "layers", local: "folder" }[scope] ?? "remote"), current: scope === now, pick: () => setScope(scope) })) }, ...others]);
}

/** A host that has no conversations any more is not waited for. */
function scopeNow() {
  const scope = state.scopes.get(computers.current?.id) ?? "all";
  return scope === "local" || hostsOf(state.overview?.workspaces).includes(scope) ? scope : "all";
}

function draw() {
  const { overview, online, place, chat, sidebarCollapsed, groupBy, pinned, folders } = state;
  document.body.dataset.sidebar = sidebarCollapsed ? "collapsed" : "open";
  const waiting = waitingCount(overview?.workspaces);
  document.title = waiting ? `(${waiting}) Cursor` : "Cursor";
  const connection = {
    told: online ? (waiting ? "waiting" : "online") : "offline",
    status: online ? (waiting ? copy.waitingTitle(waiting) : copy.nothingWaiting) : overview ? copy.offline : copy.connecting,
  };
  const scope = scopeNow();
  const within = overview && { ...overview, workspaces: scoped(overview.workspaces, scope) };
  const threads = buildThreads(overview?.workspaces, { current: place?.session, open: chat, pinned });
  sidebar.draw({ overview, online, ...connection });
  const places = new Set(within?.workspaces.map(({ id }) => id));
  conversations.draw({ overview: within, current: place?.session, groupBy, folders, threads: threads.filter(({ workspace }) => places.has(workspace)) });
  $("home-title").textContent = scope === "all" ? copy.homeTab : (copy.scopes[scope] ?? scope);
  $("home-computer").textContent = computers.current?.name ?? "";
  const line = lineOf({ online, known: Boolean(overview), slow: state.slow, reachable: overview?.reachable, chat });
  document.body.dataset.line = line.now;
  for (const id of ["line", "home-line"]) drawLine($(id), line);
  drawStretches(stretches, line, !native || Boolean(ticketOf(computers.current?.link)?.relay));
  drawTune(tuning, chat?.tuning, tune);
  drawWaiting($("home-waiting"), threads, actions);
  const projects = projectsOf(overview?.workspaces);
  $("home-new").hidden = !projects.length;
  $("home-group").hidden = !overview?.workspaces.length;
  $("tabs").style.setProperty("--at", document.body.dataset.tab === "home" ? 0 : 1);
  for (const tab of $("tabs").children) tab.setAttribute("aria-current", String(tab.dataset.tab === document.body.dataset.tab));
  if (views.now === "settings") settings.draw(state, computers);
  const workspace = overview?.workspaces.find((item) => item.id === here());
  const listed = workspace?.chats.find((item) => item.id === place?.session);
  const reachable = online && Boolean(overview?.reachable);
  const told = reachable ? (chat ? chatState(chat) : (listed?.state ?? "idle")) : "offline";
  // The conversation's own title leads; the project is where it is.
  $("name").textContent = place ? (listed?.label || (place.draft ? copy.newChat : copy.untitled)) : "";
  $("title").textContent = workspace ? placeName(workspace) : "";
  $("title").hidden = !workspace;
  // The light on the other side tells what is wrong with the connection.
  $("state").dataset.state = place && reachable ? told : "";
  $("state").textContent = copy.state[told] ?? "";
  $("chats").disabled = !workspace?.chats.length;
  $("new").hidden = !projects.length;

  const wording = chatNotice(listed);
  $("notice").hidden = !wording;
  if (wording) $("notice").replaceChildren(notice(...wording));
  const following = state.follow;
  transcript.draw(shownRows(state.rows, chat, state.sending, Date.now() - state.changed >= QUIET_MS, line));
  $("latest").hidden = !chat || following;

  $("dock").hidden = !chat?.open || !overview?.reachable;
  drawAsked($("asked"), chat, run);
  drawPanel($("panel"), chat);
  drawChips($("chips"), chat, { menu, tune: () => openSheet(chat.model, tuning) });
  drawReadOnly($("read-only"), chat);
  $("composer").hidden = Boolean(chat?.readOnly);
  drawStatusBar(workspace, told);
  composer.update(place?.session ?? `draft ${place?.draft}`, Boolean(chat?.running));
  if (following) scroller.scrollTop = scroller.scrollHeight;
}

/** A touched alert brings its conversation, once the computer has told of it. */
let touched = null;
function showOpened() {
  touched = alertOpened() ?? touched;
  if (!touched || !workspaceOf(state.overview?.workspaces, touched)) return;
  select({ id: touched });
  touched = null;
}

// ---- sheets

function openSheet(title, content) {
  const dialog = $("sheet");
  dialog.replaceChildren(h("div", { class: "sheet-head" }, h("h2", {}, title), h("button", { type: "button", class: "plain", "aria-label": copy.close, onclick: () => dialog.close() }, icon("cross"))), content);
  if (!dialog.open) dialog.showModal();
}

const sheet = (title, items) => sheets(title, [{ items }]);

function sheets(title, groups) {
  const item = ({ label, mark, current, pick }) => h("button", { type: "button", "aria-current": current ? "true" : undefined, onclick: () => ($("sheet").close(), pick()) }, mark, h("span", {}, label), current ? icon("check") : null);
  openSheet(title, h("div", {}, groups.map(({ title: of, items }) => [of ? h("h3", {}, of) : null, h("div", { class: "sheet-items" }, items.map(item))]).flat()));
}

async function menu(kind) {
  const { place } = state;
  const now = state.chat?.[kind];
  const pick = async (id) => {
    const item = menus[kind].find((other) => other.id === id);
    state.chat = { ...state.chat, [kind]: item.label ?? id, ...(kind === "model" ? { modelName: item.name ?? null } : null) };
    draw();
    const taken = await run({ type: "pick", kind, id });
    if (taken.type !== "done") return;
    const { context, effort, ...rest } = picks.get(place) ?? {};
    // Another model may not have what was set of the one before.
    picks.set(place, kind === "model" && rest.model !== id ? { ...rest, model: id } : { ...picks.get(place), [kind]: id });
  };
  const show = () => sheet(copy[kind], menus[kind].map(({ id, name, label = copy.modes[id] ?? id }) => ({ label, mark: kind === "model" ? (logo(name, label) ?? h("span", { class: "logo" })) : null, current: now === id || now === label, pick: () => pick(id) })));
  const known = menus[kind];
  if (known) show();
  const reply = await run({ type: "menu", kind });
  if (reply.type !== "menu") return;
  menus[kind] = reply.items;
  if (!known) show();
}

/** Shows at once; the computer's next word of the conversation has the last one. */
async function tune(kind, id) {
  const { place } = state;
  const before = state.chat.tuning;
  const { label } = before[kind].items.find((item) => item.id === id);
  state.chat = { ...state.chat, tuning: { ...before, [kind]: { ...before[kind], now: id, label } } };
  draw();
  const taken = await run({ type: "pick", kind, id });
  if (taken.type === "done") picks.set(place, { ...picks.get(place), [kind]: id });
  else if (state.place === place) {
    state.chat = { ...state.chat, tuning: before };
    draw();
  }
}

$("toggle-side").addEventListener("click", () => {
  state.sidebarCollapsed = !state.sidebarCollapsed;
  draw();
});
lightMedia.addEventListener("change", () => state.theme === "auto" && applyTheme());
$("chats").addEventListener("click", () => {
  const workspace = state.overview.workspaces.find((item) => item.id === here());
  sheet(placeName(workspace), workspace.chats.map(({ id, label }) => ({ label: label || copy.untitled, current: id === state.place.session, pick: () => select({ id }) })));
});
$("new").addEventListener("click", () => begin(here()));
$("home-new").addEventListener("click", () => begin());
$("home-group").addEventListener("click", actions.onGrouping);
for (const id of ["line", "home-line"]) $(id).addEventListener("click", () => openSheet(copy.line.title, stretches));
for (const id of ["back", "settings-back"]) $(id).addEventListener("click", () => views.back());
$("latest").addEventListener("click", () => {
  state.follow = true;
  scroller.scrollTop = scroller.scrollHeight;
  draw();
});
$("sheet").addEventListener("click", (event) => event.target === $("sheet") && $("sheet").close());

scroller.addEventListener("scroll", () => {
  const follow = scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight < NEAR_END;
  if (follow === state.follow) return;
  state.follow = follow;
  $("latest").hidden = !state.chat || follow;
});
// The keyboard, or an approval, takes room from the transcript.
new ResizeObserver(() => state.follow && (scroller.scrollTop = scroller.scrollHeight)).observe(scroller);

let toastTimer;
function toast(text) {
  $("toast").textContent = text;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => ($("toast").textContent = ""), 4_500);
}

function gate(wording, iconName = "link") {
  views.reset("gate");
  const others = native && wording !== copy.joining ? computers.list : [];
  $("gate").replaceChildren(h("div", {}, notice(wording, iconName), native && wording !== copy.joining ? pairing() : null, others.map(({ id, name }) => h("button", { type: "button", class: "pill other", onclick: () => turn(id, true) }, icon("monitor"), name ?? copy.computer))));
}

function join(ticket) {
  if (!link.pair(ticket)) return toast(copy.notALink);
  clear();
  gate(copy.joining, "link");
}

async function look() {
  const read = await scan().catch(() => toast(copy.noCamera));
  if (read) join(read);
}

/** The app's two ways to pair: the camera, or the link by hand. */
function pairing() {
  const typed = h("input", { type: "text", placeholder: copy.pasteLink, "aria-label": copy.pasteLink, autocomplete: "off", autocapitalize: "off", spellcheck: "false" });
  const submit = (event) => {
    event.preventDefault();
    join(typed.value);
  };
  return h("div", { class: "pairing" }, h("button", { type: "button", class: "pill", onclick: look }, copy.scanButton), h("form", { onsubmit: submit }, typed, h("button", { type: "submit", class: "pill" }, copy.join)));
}

for (const [id, name, label] of [
  ["back", "back", copy.back],
  ["toggle-side", "sidebar", copy.toggleSidebar],
  ["new", "compose", copy.newChat],
  ["home-new", "compose", copy.newChat],
  ["home-group", "sortGroup", copy.grouping],
  ["settings-back", "back", copy.back],
]) {
  $(id).append(icon(name));
  $(id).setAttribute("aria-label", label);
  $(id).title = label;
}
$("scope").append(icon("down"));
$("scope").addEventListener("click", chooseScope);
$("settings-title").textContent = copy.settings;
$("tabs").append(...[["home", "chat", copy.homeTab], ["settings", "gear", copy.settings]].map(([view, name, label]) => h("button", { type: "button", "data-tab": view, onclick: () => views.go(view) }, icon(name), h("span", {}, label))));
feelTouches(document);
$("chats").setAttribute("aria-label", copy.chats);
$("chats").append(icon("down"));
$("latest").append(icon("down"), copy.latest);

setInterval(() => document.hidden || draw(), 60_000);
document.addEventListener("visibilitychange", () => document.hidden || showOpened());
applyTheme();
gate(copy.joining, "link");
const paired = await pair(computers);
if (paired === "paired") {
  views.reset("home");
  link.connect();
} else gate(paired === "expired" ? copy.expired : native ? copy.scan : copy.gate, "link");
