//! What a touch gives back: a short vibration and, if wanted, a short sound made on the spot.
const DEFAULTS = { haptics: true, sound: false };
const key = (name) => `cursor-byok-remote-${name}`;

const read = (name) => {
  try {
    return { on: true, off: false }[localStorage.getItem(key(name))] ?? DEFAULTS[name];
  } catch {
    // Storage is closed to a private window.
    return DEFAULTS[name];
  }
};

const wanted = { haptics: read("haptics"), sound: read("sound") };

export const feels = (name) => wanted[name];

export function setFeel(name, on) {
  wanted[name] = on;
  try {
    localStorage.setItem(key(name), on ? "on" : "off");
  } catch {
    // Kept until the page closes.
  }
}

/** Milliseconds on and off, for browsers; the app has the phone's own effects. */
const VIBRATIONS = { tap: 8, toggle: 10, hold: 24, send: 14, confirm: [10, 40, 16], warn: [22, 60, 22], done: [12, 70, 20] };

export const vibrates = () => Boolean(globalThis.Feel ?? globalThis.navigator?.vibrate);

function vibrate(kind) {
  if (globalThis.Feel) globalThis.Feel.play(kind);
  else globalThis.navigator?.vibrate?.(VIBRATIONS[kind]);
}

/** Tones that slide: from and to (Hz), start and length (ms), loudness, wave. */
const SOUNDS = {
  tap: [[1900, 1100, 0, 28, 0.05, "triangle"]],
  toggle: [[900, 1300, 0, 34, 0.05, "triangle"], [1500, 1700, 38, 30, 0.035, "sine"]],
  hold: [[220, 140, 0, 90, 0.09, "sine"]],
  send: [[380, 1150, 0, 120, 0.06, "sine"], [760, 2100, 30, 90, 0.02, "triangle"]],
  confirm: [[660, 660, 0, 90, 0.05, "sine"], [990, 990, 70, 140, 0.05, "sine"]],
  warn: [[330, 300, 0, 90, 0.06, "triangle"], [247, 220, 90, 130, 0.06, "triangle"]],
  done: [[784, 784, 0, 120, 0.045, "sine"], [1175, 1175, 90, 220, 0.045, "sine"]],
};

let audio;

function sound(kind) {
  audio ??= new AudioContext({ latencyHint: "interactive" });
  // Browsers keep it silent until a touch.
  if (audio.state === "suspended") audio.resume();
  for (const [from, to, start, length, loudness, wave] of SOUNDS[kind]) {
    const at = audio.currentTime + start / 1_000;
    const end = at + length / 1_000;
    const tone = new OscillatorNode(audio, { type: wave, frequency: from });
    const volume = new GainNode(audio, { gain: 0 });
    tone.frequency.setValueAtTime(from, at);
    tone.frequency.exponentialRampToValueAtTime(to, end);
    volume.gain.setValueAtTime(0.0001, at);
    volume.gain.exponentialRampToValueAtTime(loudness, at + 0.004);
    volume.gain.exponentialRampToValueAtTime(0.0001, end);
    tone.connect(volume).connect(audio.destination);
    tone.start(at);
    tone.stop(end);
  }
}

/** `kind`: tap, toggle, hold, send, confirm, warn or done. */
export function feel(kind) {
  if (wanted.haptics) vibrate(kind);
  if (wanted.sound) sound(kind);
}

/** The first that matches says how a touch on an element feels. */
const TOUCHES = [
  [".composer button[type=submit]", "send"],
  [".go", "confirm"],
  [".quiet, .danger", "warn"],
  [".segments button, [role=switch], #tabs button, .folder-toggle, .option, .thread-pin, summary", "toggle"],
  ["button", "tap"],
];

/** Every control answers, wherever it is on the page. */
export function feelTouches(root) {
  root.addEventListener("click", ({ target }) => {
    const control = target.closest?.("button, summary");
    if (control && !control.disabled) feel(TOUCHES.find(([selector]) => control.matches(selector))[1]);
  });
}
