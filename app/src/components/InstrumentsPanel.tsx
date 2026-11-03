import { useState } from 'react'
import { open } from '@tauri-apps/plugin-dialog'
import { api, errorMessage, type InstrumentSource, type PluginDescriptor, type ProjectView, type SynthTimbre, type TrackView } from '../lib/api'
import { FORMAT_LABEL, usePlugins } from '../lib/plugins'
import { noteName, type LivePlay } from '../lib/live'
import { Icon } from './Icon'
import { tr, plural } from '../i18n'

interface Props {
  live: LivePlay
  project: ProjectView
  shownNotes: Set<string>
  onToggleNotes: (trackId: string) => void
  onProjectReplaced: (p: ProjectView) => void
  onTrackReplaced: (t: TrackView) => void
  onError: (m: string) => void
}

const TIMBRES: { id: SynthTimbre; label: string }[] = [
  { id: 'keys', label: tr('Piano eléctrico') },
  { id: 'pad', label: tr('Colchón (pad)') },
  { id: 'lead', label: tr('Melodía (lead)') },
  { id: 'pluck', label: tr('Cuerda pulsada') },
  { id: 'bass', label: tr('Bajo') },
  { id: 'drums', label: tr('Batería (mapa General MIDI)') },
]

/** Valor del desplegable para una fuente de sonido. */
const keyOf = (s: InstrumentSource | null | undefined) =>
  !s ? '' : s.type === 'synth' ? `synth:${s.timbre}` : `plugin:${s.plugin.format}:${s.plugin.path}:${s.plugin.id}`

function sourceOf(key: string, plugins: PluginDescriptor[], current?: InstrumentSource | null): InstrumentSource {
  if (key.startsWith('synth:')) return { type: 'synth', timbre: key.slice(6) as SynthTimbre }
  // Si es el mismo plugin se conserva su estado guardado.
  if (current && keyOf(current) === key) return current
  const d = plugins.find((p) => `plugin:${p.format}:${p.path}:${p.id}` === key)!
  return { type: 'plugin', plugin: { format: d.format, path: d.path, id: d.id, name: d.name, vendor: d.vendor, state: null } }
}

function SourceSelect({ value, onChange, instruments }: { value: string; onChange: (key: string) => void; instruments: PluginDescriptor[] }) {
  return (
    <select value={value} onChange={(e) => onChange(e.target.value)}>
      <optgroup label={tr('Sintetizador integrado')}>
        {TIMBRES.map((t) => (
          <option key={t.id} value={`synth:${t.id}`}>
            {t.label}
          </option>
        ))}
      </optgroup>
      {instruments.length > 0 && (
        <optgroup label={tr('Plugins instrumento')}>
          {instruments.map((d) => (
            <option key={`${d.path}:${d.id}`} value={`plugin:${d.format}:${d.path}:${d.id}`}>
              {d.name} ({FORMAT_LABEL[d.format]})
            </option>
          ))}
        </optgroup>
      )}
    </select>
  )
}

/** Pistas de instrumento: notas MIDI con el sintetizador integrado o un plugin. */
export function InstrumentsPanel(props: Props) {
  const { project } = props
  const { cache } = usePlugins()
  const instruments = (cache?.plugins ?? []).filter((d) => d.instrument)
  const [name, setName] = useState('')
  const [newKey, setNewKey] = useState('synth:keys')
  const [busy, setBusy] = useState<string | null>(null)
  const tracks = project.tracks.filter((t) => t.kind === 'instrument')
  const { live } = props
  const armed = live.status?.armed ?? null

  const run = async <T,>(label: string, f: () => Promise<T>): Promise<T | undefined> => {
    setBusy(label)
    try {
      return await f()
    } catch (e) {
      props.onError(errorMessage(e))
    } finally {
      setBusy(null)
    }
  }

  const create = () =>
    run(tr('Creando la pista…'), async () => {
      const p = await api.instrumentTrackAdd(name || tr('Instrumento'), sourceOf(newKey, instruments))
      props.onProjectReplaced(p)
      setName('')
    })

  const importMidi = async () => {
    const path = await open({ multiple: false, filters: [{ name: 'MIDI', extensions: ['mid', 'midi'] }] })
    if (typeof path !== 'string') return
    await run(tr('Importando el MIDI…'), async () => props.onProjectReplaced(await api.midiImport(path)))
  }

  const change = (t: TrackView, key: string) =>
    run(tr('Generando «{name}»…', { name: t.name }), async () => props.onTrackReplaced(await api.instrumentSet(t.id, sourceOf(key, instruments, t.instrument))))

  return (
    <div className="panel transcription-panel">
      <section className="tx-section">
        <h3>
          <Icon name="piano" size={16} /> {tr('Instrumentos MIDI')}
        </h3>
        <p className="small muted">
          {tr('Una pista de instrumento toca notas MIDI con el sintetizador integrado o con un plugin instrumento (CLAP o VST3). Escribe las notas en su piano roll (doble clic añade una nota) o importa un archivo MIDI; el sonido se actualiza al editar y se mezcla, procesa y exporta como cualquier pista.')}
        </p>
        <div className="panel-row">
          <label className="field">
            <span className="small muted">{tr('Nombre')}</span>
            <input type="text" value={name} placeholder={tr('Instrumento')} onChange={(e) => setName(e.target.value)} />
          </label>
          <label className="field">
            <span className="small muted">{tr('Suena con')}</span>
            <SourceSelect value={newKey} onChange={setNewKey} instruments={instruments} />
          </label>
          <button className="btn primary" disabled={busy !== null} onClick={create}>
            <Icon name="plus" size={16} /> {tr('Nueva pista')}
          </button>
          <button className="btn" disabled={busy !== null} onClick={importMidi}>
            <Icon name="folder" size={16} /> {tr('Importar MIDI…')}
          </button>
        </div>
        {instruments.length === 0 && (
          <span className="small muted">
            Para usar plugins instrumento (Surge XT, Vital…), escanéalos en «Sistema»; aparecerán aquí.
          </span>
        )}
        {busy && <span className="small muted">{busy}</span>}
      </section>
      {tracks.length > 0 && (
        <section className="tx-section">
          <div className="tx-list">
            {tracks.map((t) => {
              const count = project.notes?.tracks.find((n) => n.track_id === t.id)?.notes.length ?? 0
              return (
                <div key={t.id} className="tx-item">
                  <span className="lane-chip" style={{ background: 'var(--track-instrument)' }} />
                  <strong className="ellipsis">{t.name}</strong>
                  <span className="small muted">{plural(count, '{n} nota', '{n} notas')}</span>
                  <div className="tx-actions">
                    <button
                      className={`btn small ${armed === t.id ? 'primary' : 'ghost'}`}
                      title={tr('Armar la pista: el teclado MIDI (o el del ordenador) toca su instrumento')}
                      disabled={busy !== null}
                      onClick={() => run(tr('Preparando el instrumento…'), () => live.arm(armed === t.id ? null : t.id))}
                    >
                      <Icon name="piano" size={14} /> {armed === t.id ? tr('Tocando') : tr('Tocar')}
                    </button>
                    {armed === t.id && t.instrument?.type === 'plugin' && (
                      <button
                        className="btn ghost small"
                        title={tr('Interfaz del plugin; al cerrarla se guarda su sonido y se regenera la pista')}
                        onClick={() => api.pluginOpenEditor(`inst:${t.id}`).catch((e) => props.onError(errorMessage(e)))}
                      >
                        <Icon name="sliders" size={14} /> {tr('Interfaz')}
                      </button>
                    )}
                    <SourceSelect value={keyOf(t.instrument)} onChange={(k) => change(t, k)} instruments={instruments} />
                    <button
                      className={`btn ghost small ${props.shownNotes.has(t.id) ? 'active' : ''}`}
                      onClick={() => props.onToggleNotes(t.id)}
                    >
                      {tr('Piano roll')}
                    </button>
                  </div>
                </div>
              )
            })}
          </div>
        </section>
      )}
      <LiveSection live={live} hasTracks={tracks.length > 0} armedName={tracks.find((t) => t.id === armed)?.name ?? null} />
    </div>
  )
}

