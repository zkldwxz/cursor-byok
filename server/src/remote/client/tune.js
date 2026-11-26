//! How hard the model thinks and how much it keeps in mind, each a short list to pick from.
import { copy } from "./copy.js";
import { h, icon, redraw } from "./dom.js";

export const effortName = ({ now, label }) => copy.efforts[now] ?? label ?? copy.usual;
export const contextName = ({ label }) => label ?? copy.usual;

/** Rising bars, filled up to the level; `levels` may say null while nothing is set. */
export function bars(count, at) {
  const root = h("span", { class: "bars", "aria-hidden": "true" }, Array.from({ length: count }, (_, index) => h("i", { "data-on": index <= at ? "" : undefined })));
  [...root.children].forEach((bar, index) => bar.style.setProperty("--level", (index + 1) / count));
  return root;
}

export const levelOf = ({ items, now }) => items.findIndex(({ id }) => id === now);

/** Only what the computer was told is ticked, so the list and the chip below never disagree. */
function choices(title, { items, now }, name, mark, pick) {
  return [
    h("h3", {}, title),
    h("div", { class: "sheet-items", role: "radiogroup", "aria-label": title }, items.map((item, index) =>
      h("button", { type: "button", role: "radio", "aria-checked": String(item.id === now), onclick: () => item.id === now || pick(item.id) }, mark(index), h("span", {}, name(item)), item.id === now ? icon("check") : null),
    )),
  ];
}

/** `pick(kind, id)`; drawn again only when what is set or can be set changed. */
export function drawTune(root, tuning, pick) {
  const { effort, context } = tuning ?? {};
  redraw(root, JSON.stringify([effort, context]), () => [
    effort ? choices(copy.effort, effort, ({ id, label }) => copy.efforts[id] ?? label, (index) => bars(effort.items.length, index), (id) => pick("effort", id)) : null,
    context ? choices(copy.context, context, ({ label }) => label, () => null, (id) => pick("context", id)) : null,
  ].flat());
}
