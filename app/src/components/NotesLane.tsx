import { memo, useMemo, useRef, useState } from 'react'
import type { Note, PitchCurve } from '../lib/api'
import type { Notation } from '../lib/chords'
import { curveMidi, isBlackKey, moveNote, noteLabel, pitchRange, resizeNote, sortNotes, voicedRuns } from '../lib/notes'

interface Props {
  notes: Note[]
  /** Curva de tono de esta pista (afinación vocal), si la hay. */
  curve: PitchCurve | null
  duration: number
  color: string
  notation: Notation
  locked: boolean
  snap: (t: number, pxPerSec: number, free: boolean) => number
  onSeek: (t: number) => void
  onPreview: (notes: Note[]) => void
  onCommit: (before: Note[], after: Note[]) => void
}

/** Píxeles junto al borde derecho de una nota que sirven para alargarla. */
const EDGE = 6
const NEW_NOTE = 0.25

/**
 * Piano roll de una pista: arrastrar una nota la mueve (en el tiempo y en
 * altura), su borde derecho cambia la duración, doble clic en el fondo añade
 * una nota y Supr borra la seleccionada. Encima se dibuja la curva de tono.
 */
export const NotesLane = memo(function NotesLane({
  notes,
  curve,
  duration,
  color,
  notation,
  locked,
  snap,
  onSeek,
  onPreview,
  onCommit,
}: Props) {
  const areaRef = useRef<HTMLDivElement>(null)
  const [selected, setSelected] = useState<number | null>(null)
  const [tip, setTip] = useState<{ x: number; y: number; text: string } | null>(null)
  const midi = useMemo(() => (curve ? curveMidi(curve) : []), [curve])
  // El rango no cambia mientras se arrastra (si no, la nota «huiría» del ratón).
  const [frozen, setFrozen] = useState<[number, number] | null>(null)
  const range = useMemo(() => pitchRange(notes, midi), [notes, midi])
  const [lo, hi] = frozen ?? range
  const rows = hi - lo + 1

  const geometry = () => {
    const r = areaRef.current!.getBoundingClientRect()
    return { r, pxPerSec: r.width / duration, rowH: r.height / rows }
  }
  const at = (e: { clientX: number; clientY: number }) => {
    const { r, rowH } = geometry()
    const t = Math.max(0, Math.min(duration, ((e.clientX - r.left) / r.width) * duration))
    const pitch = hi - Math.floor((e.clientY - r.top) / rowH)
    return { t, pitch: Math.max(lo, Math.min(hi, pitch)) }
  }

  const beginDrag = (e: React.PointerEvent, index: number, mode: 'move' | 'resize') => {
    if (locked) return
    e.stopPropagation()
    const el = areaRef.current!
    el.setPointerCapture(e.pointerId)
    el.focus()
    const { pxPerSec, rowH } = geometry()
    const before = notes
    const orig = notes[index]
    const x0 = e.clientX
    const y0 = e.clientY
    let last = notes
    let moved = false
    setSelected(index)
    setFrozen([lo, hi])
    const update = (ev: PointerEvent) => {
      const dx = ev.clientX - x0
      const dy = ev.clientY - y0
      if (!moved && Math.abs(dx) < 3 && Math.abs(dy) < 3) return
      moved = true
      let n: Note
      if (mode === 'resize') {
        n = resizeNote(orig, snap(orig.end + dx / pxPerSec, pxPerSec, ev.altKey))
      } else {
        const start = snap(orig.start + dx / pxPerSec, pxPerSec, ev.altKey)
        n = moveNote(orig, start, orig.pitch - Math.round(dy / rowH))
      }
      last = notes.map((x, i) => (i === index ? n : x))
      const r = el.getBoundingClientRect()
      setTip({ x: ev.clientX - r.left, y: ev.clientY - r.top, text: `${noteLabel(n.pitch, notation)} · ${n.start.toFixed(2)}–${n.end.toFixed(2)} s` })
      onPreview(last)
    }
    const up = () => {
      el.removeEventListener('pointermove', update)
      el.removeEventListener('pointerup', up)
      el.removeEventListener('pointercancel', up)
      setTip(null)
      setFrozen(null)
      if (moved) {
        const sorted = sortNotes(last)
        setSelected(sorted.indexOf(last[index]))
        onCommit(before, sorted)
      }
    }
    el.addEventListener('pointermove', update)
    el.addEventListener('pointerup', up)
    el.addEventListener('pointercancel', up)
  }

  const onNoteDown = (e: React.PointerEvent<HTMLDivElement>, index: number) => {
    if (e.button !== 0) return
    const box = e.currentTarget.getBoundingClientRect()
    beginDrag(e, index, box.right - e.clientX <= EDGE && box.width > 2 * EDGE ? 'resize' : 'move')
  }

  const onBackgroundDown = (e: React.PointerEvent) => {
    if (e.button !== 0) return
    setSelected(null)
    areaRef.current?.focus()
    onSeek(at(e).t)
  }

  const onDoubleClick = (e: React.MouseEvent) => {
    if (locked || (e.target as HTMLElement).classList.contains('pr-note')) return
    const { pxPerSec } = geometry()
    const { t, pitch } = at(e)
    const start = snap(t, pxPerSec, e.altKey)
    const n: Note = { start, end: Math.min(duration, start + NEW_NOTE), pitch, velocity: 90 }
    if (n.end - n.start < 0.03) return
    const after = sortNotes([...notes, n])
    setSelected(after.indexOf(n))
    onCommit(notes, after)
  }

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (selected == null || locked) return
    if (e.code === 'Delete' || e.code === 'Backspace') {
      e.preventDefault()
      e.stopPropagation()
      onCommit(
        notes,
        notes.filter((_, i) => i !== selected),
      )
      setSelected(null)
    } else if (e.code === 'ArrowUp' || e.code === 'ArrowDown') {
      e.preventDefault()
      e.stopPropagation()
      const n = notes[selected]
      const moved = moveNote(n, n.start, n.pitch + (e.code === 'ArrowUp' ? 1 : -1) * (e.shiftKey ? 12 : 1))
      onCommit(
        notes,
        notes.map((x, i) => (i === selected ? moved : x)),
      )
    } else if (e.code === 'Escape') {
      setSelected(null)
    }
  }

  const keyRows = []
  for (let p = hi; p >= lo; p--) {
    if (isBlackKey(p) || p % 12 === 0) {
      keyRows.push(
        <div
          key={p}
          className={`pr-row ${isBlackKey(p) ? 'black' : 'c'}`}
          style={{ top: `${((hi - p) / rows) * 100}%`, height: `${100 / rows}%` }}
        />,
      )
    }
  }

  const hop = curve?.hop ?? 0.01
  return (
    <div
      ref={areaRef}
      className={`pr-area ${locked ? 'locked' : ''}`}
      tabIndex={0}
      style={{ ['--note-color' as string]: color }}
      onPointerDown={onBackgroundDown}
      onDoubleClick={onDoubleClick}
      onKeyDown={onKeyDown}
    >
      {keyRows}
      {curve && (
        <svg viewBox={`0 0 ${duration} ${rows}`} preserveAspectRatio="none" aria-hidden>
          {voicedRuns(midi).map(([a, b]) => (
            <polyline
              key={a}
              className="pr-curve"
              points={midi
                .slice(a, b)
                .map((m, k) => `${((a + k) * hop).toFixed(3)},${(hi - m + 0.5).toFixed(3)}`)
                .join(' ')}
            />
          ))}
        </svg>
      )}
      {notes.map((n, i) => (
        <div
          key={i}
          className={`pr-note ${i === selected ? 'selected' : ''}`}
          title={`${noteLabel(n.pitch, notation)} · ${(n.end - n.start).toFixed(2)} s · vel. ${n.velocity}`}
          style={{
            left: `${(n.start / duration) * 100}%`,
            width: `${((n.end - n.start) / duration) * 100}%`,
            top: `${((hi - n.pitch) / rows) * 100}%`,
            height: `${100 / rows}%`,
            opacity: 0.45 + 0.55 * (n.velocity / 127),
          }}
          onPointerDown={(e) => onNoteDown(e, i)}
        />
      ))}
      <span className="pr-label top">{noteLabel(hi, notation)}</span>
      <span className="pr-label bottom">{noteLabel(lo, notation)}</span>
      {tip && (
        <span className="auto-tip" style={{ left: tip.x, top: tip.y }}>
          {tip.text}
        </span>
      )}
    </div>
  )
})
