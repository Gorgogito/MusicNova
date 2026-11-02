import { useEffect, useRef, useState } from 'react'
import { api, errorMessage, type PluginParam } from '../lib/api'
import { Icon } from './Icon'

/** Controles de un efecto plugin: abrir su interfaz y parámetros genéricos. */
export function PluginControls({ slotId }: { slotId: string }) {
  const [error, setError] = useState<string | null>(null)
  const [params, setParams] = useState<PluginParam[] | null>(null)
  const [open, setOpen] = useState(false)
  const [filter, setFilter] = useState('')
  const refresh = useRef<number | null>(null)

  const load = () =>
    api
      .pluginParams(slotId)
      .then((p) => {
        setParams(p)
        setError(null)
      })
      .catch((e) => setError(errorMessage(e)))

  useEffect(() => {
    if (open) load()
    return () => {
      if (refresh.current) window.clearTimeout(refresh.current)
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, slotId])

  const set = (p: PluginParam, value: number) => {
    setParams((all) => all && all.map((x) => (x.id === p.id ? { ...x, value } : x)))
    api.pluginParamSet(slotId, p.id, value).catch((e) => setError(errorMessage(e)))
    // El texto («-6,0 dB») lo da el plugin: se vuelve a pedir al terminar de mover.
    if (refresh.current) window.clearTimeout(refresh.current)
    refresh.current = window.setTimeout(load, 250)
  }

  const shown = (params ?? []).filter((p) => !filter || p.name.toLowerCase().includes(filter.toLowerCase()))

  return (
    <div className="plugin-controls">
      <div className="panel-row">
        <button
          className="btn small"
          onClick={() =>
            api
              .pluginOpenEditor(slotId)
              .then(() => setError(null))
              .catch((e) => setError(errorMessage(e)))
          }
        >
          <Icon name="sliders" size={14} /> Abrir interfaz
        </button>
        <button className={`btn ghost small ${open ? 'active' : ''}`} onClick={() => setOpen(!open)}>
          Parámetros{params ? ` (${params.length})` : ''}
        </button>
      </div>
      {error && <div className="notice error small">{error}</div>}
      {open && params && (
        <>
          {params.length > 12 && (
            <input className="plugin-filter" type="text" placeholder="Buscar parámetro…" value={filter} onChange={(e) => setFilter(e.target.value)} />
          )}
          {params.length === 0 && <span className="small muted">El plugin no publica parámetros.</span>}
          <div className="fx-controls">
            {shown.map((p) => (
              <label key={p.id} className="fx-control" title={p.name}>
                <span className="small muted ellipsis">{p.name}</span>
                <input
                  type="range"
                  min={p.min}
                  max={p.max}
                  step={p.stepped && p.max - p.min >= 1 ? 1 : (p.max - p.min) / 1000 || 0.001}
                  value={p.value}
                  onChange={(e) => set(p, Number(e.target.value))}
                  onDoubleClick={() => set(p, p.default)}
                />
                <span className="small mono ellipsis">{p.text}</span>
              </label>
            ))}
          </div>
        </>
      )}
    </div>
  )
}
