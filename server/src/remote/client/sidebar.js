//! The sidebar: new chat, conversations grouped by project, date or status, pins, and the connection.
import { chinese, copy } from "./copy.js";
import { h, icon, redraw } from "./dom.js";
import { buildThreads, chatState, formatAge, groupThreads, waitingCount } from "./model.js";

const THEME_ICONS = { auto: "monitor", dark: "moon", light: "sun" };
const GROUP_OPTIONS = [
  ["project", "folder"],
  ["date", "clock"],
  ["status", "status"],
];

export const notice = ([title, body], iconName) =>
  h(
    "div",
    { class: "notice" },
    iconName ? h("div", { class: "notice-icon" }, icon(iconName)) : null,
    h("strong", {}, title),
    body ? h("p", {}, body) : null,
  );

function groupPopover(groupBy, onGroupBy) {
  return h(
    "div",
    { class: "popover popover-group", role: "menu" },
    GROUP_OPTIONS.map(([key, iconName]) =>
      h(
        "button",
        {
          type: "button",
          class: "popover-item",
          role: "menuitemradio",
          "aria-checked": String(groupBy === key),
          onclick: () => onGroupBy(key),
        },
        icon(iconName),
        h("span", { class: "popover-label" }, copy.groupOption[key]),
        groupBy === key ? icon("check") : null,
      ),
    ),
  );
}

function groupHeader(group, collapsed, { onToggleCollapse, onNewChat }) {
  const chevron = icon(collapsed ? "right" : "down");
  chevron.classList.add("group-chevron");
  let lead;
  let extra = null;
  let countText = collapsed ? String(group.items.length) : null;

  if (group.kind === "pinned") {
    lead = [chevron, h("span", { class: "group-label" }, copy.pinned)];
  } else if (group.kind === "project") {
    lead = [
      chevron,
      icon("folder"),
      h("span", { class: "project-name" }, group.title),
    ];
    extra = h(
      "button",
      {
        type: "button",
        class: "project-add",
        title: copy.newChat,
        "aria-label": copy.newChat,
        onclick: (event) => {
          event.stopPropagation();
          onNewChat(group.workspace);
        },
      },
      icon("plus"),
    );
  } else {
    const label = group.kind === "date" ? copy.dateBucket[group.bucket] : copy.statusBucket[group.bucket];
    lead = [chevron, h("span", { class: "group-label" }, label)];
    if (group.kind === "status" && !collapsed) countText = String(group.items.length);
  }

  return h(
    "div",
    { class: "side-group-head" },
    h(
      "button",
      {
        type: "button",
        class: "side-group-toggle",
        "aria-expanded": String(!collapsed),
        onclick: () => onToggleCollapse(group.id),
      },
      ...lead,
    ),
    extra,
    countText ? h("span", { class: "side-count" }, countText) : null,
  );
}

function threadRow(item, groupKind, { onSelectThread, onTogglePin }) {
  const showState = item.state === "waiting" || item.state === "running";
  const showMeta = groupKind !== "project";
  return h(
    "div",
    {
      class: "thread-row",
      "data-indented": groupKind === "project" ? "true" : undefined,
      "data-pinned": item.pinned ? "true" : undefined,
      "data-state": item.state,
      "aria-current": item.current ? "true" : undefined,
    },
    h(
      "button",
      {
        type: "button",
        class: "thread-main",
        "data-thread-key": item.id,
        onclick: () => onSelectThread(item),
      },
      h(
        "span",
        { class: "thread-lead" },
        showState ? h("span", { class: "state", "data-state": item.state, role: "img", "aria-label": copy.state[item.state] }) : null,
      ),
      h("span", { class: "thread-title" }, item.label || copy.untitled),
      showMeta
        ? h(
            "span",
            { class: "thread-meta" },
            h("span", { class: "thread-project" }, item.project),
          )
        : null,
      h("span", { class: "thread-time" }, formatAge(item.ageMinutes, chinese)),
    ),
    h(
      "button",
      {
        type: "button",
        class: "thread-pin",
        "aria-pressed": String(item.pinned),
        "aria-label": item.pinned ? copy.unpin : copy.pin,
        title: item.pinned ? copy.unpin : copy.pin,
        onclick: (event) => {
          event.stopPropagation();
          onTogglePin(item.id);
        },
      },
      icon("pin"),
    ),
  );
}

