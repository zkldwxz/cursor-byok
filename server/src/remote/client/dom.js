//! Builds elements and crisp 20x20 SVG icons; text only ever enters the page as text.
const SVG = "http://www.w3.org/2000/svg";

export function h(tag, props = {}, ...children) {
  const element = document.createElement(tag);
  for (const [name, value] of Object.entries(props)) {
    if (value === undefined || value === null || value === false) continue;
    if (name.startsWith("on")) element.addEventListener(name.slice(2), value);
    else if (name === "class") element.className = value;
    else if (name in element && typeof value !== "string") element[name] = value;
    else element.setAttribute(name, value);
  }
  element.append(...children.flat().filter((child) => child !== null && child !== undefined && child !== false));
  return element;
}

export const ICONS = {
  back: "M12.5 4 6.5 10l6 6",
  down: "m5.5 8 4.5 4.5L14.5 8",
  up: "m5.5 12 4.5-4.5L14.5 12",
  right: "m8 5.5 4.5 4.5L8 14.5",
  plus: "M10 4.5v11M4.5 10h11",
  pin: [
    ["path", { d: "M7.5 4h5M8 4v4.2L6.2 11h7.6L12 8.2V4M10 11v5.5" }],
  ],
  remote: "M4.5 6.5 8 10l-3.5 3.5M15.5 6.5 12 10l3.5 3.5",
  status: [
    ["circle", { cx: "10", cy: "10", r: "6.5" }],
    ["circle", { cx: "10", cy: "10", r: "2.4", fill: "currentColor", stroke: "none" }],
  ],
  sortGroup: [
    ["rect", { x: "3.5", y: "4", width: "13", height: "12", rx: "2.2" }],
    ["path", { d: "M6.5 8h7M6.5 12h4.5" }],
  ],
  sidebar: [
    ["rect", { x: "3", y: "4", width: "14", height: "12", rx: "2.2" }],
    ["path", { d: "M7.5 4v12" }],
  ],
  infinity: "M7 13a3 3 0 1 1 0-6c2.2 0 3.8 6 6 6a3 3 0 1 0 0-6c-2.2 0-3.8 6-6 6Z",
  branch: [
    ["circle", { cx: "6.5", cy: "5.5", r: "1.8" }],
    ["circle", { cx: "6.5", cy: "14.5", r: "1.8" }],
    ["circle", { cx: "13.5", cy: "7.5", r: "1.8" }],
    ["path", { d: "M6.5 7.3v5.4M13.5 9.3c0 2.5-2.2 3.2-7 3.2" }],
  ],
  compose: [
    ["path", { d: "M11.5 3.5H5.5a2 2 0 0 0-2 2v9a2 2 0 0 0 2 2h9a2 2 0 0 0 2-2v-6" }],
    ["path", { d: "M14.5 2.5a1.5 1.5 0 0 1 2.1 2.1L10 11.2 7.2 12l.8-2.8Z" }],
  ],
  send: "M10 15.5V4.5M5.5 9l4.5-4.5L14.5 9",
  stop: "M6.5 6.5h7v7h-7z",
  check: "m4.5 10.5 3.5 3.5 7.5-8",
  cross: "m5.5 5.5 9 9M14.5 5.5l-9 9",
  home: "M3.5 9.5 10 4l6.5 5.5V16a.8.8 0 0 1-.8.8h-3.4v-4.6H7.7v4.6H4.3a.8.8 0 0 1-.8-.8Z",
  gear: [
    ["circle", { cx: "10", cy: "10", r: "2.4" }],
    ["path", { d: "M10 2.8v2M10 15.2v2M2.8 10h2M15.2 10h2M4.9 4.9l1.4 1.4M13.7 13.7l1.4 1.4M4.9 15.1l1.4-1.4M13.7 6.3l1.4-1.4" }],
    ["circle", { cx: "10", cy: "10", r: "5.2" }],
  ],
  copy: [
    ["rect", { x: "7", y: "7", width: "9", height: "9", rx: "1.5" }],
    ["path", { d: "M13 7V5.5A1.5 1.5 0 0 0 11.5 4h-6A1.5 1.5 0 0 0 4 5.5v6A1.5 1.5 0 0 0 5.5 13H7" }],
  ],
  sun: [
    ["circle", { cx: "10", cy: "10", r: "3.5" }],
    ["path", { d: "M10 2.5V4M10 16v1.5M2.5 10H4M16 10h1.5M4.7 4.7l1.1 1.1M14.2 14.2l1.1 1.1M15.3 4.7l-1.1 1.1M5.8 14.2l-1.1 1.1" }],
  ],
  moon: "M15.5 12.2A5.8 5.8 0 0 1 7.8 4.5a6.2 6.2 0 1 0 7.7 7.7Z",
  monitor: [
    ["rect", { x: "3", y: "4", width: "14", height: "10", rx: "1.5" }],
    ["path", { d: "M7.5 17h5M10 14v3" }],
  ],
  file: [
    ["path", { d: "M5.5 3h6l3 3v11a1 1 0 0 1-1 1h-8a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1Z" }],
    ["path", { d: "M11.5 3v3h3" }],
  ],
  edit: [
    ["path", { d: "M13.5 3.5a1.5 1.5 0 0 1 2.1 2.1L7 14.2 4 15l.8-3Z" }],
    ["path", { d: "M12 5l2.1 2.1" }],
  ],
  terminal: [
    ["rect", { x: "3", y: "4", width: "14", height: "12", rx: "2" }],
    ["path", { d: "m6.5 8 2.5 2-2.5 2M10.5 12.5h3" }],
  ],
  search: [
    ["circle", { cx: "9", cy: "9", r: "5" }],
    ["path", { d: "m13 13 3.5 3.5" }],
  ],
  wrench: "M12.5 3.8a3.8 3.8 0 0 0-4.9 4.9L4 12.3a1.5 1.5 0 0 0 2.1 2.1l3.6-3.6a3.8 3.8 0 0 0 4.9-4.9l-2 2-1.8-.3-.3-1.8Z",
  layers: [
    ["path", { d: "M10 3.5 3.5 7 10 10.5 16.5 7Z" }],
    ["path", { d: "M3.5 10.2 10 13.7l6.5-3.5M3.5 13.4 10 16.9l6.5-3.5" }],
  ],
  alert: [
    ["circle", { cx: "10", cy: "10", r: "7" }],
    ["path", { d: "M10 6.5v4" }],
    ["circle", { cx: "10", cy: "13.3", r: "0.75", fill: "currentColor", stroke: "none" }],
  ],
  context: [
    ["path", { d: "M6.5 4H4.8A.8.8 0 0 0 4 4.8v10.4a.8.8 0 0 0 .8.8h1.7M13.5 4h1.7a.8.8 0 0 1 .8.8v10.4a.8.8 0 0 1-.8.8h-1.7M7.5 8h5M7.5 12h3" }],
  ],
  clock: [
    ["circle", { cx: "10", cy: "10", r: "7" }],
    ["path", { d: "M10 6v4.2l2.6 1.6" }],
  ],
  question: [
    ["circle", { cx: "10", cy: "10", r: "7" }],
    ["path", { d: "M7.8 7.8a2.3 2.3 0 0 1 4.4.7c0 1.4-2.2 1.8-2.2 3" }],
    ["circle", { cx: "10", cy: "14", r: "0.75", fill: "currentColor", stroke: "none" }],
  ],
  diff: [
    ["rect", { x: "3.5", y: "3.5", width: "13", height: "13", rx: "2" }],
    ["path", { d: "M10 7v4M8 9h4M8 13.5h4" }],
  ],
  cpu: [
    ["rect", { x: "6", y: "6", width: "8", height: "8", rx: "1.2" }],
    ["path", { d: "M8.5 3.5V6M11.5 3.5V6M8.5 14v2.5M11.5 14v2.5M3.5 8.5H6M3.5 11.5H6M14 8.5h2.5M14 11.5h2.5" }],
  ],
  folder: "M3 6a1.5 1.5 0 0 1 1.5-1.5h3.8l1.5 1.8h5.7A1.5 1.5 0 0 1 17 7.8v6.7a1.5 1.5 0 0 1-1.5 1.5h-11A1.5 1.5 0 0 1 3 14.5Z",
  chat: "M3.5 5.5A1.5 1.5 0 0 1 5 4h10a1.5 1.5 0 0 1 1.5 1.5v6A1.5 1.5 0 0 1 15 13H8.2L5 16v-3A1.5 1.5 0 0 1 3.5 11.5Z",
  link: "M8.5 11.5a3.5 3.5 0 0 0 5 0l2-2a3.5 3.5 0 0 0-5-5l-1 1M11.5 8.5a3.5 3.5 0 0 0-5 0l-2 2a3.5 3.5 0 0 0 5 5l1-1",
  wifiOff: "M3 3l14 14M6.5 10.5a5 5 0 0 1 3.2-1.4M13.5 10.5a5 5 0 0 1 1 .9M4 8a8.5 8.5 0 0 1 4.5-2.1M11.8 5.9A8.5 8.5 0 0 1 16 8M10 14.5h.01",
};

export function svg(className, viewBox, shapes) {
  const root = document.createElementNS(SVG, "svg");
  root.setAttribute("viewBox", viewBox);
  root.setAttribute("class", className);
  root.setAttribute("aria-hidden", "true");
  for (const [tag, attrs] of shapes) {
    const node = document.createElementNS(SVG, tag);
    for (const [key, value] of Object.entries(attrs)) node.setAttribute(key, value);
    root.append(node);
  }
  return root;
}

export function icon(name) {
  const spec = ICONS[name];
  return svg(`icon icon-${name}`, "0 0 20 20", typeof spec === "string" ? [["path", { d: spec }]] : spec);
}

/** Replaces the children of `parent` only when `signature` is new. */
export function redraw(parent, signature, draw) {
  if (parent.drawn === signature) return;
  parent.drawn = signature;
  parent.replaceChildren(...draw().flat().filter(Boolean));
}
