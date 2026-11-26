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

/**
 * Tones that slide: from and to (Hz), start and length (ms), loudness, wave.
 * A phone's speaker barely gives back what is shorter than about 50 ms or quieter than a fifth.
 */
const SOUNDS = {
  tap: [[1300, 950, 0, 60, 0.22, "sine"]],
  toggle: [[880, 1100, 0, 60, 0.22, "sine"], [1320, 1320, 55, 80, 0.18, "sine"]],
  hold: [[300, 200, 0, 130, 0.3, "sine"]],
  send: [[520, 1040, 0, 150, 0.25, "sine"]],
  confirm: [[660, 660, 0, 110, 0.25, "sine"], [990, 990, 90, 180, 0.25, "sine"]],
  warn: [[440, 415, 0, 130, 0.26, "triangle"], [330, 311, 130, 170, 0.26, "triangle"]],
  done: [[784, 784, 0, 150, 0.26, "sine"], [1175, 1175, 120, 260, 0.26, "sine"]],
};

/** Android closes the speaker after a quiet spell and takes about 90 ms to open it again; a tone played sooner is lost. */
const LEAD = 0.05;

let audio;
let out;

async function sound(kind) {
  if (!audio) {
    audio = new AudioContext({ latencyHint: "interactive" });
    // Tones that meet are kept from clipping.
    out = new DynamicsCompressorNode(audio, { threshold: -12, ratio: 4 });
    out.connect(audio.destination);
  }
  // Browsers keep it silent until a touch.
  if (audio.state !== "running") await audio.resume();
  for (const [from, to, start, length, loudness, wave] of SOUNDS[kind]) {
    const at = audio.currentTime + LEAD + start / 1_000;
    const end = at + length / 1_000;
    const tone = new OscillatorNode(audio, { type: wave, frequency: from });
    const volume = new GainNode(audio, { gain: 0 });
    tone.frequency.setValueAtTime(from, at);
    tone.frequency.exponentialRampToValueAtTime(to, end);
    volume.gain.setValueAtTime(0.0001, at);
    volume.gain.exponentialRampToValueAtTime(loudness, at + 0.008);
    volume.gain.exponentialRampToValueAtTime(0.0001, end);
    tone.connect(volume).connect(out);
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
  [".segments button, .tune button, [role=switch], #tabs button, .folder-toggle, .option, .thread-pin, summary", "toggle"],
  ["button", "tap"],
];

/** Every control answers, wherever it is on the page. */
export function feelTouches(root) {
  root.addEventListener("click", ({ target }) => {
    const control = target.closest?.("button, summary");
    if (control && !control.disabled) feel(TOUCHES.find(([selector]) => control.matches(selector))[1]);
  });
}
