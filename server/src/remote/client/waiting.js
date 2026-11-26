//! What waits for an answer, ahead of every conversation.
import { threadRow } from "./conversations.js";
import { copy } from "./copy.js";
import { h, redraw } from "./dom.js";

/** A folded folder must not hide that Cursor waits. */
export function drawWaiting(root, threads, actions) {
  const waiting = threads.filter(({ state }) => state === "waiting");
  redraw(root, JSON.stringify(waiting.map(({ current, ...item }) => item)), () =>
    waiting.length ? [h("h2", {}, copy.state.waiting), h("nav", { class: "thread-list", "aria-label": copy.state.waiting }, waiting.map((item) => threadRow(item, actions, item.project)))] : [],
  );
  root.hidden = !waiting.length;
}
