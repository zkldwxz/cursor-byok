//! The conversations: grouped by project, date or status, with pins; beside the chat where there is room.
import { chinese, copy } from "./copy.js";
import { h, icon, redraw } from "./dom.js";
import { formatAge, groupThreads, shownCount } from "./model.js";

export const notice = ([title, body], iconName) =>
  h("div", { class: "notice" }, iconName ? h("div", { class: "notice-icon" }, icon(iconName)) : null, h("strong", {}, title), body ? h("p", {}, body) : null);

export function threadRow(item, groupKind, { onSelectThread, onTogglePin }) {
  const showState = item.state === "waiting" || item.state === "running";
  return h(
    "div",
    { class: "thread-row", "data-session": item.id, "data-indented": groupKind === "project" ? "true" : undefined, "data-state": item.state },
    h(
      "button",
      { type: "button", class: "thread-main", onclick: () => onSelectThread(item) },
      h("span", { class: "thread-lead" }, showState ? h("span", { class: "state", "data-state": item.state, role: "img", "aria-label": copy.state[item.state] }) : null),
      h("span", { class: "thread-title" }, item.label || copy.untitled),
      groupKind === "project" ? null : h("span", { class: "thread-meta" }, h("span", { class: "thread-project" }, item.project)),
      h("span", { class: "thread-time" }, formatAge(item.ageMinutes, chinese)),
    ),
    groupKind === "home"
      ? null
      : h(
          "button",
          { type: "button", class: "thread-pin", "aria-pressed": String(item.pinned), "aria-label": item.pinned ? copy.unpin : copy.pin, title: item.pinned ? copy.unpin : copy.pin, onclick: () => onTogglePin(item.id) },
          icon("pin"),
        ),
  );
}

/** Groups keep their elements while they read the same, so opening and folding one moves nothing else. */
export class Sidebar {
  #root;
  #actions;
  #groups = new Map();
  /** How often "more" was asked for, by group. */
  #steps = new Map();
  #last = null;
  #body = h("div", { class: "side-body" });
  #new;
  #status = h("div", { class: "side-status", role: "status" });

  constructor(root, actions) {
    this.#root = root;
    this.#actions = actions;
    const button = (name, label, onclick) => h("button", { type: "button", class: "plain", "aria-label": label, title: label, onclick }, icon(name));
    const settings = () => button("gear", copy.settings, actions.onSettings);
    const grouping = () => button("sortGroup", copy.grouping, actions.onGrouping);
    const begin = () => actions.onNewChat();
    this.#new = [
      button("compose", copy.newChat, begin),
      h("button", { type: "button", class: "side-new", onclick: begin }, icon("plus"), h("span", {}, copy.newChat)),
    ];
    root.replaceChildren(
      h("header", { class: "page-head" }, h("h1", {}, copy.chatsTab), h("div", { class: "page-actions" }, grouping(), this.#new[0], settings())),
      h("div", { class: "side-top" }, this.#new[1], grouping()),
      this.#body,
      h("footer", { class: "side-foot" }, this.#status, settings()),
    );
  }

  draw(view) {
    this.#last = view;
    const { overview, online, current, groupBy, collapsed, threads, told, status } = view;
    const workspaces = overview?.workspaces ?? [];
    for (const button of this.#new) button.disabled = !workspaces.length || !online;
    redraw(this.#status, JSON.stringify([told, status]), () => [h("span", { class: "state", "data-state": told }), h("span", { class: "side-status-text" }, status)]);
    if (!workspaces.length) {
      this.#groups.clear();
      const wording = overview ? (overview.reachable ? [copy.noWorkspaces, "folder"] : [copy.unreachable, "wifiOff"]) : null;
      redraw(this.#body, JSON.stringify(wording), () => [wording ? notice(...wording) : h("div", { class: "skeleton", "aria-label": copy.connecting }, h("i"), h("i"), h("i"))]);
      return;
    }
    const next = new Map();
    for (const group of groupThreads(workspaces, threads, groupBy)) {
      // Which one is open is marked below, and rebuilds nothing.
      const step = this.#steps.get(group.id) ?? 0;
      const signature = JSON.stringify([group, step], (key, value) => (key === "current" ? undefined : value));
      const last = this.#groups.get(group.id);
      next.set(group.id, last?.signature === signature ? last : { signature, step, element: this.#group(group, step, last?.step ?? step) });
    }
    this.#groups = next;
    const elements = [...next.values()].map(({ element }) => element);
    if (elements.length !== this.#body.childElementCount || elements.some((element, index) => this.#body.children[index] !== element)) this.#body.replaceChildren(...elements);
    this.#body.drawn = null;
    for (const [id, { element }] of next) {
      const folded = collapsed.has(id);
      element.dataset.collapsed = String(folded);
      element.querySelector(".side-group-toggle").setAttribute("aria-expanded", String(!folded));
      element.querySelector(".side-fold").inert = folded;
    }
    for (const row of this.#body.querySelectorAll(".thread-row")) row.setAttribute("aria-current", String(row.dataset.session === current));
  }

  #more(id, step) {
    this.#steps.set(id, step);
    this.draw(this.#last);
  }

  #group(group, step, before) {
    const { onToggleCollapse, onNewChat } = this.#actions;
    const label = group.title ?? (group.kind === "pinned" ? copy.pinned : group.kind === "date" ? copy.dateBucket[group.bucket] : copy.statusBucket[group.bucket]);
    const project = group.kind === "project";
    const total = group.items.length;
    const [shown, known, after] = [step, before, step + 1].map((it) => shownCount(total, it));
    const rows = group.items.slice(0, shown).map((item) => threadRow(item, group.kind, this.#actions));
    if (step > before) for (const row of rows.slice(known)) row.classList.add("enter");
    const more =
      shown < total
        ? h("button", { type: "button", class: "more", onclick: () => this.#more(group.id, step + 1) }, after < total ? copy.showMore(after - shown) : copy.showAll(total), icon("down"))
        : step
          ? h("button", { type: "button", class: "more", onclick: () => this.#more(group.id, 0) }, copy.showLess, icon("up"))
          : null;
    return h(
      "section",
      { class: "side-group", "data-kind": group.kind },
      h(
        "div",
        { class: "side-group-head" },
        h("button", { type: "button", class: "side-group-toggle", onclick: () => onToggleCollapse(group.id) }, icon("down"), project ? icon("folder") : null, h("span", { class: "group-label" }, label), h("span", { class: "side-count" }, String(total))),
        project ? h("button", { type: "button", class: "project-add", title: copy.newChat, "aria-label": `${copy.newChat}: ${label}`, onclick: () => onNewChat(group.workspace) }, icon("plus")) : null,
      ),
      h("div", { class: "side-fold" }, h("nav", { class: "thread-list", "aria-label": label }, rows, more)),
    );
  }
}
