//! The computers this device is paired with, and the one on screen.
const KEY = "cursor-byok-remote-computers";

/** What an app's pairing link holds: the computer's key, a one-time code, and its relay; null when it is not one. */
export function ticketOf(link) {
  const data = /^cursor-byok:\/\/pair\?d=([\w-]+)$/.exec(link?.trim() ?? "")?.[1];
  try {
    const { k: key, c: code, r: relay } = JSON.parse(atob(data.replaceAll("-", "+").replaceAll("_", "/")));
    return key ? { key, code: code || null, relay: Boolean(relay) } : null;
  } catch {
    return null;
  }
}

/** Each is `{ id, name, link, token }`; in a browser there is one, without a link. */
export class Computers {
  #storage;
  #kept;

  constructor(storage) {
    this.#storage = storage;
    this.#kept = JSON.parse(storage.getItem(KEY) ?? '{"current":null,"list":[]}');
  }

  get list() {
    return this.#kept.list;
  }

  get current() {
    return this.list.find(({ id }) => id === this.#kept.current) ?? null;
  }

  /** Paired again, it takes the place of what was known of it. */
  add({ id, link, token }) {
    const name = this.list.find((known) => known.id === id)?.name ?? null;
    this.#keep({ current: id, list: [...this.list.filter((known) => known.id !== id), { id, name, link, token }] });
  }

  /** What the computer on screen says of itself: where it is now may have changed. */
  told({ name, link }) {
    const { current } = this;
    if (current) this.#keep({ ...this.#kept, list: this.list.map((known) => (known === current ? { ...known, name, link: link ?? known.link } : known)) });
  }

  choose(id) {
    this.#keep({ ...this.#kept, current: id });
  }

  forget(id) {
    const list = this.list.filter((known) => known.id !== id);
    this.#keep({ current: this.#kept.current === id ? (list[0]?.id ?? null) : this.#kept.current, list });
  }

  #keep(kept) {
    this.#kept = kept;
    this.#storage.setItem(KEY, JSON.stringify(kept));
  }

  /** One per browser profile or app install, made once and kept: a re-pairing is the same device again, not another entry on the computer. */
  get deviceId() {
    let id = this.#storage.getItem("cursor-byok-device-id");
    if (!id) {
      id = crypto.randomUUID();
      this.#storage.setItem("cursor-byok-device-id", id);
    }
    return id;
  }
}
