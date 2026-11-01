import { useEffect, useRef, useState } from 'react'
import { toDb } from '../lib/format'

const FLOOR_DB = -48

/** Medidor de pico estéreo con caída suave. */
export function Meter({ left, right }: { left: number; right: number }) {
  const [levels, setLevels] = useState({ l: FLOOR_DB, r: FLOOR_DB })
  const held = useRef({ l: FLOOR_DB, r: FLOOR_DB })

  useEffect(() => {
    const decay = (prev: number, next: number) => Math.max(next, prev - 1.5)
    held.current = { l: decay(held.current.l, toDb(left)), r: decay(held.current.r, toDb(right)) }
    setLevels(held.current)
  }, [left, right])

  const pct = (db: number) => `${Math.max(0, Math.min(1, (db - FLOOR_DB) / -FLOOR_DB)) * 100}%`
  const clip = levels.l >= -0.1 || levels.r >= -0.1

  return (
    <div className="meter" title="Nivel de salida">
      {(['l', 'r'] as const).map((ch) => (
        <div key={ch} className="meter-bar">
          <div className="meter-fill" style={{ width: pct(levels[ch]) }} />
        </div>
      ))}
      <span className={`meter-clip ${clip ? 'on' : ''}`} />
    </div>
  )
}
