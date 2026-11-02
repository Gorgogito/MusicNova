// Teoría de acordes para la interfaz: nombres, transposición y digitaciones.
// Los tipos de acorde coinciden con los que reconoce el proceso de IA
// (ai-worker/musicnova_ai/analysis/harmony.py): 7 con las plantillas y 14 con BTC.

export type Notation = 'english' | 'latin'

const SHARPS_EN = ['C', 'C#', 'D', 'D#', 'E', 'F', 'F#', 'G', 'G#', 'A', 'A#', 'B']
const FLATS_EN = ['C', 'Db', 'D', 'Eb', 'E', 'F', 'Gb', 'G', 'Ab', 'A', 'Bb', 'B']
const SHARPS_LA = ['Do', 'Do#', 'Re', 'Re#', 'Mi', 'Fa', 'Fa#', 'Sol', 'Sol#', 'La', 'La#', 'Si']
const FLATS_LA = ['Do', 'Reb', 'Re', 'Mib', 'Mi', 'Fa', 'Solb', 'Sol', 'Lab', 'La', 'Sib', 'Si']

/** Intervalos (semitonos) de cada tipo de acorde. */
export const QUALITY_INTERVALS: Record<string, number[]> = {
  '': [0, 4, 7],
  m: [0, 3, 7],
  '7': [0, 4, 7, 10],
  maj7: [0, 4, 7, 11],
  m7: [0, 3, 7, 10],
  dim: [0, 3, 6],
  sus4: [0, 5, 7],
  sus2: [0, 2, 7],
  aug: [0, 4, 8],
  '6': [0, 4, 7, 9],
  m6: [0, 3, 7, 9],
  mMaj7: [0, 3, 7, 11],
  dim7: [0, 3, 6, 9],
  m7b5: [0, 3, 6, 10],
}

const QUALITY_SUFFIX: Record<string, string> = {
  '': '',
  m: 'm',
  '7': '7',
  maj7: 'maj7',
  m7: 'm7',
  dim: '°',
  sus4: 'sus4',
  sus2: 'sus2',
  aug: '+',
  '6': '6',
  m6: 'm6',
  mMaj7: 'm(maj7)',
  dim7: '°7',
  m7b5: 'm7♭5',
}

export const mod12 = (n: number) => ((n % 12) + 12) % 12

/** Tonalidades que se escriben con bemoles (tónica mayor / tónica menor). */
const FLAT_MAJOR = new Set([5, 10, 3, 8, 1, 6])
const FLAT_MINOR = new Set([2, 7, 0, 5, 10, 3])

export function prefersFlats(tonic: number, mode: 'major' | 'minor'): boolean {
  return (mode === 'minor' ? FLAT_MINOR : FLAT_MAJOR).has(mod12(tonic))
}

export function noteName(pc: number, notation: Notation, flats: boolean): string {
  const table = notation === 'latin' ? (flats ? FLATS_LA : SHARPS_LA) : flats ? FLATS_EN : SHARPS_EN
  return table[mod12(pc)]
}

export function chordName(root: number | null, quality: string | null, notation: Notation, flats: boolean): string {
  if (root == null || quality == null) return 'N.C.'
  return noteName(root, notation, flats) + (QUALITY_SUFFIX[quality] ?? quality)
}

export function keyName(tonic: number, mode: 'major' | 'minor', notation: Notation): string {
  const flats = prefersFlats(tonic, mode)
  return `${noteName(tonic, notation, flats)} ${mode === 'minor' ? 'menor' : 'mayor'}`
}

/** Notas (clases de altura) de un acorde. */
export function chordNotes(root: number, quality: string): number[] {
  return (QUALITY_INTERVALS[quality] ?? QUALITY_INTERVALS['']).map((i) => mod12(root + i))
}

// ---------------------------------------------------------------- Guitarra

/** Afinación estándar, de la 6.ª (grave) a la 1.ª cuerda. */
export const GUITAR_TUNING = [4, 9, 2, 7, 11, 4]

export interface GuitarShape {
  /** Traste por cuerda (6.ª → 1.ª); -1 = no se toca, 0 = al aire. */
  frets: number[]
  /** Traste de la cejilla (0 si no hay). */
  barre: number
}

