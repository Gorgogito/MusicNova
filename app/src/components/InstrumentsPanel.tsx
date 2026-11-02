import { useState } from 'react'
import { open } from '@tauri-apps/plugin-dialog'
import { api, errorMessage, type InstrumentSource, type PluginDescriptor, type ProjectView, type SynthTimbre, type TrackView } from '../lib/api'
import { FORMAT_LABEL, usePlugins } from '../lib/plugins'
import { Icon } from './Icon'

interface Props {
  project: ProjectView
  shownNotes: Set<string>
  onToggleNotes: (trackId: string) => void
  onProjectReplaced: (p: ProjectView) => void
  onTrackReplaced: (t: TrackView) => void
  onError: (m: string) => void
}

const TIMBRES: { id: SynthTimbre; label: string }[] = [
  { id: 'keys', label: 'Piano eléctrico' },
  { id: 'pad', label: 'Colchón (pad)' },
  { id: 'lead', label: 'Melodía (lead)' },
  { id: 'pluck', label: 'Cuerda pulsada' },
  { id: 'bass', label: 'Bajo' },
  { id: 'drums', label: 'Batería (mapa General MIDI)' },
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
      <optgroup label="Sintetizador integrado">
        {TIMBRES.map((t) => (
          <option key={t.id} value={`synth:${t.id}`}>
            {t.label}
          </option>
        ))}
      </optgroup>
      {instruments.length > 0 && (
        <optgroup label="Plugins instrumento">
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
    run('Creando la pista…', async () => {
      const p = await api.instrumentTrackAdd(name || 'Instrumento', sourceOf(newKey, instruments))
      props.onProjectReplaced(p)
      setName('')
    })

  const importMidi = async () => {
    const path = await open({ multiple: false, filters: [{ name: 'MIDI', extensions: ['mid', 'midi'] }] })
    if (typeof path !== 'string') return
    await run('Importando el MIDI…', async () => props.onProjectReplaced(await api.midiImport(path)))
  }

  const change = (t: TrackView, key: string) =>
    run(`Generando «${t.name}»…`, async () => props.onTrackReplaced(await api.instrumentSet(t.id, sourceOf(key, instruments, t.instrument))))

  return (
    <div className="panel transcription-panel">
      <section className="tx-section">
        <h3>
          <Icon name="piano" size={16} /> Instrumentos MIDI
        </h3>
        <p className="small muted">
          Una pista de instrumento toca notas MIDI con el sintetizador integrado o con un plugin instrumento (CLAP o VST3).
          Escribe las notas en su piano roll (doble clic añade una nota) o importa un archivo MIDI; el sonido se actualiza
          al editar y se mezcla, procesa y exporta como cualquier pista.
        </p>
        <div className="panel-row">
          <label className="field">
            <span className="small muted">Nombre</span>
            <input type="text" value={name} placeholder="Instrumento" onChange={(e) => setName(e.target.value)} />
          </label>
          <label className="field">
            <span className="small muted">Suena con</span>
            <SourceSelect value={newKey} onChange={setNewKey} instruments={instruments} />
          </label>
          <button className="btn primary" disabled={busy !== null} onClick={create}>
            <Icon name="plus" size={16} /> Nueva pista
          </button>
          <button className="btn" disabled={busy !== null} onClick={importMidi}>
            <Icon name="folder" size={16} /> Importar MIDI…
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
                  <span className="small muted">{count} notas</span>
                  <div className="tx-actions">
                    <SourceSelect value={keyOf(t.instrument)} onChange={(k) => change(t, k)} instruments={instruments} />
                    <button
                      className={`btn ghost small ${props.shownNotes.has(t.id) ? 'active' : ''}`}
                      onClick={() => props.onToggleNotes(t.id)}
                    >
                      Piano roll
                    </button>
                  </div>
                </div>
              )
            })}
          </div>
        </section>
      )}
    </div>
  )
}
