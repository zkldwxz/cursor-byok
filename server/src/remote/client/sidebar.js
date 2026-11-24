//! Beside the chat, where there is room: the conversations, how to begin one, and the connection.
import { copy } from "./copy.js";
import { h, icon, redraw } from "./dom.js";

export class Sidebar {
  /** Where the conversations go. */
  body = h("div", { class: "side-body" });
  #new;
  #status = h("div", { class: "side-status", role: "status" });

  constructor(root, actions) {
    const button = (name, label, onclick) => h("button", { type: "button", class: "plain", "aria-label": label, title: label, onclick }, icon(name));
    this.#new = h("button", { type: "button", class: "side-new", onclick: () => actions.onNewChat() }, icon("plus"), h("span", {}, copy.newChat));
    root.replaceChildren(h("div", { class: "side-top" }, this.#new, button("sortGroup", copy.grouping, actions.onGrouping)), this.body, h("footer", { class: "side-foot" }, this.#status, button("gear", copy.settings, actions.onSettings)));
  }

  draw({ overview, online, told, status }) {
    this.#new.disabled = !overview?.workspaces.length || !online;
    redraw(this.#status, JSON.stringify([told, status]), () => [h("span", { class: "state", "data-state": told }), h("span", { class: "side-status-text" }, status)]);
  }
}