// Acordes abiertos habituales (clave: "raíz:tipo").
const OPEN: Record<string, number[]> = {
  '0:': [-1, 3, 2, 0, 1, 0],
  '2:': [-1, -1, 0, 2, 3, 2],
  '4:': [0, 2, 2, 1, 0, 0],
  '7:': [3, 2, 0, 0, 0, 3],
  '9:': [-1, 0, 2, 2, 2, 0],
  '9:m': [-1, 0, 2, 2, 1, 0],
  '2:m': [-1, -1, 0, 2, 3, 1],
  '4:m': [0, 2, 2, 0, 0, 0],
  '0:7': [-1, 3, 2, 3, 1, 0],
  '2:7': [-1, -1, 0, 2, 1, 2],
  '4:7': [0, 2, 0, 1, 0, 0],
  '7:7': [3, 2, 0, 0, 0, 1],
  '9:7': [-1, 0, 2, 0, 2, 0],
  '11:7': [-1, 2, 1, 2, 0, 2],
  '0:maj7': [-1, 3, 2, 0, 0, 0],
  '2:maj7': [-1, -1, 0, 2, 2, 2],
  '4:maj7': [0, 2, 1, 1, 0, 0],
  '5:maj7': [-1, -1, 3, 2, 1, 0],
  '7:maj7': [3, 2, 0, 0, 0, 2],
  '9:maj7': [-1, 0, 2, 1, 2, 0],
  '9:m7': [-1, 0, 2, 0, 1, 0],
  '2:m7': [-1, -1, 0, 2, 1, 1],
  '4:m7': [0, 2, 0, 0, 0, 0],
  '9:sus4': [-1, 0, 2, 2, 3, 0],
  '2:sus4': [-1, -1, 0, 2, 3, 3],
  '4:sus4': [0, 2, 2, 2, 0, 0],
}

// Formas móviles con cejilla: desplazamientos respecto al traste de la cejilla.
const E_SHAPE: Record<string, number[]> = {
  '': [0, 2, 2, 1, 0, 0],
  m: [0, 2, 2, 0, 0, 0],
  '7': [0, 2, 0, 1, 0, 0],
  maj7: [0, -1, 1, 1, 0, -1],
  m7: [0, 2, 0, 0, 0, 0],
  dim: [0, 1, 2, 0, -1, -1],
  sus4: [0, 2, 2, 2, 0, 0],
  sus2: [0, 2, 4, 4, 0, 0],
  aug: [0, 3, 2, 1, 1, 0],
  '6': [0, 2, 2, 1, 2, 0],
  m6: [0, 2, 2, 0, 2, 0],
  mMaj7: [0, 2, 1, 0, 0, 0],
  dim7: [0, 1, 2, 0, 2, 0],
  m7b5: [0, 1, 0, 0, 3, -1],
}
const A_SHAPE: Record<string, number[]> = {
  '': [-1, 0, 2, 2, 2, 0],
  m: [-1, 0, 2, 2, 1, 0],
  '7': [-1, 0, 2, 0, 2, 0],
  maj7: [-1, 0, 2, 1, 2, 0],
  m7: [-1, 0, 2, 0, 1, 0],
  dim: [-1, 0, 1, 2, 1, -1],
  sus4: [-1, 0, 2, 2, 3, 0],
  sus2: [-1, 0, 2, 2, 0, 0],
  aug: [-1, 0, 3, 2, 2, 1],
  '6': [-1, 0, 2, 2, 2, 2],
  m6: [-1, 0, 2, 2, 1, 2],
  mMaj7: [-1, 0, 2, 1, 1, 0],
  dim7: [-1, 0, 1, 2, 1, 2],
  m7b5: [-1, 0, 1, 0, 1, -1],
}

const movable = (offsets: number[], fret: number) => offsets.map((o) => (o < 0 ? -1 : fret + o))

