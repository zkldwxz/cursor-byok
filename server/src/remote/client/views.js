//! Which view is on screen, and how the phone's back leads from one to another.
const TABS = ["home", "sidebar"];

export class Views {
  #changed;

  /** `changed` hears the view that came on screen. */
  constructor(changed) {
    this.#changed = changed;
    history.replaceState({ view: "home" }, "");
    addEventListener("popstate", (event) => this.now !== "gate" && this.#show(event.state?.view ?? "home"));
  }

  get now() {
    return document.body.dataset.view;
  }

  /** Home is where back ends; every other view is one step further. */
  go(view) {
    if (view === this.now) return;
    if (view === "home" && this.now === "sidebar") return history.back();
    history.pushState({ view }, "");
    this.#show(view);
  }

  back() {
    history.back();
  }

  /** Nothing to go back to. */
  reset(view) {
    history.replaceState({ view: view === "gate" ? "home" : view }, "");
    this.#show(view);
  }

  #show(view) {
    document.body.dataset.view = view;
    // A view pushed over a tab keeps it underneath.
    if (TABS.includes(view)) document.body.dataset.tab = view;
    this.#changed(view);
  }
}
