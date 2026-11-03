import type { TrackKind } from './api'
import { tr } from '../i18n'

/** Color y etiqueta de cada tipo de pista. Los colores viven en styles.css. */
export const TRACK_KINDS: Record<TrackKind, { label: string; color: string }> = {
  mix: { label: tr('Mezcla'), color: 'var(--track-mix)' },
  vocals: { label: tr('Voz'), color: 'var(--track-vocals)' },
  backing_vocals: { label: tr('Coros'), color: 'var(--track-vocals)' },
  drums: { label: tr('Batería'), color: 'var(--track-drums)' },
  bass: { label: tr('Bajo'), color: 'var(--track-bass)' },
  guitar: { label: tr('Guitarra'), color: 'var(--track-guitar)' },
  piano: { label: tr('Piano'), color: 'var(--track-piano)' },
  other: { label: tr('Otros'), color: 'var(--track-other)' },
  generated: { label: tr('Generada'), color: 'var(--track-generated)' },
  recording: { label: tr('Grabación'), color: 'var(--track-recording)' },
  instrument: { label: tr('Instrumento'), color: 'var(--track-instrument)' },
}
