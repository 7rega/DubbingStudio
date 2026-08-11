// sfx — деликатные синтезированные UI-звуки (Web Audio API, без файлов -> CSP-safe, крошечные).
// Best-practice звукового UX: коротко (<300мс), мягкая атака/спад, тихо, различимо по событию, с тумблером.
// Тумблер хранится в localStorage; по умолчанию включено.

let ctx: AudioContext | null = null;
const KEY = "dub-sfx";

export function sfxEnabled(): boolean {
  return localStorage.getItem(KEY) !== "0";   // по умолчанию ON
}
export function setSfxEnabled(on: boolean): void {
  localStorage.setItem(KEY, on ? "1" : "0");
}

function ac(): AudioContext | null {
  if (typeof window === "undefined") return null;
  try {
    if (!ctx) ctx = new (window.AudioContext || (window as unknown as { webkitAudioContext: typeof AudioContext }).webkitAudioContext)();
    if (ctx.state === "suspended") ctx.resume().catch(() => {});
    return ctx;
  } catch {
    return null;
  }
}

// один мягкий тон: синус/треугольник с плавной атакой и экспоненциальным спадом (без щелчков).
function tone(c: AudioContext, freq: number, start: number, dur: number, gain = 0.11, type: OscillatorType = "sine"): void {
  const t0 = c.currentTime + start;
  const osc = c.createOscillator();
  const g = c.createGain();
  osc.type = type;
  osc.frequency.value = freq;
  g.gain.setValueAtTime(0.0001, t0);
  g.gain.linearRampToValueAtTime(gain, t0 + 0.012);
  g.gain.exponentialRampToValueAtTime(0.0001, t0 + dur);
  osc.connect(g);
  g.connect(c.destination);
  osc.start(t0);
  osc.stop(t0 + dur + 0.03);
}

export type Sfx = "success" | "notify" | "error";

// success — восходящее E5->B5 (приятное «готово»); notify — один G5 (короткое «сделано»);
// error — нисходящее G4->D4 треугольником (мягкое, не резкое).
export function playSfx(kind: Sfx): void {
  if (!sfxEnabled()) return;
  const c = ac();
  if (!c) return;
  if (kind === "success") {
    tone(c, 659.25, 0, 0.16);
    tone(c, 987.77, 0.085, 0.22);
  } else if (kind === "notify") {
    tone(c, 783.99, 0, 0.15);
  } else {
    tone(c, 392.0, 0, 0.18, 0.1, "triangle");
    tone(c, 293.66, 0.1, 0.3, 0.1, "triangle");
  }
}

// Тактильный «тап» — на мобильных/поддерживающих устройствах; на десктопе (Tauri webview) это no-op
// (нет виброжелеза). Оставляем guarded-вызов, чтобы работало там, где железо есть.
export function tap(ms = 8): void {
  if (!sfxEnabled()) return;
  try { navigator.vibrate?.(ms); } catch { /* нет поддержки */ }
}
