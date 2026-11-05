import type { EffectSlot, EffectType } from '../lib/api'
import { CONTROLS, EFFECT_LABELS, EFFECT_ORDER, controlSpec, defaultParams, getParam, newSlot, setParam } from '../lib/effects'
import { Icon } from './Icon'
import { ParamControl } from './ParamControl'
import { PluginControls } from './PluginControls'
import { FORMAT_LABEL, pluginSlot, usePlugins } from '../lib/plugins'
import { tr } from '../i18n'

interface Props {
  effects: EffectSlot[]
  onChange: (effects: EffectSlot[]) => void
  /** Pistas que pueden alimentar el sidechain del compresor (sin la propia; vacío en el máster). */
  sidechainSources?: { id: string; name: string }[]
}

/** Cadena de efectos editable: añadir, ordenar, activar, quitar y ajustar. */
export function EffectRack({ effects, onChange, sidechainSources = [] }: Props) {
  const { cache } = usePlugins()
  // Los instrumentos van en pistas de instrumento, no en cadenas de efectos.
  const plugins = (cache?.plugins ?? []).filter((d) => !d.instrument)
  const update = (i: number, slot: EffectSlot) => onChange(effects.map((s, j) => (j === i ? slot : s)))
  const move = (i: number, d: -1 | 1) => {
    const next = [...effects]
    ;[next[i], next[i + d]] = [next[i + d], next[i]]
    onChange(next)
  }

  return (
    <div className="rack">
      {effects.length === 0 && <span className="small muted">{tr('Sin efectos. Añade uno para empezar.')}</span>}
      {effects.map((slot, i) => {
        const type = slot.params.type
        const plugin = type === 'plugin' ? slot.plugin : undefined
        const delay = slot.params.type === 'delay' ? slot.params : null
        const limiter = slot.params.type === 'limiter' ? slot.params : null
        return (
          <section key={slot.id} className={`fx ${slot.enabled ? '' : 'off'}`}>
            <header>
              <label className="switch" title={slot.enabled ? tr('Desactivar') : tr('Activar')}>
                <input type="checkbox" checked={slot.enabled} onChange={(e) => update(i, { ...slot, enabled: e.target.checked })} />
                <span />
              </label>
              {type === 'plugin' ? (
                <strong className="ellipsis" title={plugin ? `${plugin.name} · ${plugin.vendor}
${plugin.path}` : ''}>
                  {plugin?.name ?? tr('Plugin')} <span className="badge">{plugin ? FORMAT_LABEL[plugin.format] : '?'}</span>
                </strong>
              ) : (
                <strong>{EFFECT_LABELS[type]}</strong>
              )}
              <div className="fx-actions">
                <button className="icon-btn" title={tr('Subir')} disabled={i === 0} onClick={() => move(i, -1)}>
                  ↑
                </button>
                <button className="icon-btn" title={tr('Bajar')} disabled={i === effects.length - 1} onClick={() => move(i, 1)}>
                  ↓
                </button>
                <button className="icon-btn" title={tr('Quitar')} onClick={() => onChange(effects.filter((_, j) => j !== i))}>
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
                  <div key={c.path} className="fx-control">
                    <span className="small muted">{c.label}</span>
                    <ParamControl
                      spec={controlSpec(c, getParam(defaultParams(type), c.path))}
                      value={v}
                      label={c.label}
                      onChange={(x) => update(i, { ...slot, params: setParam(slot.params, c.path, x) })}
                    />
                  </div>
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
              {type === 'compressor' && sidechainSources.length > 0 && (
                <label
                  className="field small"
                  title={tr('El compresor reacciona al nivel de otra pista (p. ej. el bombo hace «respirar» al bajo o a un pad). Funciona aunque esa pista esté silenciada')}
                >
                  <span className="small muted">{tr('Sidechain')}</span>
                  <select
                    value={slot.sidechain ?? ''}
                    onChange={(e) => update(i, { ...slot, sidechain: e.target.value || null })}
                  >
                    <option value="">{tr('Ninguno (su propia señal)')}</option>
                    {sidechainSources.map((t) => (
                      <option key={t.id} value={t.id}>
                        {t.name}
                      </option>
                    ))}
                  </select>
                </label>
              )}
              {limiter && (
                <label className="inline small" title={tr('Limita también los picos entre muestras (lo que miden las plataformas en dBTP)')}>
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
        aria-label={tr('Añadir efecto')}
        disabled={effects.length >= 16}
      >
        <option value="">{tr('+ Añadir efecto…')}</option>
        <optgroup label="MusicNova">
          {EFFECT_ORDER.map((t) => (
            <option key={t} value={t}>
              {EFFECT_LABELS[t]}
            </option>
          ))}
        </optgroup>
        {plugins.length > 0 ? (
          <optgroup label={tr('Plugins')}>
            {plugins.map((d, k) => (
              <option key={`${d.format}:${d.path}:${d.id}`} value={`plugin:${k}`}>
                {d.name} · {d.vendor} ({FORMAT_LABEL[d.format]})
              </option>
            ))}
          </optgroup>
        ) : (
          <option disabled>{tr('Plugins: escanéalos en «Sistema»')}</option>
        )}
      </select>
    </div>
  )
}
