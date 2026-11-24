//! The transcript: prose, steps, to-dos, and what was asked.
import { copy } from "./copy.js";
import { h, icon } from "./dom.js";
import { foldSteps, groupRows, stepKind } from "./model.js";

const runs = (list) =>
  list.map((run) => {
    const tag = { code: "code", strong: "strong", em: "em" }[run.style];
    return tag ? h(tag, {}, run.text) : run.text;
  });

const codeBlock = (text) => {
  const button = h("button", { type: "button", class: "copy" }, icon("copy"), copy.copy);
  button.addEventListener("click", async () => {
    try {
      await navigator.clipboard.writeText(text);
    } catch {
      return;
    }
    button.replaceChildren(icon("check"), copy.copied);
    setTimeout(() => button.replaceChildren(icon("copy"), copy.copy), 1_500);
  });
  return h("div", { class: "code" }, h("pre", { tabindex: "0" }, text), button);
};

const block = (item) => {
  switch (item.kind) {
    case "code":
      return codeBlock(item.text);
    case "heading":
      return h("h2", {}, runs(item.runs));
    case "quote":
      return h("blockquote", {}, runs(item.runs));
    case "item":
      return h("div", { class: "item", "data-depth": String(Math.min(item.depth, 3)) }, h("span", { class: "marker", "aria-hidden": "true" }, item.marker), h("div", {}, runs(item.runs)));
    case "table": {
      const [head, ...body] = item.rows;
      const cells = (row, tag) => h("tr", {}, row.map((cell) => h(tag, {}, cell)));
      return h("div", { class: "table", tabindex: "0" }, h("table", {}, h("thead", {}, cells(head ?? [], "th")), h("tbody", {}, body.map((row) => cells(row, "td")))));
    }
    default:
      return h("p", {}, runs(item.runs));
  }
};

const stepState = (row) => {
  return { error: "failed", loading: "running" }[row.status] ?? "done";
};

const stepVerb = (tool) => {
  if (tool.kind === "edit" && tool.title !== "Delete") return copy.edited;
  return tool.kind === "run" ? copy.ran : null;
};

const thought = (row) => (row.status === "loading" || row.seconds === undefined ? copy.thinking : copy.thought(copy.duration(row.seconds)));

const KIND_ICONS = {
  edit: "edit",
  run: "terminal",
  thinking: "sparkles",
  read: "file",
  search: "search",
  tool: "wrench",
};

const stepIcon = (name) => {
  const element = icon(name);
  element.classList.add("step-icon");
  return element;
};

export class Transcript {
  #drawn = new Map();
  #open = new Set();

  constructor(root) {
    this.root = root;
  }

  forget() {
    this.#drawn.clear();
    this.#open.clear();
    this.root.replaceChildren();
  }

  /** Groups that read the same keep their elements, so selections and scroll stay. */
  draw(rows) {
    const groups = groupRows(rows);
    const gone = (key) => this.#drawn.has(key) && !groups.some((group) => group.key === key);
    // What takes the place of a message or a wait shown early is not new to the eye.
    const replaces = { human: gone("sending"), steps: gone("thinking"), assistant: gone("thinking") };
    const live = this.#drawn.size > 0;
    const next = new Map();
    for (const group of groups) {
      const signature = JSON.stringify(group);
      const last = this.#drawn.get(group.key);
      if (last?.signature === signature) {
        next.set(group.key, last);
        continue;
      }
      const element = last && group.kind === "assistant" ? this.#prose(last.element, group.rows[0]) : this.#group(group);
      if (live && !last && !replaces[group.kind]) element.classList.add("enter");
      next.set(group.key, { signature, element });
    }
    this.#drawn = next;
    const wanted = [...next.values()].map((item) => item.element);
    wanted.forEach((element, index) => {
      if (this.root.children[index] !== element) this.root.insertBefore(element, this.root.children[index] ?? null);
    });
    while (this.root.children.length > wanted.length) this.root.lastElementChild.remove();
  }