export function drawSidebar(
  root,
  { overview, online, current, chat, theme, groupBy, openMenu, pinned, collapsed: folded },
  actions,
) {
  const { onSelectThread, onNewChat, onTheme, onGroupBy, onToggleMenu, onToggleCollapse, onTogglePin } = actions;
  const workspaces = overview?.workspaces ?? [];
  const waiting = waitingCount(workspaces);
  document.title = waiting ? `(${waiting}) Cursor` : "Cursor";

  const now = Date.now();
  const signature = JSON.stringify([overview, online, current, chat ? chatState(chat) : null, theme, groupBy, openMenu, [...pinned], [...folded], Math.floor(now / 60_000)]);

  redraw(root, signature, () => {
    const active = document.activeElement;
    const focusedThread = root.contains(active) ? active.dataset.threadKey : null;
    const focusedTheme = root.contains(active) && active.classList.contains("sidebar-theme");
    queueMicrotask(() => {
      if (focusedThread) root.querySelector(`[data-thread-key="${CSS.escape(focusedThread)}"]`)?.focus();
      else if (focusedTheme) root.querySelector(".sidebar-theme")?.focus();
    });

    const status = online ? (waiting ? copy.waitingTitle(waiting) : copy.nothingWaiting) : overview ? copy.offline : copy.connecting;
    const themeLabel = copy.theme[theme] ?? copy.theme.auto;

    const toolbar = workspaces.length
      ? h(
          "div",
          { class: "side-toolbar" },
          h(
            "div",
            { class: "popover-wrap popover-wrap-right" },
            h(
              "button",
              {
                type: "button",
                class: "toolbar-btn",
                "aria-expanded": String(openMenu === "group"),
                onclick: () => onToggleMenu("group"),
              },
              icon("sortGroup"),
              h("span", {}, copy.groupBy[groupBy]),
              icon("down"),
            ),
            openMenu === "group" ? groupPopover(groupBy, onGroupBy) : null,
          ),
        )
      : null;

    const top = h(
      "div",
      { class: "side-top" },
      h(
        "header",
        { class: "sidebar-head mobile-only" },
        h(
          "div",
          { class: "sidebar-title" },
          h("span", { class: "state", "data-state": online ? (waiting ? "waiting" : "online") : "offline" }),
          h("h1", {}, copy.title),
        ),
        h(
          "button",
          { type: "button", class: "plain sidebar-theme", "aria-label": themeLabel, title: themeLabel, onclick: onTheme },
          icon(THEME_ICONS[theme] ?? "monitor"),
        ),
      ),
      h(
        "button",
        { type: "button", class: "side-new", disabled: !workspaces.length || !online, onclick: () => onNewChat() },
        icon("plus"),
        h("span", {}, copy.newChat),
      ),
      toolbar,
    );

    const bodyItems = [];
    if (!overview) {
      bodyItems.push(h("p", { class: "sidebar-status", role: "status" }, status));
    } else if (!overview.reachable) {
      bodyItems.push(notice(copy.unreachable, "wifiOff"));
    } else if (!workspaces.length) {
      bodyItems.push(notice(copy.noWorkspaces, "folder"));
    } else {
      const threads = buildThreads(workspaces, { current, open: chat, pinned, now });
      for (const group of groupThreads(workspaces, threads, groupBy, now)) {
        const collapsed = folded.has(group.id);
        bodyItems.push(
          h(
            "section",
            { class: "side-group", "data-kind": group.kind, "data-collapsed": String(collapsed) },
            groupHeader(group, collapsed, { onToggleCollapse, onNewChat }),
            collapsed
              ? null
              : h(
                  "nav",
                  { class: "thread-list", "aria-label": group.title ?? group.id },
                  group.items.map((item) => threadRow(item, group.kind, { onSelectThread, onTogglePin })),
                ),
          ),
        );
      }
    }

    const body = h("div", { class: "side-body" }, bodyItems);

    const foot = h(
      "footer",
      { class: "side-foot" },
      h(
        "div",
        { class: "side-status", role: "status" },
        h("span", { class: "state", "data-state": online ? (waiting ? "waiting" : "online") : "offline" }),
        h("span", { class: "side-status-text" }, status),
      ),
      h(
        "button",
        { type: "button", class: "plain sidebar-theme", "aria-label": themeLabel, title: themeLabel, onclick: onTheme },
        icon(THEME_ICONS[theme] ?? "monitor"),
      ),
    );

    return [top, body, foot];
  });
}
