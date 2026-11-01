import { useState } from 'react'
import type { Analysis, Chord, PracticeSettings } from '../lib/api'
import { canonicalLabel, chordIndexAt, chordName, chordShift, mod12, parseChord, prefersFlats, type Notation } from '../lib/chords'

interface Props {
  analysis: Analysis
  practice: PracticeSettings
  duration: number
  position: number
  notation: Notation
  onSeek: (t: number) => void
  onLoop: (a: number, b: number) => void
  onChange: (a: Analysis) => void
  onError: (m: string) => void
}

const SECTION_HUES = [190, 280, 30, 140, 330, 60, 230, 0]

type Editing = { kind: 'chord' | 'section'; index: number } | null

/** Carriles de secciones y acordes alineados con las formas de onda. */
export function AnalysisLanes({ analysis, practice, duration, position, notation, onSeek, onLoop, onChange, onError }: Props) {
  const [editing, setEditing] = useState<Editing>(null)
  const shift = chordShift(practice)
  const flats = prefersFlats(mod12(analysis.key.tonic + shift), analysis.key.mode)
  const pct = (t: number) => `${(Math.min(t, duration) / duration) * 100}%`
  const width = (a: number, b: number) => `${((Math.min(b, duration) - a) / duration) * 100}%`
  const current = chordIndexAt(analysis.chords, position)
  const labels = [...new Set(analysis.sections.map((s) => s.label))]
  const display = (c: Chord) => (c.root == null ? '' : chordName(mod12(c.root + shift), c.quality, notation, flats))

  const commitChord = (index: number, text: string) => {
    setEditing(null)
    const parsed = parseChord(text)
    if (!parsed) {
      if (text.trim()) onError(`No se reconoce el acorde «${text}». Ejemplos: Am7, F#, Sib, Lam, N`)
      return
    }
    // Lo escrito está en el tono mostrado: se vuelve al tono original del audio.
    const root = parsed.root == null ? null : mod12(parsed.root - shift)
    const chords = analysis.chords.map((c, i) =>
      i === index ? { ...c, root, quality: parsed.quality, label: canonicalLabel(root, parsed.quality) } : c,
    )
    onChange({ ...analysis, chords })
  }

  const commitSection = (index: number, text: string) => {
    setEditing(null)
    const label = text.trim()
    if (!label) return
    onChange({ ...analysis, sections: analysis.sections.map((s, i) => (i === index ? { ...s, label } : s)) })
  }

  const editor = (initial: string, commit: (v: string) => void) => (
    <input
      autoFocus
      type="text"
      className="lane-edit"
      defaultValue={initial}
      onMouseDown={(e) => e.stopPropagation()}
      onBlur={(e) => commit(e.target.value)}
      onKeyDown={(e) => {
        if (e.key === 'Enter') (e.target as HTMLInputElement).blur()
        if (e.key === 'Escape') setEditing(null)
      }}
    />
  )

  return (
    <>
      <div className="lane analysis-lane sections-lane">
        <div className="lane-header compact-header">
          <span className="small muted">Secciones</span>
        </div>
        <div className="analysis-track">
          {analysis.sections.map((s, i) => {
            const hue = SECTION_HUES[labels.indexOf(s.label) % SECTION_HUES.length]
            return (
              <div
                key={`${s.start}-${i}`}
                className="section-block"
                style={{ left: pct(s.start), width: width(s.start, s.end), ['--hue' as string]: hue }}
                onMouseDown={(e) => e.button === 0 && onSeek(s.start)}
                onDoubleClick={() => setEditing({ kind: 'section', index: i })}
                title={`${s.label} · doble clic para renombrar`}
              >
                {editing?.kind === 'section' && editing.index === i ? (
                  editor(s.label, (v) => commitSection(i, v))
                ) : (
                  <>
                    <span>{s.label}</span>
                    <button
                      className="section-loop"
                      title="Repetir esta sección en bucle"
                      onMouseDown={(e) => {
                        e.stopPropagation()
                        onLoop(s.start, s.end)
                      }}
                    >
                      ⟲
                    </button>
                  </>
                )}
              </div>
            )
          })}
        </div>
      </div>
      <div className="lane analysis-lane chords-lane">
        <div className="lane-header compact-header">
          <span className="small muted">Acordes</span>
        </div>
        <div className="analysis-track">
          {analysis.downbeats.map((d) => (
            <span key={d} className="bar-line" style={{ left: pct(d) }} />
          ))}
          {analysis.chords.map((c, i) =>
            c.root == null && !(editing?.kind === 'chord' && editing.index === i) ? null : (
              <div
                key={`${c.start}-${i}`}
                className={`chord-block ${i === current ? 'now' : ''}`}
                style={{ left: pct(c.start), width: width(c.start, c.end) }}
                onMouseDown={(e) => e.button === 0 && onSeek(c.start)}
                onDoubleClick={() => setEditing({ kind: 'chord', index: i })}
                title={`${display(c) || 'Sin acorde'} · doble clic para corregir`}
              >
                {editing?.kind === 'chord' && editing.index === i ? editor(display(c), (v) => commitChord(i, v)) : display(c)}
              </div>
            ),
          )}
        </div>
      </div>
    </>
  )
}