  #group({ kind, rows }) {
    const [row] = rows;
    if (kind === "human") return h("div", { class: "human", "data-status": row.status }, row.text);
    if (kind === "assistant") return this.#prose(h("div", { class: "prose" }), row);
    if (kind === "todos") return this.#todos(row);
    return h("ol", { class: "steps" }, foldSteps(rows).map((item) => (item.tally ? this.#fold(item) : this.#step(item.rows[0]))));
  }

  /** Only the blocks that changed are drawn again: usually the last, while it is written. */
  #prose(element, row) {
    const blocks = row.blocks ?? [];
    blocks.forEach((item, index) => {
      const signature = JSON.stringify(item);
      const old = element.children[index];
      if (old?.drawn === signature) return;
      const drawn = block(item);
      drawn.drawn = signature;
      if (old) old.replaceWith(drawn);
      else element.append(drawn);
    });
    while (element.children.length > blocks.length) element.lastElementChild.remove();
    return element;
  }

  #details(key, line, body) {
    const toggle = ({ target }) => (target.open ? this.#open.add(key) : this.#open.delete(key));
    return h("details", { open: this.#open.has(key), ontoggle: toggle }, h("summary", { class: "step-line" }, line), body);
  }

  #fold({ key, rows, tally }) {
    const counted = ["read", "searched", "ran", "used"].filter((name) => tally[name]);
    const phrase = (name, index) => [index ? copy.tally.separator : "", copy.tally[name][0], h("b", {}, String(tally[name])), copy.tally[name][1]];
    const line = [
      stepIcon("layers"),
      h("span", { class: "step-detail" }, counted.flatMap(phrase), tally.seconds ? h("span", { class: "faint" }, counted.length ? " · " : "", copy.tally.thought(tally.seconds)) : null),
      icon("right"),
    ];
    return h("li", { class: "step" }, this.#details(`fold ${key}`, line, h("ol", { class: "steps folded" }, rows.map((row) => this.#step(row)))));
  }

  #step(row) {
    const tool = row.tool ?? {};
    const state = stepState(row);
    const kind = stepKind(row);
    const verb = stepVerb(tool);
    const lead = state === "failed" ? "alert" : KIND_ICONS[kind];
    const title = kind === "thinking" ? thought(row) : (copy.tools[tool.title] ?? tool.title ?? "");
    const line = (more) => [
      stepIcon(lead),
      verb ? h("span", { class: "step-verb" }, verb) : null,
      h("span", { class: verb ? "step-detail" : "step-name" }, title),
      tool.detail ? h("span", { class: "step-detail" }, tool.detail) : null,
      tool.added ? h("span", { class: "added" }, `+${tool.added}`) : null,
      tool.removed ? h("span", { class: "removed" }, `−${tool.removed}`) : null,
      state === "done" || kind === "thinking" ? null : h("span", { class: "step-state" }, copy[state]),
      more ? icon("right") : null,
    ];
    const body = [tool.command && tool.command !== tool.title ? h("pre", { class: "command" }, tool.command) : null, tool.output || row.text ? h("pre", { class: "output", tabindex: "0" }, tool.output ?? row.text) : null].filter(Boolean);
    const content = body.length ? this.#details(row.key, line(true), body) : h("div", { class: "step-line" }, line(false));
    return h("li", { class: "step", "data-state": state, "data-kind": kind }, content);
  }

  #todos(row) {
    const done = row.todos.filter((todo) => todo.state === "completed").length;
    return h(
      "section",
      { class: "todos" },
      h("h2", {}, copy.todos(done, row.todos.length)),
      h("ul", {}, row.todos.map((todo) => h("li", { "data-state": todo.state }, h("span", { class: "box" }, todo.state === "completed" ? icon("check") : todo.state === "cancelled" ? icon("cross") : null), todo.text))),
    );
  }
}
