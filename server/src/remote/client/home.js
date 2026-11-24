//! Home's own parts: where a message from there goes, and what waits for an answer.
import { threadRow } from "./conversations.js";
import { copy } from "./copy.js";
import { h, icon, redraw } from "./dom.js";

export function drawProject(root, workspace, choose) {
  redraw(root, JSON.stringify([workspace?.id, workspace?.title]), () =>
    workspace ? [h("button", { type: "button", class: "chip chip-project", "aria-label": `${copy.project}: ${workspace.title}`, onclick: choose }, icon("folder"), h("span", {}, workspace.title), icon("down"))] : [],
  );
}

/** A folded folder must not hide that Cursor waits. */
export function drawWaiting(root, threads, actions) {
  const waiting = threads.filter(({ state }) => state === "waiting");
  redraw(root, JSON.stringify(waiting.map(({ current, ...item }) => item)), () =>
    waiting.length ? [h("h2", {}, copy.state.waiting), h("nav", { class: "thread-list", "aria-label": copy.state.waiting }, waiting.map((item) => threadRow(item, actions, item.project)))] : [],
  );
  root.hidden = !waiting.length;
}
