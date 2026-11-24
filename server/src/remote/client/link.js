//! Pairs this device and keeps its live session with the computer.
import { codeOf, deviceName } from "./model.js";
import { AppSocket, native } from "./native.js";

const TOKEN = "cursor-byok-remote-token";
/** Where the app finds the computer; a browser is already there. */
const COMPUTER = "cursor-byok-remote-computer";
const RETRY_MS = 2_000;
const REPLY_MS = 6_000;

/** Exchanges the code in the link, if any; returns whether this device may connect. */
export async function pair() {
  const code = location.hash.slice(1);
  if (code) {
    history.replaceState(null, "", location.pathname + location.search);
    const reply = await fetch("api/pair", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ code, name: deviceName(navigator.userAgent) }),
    }).catch(() => null);
    if (reply?.ok) localStorage.setItem(TOKEN, (await reply.json()).token);
    else if (!localStorage.getItem(TOKEN)) return "expired";
  }
  return localStorage.getItem(TOKEN) ? "paired" : "unpaired";
}

function address() {
  const url = new URL("ws", location.href);
  url.protocol = url.protocol.replace("http", "ws");
  return url;
}

export class Link {
  #socket = null;
  #waiting = [];
  #probe = null;
  #on;
  /** The pairing link the app is trying. */
  #pending = null;
  #failures = 0;

  /** `on` hears open(), closed(failures), unpaired(), expired(), and message(data). */
  constructor(on) {
    this.#on = on;
    // A phone that slept may hold a connection that died unnoticed.
    document.addEventListener("visibilitychange", () => document.hidden || this.#check());
    addEventListener("online", () => this.#check());
  }

  /** Pairs the app with the computer of this link; returns whether it is one. */
  pair(ticket) {
    if (!codeOf(ticket)) return false;
    this.#pending = ticket;
    this.#failures = 0;
    this.connect();
    return true;
  }

  connect() {
    const replaced = this.#socket;
    const socket = native ? new AppSocket(this.#pending ?? localStorage.getItem(COMPUTER)) : new WebSocket(address());
    this.#socket = socket;
    replaced?.close();
    socket.onopen = () => {
      if (this.#socket !== socket) return;
      socket.send(JSON.stringify(this.#pending ? { code: codeOf(this.#pending), name: deviceName(navigator.userAgent) } : { token: localStorage.getItem(TOKEN) }));
      this.#on.open();
    };
    socket.onmessage = (event) => this.#socket === socket && this.#hear(JSON.parse(event.data));
    socket.onclose = () => this.#socket === socket && this.#drop();
  }

  tell(request) {
    if (this.#socket?.readyState === WebSocket.OPEN) this.#socket.send(JSON.stringify(request));
  }

  /** Resolves to the reply: done, refused or a menu. The server answers in order. */
  command(command) {
    if (this.#socket?.readyState !== WebSocket.OPEN) return Promise.resolve({ type: "refused", code: "offline" });
    return new Promise((resolve) => {
      this.#waiting.push(resolve);
      this.tell({ command });
    });
  }

  #hear(message) {
    clearTimeout(this.#probe);
    this.#failures = 0;
    if (message.type === "paired") {
      localStorage.setItem(TOKEN, message.token);
      localStorage.setItem(COMPUTER, this.#pending);
      this.#pending = null;
    } else if (message.type === "unpaired") {
      this.#socket = null;
      // A link that no longer works leaves the pairing there was.
      if (this.#pending) {
        this.#pending = null;
        this.#on.expired();
        return;
      }
      localStorage.removeItem(TOKEN);
      this.#on.unpaired();
    } else if (["done", "refused", "menu"].includes(message.type)) this.#waiting.shift()?.(message);
    else this.#on.message(message);
  }

  #drop() {
    clearTimeout(this.#probe);
    this.#socket = null;
    for (const resolve of this.#waiting.splice(0)) resolve({ type: "refused", code: "offline" });
    this.#on.closed(++this.#failures);
    setTimeout(() => this.#socket ?? this.connect(), RETRY_MS);
  }

  #check() {
    if (this.#socket?.readyState !== WebSocket.OPEN) return;
    this.tell("overview");
    clearTimeout(this.#probe);
    this.#probe = setTimeout(() => {
      this.#socket?.close();
      this.#drop();
    }, REPLY_MS);
  }
}
