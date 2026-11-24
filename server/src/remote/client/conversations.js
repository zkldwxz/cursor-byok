//! The conversations as folders: by project, date or status, a few of each until more are asked for.
import { chinese, copy } from "./copy.js";
import { h, icon, redraw } from "./dom.js";
import { formatAge, groupOpen, groupThreads, shownCount } from "./model.js";

const HOLD_MS = 450;

export const notice = ([title, body], iconName) =>
  h("div", { class: "notice" }, iconName ? h("div", { class: "notice-icon" }, icon(iconName)) : null, h("strong", {}, title), body ? h("p", {}, body) : null);

const stateMark = (state) => (state === "waiting" || state === "running" ? h("span", { class: "state", "data-state": state, role: "img", "aria-label": copy.state[state] }) : null);

/** Calls `held` when an element is held, and then lets no tap follow. */
function onHold(element, held) {
  let timer;
  let done = false;
  const cancel = () => clearTimeout(timer);
  element.addEventListener("pointerdown", (event) => {
    done = false;
    if (event.pointerType !== "mouse") timer = setTimeout(() => ((done = true), held()), HOLD_MS);
  });
  for (const name of ["pointerup", "pointercancel", "pointerleave"]) element.addEventListener(name, cancel);
  element.addEventListener("contextmenu", (event) => event.preventDefault());
  element.addEventListener("click", (event) => done && event.stopImmediatePropagation(), true);
  return element;
}

export function threadRow(item, { onSelectThread, onTogglePin, onThreadMenu }, project = null) {
  return h(
    "div",
    { class: "thread-row", "data-session": item.id },
    onHold(
      h(
        "button",
        { type: "button", class: "thread-main", onclick: () => onSelectThread(item) },
        stateMark(item.state),
        h("span", { class: "thread-title" }, item.label || copy.untitled),
        project ? h("span", { class: "thread-project" }, project) : null,
        h("span", { class: "thread-time" }, formatAge(item.ageMinutes, chinese)),
      ),
      () => onThreadMenu(item),
    ),
    h("button", { type: "button", class: "thread-pin", "aria-pressed": String(item.pinned), "aria-label": item.pinned ? copy.unpin : copy.pin, title: item.pinned ? copy.unpin : copy.pin, onclick: () => onTogglePin(item.id) }, icon("pin")),
  );
}

/** Shown on what is there already: neither time passing nor another chat opening rebuilds a folder mid-motion. */
const PASSING = ["current", "ageMinutes"];
const lasting = (key, value) => (PASSING.includes(key) ? undefined : value);

const noteOf = ({ items }) => {
  if (!items.length) return copy.noChats;
  const age = Math.min(...items.map((item) => item.ageMinutes));
  return copy.folderNote(items.length, formatAge(age, chinese), age < 1);
};

/** Folders keep their elements while they read the same, so opening and folding one moves nothing else. */
export class Conversations {
  root = h("div", { class: "conversations" });
  #actions;
  #groups = new Map();
  /** How often "more" was asked for, by group. */
  #steps = new Map();
  #last = null;

  constructor(actions) {
    this.#actions = actions;
  }

  draw(view) {
    this.#last = view;
    const { overview, current, groupBy, folders, threads } = view;
    const workspaces = overview?.workspaces ?? [];
    if (!workspaces.length) {
      this.#groups.clear();
      const wording = overview ? (overview.reachable ? [copy.noWorkspaces, "folder"] : [copy.unreachable, "wifiOff"]) : null;
      redraw(this.root, JSON.stringify(wording), () => [wording ? notice(...wording) : h("div", { class: "skeleton", "aria-label": copy.connecting }, h("i"), h("i"), h("i"))]);
      return;
    }
    const groups = groupThreads(workspaces, threads, groupBy);
    const next = new Map();
    for (const group of groups) {
      const last = this.#groups.get(group.id);
      const step = this.#steps.get(group.id) ?? 0;
      const signature = JSON.stringify([group, step], lasting);
      next.set(group.id, last?.signature === signature ? last : { signature, step, element: this.#group(group, step, last?.step ?? step) });
    }
    this.#groups = next;
    const elements = [...next.values()].map(({ element }) => element);
    if (elements.length !== this.root.childElementCount || elements.some((element, index) => this.root.children[index] !== element)) this.root.replaceChildren(...elements);
    this.root.drawn = null;
    for (const group of groups) {
      const { element } = next.get(group.id);
      const open = groupOpen(groups, group, folders);
      element.dataset.open = String(open);
      element.querySelector(".folder-note").textContent = noteOf(group);
      element.querySelectorAll(".thread-time").forEach((time, index) => (time.textContent = formatAge(group.items[index].ageMinutes, chinese)));
      element.querySelector(".folder-toggle").setAttribute("aria-expanded", String(open));
      element.querySelector(".folder-fold").inert = !open;
    }
    for (const row of this.root.querySelectorAll(".thread-row")) row.setAttribute("aria-current", String(row.dataset.session === current));
  }

  #more(id, step) {
    this.#steps.set(id, step);
    this.draw(this.#last);
  }

  #group(group, step, before) {
    const { onFold, onNewChat } = this.#actions;
    const project = group.kind === "project";
    const label = group.title ?? (group.kind === "pinned" ? copy.pinned : group.kind === "date" ? copy.dateBucket[group.bucket] : copy.statusBucket[group.bucket]);
    const total = group.items.length;
    const state = ["waiting", "running"].find((it) => group.items.some((item) => item.state === it));
    const [shown, known, after] = [step, before, step + 1].map((it) => shownCount(total, it));
    const rows = group.items.slice(0, shown).map((item) => threadRow(item, this.#actions, project ? null : item.project));
    if (step > before) for (const row of rows.slice(known)) row.classList.add("enter");
    const more =
      shown < total
        ? h("button", { type: "button", class: "more", onclick: () => this.#more(group.id, step + 1) }, after < total ? copy.showMore(after - shown) : copy.showAll(total), icon("down"))
        : step
          ? h("button", { type: "button", class: "more", onclick: () => this.#more(group.id, 0) }, copy.showLess, icon("up"))
          : null;
    return h(
      "section",
      { class: "folder", "data-kind": group.kind, "data-empty": total ? undefined : "true" },
      h(
        "div",
        { class: "folder-head" },
        h(
          "button",
          { type: "button", class: "folder-toggle", onclick: () => (total ? onFold(group.id, !groupOpen([...this.#shownGroups()], group, this.#last.folders)) : onNewChat(group.workspace)) },
          h("span", { class: "folder-icon" }, icon({ pinned: "pin", project: "folder", date: "clock", status: "status" }[group.kind])),
          h("span", { class: "folder-names" }, h("span", { class: "folder-name" }, label), h("span", { class: "folder-note" })),
          stateMark(state),
          total ? icon("down") : null,
        ),
        project ? h("button", { type: "button", class: "folder-add", title: copy.newChat, "aria-label": `${copy.newChat}: ${label}`, onclick: () => onNewChat(group.workspace) }, icon("plus")) : null,
      ),
      h("div", { class: "folder-fold" }, h("nav", { class: "thread-list", "aria-label": label }, rows, more)),
    );
  }

  #shownGroups() {
    const { overview, threads, groupBy } = this.#last;
    return groupThreads(overview.workspaces, threads, groupBy);
  }
}
