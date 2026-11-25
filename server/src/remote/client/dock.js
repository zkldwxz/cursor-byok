//! Below the transcript, in thumb's reach: what Cursor waits for, its mode and model, and the message box.
import { logo } from "./brands.js";
import { copy } from "./copy.js";
import { h, icon, redraw } from "./dom.js";
import { answersOf, toggleOption } from "./model.js";

const ASKED_ICONS = { command: "terminal", write: "edit", delete: "edit", read: "file", mcp: "wrench", web_search: "search", web_fetch: "search", plan: "layers" };

// Browsers break lines inside "--"; a command must read as it will run.
const words = (text) => text.split(/(\s+)/).map((part) => (part.trim() ? h("span", { class: "word" }, part) : part));

/** What is chosen and typed so far, by what asked; Cursor hears of it only when it is submitted. */
const given = new Map();

/** Shows what is asked, oldest first, or nothing. `run` carries out a command. */
export function drawAsked(root, chat, run) {
  const asked = chat?.asked ?? [];
  redraw(root, JSON.stringify(asked), () => {
    for (const id of given.keys()) if (!asked.some((item) => item.id === id)) given.delete(id);
    if (!asked.length) return [];
    const [first, ...rest] = asked;
    return [
      ...(first.kind === "questions" ? questions(first, run) : approval(first, run)),
      rest.length ? h("p", { class: "asked-more" }, copy.more(rest.length)) : null,
    ];
  });
  root.hidden = !root.childElementCount;
}

/** Buttons that answer once: they come back only if the answer was not taken. */
function answering(id, run) {
  const buttons = [];
  const button = (label, kind, verdict) => {
    const element = h("button", { type: "button", class: kind, onclick: async () => {
      for (const other of buttons) other.disabled = true;
      const reply = await run({ type: "answer", id, verdict: verdict() });
      if (reply.type === "refused") for (const other of buttons) other.disabled = false;
    } }, label);
    buttons.push(element);
    return element;
  };
  return button;
}

function approval({ id, kind, subject, why }, run) {
  const button = answering(id, run);
  const text = kind === "switch_mode" ? (copy.modes[subject] ?? subject) : subject;
  return [
    h("p", { class: "asked-title", "data-tone": "caution" }, icon(ASKED_ICONS[kind] ?? "alert"), copy.asked[kind] ?? copy.asked.unknown),
    text ? h("pre", { class: "asked-subject", tabindex: "0" }, words(text)) : null,
    why ? h("p", { class: "asked-why" }, why) : null,
    kind === "unknown" ? null : h("div", { class: "asked-buttons" }, button(copy.approve, "go", () => "approve"), button(copy.reject, "quiet", () => "reject")),
  ];
}

function questions({ id, list }, run) {
  const answers = given.get(id) ?? {};
  given.set(id, answers);
  const button = answering(id, run);
  const submit = button(copy.answer, "go", () => ({ answers: answersOf(list, answers) }));
  const ready = () => (submit.disabled = !answersOf(list, answers));
  const question = ({ id: question, text, multiple, options }) => {
    const answer = (answers[question] ??= { options: [], text: "" });
    const show = () => {
      elements.forEach((element, index) => element.setAttribute("aria-pressed", String(answer.options.includes(options[index].id))));
      ready();
    };
    const elements = options.map((option, index) =>
      h("button", { type: "button", class: "option", onclick: () => ((answer.options = toggleOption(answer.options, option.id, multiple)), show()) }, h("span", { class: "box", "aria-hidden": "true" }, String(index + 1)), option.label),
    );
    const field = h("textarea", { class: "freeform", rows: "1", placeholder: copy.freeform, "aria-label": copy.freeform, oninput: () => ((answer.text = field.value), ready()) });
    field.value = answer.text;
    show();
    return h("fieldset", { class: "question" }, h("legend", {}, text), elements, field);
  };
  return [
    h("p", { class: "asked-title", "data-tone": "accent" }, icon("question"), copy.question),
    list.map(question),
    h("div", { class: "asked-buttons" }, submit, button(copy.skip, "quiet", () => "reject")),
  ];
}

