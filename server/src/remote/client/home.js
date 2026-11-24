//! Home: begin a conversation at once, what waits for an answer, and the latest conversations.
import { copy } from "./copy.js";
import { h, icon, redraw } from "./dom.js";
import { homeThreads } from "./model.js";
import { notice, threadRow } from "./sidebar.js";

const RECENT = 3;

/** Where the message from home goes. */
export function drawProject(root, workspace, choose) {
  redraw(root, JSON.stringify([workspace?.id, workspace?.title]), () =>
    workspace ? [h("button", { type: "button", class: "chip chip-project", "aria-label": `${copy.project}: ${workspace.title}`, onclick: choose }, icon("folder"), h("span", {}, workspace.title), icon("down"))] : [],
  );
}

export function drawHome(root, { overview, threads }, actions) {
  const { waiting, recent } = homeThreads(threads, RECENT);
  redraw(root, JSON.stringify([overview?.reachable, overview?.workspaces.length, waiting, recent], (key, value) => (key === "current" ? undefined : value)), () => {
    if (!overview) return [h("div", { class: "skeleton", "aria-label": copy.connecting }, h("i"), h("i"), h("i"))];
    if (!overview.reachable) return [notice(copy.unreachable, "wifiOff")];
    if (!overview.workspaces.length) return [notice(copy.noWorkspaces, "folder")];
    return [section(copy.state.waiting, waiting, actions), section(copy.recent, recent, actions, h("button", { type: "button", class: "more", onclick: actions.onAll }, copy.all, icon("right")))];
  });
}

const section = (title, threads, actions, more = null) =>
  threads.length
    ? h("section", { class: "home-section" }, h("header", {}, h("h2", {}, title), more), h("nav", { class: "thread-list", "aria-label": title }, threads.map((item) => threadRow(item, "home", actions))))
    : null;
