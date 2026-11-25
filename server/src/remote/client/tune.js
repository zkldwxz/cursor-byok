//! How hard the model thinks, as a meter drawn by the finger, and how much it keeps in mind.
import { copy } from "./copy.js";
import { h, redraw } from "./dom.js";
import { feel } from "./feel.js";

export const effortName = ({ now, label }) => copy.efforts[now] ?? label ?? copy.usual;
export const contextName = ({ label }) => label ?? copy.usual;

/** Rising bars, filled up to the level; `levels` may say null while nothing is set. */
export function bars(count, at) {
  const root = h("span", { class: "bars", "aria-hidden": "true" }, Array.from({ length: count }, (_, index) => h("i", { "data-on": index <= at ? "" : undefined })));
  [...root.children].forEach((bar, index) => bar.style.setProperty("--level", (index + 1) / count));
  return root;
}

export const levelOf = ({ items, now }) => items.findIndex(({ id }) => id === now);

function meter(effort, pick) {
  const { items } = effort;
  let at = levelOf(effort);
  const value = h("strong", {}, effortName(effort));
  const track = bars(items.length, at);
  const root = h("div", { class: "meter", role: "slider", tabindex: "0", "aria-label": copy.effort, "aria-valuemin": "0", "aria-valuemax": String(items.length - 1) }, track);
  const show = (next) => {
    at = next;
    const name = copy.efforts[items[at].id] ?? items[at].label;
    value.textContent = name;
    root.setAttribute("aria-valuenow", String(at));
    root.setAttribute("aria-valuetext", name);
    [...track.children].forEach((bar, index) => bar.toggleAttribute("data-on", index <= at));
  };
  const under = ({ clientX }) => {
    const { left, width } = root.getBoundingClientRect();
    return Math.min(items.length - 1, Math.max(0, Math.floor(((clientX - left) / width) * items.length)));
  };
  const follow = (event) => {
    const next = under(event);
    if (next === at) return;
    show(next);
    feel("toggle");
  };
  let held = null;
  root.addEventListener("pointerdown", (event) => {
    held = at;
    root.setPointerCapture(event.pointerId);
    follow(event);
  });
  root.addEventListener("pointermove", (event) => held !== null && follow(event));
  const release = () => {
    if (held !== null && held !== at) pick(items[at].id);
    held = null;
  };
  root.addEventListener("pointerup", release);
  root.addEventListener("pointercancel", release);
  root.addEventListener("keydown", (event) => {
    const step = { ArrowRight: 1, ArrowUp: 1, ArrowLeft: -1, ArrowDown: -1 }[event.key];
    const next = at + (step ?? 0);
    if (!step || !items[next]) return;
    event.preventDefault();
    show(next);
    pick(items[at].id);
  });
  return [h("div", { class: "tune-head" }, h("span", {}, copy.effort), value), root, h("div", { class: "meter-ends", "aria-hidden": "true" }, h("span", {}, copy.faster), h("span", {}, copy.deeper))];
}

const pills = ({ items, now }, pick) =>
  h("div", { class: "pills", role: "radiogroup", "aria-label": copy.context }, items.map(({ id, label }) => h("button", { type: "button", role: "radio", "aria-checked": String(id === now), onclick: () => pick(id) }, label)));

/** `pick(kind, id)`; drawn again only when what is set or can be set changed. */
export function drawTune(root, tuning, pick) {
  const { effort, context } = tuning ?? {};
  const held = root.contains(document.activeElement) && document.activeElement.getAttribute("role");
  redraw(root, JSON.stringify([effort, context]), () => [
    effort ? h("section", {}, meter(effort, (id) => pick("effort", id))) : null,
    context ? h("section", {}, h("div", { class: "tune-head" }, h("span", {}, copy.context), h("strong", {}, contextName(context))), pills(context, (id) => pick("context", id))) : null,
  ]);
  // A key that moved the meter moves it on.
  if (held === "slider") root.querySelector(".meter")?.focus();
}
