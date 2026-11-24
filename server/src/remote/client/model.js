//! What the views show, worked out from what the server reports; touches no page.
const everyChat = (workspaces) => (workspaces ?? []).flatMap((workspace) => workspace.chats);

export const waitingCount = (workspaces) => everyChat(workspaces).filter((chat) => chat.state === "waiting").length;

/** Where a message from home goes: the project last chosen, else the one last worked in. */
export function homeWorkspace(workspaces, chosen) {
  const latest = (workspace) => workspace.chats.reduce((latest, chat) => Math.max(latest, chat.updatedAt), 0);
  const all = workspaces ?? [];
  return all.find(({ id }) => id === chosen) ?? all.toSorted((a, b) => latest(b) - latest(a))[0] ?? null;
}

/** Everything that waits, and the latest of the others. */
export const homeThreads = (threads, count) => ({
  waiting: threads.filter(({ state }) => state === "waiting"),
  recent: threads.filter(({ state }) => state !== "waiting").toSorted((a, b) => a.ageMinutes - b.ageMinutes).slice(0, count),
});

export const workspaceOf = (workspaces, session) =>
  (workspaces ?? []).find((workspace) => workspace.chats.some((chat) => chat.id === session)) ?? null;

/** Conversations in the server's order; the open one's state is the one its own stream reports. */
export function buildThreads(workspaces, { current = null, open = null, pinned = new Set(), now = Date.now() } = {}) {
  return (workspaces ?? []).flatMap((workspace) =>
    workspace.chats.map((chat) => ({
      id: chat.id,
      label: chat.label,
      workspace: workspace.id,
      project: workspace.title,
      current: chat.id === current,
      state: chat.id === current && open ? chatState(open) : chat.state,
      ageMinutes: Math.max(0, Math.floor((now - chat.updatedAt) / 60_000)),
      pinned: pinned.has(chat.id),
    })),
  );
}

export function dateBucket(ageMinutes, nowMs = Date.now()) {
  const age = Number.isFinite(ageMinutes) ? Math.max(0, ageMinutes) : 0;
  const itemTime = nowMs - age * 60_000;
  const now = new Date(nowMs);
  const startOfToday = new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime();
  const startOfYesterday = startOfToday - 86_400_000;
  const startOfWeek = startOfToday - 7 * 86_400_000;
  if (itemTime >= startOfToday) return "today";
  if (itemTime >= startOfYesterday) return "yesterday";
  if (itemTime >= startOfWeek) return "week";
  return "earlier";
}

