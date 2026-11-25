//! The light that tells whether what is on screen is live and, when it is not, which stretch of the way is broken.
import { copy } from "./copy.js";
import { h, redraw } from "./dom.js";

const said = (words, attempt) => (typeof words === "function" ? words(attempt) : words);

/** Why nothing moves, in a few words. */
export const lineText = ({ now, attempt }) => said(copy.line.now[now], attempt);

/** A lamp alone while all is well: words appear only when there is something to say. */
export function drawLine(button, line) {
  redraw(button, JSON.stringify(line), () => [h("span", { class: "lamp", "data-now": line.now }), line.now === "ok" ? null : h("span", { class: "line-text" }, lineText(line))]);
  button.dataset.now = line.now;
  button.setAttribute("aria-label", `${copy.line.title}: ${lineText(line)}`);
}

const MARKS = { ok: "online", idle: "online", lost: "offline", slow: "waiting", connecting: "waiting", retrying: "waiting", unknown: "unknown" };

/** Each stretch of the way, and what to do about the one that is broken. */
export function drawStretches(root, line) {
  redraw(root, JSON.stringify(line), () => [
    h("div", { class: "card" }, ["phone", "cursor", "model"].map((name) => {
      const state = line[name];
      return h("div", { class: "card-row" }, h("span", {}, copy.line[name].name), h("span", { class: "card-value" }, h("span", { class: "state", "data-state": MARKS[state] }), said(copy.line[name][state], line.attempt)));
    })),
    copy.line.hint[line.now] ? h("p", { class: "line-hint" }, copy.line.hint[line.now]) : null,
  ]);
}