/** Why the last turn ended early. */
export function drawPanel(root, chat) {
  const error = chat?.error ?? null;
  redraw(root, JSON.stringify(error), () => (error === null ? [] : [h("div", { class: "turn-error", role: "status" }, icon("alert"), h("span", {}, error || copy.failedTurn))]));
  root.hidden = !root.childElementCount;
}

const label = (chat, kind) => (kind === "mode" ? (copy.modes[chat?.mode] ?? chat?.mode) : chat?.model);

export function drawChips(root, chat, menu) {
  redraw(root, JSON.stringify([chat?.mode, chat?.model, chat?.modelName]), () =>
    ["mode", "model"].map((kind) =>
      label(chat, kind)
        ? h(
            "button",
            {
              type: "button",
              class: `chip chip-${kind}`,
              "aria-label": `${copy[kind]}: ${label(chat, kind)}`,
              onclick: () => menu(kind),
            },
            kind === "mode" ? icon("infinity") : logo(chat.modelName, chat.model),
            h("span", {}, label(chat, kind)),
            icon("down"),
          )
        : null,
    ),
  );
}

/** In place of the message box, where Cursor lets nothing be sent. */
export function drawReadOnly(root, chat) {
  redraw(root, JSON.stringify([chat?.readOnly, chat?.model, chat?.modelName]), () => (chat?.readOnly ? [h("span", {}, copy.readOnly), chat.model ? h("span", { class: "chip chip-model" }, logo(chat.modelName, chat.model), h("span", {}, chat.model)) : null] : []));
  root.hidden = !root.childElementCount;
}

/** The message box; it empties at once, and gets its text back if Cursor does not take it. */
export class Composer {
  #drafts = new Map();
  #place = null;
  #running = false;
  #busy = false;
  #send;
  #stop;

  constructor(form, send, stop) {
    this.#send = send;
    this.#stop = stop;
    this.field = form.querySelector("textarea");
    this.button = form.querySelector("button[type=submit]");
    this.field.setAttribute("aria-label", copy.message);
    form.addEventListener("submit", (event) => {
      event.preventDefault();
      this.#submit();
    });
    this.field.addEventListener("input", () => this.#show());
    this.field.addEventListener("keydown", (event) => {
      // A phone's keyboard has no other way to start a new line.
      const enter = event.key === "Enter" && !event.shiftKey && !event.isComposing;
      if (enter && matchMedia("(pointer: fine)").matches) {
        event.preventDefault();
        this.#submit();
      }
    });
  }

  update(place, running) {
    if (place !== this.#place) {
      this.#drafts.set(this.#place, this.field.value);
      this.#place = place;
      this.field.value = this.#drafts.get(place) ?? "";
    }
    this.#running = running;
    this.#show();
  }

  #stops = () => this.#running && !this.field.value.trim();

  #show() {
    const stops = this.#stops();
    this.field.placeholder = this.#running ? copy.queueMessage : copy.message;
    this.button.disabled = this.#busy || (!stops && !this.field.value.trim());
    this.button.dataset.action = stops ? "stop" : "send";
    this.button.setAttribute("aria-label", stops ? copy.stop : copy.send);
    redraw(this.button, String(stops), () => [icon(stops ? "stop" : "send")]);
    this.field.style.height = "auto";
    this.field.style.height = `${this.field.scrollHeight + 2}px`;
  }

  async #submit() {
    if (this.#busy) return;
    const stops = this.#stops();
    const text = this.field.value;
    if (!stops && !text.trim()) return;
    const place = this.#place;
    this.#busy = true;
    if (!stops) this.field.value = "";
    this.#show();
    const sent = stops ? (await this.#stop(), { taken: true }) : await this.#send(text);
    this.#busy = false;
    if (!sent.taken && !sent.here) this.#drafts.set(place, text);
    else if (!sent.taken && !this.field.value) this.field.value = text;
    this.#show();
  }
}