/** Digitación recomendada: acorde abierto si existe; si no, la cejilla más baja. */
export function guitarShape(root: number, quality: string): GuitarShape | null {
  if (!(quality in QUALITY_INTERVALS)) return null
  const r = mod12(root)
  const open = OPEN[`${r}:${quality}`]
  if (open) return { frets: open, barre: 0 }
  const fretE = mod12(r - 4) || 12
  const fretA = mod12(r - 9) || 12
  return fretE <= fretA
    ? { frets: movable(E_SHAPE[quality], fretE), barre: fretE }
    : { frets: movable(A_SHAPE[quality], fretA), barre: fretA }
}

/** Clases de altura que suenan en una digitación (para comprobarla). */
export function shapeNotes(shape: GuitarShape): number[] {
  return shape.frets.flatMap((f, i) => (f < 0 ? [] : [mod12(GUITAR_TUNING[i] + f)]))
}

// ---------------------------------------------------------------- Utilidades

/** Acorde que suena en `t` (búsqueda binaria; los acordes están ordenados). */
export function chordIndexAt<T extends { start: number; end: number }>(chords: T[], t: number): number {
  let lo = 0
  let hi = chords.length - 1
  while (lo <= hi) {
    const mid = (lo + hi) >> 1
    if (t < chords[mid].start) hi = mid - 1
    else if (t >= chords[mid].end) lo = mid + 1
    else return mid
  }
  return -1
}

const LATIN_ROOTS: [string, number][] = [['sol', 7], ['do', 0], ['re', 2], ['mi', 4], ['fa', 5], ['la', 9], ['si', 11]]
const EN_ROOTS: Record<string, number> = { c: 0, d: 2, e: 4, f: 5, g: 7, a: 9, b: 11 }
const QUALITY_ALIASES: Record<string, string> = {
  '': '', maj: '', m: 'm', min: 'm', '-': 'm', '7': '7', dom7: '7', maj7: 'maj7', M7: 'maj7', 'Δ': 'maj7',
  m7: 'm7', min7: 'm7', '-7': 'm7', dim: 'dim', '°': 'dim', o: 'dim', sus4: 'sus4', sus: 'sus4',
  sus2: 'sus2', aug: 'aug', '+': 'aug', '6': '6', maj6: '6', m6: 'm6', min6: 'm6', mmaj7: 'mMaj7', mMaj7: 'mMaj7',
  'm(maj7)': 'mMaj7', minmaj7: 'mMaj7', dim7: 'dim7', '°7': 'dim7', o7: 'dim7', m7b5: 'm7b5', 'm7♭5': 'm7b5', ø: 'm7b5',
  ø7: 'm7b5', hdim7: 'm7b5',
}

/** Interpreta un acorde escrito ("Am7", "Lam7", "F#", "Sib", "N"). `null` si no se entiende. */
export function parseChord(text: string): { root: number | null; quality: string | null } | null {
  const t = text.trim()
  if (/^(n|n\.c\.?|nc|-)$/i.test(t)) return { root: null, quality: null }
  let root: number | null = null
  let rest = ''
  const latin = LATIN_ROOTS.find(([name]) => t.toLowerCase().startsWith(name))
  if (latin) {
    root = latin[1]
    rest = t.slice(latin[0].length)
  } else if (t[0] && t[0].toLowerCase() in EN_ROOTS) {
    root = EN_ROOTS[t[0].toLowerCase()]
    rest = t.slice(1)
  } else return null
  if (rest.startsWith('#') || rest.startsWith('♯')) {
    root += 1
    rest = rest.slice(1)
  } else if (rest.startsWith('b') || rest.startsWith('♭')) {
    root -= 1
    rest = rest.slice(1)
  }
  const quality = QUALITY_ALIASES[rest] ?? QUALITY_ALIASES[rest.toLowerCase()]
  if (quality === undefined) return null
  return { root: mod12(root), quality }
}

/** Etiqueta canónica (notación inglesa) que guarda el análisis. */
export function canonicalLabel(root: number | null, quality: string | null): string {
  if (root == null || quality == null) return 'N'
  return FLATS_EN[mod12(root)] + quality
}

/** Semitonos que hay que sumar a los acordes del análisis para lo que se toca (tono menos cejilla). */
export function chordShift(p: { pitch: number; capo: number }): number {
  return Math.round(p.pitch) - p.capo
}
