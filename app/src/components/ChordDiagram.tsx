import type { ReactElement } from 'react'
import { chordNotes, guitarShape, mod12 } from '../lib/chords'
import { tr } from '../i18n'

interface Props {
  root: number
  quality: string
  instrument: 'guitar' | 'piano'
}

export function ChordDiagram({ root, quality, instrument }: Props) {
  return instrument === 'guitar' ? <Guitar root={root} quality={quality} /> : <Piano root={root} quality={quality} />
}

const FRETS = 5

function Guitar({ root, quality }: { root: number; quality: string }) {
  const shape = guitarShape(root, quality)
  if (!shape) return null
  const played = shape.frets.filter((f) => f > 0)
  const maxFret = Math.max(0, ...played)
  // Con acordes abiertos se ve la cejuela; si no, la ventana empieza en el traste más bajo.
  const start = maxFret <= FRETS - 1 && shape.barre === 0 ? 1 : Math.min(...played)
  const W = 120
  const H = 140
  const left = 22
  const top = 26
  const sx = (W - left - 12) / 5
  const sy = (H - top - 10) / FRETS
  const x = (i: number) => left + i * sx
  const y = (fret: number) => top + (fret - start + 0.5) * sy

  // Cejilla: cuerdas pisadas en el traste de la cejilla desde la más grave hasta la 1.ª.
  const barreStrings = shape.barre > 0 ? shape.frets.map((f, i) => (f === shape.barre ? i : -1)).filter((i) => i >= 0) : []
  return (
    <svg className="chord-diagram" viewBox={`0 0 ${W} ${H}`} role="img" aria-label={tr('Diagrama de guitarra')}>
      {start === 1 ? (
        <rect x={x(0)} y={top - 3} width={x(5) - x(0)} height={3} className="nut" />
      ) : (
        <text x={4} y={top + sy * 0.6} className="fret-label">
          {start}
        </text>
      )}
      {Array.from({ length: FRETS + 1 }, (_, f) => (
        <line key={`f${f}`} x1={x(0)} x2={x(5)} y1={top + f * sy} y2={top + f * sy} className="fret" />
      ))}
      {Array.from({ length: 6 }, (_, s) => (
        <line key={`s${s}`} x1={x(s)} x2={x(s)} y1={top} y2={top + FRETS * sy} className="string" />
      ))}
      {barreStrings.length > 1 && (
        <rect
          x={x(barreStrings[0]) - 5}
          y={y(shape.barre) - 5}
          width={x(barreStrings[barreStrings.length - 1]) - x(barreStrings[0]) + 10}
          height={10}
          rx={5}
          className="dot"
        />
      )}
      {shape.frets.map((f, s) =>
        f < 0 ? (
          <text key={s} x={x(s)} y={top - 9} className="open-mark">
            ×
          </text>
        ) : f === 0 ? (
          <circle key={s} cx={x(s)} cy={top - 12} r={3.5} className="open-circle" />
        ) : (
          <circle key={s} cx={x(s)} cy={y(f)} r={5.5} className="dot" />
        ),
      )}
    </svg>
  )
}

const WHITE = [0, 2, 4, 5, 7, 9, 11]
const BLACK_AFTER: Record<number, number> = { 1: 0, 3: 1, 6: 3, 8: 4, 10: 5 }

function Piano({ root, quality }: { root: number; quality: string }) {
  // Posición cerrada ascendente desde la fundamental, en dos octavas a partir de Do.
  const notes = chordNotes(root, quality)
  const midi = new Set<number>()
  let prev = mod12(root)
  midi.add(prev)
  for (const n of notes.slice(1)) {
    let m = Math.floor(prev / 12) * 12 + n
    while (m <= prev) m += 12
    midi.add(m)
    prev = m
  }
  const octaves = prev >= 24 ? 3 : 2
  const ww = 14
  const W = WHITE.length * octaves * ww
  const H = 64
  const whites: ReactElement[] = []
  const blacks: ReactElement[] = []
  for (let o = 0; o < octaves; o++) {
    WHITE.forEach((pc, i) => {
      const n = o * 12 + pc
      whites.push(
        <rect key={`w${n}`} x={(o * 7 + i) * ww} y={0} width={ww} height={H} className={`key white ${midi.has(n) ? 'on' : ''}`} />,
      )
    })
    for (const [pc, after] of Object.entries(BLACK_AFTER)) {
      const n = o * 12 + Number(pc)
      blacks.push(
        <rect
          key={`b${n}`}
          x={(o * 7 + after + 1) * ww - ww * 0.32}
          y={0}
          width={ww * 0.64}
          height={H * 0.62}
          className={`key black ${midi.has(n) ? 'on' : ''}`}
        />,
      )
    }
  }
  return (
    <svg className="chord-diagram piano" viewBox={`0 0 ${W} ${H}`} role="img" aria-label={tr('Diagrama de piano')}>
      {whites}
      {blacks}
    </svg>
  )
}