export function formatAge(ageMinutes, chinese) {
  const minutes = Number.isFinite(ageMinutes) ? Math.max(0, Math.floor(ageMinutes)) : 0;
  if (minutes < 1) return chinese ? "刚刚" : "now";
  if (minutes < 60) return chinese ? `${minutes}分钟` : `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return chinese ? `${hours}小时` : `${hours}h`;
  const days = Math.floor(hours / 24);
  if (days < 7) return chinese ? `${days}天` : `${days}d`;
  const weeks = Math.floor(days / 7);
  return chinese ? `${weeks}周` : `${weeks}w`;
}

export function groupThreads(workspaces, threads, mode = "project", now = Date.now()) {
  const groups = [];
  const pinned = threads.filter((item) => item.pinned);
  if (pinned.length) groups.push({ id: "pinned", kind: "pinned", items: pinned });
  const rest = threads.filter((item) => !item.pinned);
  const bucketed = (names, bucketOf) => {
    for (const bucket of names) {
      const items = rest.filter((item) => bucketOf(item) === bucket);
      if (items.length) groups.push({ id: `${mode}:${bucket}`, kind: mode, bucket, items });
    }
  };
  if (mode === "date") {
    rest.sort((a, b) => a.ageMinutes - b.ageMinutes);
    bucketed(["today", "yesterday", "week", "earlier"], (item) => dateBucket(item.ageMinutes, now));
  } else if (mode === "status") bucketed(["waiting", "running", "idle"], (item) => item.state);
  // A project without conversations is still where one can begin.
  else for (const { id, title } of workspaces ?? []) groups.push({ id: `project:${id}`, kind: "project", workspace: id, title, items: rest.filter((item) => item.workspace === id) });
  return groups;
}

const SHOWN = [3, 5];

/** How many of a group's conversations show after "more" was asked for `step` times. */
export const shownCount = (total, step) => Math.min(total, SHOWN[step] ?? total);

/** A message carries the rows from where they differ from what was sent before. */
export const spliceRows = (rows, { from, rows: changed }) => [...rows.slice(0, from), ...changed];

const groupKind = (row) => {
  if (row.todos) return "todos";
  return row.kind === "human" || row.kind === "assistant" ? row.kind : "steps";
};

export function groupRows(rows) {
  const groups = [];
  for (const row of rows) {
    const kind = groupKind(row);
    const last = groups.at(-1);
    if (kind === "steps" && last?.kind === "steps") last.rows.push(row);
    else groups.push({ kind, key: row.key, rows: [row] });
  }
  return groups;
}

export const stepKind = (row) => (row.kind === "thinking" ? "thinking" : (row.tool?.kind ?? "tool"));

function tallyOf(rows) {
  const tally = { read: 0, searched: 0, ran: 0, used: 0, seconds: 0 };
  for (const row of rows) {
    const kind = stepKind(row);
    if (kind === "thinking") tally.seconds += row.seconds ?? 0;
    else if (kind === "run") tally.ran += 1;
    else if (kind === "read") tally.read += 1;
    else if (kind === "search") tally.searched += 1;
    else tally.used += 1;
  }
  return tally;
}

export const resolveTheme = (pref, prefersLight) =>
  pref === "light" ? "light" : pref === "dark" ? "dark" : prefersLight ? "light" : "dark";

/** Steps that just went by become one counted line; edits, failures and what still runs stand alone. */
export function foldSteps(rows) {
  const items = [];
  const folds = (row) => stepKind(row) !== "edit" && row.status !== "error" && row.status !== "loading";
  for (const row of rows) {
    const last = items.at(-1);
    if (folds(row) && last?.folds) last.rows.push(row);
    else items.push({ key: row.key, rows: [row], folds: folds(row) });
  }
  return items.map(({ key, rows }) => (rows.length > 1 ? { key, rows, tally: tallyOf(rows) } : { key, rows }));
}

export function chatState(chat) {
  if (chat.asked.length) return "waiting";
  return chat.running ? "running" : "idle";
}

/** Answers as the server takes them, or null while a question has none. */
export function answersOf(questions, given) {
  const answers = questions.map(({ id }) => ({ question: id, options: [...(given[id]?.options ?? [])], text: (given[id]?.text ?? "").trim() }));
  return answers.every((answer) => answer.options.length || answer.text) ? answers : null;
}

export function toggleOption(chosen = [], id, multiple) {
  if (chosen.includes(id)) return chosen.filter((other) => other !== id);
  return multiple ? [...chosen, id] : [id];
}

/** Android browsers also say Linux, and iPhones also say Mac. */
const SYSTEMS = ["iPhone", "iPad", "Android", "Macintosh", "Windows", "Linux"];

/** The one-time code in an app's pairing link; null when it is not one. */
export function codeOf(link) {
  const data = /^cursor-byok:\/\/pair\?d=([\w-]+)$/.exec(link?.trim() ?? "")?.[1];
  try {
    return JSON.parse(atob(data.replaceAll("-", "+").replaceAll("_", "/"))).c || null;
  } catch {
    return null;
  }
}

export const deviceName = (userAgent) => SYSTEMS.find((name) => userAgent.includes(name))?.replace("Macintosh", "Mac") ?? "Browser";

const humans = (rows) => rows.filter((row) => row.kind === "human").length;

/** A message on its way: how many there were before tells when Cursor has it. */
export const sendingAfter = (rows, text) => ({ text, after: humans(rows) });
export const delivered = (rows, sending) => humans(rows) > sending.after;

/** What the conversation is about to show, shown already: the message being sent, and Cursor at work on it. */
export function shownRows(rows, chat, sending, quiet = false) {
  const last = rows.at(-1);
  const writing = last?.status === "loading" || (last?.kind === "assistant" && !quiet);
  const thinks = chat?.running ? !chat.asked.length && !writing : Boolean(sending);
  return [
    ...rows,
    ...(sending ? [{ key: "sending", kind: "human", text: sending.text, status: "sending" }] : []),
    ...(thinks ? [{ key: "thinking", kind: "thinking", status: "loading" }] : []),
  ];
}
