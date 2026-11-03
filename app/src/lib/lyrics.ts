// Utilidades de letras: línea activa, edición conservando tiempos y desplazamientos.
import type { LyricLine, LyricWord, Lyrics } from './api'
import { tr } from '../i18n'

/** Idiomas más habituales (Whisper admite 99; «auto» los detecta todos). */
export const LANGUAGES: [string, string][] = [
  ['es', tr('Español')], ['en', tr('Inglés')], ['pt', tr('Portugués')], ['fr', tr('Francés')], ['it', tr('Italiano')],
  ['de', tr('Alemán')], ['ca', tr('Catalán')], ['gl', tr('Gallego')], ['eu', tr('Euskera')], ['nl', tr('Neerlandés')],
  ['pl', tr('Polaco')], ['ru', tr('Ruso')], ['uk', tr('Ucraniano')], ['ja', tr('Japonés')], ['ko', tr('Coreano')],
  ['zh', tr('Chino')], ['ar', tr('Árabe')], ['hi', tr('Hindi')], ['tr', tr('Turco')], ['sv', tr('Sueco')],
  ['no', tr('Noruego')], ['da', tr('Danés')], ['fi', tr('Finés')], ['el', tr('Griego')], ['he', tr('Hebreo')],
  ['cs', tr('Checo')], ['ro', tr('Rumano')], ['hu', tr('Húngaro')], ['id', tr('Indonesio')], ['vi', tr('Vietnamita')],
  ['th', tr('Tailandés')], ['la', tr('Latín')],
]

export const languageName = (code: string) => LANGUAGES.find(([c]) => c === code)?.[1] ?? code

/** Adelanto con el que se muestra la línea siguiente (s). */
const LEAD = 0.25

/** Línea activa en `t`: la última que ya empezó (o -1 antes de la primera). */
export function activeLine(lines: LyricLine[], t: number): number {
  let lo = 0
  let hi = lines.length - 1
  let found = -1
  while (lo <= hi) {
    const mid = (lo + hi) >> 1
    if (lines[mid].start <= t + LEAD) {
      found = mid
      lo = mid + 1
    } else hi = mid - 1
  }
  return found
}

/** Fracción cantada de una palabra (0 a 1). */
export function wordProgress(w: LyricWord, t: number): number {
  if (t <= w.start) return 0
  if (t >= w.end) return 1
  return (t - w.start) / Math.max(1e-3, w.end - w.start)
}

/**
 * Sustituye el texto de una línea. Si el número de palabras no cambia se
 * conservan sus tiempos; si cambia, la duración de la línea se reparte en
 * proporción a la longitud de cada palabra.
 */
export function retextLine(line: LyricLine, text: string): LyricLine {
  const tokens = text.split(/\s+/).filter(Boolean)
  if (tokens.length === 0) return { ...line, text: '', words: [] }
  if (tokens.length === line.words.length) {
    return { ...line, text: tokens.join(' '), words: line.words.map((w, i) => ({ ...w, text: tokens[i] })) }
  }
  const total = tokens.reduce((n, t) => n + t.length + 1, 0)
  const span = Math.max(0.2, line.end - line.start)
  let t = line.start
  const words = tokens.map((tok) => {
    const d = (span * (tok.length + 1)) / total
    const w = { start: t, end: t + d * 0.9, text: tok, prob: 1 }
    t += d
    return w
  })
  return { ...line, text: tokens.join(' '), words }
}

export function shiftLine(line: LyricLine, delta: number): LyricLine {
  const d = Math.max(delta, -line.start)
  return {
    ...line,
    start: line.start + d,
    end: line.end + d,
    words: line.words.map((w) => ({ ...w, start: w.start + d, end: w.end + d })),
  }
}

export function shiftAll(lyrics: Lyrics, delta: number): Lyrics {
  return { ...lyrics, lines: lyrics.lines.map((l) => shiftLine(l, delta)) }
}

/** Línea nueva vacía en `t` (dura hasta la siguiente o 3 s). */
export function newLineAt(lines: LyricLine[], t: number): LyricLine {
  const next = lines.find((l) => l.start > t)
  const end = Math.min(t + 3, next ? next.start - 0.05 : t + 3)
  return { start: t, end: Math.max(t + 0.2, end), text: '…', words: [{ start: t, end: Math.max(t + 0.2, end), text: '…', prob: 1 }] }
}
