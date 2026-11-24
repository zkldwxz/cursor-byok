//! Reaches the computer through the app: its encrypted connection, and the camera that pairs.
const tauri = globalThis.__TAURI__;

/** Whether this page is the app's, rather than one a browser loaded from the computer. */
export const native = Boolean(tauri);

/** Resolves to what the QR code says; null when the person went back. Fails without a camera. */
export async function scan() {
  const { scan, cancel, checkPermissions, requestPermissions, Format } = tauri.barcodeScanner;
  if ((await checkPermissions()) !== "granted" && (await requestPermissions()) !== "granted") throw new Error("the camera is not allowed");
  // The camera covers the page: only the phone's own back gesture leaves it.
  history.pushState({ view: "scan" }, "");
  let left = false;
  const leaving = new Promise((resolve) => addEventListener("popstate", () => resolve((left = true)), { once: true }));
  try {
    const read = await Promise.race([scan({ formats: [Format.QRCode] }), leaving]);
    return left ? null : read.content;
  } finally {
    if (left) await cancel();
    else history.back();
  }
}

/** What WebSocket is to a browser, as far as the page uses it. */
export class AppSocket {
  readyState = WebSocket.CONNECTING;
  #id = null;

  constructor(ticket) {
    const events = new tauri.core.Channel();
    events.onmessage = (event) => this.#hear(event);
    tauri.core.invoke("dial", { ticket, events }).then(
      (id) => {
        this.#id = id;
        if (this.readyState === WebSocket.CLOSED) return tauri.core.invoke("hang_up", { id });
        this.readyState = WebSocket.OPEN;
        this.onopen?.();
      },
      () => this.#end(),
    );
  }

  send(text) {
    tauri.core.invoke("say", { id: this.#id, text });
  }

  close() {
    if (this.#id !== null) tauri.core.invoke("hang_up", { id: this.#id });
    this.#end();
  }

  #hear({ message }) {
    if (this.readyState === WebSocket.CLOSED) return;
    if (message === undefined || message === null) this.#end();
    else this.onmessage?.({ data: message });
  }

  #end() {
    if (this.readyState === WebSocket.CLOSED) return;
    this.readyState = WebSocket.CLOSED;
    this.onclose?.();
  }
}
