//! Holds what is known about the computer and shows it: home, the conversations, one of them, or why none.
import { Transcript } from "./chat.js";
import { Conversations, notice } from "./conversations.js";
import { chinese, copy } from "./copy.js";
import { Composer, drawAsked, drawChips, drawPanel } from "./dock.js";
import { h, icon, redraw } from "./dom.js";
import { drawProject, drawWaiting } from "./home.js";
import { Link, pair } from "./link.js";
import { buildThreads, chatState, delivered, homeWorkspace, resolveTheme, sendingAfter, shownRows, spliceRows, waitingCount, workspaceOf } from "./model.js";
import { buzz, native, paintBars, scan } from "./native.js";
import { Settings } from "./settings.js";
import { Sidebar } from "./sidebar.js";
import { Views } from "./views.js";

const $ = (id) => document.getElementById(id);
const NEAR_END = 80;
/** Text that stops for this long is no longer being written. */
const QUIET_MS = 1_000;
const THEME_KEY = "cursor-byok-remote-theme";
const GROUP_KEY = "cursor-byok-remote-group";
const PINNED_KEY = "cursor-byok-remote-pinned";
const FOLDERS_KEY = "cursor-byok-remote-folders";
const PROJECT_KEY = "cursor-byok-remote-project";
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
  folders: new Map(readJson(FOLDERS_KEY, [])),
};

document.documentElement.lang = chinese ? "zh-CN" : "en";

/** Modes and models chosen here; the server forgets them with the connection. */
const picks = new WeakMap();
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