/** Teclado MIDI, teclado del ordenador y grabación de lo tocado. */
function LiveSection({ live, hasTracks, armedName }: { live: LivePlay; hasTracks: boolean; armedName: string | null }) {
  const devices = live.status?.devices ?? []
  const open = live.status?.open ?? null
  return (
    <section className="tx-section">
      <h3>
        <Icon name="piano" size={16} /> {tr('Tocar en vivo')}
      </h3>
      <p className="small muted">
        {armedName
          ? tr('Lo que toques suena con «{name}», por sus efectos y su volumen, también con la canción parada.', { name: armedName })
          : hasTracks
            ? tr('Pulsa «Tocar» en una pista de instrumento para tocarla con un teclado MIDI o con el del ordenador.')
            : tr('Crea una pista de instrumento para tocarla en vivo.')}
      </p>
      <div className="panel-row">
        <label className="field">
          <span className="small muted">{tr('Teclado MIDI')}</span>
          <select value={open ? String(devices.indexOf(open)) : ''} onChange={(e) => live.openDevice(e.target.value === '' ? null : Number(e.target.value))}>
            <option value="">{devices.length ? tr('Ninguno') : tr('No hay teclados MIDI conectados')}</option>
            {devices.map((d, i) => (
              <option key={`${i}:${d}`} value={i}>
                {d}
              </option>
            ))}
          </select>
        </label>
        <button className="icon-btn" title={tr('Buscar teclados MIDI conectados')} onClick={live.refresh}>
          <Icon name="refresh" size={14} />
        </button>
        <label className="check small" title={tr('Las teclas A W S E D F T G Y H U J K O L P Ñ tocan una octava y media; Z y X cambian de octava')}>
          <input type="checkbox" checked={live.keyboard} onChange={(e) => live.setKeyboard(e.target.checked)} />
          {tr('Teclado del ordenador')}
        </label>
        {live.keyboard && (
          <>
            <span className="small mono" title={tr('Z: octava abajo · X: octava arriba')}>
              Octava {live.octave} (A = {noteName(12 * (live.octave + 1))})
            </span>
            <label className="field small">
              <span className="small muted">{tr('Fuerza')}</span>
              <input type="range" min={1} max={127} value={live.velocity} onChange={(e) => live.setVelocity(Number(e.target.value))} />
            </label>
          </>
        )}
      </div>
      <div className="panel-row">
        <button
          className={`btn record-btn ${live.recording ? 'on' : ''}`}
          disabled={!armedName}
          title={tr('Con la grabación activa, al reproducir se anota lo que tocas; al parar, se añade a la pista (Ctrl+Z lo deshace)')}
          onClick={() => live.setRecording(!live.recording)}
        >
          <Icon name="record" size={16} /> {live.recording ? tr('Grabando notas') : tr('Grabar notas')}
        </button>
        <button className="btn ghost small" title={tr('Suelta todas las notas (por si alguna se queda sonando)')} onClick={live.panic}>
          {tr('Soltar todo')}
        </button>
        {live.held.length > 0 && <span className="small mono">{live.held.map(noteName).join(' ')}</span>}
      </div>
      {live.recording && (
        <span className="small muted">{tr('Pulsa reproducir y toca. Cada vez que pares, lo tocado se añade a la pista en el sitio donde sonaba.')}</span>
      )}
    </section>
  )
}
