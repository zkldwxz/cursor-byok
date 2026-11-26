//! Pairs this device and keeps its live session with the computer.
import { ticketOf } from "./computers.js";
import { deviceName } from "./model.js";
import { AppSocket, native } from "./native.js";

const RETRY_MS = 2_000;
/** A computer that reads a long conversation answers late; one that is gone, never. */
const REPLY_MS = 12_000;
const SLOW_MS = 2_500;
/** Silence for this long is asked about. */
const QUIET_MS = 10_000;
/** A phone that changes networks is back sooner than this. */
const AWAY_MS = 15_000;

/** Exchanges the code in the link, if any; returns whether this device may connect. */
export async function pair(computers) {
  const code = location.hash.slice(1);
  if (code) {
    history.replaceState(null, "", location.pathname + location.search);
    const reply = await fetch("api/pair", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ code, name: deviceName(navigator.userAgent) }),
    }).catch(() => null);
    if (reply?.ok) computers.add({ id: location.host, link: null, token: (await reply.json()).token });
    else if (!computers.current) return "expired";
  }
  return computers.current ? "paired" : "unpaired";
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
  #late = null;
  #quiet = null;
  #slow = false;
  #on;
  #computers;
  /** The pairing link the app is trying. */
  #pending = null;
  #retry = null;
  #away = null;
  #awayMs;

  /** `on` hears open(), closed(), away(), unpaired(), expired(), slow(whether), and message(data). */
  constructor(computers, on, { awayMs = AWAY_MS } = {}) {
    this.#computers = computers;
    this.#on = on;
    this.#awayMs = awayMs;
    // A phone that slept may hold a connection that died unnoticed.
    document.addEventListener("visibilitychange", () => document.hidden || this.#check());
    addEventListener("online", () => this.#check());
  }

  /** Pairs the app with the computer of this link; returns whether it is one. */
  pair(ticket) {
    if (!ticketOf(ticket)?.code) return false;
    this.#pending = ticket;
    this.#found();
    this.connect();
    return true;
  }

  connect() {
    clearTimeout(this.#retry);
    this.#still();
    this.#refuse();
    const replaced = this.#socket;
    const socket = native ? new AppSocket(this.#pending ?? this.#computers.current.link) : new WebSocket(address());
    this.#socket = socket;
    replaced?.close();
    socket.onopen = () => {
      if (this.#socket !== socket) return;
      socket.send(JSON.stringify(this.#pending ? { code: ticketOf(this.#pending).code, name: deviceName(navigator.userAgent) } : { token: this.#computers.current.token }));
      this.#heard();
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
    this.#heard();
    this.#found();
    if (message.type === "paired") {
      this.#computers.add({ id: ticketOf(this.#pending).key, link: this.#pending, token: message.token });
      this.#pending = null;
    } else if (message.type === "computer") {
      this.#computers.told(message);
      this.#on.message(message);
    } else if (message.type === "unpaired") {
      this.#socket = null;
      this.#still();
      // A link that no longer works leaves the pairing there was.
      if (this.#pending) {
        this.#pending = null;
        this.#on.expired();
        return;
      }
      this.#computers.forget(this.#computers.current?.id);
      this.#on.unpaired();
    } else if (["done", "refused", "menu"].includes(message.type)) this.#waiting.shift()?.(message);
    else this.#on.message(message);
  }

  #found() {
    clearTimeout(this.#away);
    this.#away = null;
  }

  /** Turns to another of the computers. */
  choose(id) {
    this.#computers.choose(id);
    this.#pending = null;
    this.#slow = false;
    this.#found();
    this.connect();
  }

  /** Forgets the computer on screen. */
  leave() {
    const socket = this.#socket;
    this.#socket = null;
    this.#pending = null;
    clearTimeout(this.#retry);
    this.#still();
    this.#found();
    this.#computers.forget(this.#computers.current?.id);
    socket?.close();
  }

  /** What was asked of a connection that is gone. */
  #refuse() {
    for (const resolve of this.#waiting.splice(0)) resolve({ type: "refused", code: "offline" });
  }

  #still() {
    for (const timer of [this.#probe, this.#late, this.#quiet]) clearTimeout(timer);
  }

  /** The computer answers: asks again once it has been silent for a while. */
  #heard() {
    this.#still();
    this.#quiet = setTimeout(() => (document.hidden ? this.#heard() : this.#check()), QUIET_MS);
    if (this.#slow) this.#on.slow((this.#slow = false));
  }

  #drop() {
    this.#still();
    this.#slow = false;
    this.#socket = null;
    this.#refuse();
    this.#on.closed();
    this.#away ??= setTimeout(() => this.#on.away(), this.#awayMs);
    this.#retry = setTimeout(() => this.#socket ?? this.connect(), RETRY_MS);
  }

  #check() {
    if (this.#socket?.readyState !== WebSocket.OPEN) return;
    this.tell("overview");
    this.#still();
    this.#late = setTimeout(() => this.#on.slow((this.#slow = true)), SLOW_MS);
    this.#probe = setTimeout(() => {
      this.#socket?.close();
      this.#drop();
    }, REPLY_MS);
  }
}
