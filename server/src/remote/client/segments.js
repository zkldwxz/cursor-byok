//! One of a few, all of them in sight.
import { h, icon } from "./dom.js";

/** `options` are [value, label, icon]; `show(value)` moves the mark without rebuilding anything. */
export function segments(label, options, pick) {
  const buttons = options.map(([value, text, iconName]) => h("button", { type: "button", role: "radio", onclick: () => pick(value) }, iconName ? icon(iconName) : null, h("span", {}, text)));
  const root = h("div", { class: "segments", role: "radiogroup", "aria-label": label }, buttons);
  root.style.setProperty("--count", options.length);
  root.show = (value) => {
    const at = options.findIndex(([option]) => option === value);
    root.style.setProperty("--at", at);
    buttons.forEach((button, index) => button.setAttribute("aria-checked", String(index === at)));
  };
  return root;
}
