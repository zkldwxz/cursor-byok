//! Pairs this device and keeps its live session with the computer.
import { deviceName } from "./model.js";

const TOKEN = "cursor-byok-remote-token";
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

export class Link {
  #socket = null;
  #waiting = [];
  #probe = null;
  #on;

  /** `on` hears open(), closed(), unpaired(), and message(data). */
  constructor(on) {
    this.#on = on;
    // A phone that slept may hold a connection that died unnoticed.
    document.addEventListener("visibilitychange", () => document.hidden || this.#check());
    addEventListener("online", () => this.#check());
  }

  connect() {
    const address = new URL("ws", location.href);
    address.protocol = address.protocol.replace("http", "ws");
    const socket = new WebSocket(address);
    this.#socket = socket;
    socket.onopen = () => {
      socket.send(JSON.stringify({ token: localStorage.getItem(TOKEN) }));
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
    if (message.type === "unpaired") {
      localStorage.removeItem(TOKEN);
      this.#socket = null;
      this.#on.unpaired();
    } else if (["done", "refused", "menu"].includes(message.type)) this.#waiting.shift()?.(message);
    else this.#on.message(message);
  }

  #drop() {
    clearTimeout(this.#probe);
    this.#socket = null;
    for (const resolve of this.#waiting.splice(0)) resolve({ type: "refused", code: "offline" });
    this.#on.closed();
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
