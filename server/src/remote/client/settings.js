//! Settings: how the page looks and feels, and the computer it is connected to.
import { copy } from "./copy.js";
import { h, icon, redraw } from "./dom.js";
import { feels, vibrates } from "./feel.js";
import { native, route, version } from "./native.js";
import { segments } from "./segments.js";

export class Settings {
  #look;
  #computer = h("div", { class: "card" });
  #feel = h("div", { class: "card" });
  #about = h("div", { class: "card" });
  #confirming = false;
  #actions;
  #shown = null;

  /** `actions`: onTheme(theme), onFeel(name, on), onPair(), onLeave(). */
  constructor(root, actions) {
    this.#actions = actions;
    this.#look = segments(copy.appearance, ["auto", "light", "dark"].map((theme, index) => [theme, copy.themeOption[theme], ["monitor", "sun", "moon"][index]]), actions.onTheme);
    const group = (title, ...content) => h("section", { class: "settings-group" }, h("h2", {}, title), ...content);
    root.replaceChildren(group(copy.appearance, this.#look), group(copy.feel.title, this.#feel), group(copy.computer, this.#computer), ...(native ? [group(copy.about, this.#about)] : []));
  }

  async draw(state) {
    this.#shown = state;
    this.#look.show(state.theme);
    this.#drawFeel();
    this.#drawComputer(state, native && state.online ? await route() : null);
  }

  opened() {
    this.#confirming = false;
    if (native) version().then((text) => this.#about.replaceChildren(row(copy.version, text)));
  }

  #drawFeel() {
    const names = ["haptics", "sound"].filter((name) => name !== "haptics" || vibrates());
    redraw(this.#feel, JSON.stringify(names.map(feels)), () =>
      names.map((name) =>
        h("button", { type: "button", class: "card-row", role: "switch", "aria-checked": String(feels(name)), onclick: () => this.#actions.onFeel(name, !feels(name)) }, h("span", { class: "switch-words" }, h("span", {}, copy.feel[name]), h("small", {}, copy.feel.notes[name])), h("span", { class: "switch", "aria-hidden": "true" })),
      ),
    );
  }

  #drawComputer({ online, overview }, way) {
    const reachable = online && Boolean(overview?.reachable);
    redraw(this.#computer, JSON.stringify([online, reachable, way, this.#confirming]), () => [
      row(copy.connection, online ? copy.connected : copy.offline, online ? "online" : "offline"),
      way ? row(copy.route, copy.routes[way]) : null,
      row("Cursor", reachable ? copy.connected : copy.notConnected, reachable ? "online" : "offline"),
      native ? h("button", { type: "button", class: "card-row", onclick: this.#actions.onPair }, icon("link"), h("span", {}, copy.pairAgain)) : null,
      h("button", { type: "button", class: "card-row danger", onclick: () => this.#leave() }, icon("cross"), h("span", {}, this.#confirming ? copy.leaveSure : copy.leave)),
    ]);
  }

  /** Asked twice: pairing again needs the computer. */
  #leave() {
    if (this.#confirming) return this.#actions.onLeave();
    this.#confirming = true;
    this.draw(this.#shown);
  }
}

const row = (name, value, state) => h("div", { class: "card-row" }, h("span", {}, name), h("span", { class: "card-value" }, state ? h("span", { class: "state", "data-state": state }) : null, value));