const link = new Link({
  open() {
    state.online = true;
    if (state.place) enter();
    draw();
  },
  closed() {
    state.online = false;
    draw();
  },
  // A browser's page came from the computer, so only its address could be wrong.
  away: () => native && gate(copy.away, "wifiOff"),
  unpaired: () => gate(native ? copy.scan : copy.gate, "link"),
  expired: () => gate(copy.expired, "link"),
  message(message) {
    if (views.now === "gate") views.reset("home");
    if (message.type === "overview") {
      state.overview = message;
      const newest = message.workspaces.flatMap((workspace) => workspace.chats)[0];
      if (wide.matches && !state.place && newest) open({ session: newest.id });
      if (!wide.matches && views.now === "home") ready();
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
  for (const [kind, id] of Object.entries(picks.get(place) ?? {})) link.command({ type: "pick", kind, id });
}

const run = async (command) => {
  const { place } = state;
  const reply = await link.command(command);
  if (reply.type === "refused") toast(copy.refused[reply.code] ?? copy.refused.failed);
  // The first message made a conversation of the draft.
  if (place?.draft && reply.session && state.place === place) {
    state.place = { session: reply.session };
    picks.set(state.place, picks.get(place) ?? {});
    draw();
  }
  return reply;
};

const scroller = $("scroll");
const transcript = new Transcript($("transcript"));
const composer = new Composer($("composer"), send, () => run({ type: "stop" }));
// Seen in its conversation from the first moment.
const homeComposer = new Composer($("home-composer"), (text) => {
  views.go("chat");
  return send(text);
});

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

/** Home always holds a conversation not begun, so its mode and model are there to choose. */
function ready() {
  const workspace = homeWorkspace(state.overview?.workspaces, readStorage(PROJECT_KEY, null));
  if (workspace && state.place?.draft !== workspace.id) open({ draft: workspace.id });
}

function begin(workspace = here() ?? state.overview?.workspaces[0]?.id) {
  if (!workspace) return;
  if (state.place?.draft !== workspace) open({ draft: workspace });
  views.go("chat");
}

function chooseProject() {
  const { workspaces } = state.overview;
  sheet(copy.project, workspaces.map(({ id, title }) => ({
    label: title,
    current: id === here(),
    pick: () => {
      writeStorage(PROJECT_KEY, id);
      ready();
      draw();
    },
  })));
}

const views = new Views((view) => {
  if (view === "home" && !wide.matches) ready();
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
  redraw($("status-left"), workspace.title, () => [h("span", { class: "status-project" }, icon("folder"), h("span", {}, workspace.title))]);
  redraw($("status-right"), told, () => [
    h("span", { class: "status-pill", "data-state": told }, h("span", { class: "state", "data-state": told }), h("span", {}, copy.state[told] ?? copy.offline)),
  ]);
}

const actions = {
  onSelectThread: select,
  onNewChat: begin,
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
    buzz();
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
  onPair: look,
  onLeave: () => {
    link.leave();
    Object.assign(state, { overview: null, online: false, place: null, chat: null, rows: [] });
    seen.clear();
    gate(native ? copy.scan : copy.gate, "link");
  },
});

function draw() {
  const { overview, online, place, chat, sidebarCollapsed, groupBy, pinned, folders } = state;
  document.body.dataset.sidebar = sidebarCollapsed ? "collapsed" : "open";
  const waiting = waitingCount(overview?.workspaces);
  document.title = waiting ? `(${waiting}) Cursor` : "Cursor";
  const connection = {
    told: online ? (waiting ? "waiting" : "online") : "offline",
    status: online ? (waiting ? copy.waitingTitle(waiting) : copy.nothingWaiting) : overview ? copy.offline : copy.connecting,
  };
  const threads = buildThreads(overview?.workspaces, { current: place?.session, open: chat, pinned });
  sidebar.draw({ overview, online, ...connection });
  conversations.draw({ overview, current: place?.session, groupBy, folders, threads });
  redraw($("home-state"), JSON.stringify(connection), () => [h("span", { class: "state", "data-state": connection.told }), h("span", { class: "side-status-text" }, connection.status)]);
  drawWaiting($("home-waiting"), threads, actions);
  $("home-title").hidden = !overview?.workspaces.length;
  $("home-by").textContent = copy.groupTitle[groupBy];
  $("tabs").style.setProperty("--at", document.body.dataset.tab === "home" ? 0 : 1);
  for (const tab of $("tabs").children) tab.setAttribute("aria-current", String(tab.dataset.tab === document.body.dataset.tab));
  if (views.now === "settings") settings.draw(state);
  const workspace = overview?.workspaces.find((item) => item.id === here());
  const listed = workspace?.chats.find((item) => item.id === place?.session);
  const reachable = online && Boolean(overview?.reachable);
  const told = reachable ? (chat ? chatState(chat) : (listed?.state ?? "idle")) : "offline";
  // The conversation's own title leads; the project is where it is.
  $("name").textContent = place ? (listed?.label || (place.draft ? copy.newChat : copy.untitled)) : "";
  $("title").textContent = workspace?.title ?? "";
  $("title").hidden = !workspace;
  $("state").dataset.state = place ? told : "";
  $("state").textContent = copy.state[told] ?? copy.offline;
  $("chats").disabled = !workspace?.chats.length;
  $("new").hidden = !workspace;

  const wording = chatNotice(listed);
  $("notice").hidden = !wording;
  if (wording) $("notice").replaceChildren(notice(...wording));
  const following = state.follow;
  transcript.draw(shownRows(state.rows, chat, state.sending, Date.now() - state.changed >= QUIET_MS));
  $("latest").hidden = !chat || following;

  $("dock").hidden = !chat?.open || !overview?.reachable;
  drawAsked($("asked"), chat, run);
  drawPanel($("panel"), chat);
  drawChips($("chips"), chat, menu);
  drawHomeComposer(workspace);
  drawStatusBar(workspace, told);
  composer.update(place?.session ?? `draft ${place?.draft}`, Boolean(chat?.running));
  if (following) scroller.scrollTop = scroller.scrollHeight;
}

function drawHomeComposer(workspace) {
  const { overview, place, chat } = state;
  $("home-composer").hidden = !overview?.reachable || !overview.workspaces.length;
  if (!place?.draft) return;
  drawProject($("home-project"), workspace, chooseProject);
  drawChips($("home-chips"), chat, menu);
  homeComposer.update(`home ${place.draft}`, false);
}

// ---- sheets

function sheet(title, items) {
  const dialog = $("sheet");
  const close = () => dialog.close();
  dialog.replaceChildren(
    h("div", { class: "sheet-head" }, h("h2", {}, title), h("button", { type: "button", class: "plain", "aria-label": copy.close, onclick: close }, icon("cross"))),
    h("div", { class: "sheet-items" }, items.map(({ label, current, pick }) => h("button", { type: "button", "aria-current": current ? "true" : undefined, onclick: () => (close(), pick()) }, h("span", {}, label), current ? icon("check") : null))),
  );
  if (!dialog.open) dialog.showModal();
}

async function menu(kind) {
  const { place } = state;
  const now = state.chat?.[kind];
  const pick = async (id) => {
    const item = menus[kind].find((other) => other.id === id);
    state.chat = { ...state.chat, [kind]: item.label ?? id };
    draw();
    const taken = await run({ type: "pick", kind, id });
    if (taken.type === "done") picks.set(place, { ...picks.get(place), [kind]: id });
  };
  const show = () => sheet(copy[kind], menus[kind].map(({ id, label = copy.modes[id] ?? id }) => ({ label, current: now === id || now === label, pick: () => pick(id) })));
  const known = menus[kind];
  if (known) show();
  const reply = await run({ type: "menu", kind });
  if (reply.type !== "menu") return;
  menus[kind] = reply.items;
  if (!known) show();
}

$("toggle-side").addEventListener("click", () => {
  state.sidebarCollapsed = !state.sidebarCollapsed;
  draw();
});
lightMedia.addEventListener("change", () => state.theme === "auto" && applyTheme());
$("chats").addEventListener("click", () => {
  const { title, chats } = state.overview.workspaces.find((item) => item.id === here());
  sheet(title, chats.map(({ id, label }) => ({ label: label || copy.untitled, current: id === state.place.session, pick: () => select({ id }) })));
});
$("new").addEventListener("click", () => begin());
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
  $("gate").replaceChildren(h("div", {}, notice(wording, iconName), native && wording !== copy.joining ? pairing() : null));
}

function join(ticket) {
  if (link.pair(ticket)) gate(copy.joining, "link");
  else toast(copy.notALink);
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
  ["settings-back", "back", copy.back],
]) {
  $(id).append(icon(name));
  $(id).setAttribute("aria-label", label);
  $(id).title = label;
}
$("hello").textContent = copy.hello;
$("settings-title").textContent = copy.settings;
$("home-title").append(h("button", { type: "button", class: "plain", "aria-label": copy.grouping, title: copy.grouping, onclick: actions.onGrouping }, icon("sortGroup")));
$("tabs").append(...[["home", "home", copy.homeTab], ["settings", "gear", copy.settings]].map(([view, name, label]) => h("button", { type: "button", "data-tab": view, onclick: () => views.go(view) }, icon(name), h("span", {}, label))));
// Felt where something is set off, not on every touch.
document.addEventListener("click", (event) => event.target.closest?.("#tabs button, .segments button, .go, .quiet, button[type=submit]") && buzz());
$("chats").setAttribute("aria-label", copy.chats);
$("chats").append(icon("down"));
$("latest").append(icon("down"), copy.latest);

setInterval(() => document.hidden || draw(), 60_000);
applyTheme();
gate(copy.joining, "link");
const paired = await pair();
if (paired === "paired") {
  views.reset("home");
  link.connect();
} else gate(paired === "expired" ? copy.expired : native ? copy.scan : copy.gate, "link");
