import type { EffectSlot, EffectType } from '../lib/api'
import {
  CONTROLS,
  EFFECT_LABELS,
  EFFECT_ORDER,
  formatValue,
  fromSlider,
  getParam,
  newSlot,
  setParam,
  toSlider,
} from '../lib/effects'
import { Icon } from './Icon'
import { PluginControls } from './PluginControls'
import { FORMAT_LABEL, pluginSlot, usePlugins } from '../lib/plugins'

interface Props {
  effects: EffectSlot[]
  onChange: (effects: EffectSlot[]) => void
}

/** Cadena de efectos editable: añadir, ordenar, activar, quitar y ajustar. */
export function EffectRack({ effects, onChange }: Props) {
  const { cache } = usePlugins()
  const plugins = cache?.plugins ?? []
  const update = (i: number, slot: EffectSlot) => onChange(effects.map((s, j) => (j === i ? slot : s)))
  const move = (i: number, d: -1 | 1) => {
    const next = [...effects]
    ;[next[i], next[i + d]] = [next[i + d], next[i]]
    onChange(next)
  }

  return (
    <div className="rack">
      {effects.length === 0 && <span className="small muted">Sin efectos. Añade uno para empezar.</span>}
      {effects.map((slot, i) => {
        const type = slot.params.type
        const plugin = type === 'plugin' ? slot.plugin : undefined
        const delay = slot.params.type === 'delay' ? slot.params : null
        const limiter = slot.params.type === 'limiter' ? slot.params : null
        return (
          <section key={slot.id} className={`fx ${slot.enabled ? '' : 'off'}`}>
            <header>
              <label className="switch" title={slot.enabled ? 'Desactivar' : 'Activar'}>
                <input type="checkbox" checked={slot.enabled} onChange={(e) => update(i, { ...slot, enabled: e.target.checked })} />
                <span />
              </label>
              {type === 'plugin' ? (
                <strong className="ellipsis" title={plugin ? `${plugin.name} · ${plugin.vendor}
${plugin.path}` : ''}>
                  {plugin?.name ?? 'Plugin'} <span className="badge">{plugin ? FORMAT_LABEL[plugin.format] : '?'}</span>
                </strong>
              ) : (
                <strong>{EFFECT_LABELS[type]}</strong>
              )}
              <div className="fx-actions">
                <button className="icon-btn" title="Subir" disabled={i === 0} onClick={() => move(i, -1)}>
                  ↑
                </button>
                <button className="icon-btn" title="Bajar" disabled={i === effects.length - 1} onClick={() => move(i, 1)}>
                  ↓
                </button>
                <button className="icon-btn" title="Quitar" onClick={() => onChange(effects.filter((_, j) => j !== i))}>
                  <Icon name="trash" size={14} />
                </button>
              </div>
            </header>
            {type === 'plugin' ? (
              <PluginControls slotId={slot.id} />
            ) : (
            <div className="fx-controls">
              {CONTROLS[type].map((c) => {
                const v = getParam(slot.params, c.path)
                return (
                  <label key={c.path} className="fx-control">
                    <span className="small muted">{c.label}</span>
                    <input
                      type="range"
                      min={0}
                      max={1000}
                      value={toSlider(c, v)}
                      onChange={(e) => update(i, { ...slot, params: setParam(slot.params, c.path, fromSlider(c, Number(e.target.value))) })}
                    />
                    <span className="small mono">{formatValue(c, v)}</span>
                  </label>
                )
              })}
              {delay && (
                <label className="inline small">
                  <input
                    type="checkbox"
                    checked={delay.ping_pong}
                    onChange={(e) => update(i, { ...slot, params: { ...delay, ping_pong: e.target.checked } })}
                  />
                  Ping-pong
                </label>
              )}
              {limiter && (
                <label className="inline small" title="Limita también los picos entre muestras (lo que miden las plataformas en dBTP)">
                  <input
                    type="checkbox"
                    checked={limiter.true_peak}
                    onChange={(e) => update(i, { ...slot, params: { ...limiter, true_peak: e.target.checked } })}
                  />
                  Pico real (dBTP)
                </label>
              )}
            </div>
            )}
          </section>
        )
      })}
      <select
        className="compact"
        value=""
        onChange={(e) => {
          const v = e.target.value
          if (!v) return
          if (v.startsWith('plugin:')) {
            const d = plugins[Number(v.slice(7))]
            if (d) onChange([...effects, pluginSlot(d)])
          } else onChange([...effects, newSlot(v as EffectType)])
        }}
        aria-label="Añadir efecto"
        disabled={effects.length >= 16}
      >
        <option value="">+ Añadir efecto…</option>
        <optgroup label="MusicNova">
          {EFFECT_ORDER.map((t) => (
            <option key={t} value={t}>
              {EFFECT_LABELS[t]}
            </option>
          ))}
        </optgroup>
        {plugins.length > 0 ? (
          <optgroup label="Plugins">
            {plugins.map((d, k) => (
              <option key={`${d.format}:${d.path}:${d.id}`} value={`plugin:${k}`}>
                {d.name} · {d.vendor} ({FORMAT_LABEL[d.format]})
              </option>
            ))}
          </optgroup>
        ) : (
          <option disabled>Plugins: escanéalos en «Sistema»</option>
        )}
      </select>
    </div>
  )
}
