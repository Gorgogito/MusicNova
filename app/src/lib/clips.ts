// Edición no destructiva de clips: todas las funciones devuelven copias.
import type { Clip, PracticeSettings, Analysis } from './api'

/** Duración mínima de un clip (s); el doble que en el núcleo, para que el redondeo nunca lo deje por debajo. */
export const MIN_CLIP = 0.02

export const clipEnd = (c: Clip) => c.start + c.length

export function newClipId(): string {
  return crypto.randomUUID()
}

/** Divide un clip en `at` (segundo de la línea de tiempo); `null` si cae fuera. */
export function splitClip(clips: Clip[], id: string, at: number): Clip[] | null {
  const c = clips.find((x) => x.id === id)
  if (!c || at <= c.start + MIN_CLIP || at >= clipEnd(c) - MIN_CLIP) return null
  const leftLen = at - c.start
  const left: Clip = { ...c, length: leftLen, fade_in: Math.min(c.fade_in, leftLen), fade_out: 0 }
  const rightLen = clipEnd(c) - at
  const right: Clip = {
    ...c,
    id: newClipId(),
    start: at,
    source_start: c.source_start + leftLen,
    length: rightLen,
    fade_in: 0,
    fade_out: Math.min(c.fade_out, rightLen),
  }
  return clips.flatMap((x) => (x.id === id ? [left, right] : [x]))
}

/** Divide los clips que están bajo `at` (todos, o solo el indicado). */
export function splitAt(clips: Clip[], at: number, onlyId?: string | null): Clip[] | null {
  let out: Clip[] | null = null
  for (const c of clips) {
    if (onlyId && c.id !== onlyId) continue
    const next = splitClip(out ?? clips, c.id, at)
    if (next) out = next
  }
  return out
}

export function moveClip(c: Clip, start: number): Clip {
  return { ...c, start: Math.max(0, start) }
}

/** Mueve el borde izquierdo: no puede ir antes del principio del audio. */
export function trimStart(c: Clip, newStart: number): Clip {
  const min = Math.max(0, c.start - c.source_start)
  const s = Math.min(Math.max(newStart, min), clipEnd(c) - MIN_CLIP)
  const delta = s - c.start
  const length = c.length - delta
  return { ...c, start: s, source_start: c.source_start + delta, length, fade_in: Math.min(c.fade_in, length - c.fade_out) }
}

/** Mueve el borde derecho: no puede ir más allá del final del audio. */
export function trimEnd(c: Clip, newEnd: number, sourceDuration: number): Clip {
  const max = c.start + (sourceDuration - c.source_start)
  const e = Math.max(Math.min(newEnd, max), c.start + MIN_CLIP)
  const length = e - c.start
  return { ...c, length, fade_out: Math.min(c.fade_out, length - c.fade_in) }
}

export function setFadeIn(c: Clip, v: number): Clip {
  return { ...c, fade_in: Math.min(Math.max(0, v), c.length - c.fade_out) }
}

export function setFadeOut(c: Clip, v: number): Clip {
  return { ...c, fade_out: Math.min(Math.max(0, v), c.length - c.fade_in) }
}

export function replaceClip(clips: Clip[], c: Clip): Clip[] {
  return clips.map((x) => (x.id === c.id ? c : x))
}

/** Pulsos para ajustar: los detectados o la rejilla manual del metrónomo. */
export function snapGrid(practice: PracticeSettings, analysis: Analysis | null, duration: number): number[] {
  if (practice.beat_source === 'detected' && analysis && analysis.beats.length > 0) return analysis.beats
  const step = 60 / Math.min(400, Math.max(20, practice.bpm))
  const out: number[] = []
  let t = practice.beat_offset - Math.floor(practice.beat_offset / step) * step
  for (; t <= duration + 60; t += step) out.push(t)
  return out
}

/** El pulso más cercano a `t` si está a menos de `maxDist` segundos. */
export function snapTo(grid: number[], t: number, maxDist: number): number {
  if (grid.length === 0) return t
  let lo = 0
  let hi = grid.length - 1
  while (lo < hi) {
    const mid = (lo + hi) >> 1
    if (grid[mid] < t) lo = mid + 1
    else hi = mid
  }
  let best = grid[lo]
  if (lo > 0 && Math.abs(grid[lo - 1] - t) < Math.abs(best - t)) best = grid[lo - 1]
  return Math.abs(best - t) <= maxDist ? best : t
}

/** Muestras de la curva de fundido (sen²), de 0 a 1. */
export function fadeCurve(points = 16): number[] {
  return Array.from({ length: points + 1 }, (_, i) => Math.sin(((i / points) * Math.PI) / 2) ** 2)
}
