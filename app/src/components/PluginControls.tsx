import { useEffect, useRef, useState } from 'react'
import { api, errorMessage, events, type PluginParam } from '../lib/api'
import { Icon } from './Icon'
import { ParamControl } from './ParamControl'
import { parseNumber, roundTo, type ParamSpec } from '../lib/param'
import { tr } from '../i18n'

/** Cada cuánto se releen los parámetros con el panel abierto: refleja lo que se
 * mueve en la ventana del propio plugin (y los plugins que cambian su lista). */
const POLL_MS = 1000

/** Parámetro de plugin: lineal en su rango; el texto y su lectura los da el plugin. */
function pluginSpec(slotId: string, p: PluginParam): ParamSpec {
  const range = p.max - p.min || 1
  // Escalonados: de paso en paso (un clic cambia de opción). Continuos: 1 % del rango.
  const unit = p.steps > 0 ? range / p.steps : range / 100
  const fit = (v: number) => {
    const x = Math.max(p.min, Math.min(p.max, v))
    return p.steps > 0 ? p.min + Math.round((x - p.min) / unit) * unit : x
  }
  return {
    min: p.min,
    max: p.max,
    def: p.default,
    toNorm: (v) => Math.max(0, Math.min(1, (v - p.min) / range)),
    fromNorm: (n) => fit(p.min + Math.max(0, Math.min(1, n)) * range),
    nudge: (v, dir, big) => fit(roundTo(v + dir * unit * (big && p.steps === 0 ? 10 : 1), unit / 100)),
    format: () => p.text,
    // El plugin interpreta el texto («-6 dB», «Sí»). Si no sabe, se acepta un
    // número dentro de su rango.
    parse: async (text) => {
      const v = await api.pluginParamParse(slotId, p.id, text).catch(() => null)
      if (v != null) return fit(v)
      const n = parseNumber(text)
      return n != null && n >= p.min && n <= p.max ? fit(n) : null
    },
  }
}

/** Controles de un efecto plugin: abrir su interfaz y parámetros genéricos. */
export function PluginControls({ slotId }: { slotId: string }) {
  const [error, setError] = useState<string | null>(null)
  const [params, setParams] = useState<PluginParam[] | null>(null)
  const [open, setOpen] = useState(false)
  /** El plugin se cayó o dejó de responder: se ofrece reiniciarlo. */
  const [failed, setFailed] = useState(false)
  const [filter, setFilter] = useState('')
  const refresh = useRef<number | null>(null)
  /** Mientras el usuario arrastra un deslizador no se pisa su valor. */
  const editing = useRef(false)

  const load = () =>
    api
      .pluginParams(slotId)
      .then((p) => {
        if (editing.current) return
        setParams(p)
        setError(null)
      })
      .catch((e) => setError(errorMessage(e)))

  useEffect(() => {
    const un = events.pluginFailed((e) => {
      if (e.slot_id !== slotId) return
      setFailed(true)
      setError(e.message) // ya traducido por events.pluginFailed
    })
    return () => {
      un.then((f) => f())
    }
  }, [slotId])

  const restart = () =>
    api
      .pluginRestart(slotId)
      .then(() => {
        setFailed(false)
        setError(null)
        if (open) load()
      })
      .catch((e) => setError(errorMessage(e)))

  useEffect(() => {
    if (!open) return
    load()
    const poll = window.setInterval(() => {
      if (!editing.current && refresh.current === null) load()
    }, POLL_MS)
    return () => {
      window.clearInterval(poll)
      if (refresh.current) window.clearTimeout(refresh.current)
      refresh.current = null
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, slotId])

  const set = (p: PluginParam, value: number) => {
    setParams((all) => all && all.map((x) => (x.id === p.id ? { ...x, value } : x)))
    api.pluginParamSet(slotId, p.id, value).catch((e) => setError(errorMessage(e)))
    // El texto («-6,0 dB») lo da el plugin: se vuelve a pedir al terminar de mover.
    if (refresh.current) window.clearTimeout(refresh.current)
    refresh.current = window.setTimeout(() => {
      refresh.current = null
      load()
    }, 250)
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
          <Icon name="sliders" size={14} /> {tr('Abrir interfaz')}
        </button>
        <button className={`btn ghost small ${open ? 'active' : ''}`} onClick={() => setOpen(!open)}>
          Parámetros{params ? ` (${params.length})` : ''}
        </button>
      </div>
      {error && (
        <div className="notice error small">
          {error}
          {(failed || /dejó de responder|se cerró/.test(error)) && (
            <button className="btn small" style={{ marginLeft: 8 }} onClick={restart}>
              {tr('Reiniciar plugin')}
            </button>
          )}
        </div>
      )}
      {open && params && (
        <>
          {params.length > 12 && (
            <input className="plugin-filter" type="text" placeholder={tr('Buscar parámetro…')} value={filter} onChange={(e) => setFilter(e.target.value)} />
          )}
          {params.length === 0 && <span className="small muted">{tr('El plugin no publica parámetros.')}</span>}
          <div className="fx-controls">
            {shown.map((p) => (
              <div key={p.id} className="fx-control" title={p.name}>
                <span className="small muted ellipsis">{p.name}</span>
                <ParamControl
                  spec={pluginSpec(slotId, p)}
                  value={p.value}
                  label={p.name}
                  onChange={(v) => set(p, v)}
                  onEditStart={() => (editing.current = true)}
                  onEditEnd={() => (editing.current = false)}
                />
              </div>
            ))}
          </div>
        </>
      )}
    </div>
  )
}
