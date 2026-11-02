import type { TrackKind } from './api'

/** Color y etiqueta de cada tipo de pista. Los colores viven en styles.css. */
export const TRACK_KINDS: Record<TrackKind, { label: string; color: string }> = {
  mix: { label: 'Mezcla', color: 'var(--track-mix)' },
  vocals: { label: 'Voz', color: 'var(--track-vocals)' },
  backing_vocals: { label: 'Coros', color: 'var(--track-vocals)' },
  drums: { label: 'Batería', color: 'var(--track-drums)' },
  bass: { label: 'Bajo', color: 'var(--track-bass)' },
  guitar: { label: 'Guitarra', color: 'var(--track-guitar)' },
  piano: { label: 'Piano', color: 'var(--track-piano)' },
  other: { label: 'Otros', color: 'var(--track-other)' },
  generated: { label: 'Generada', color: 'var(--track-generated)' },
  recording: { label: 'Grabación', color: 'var(--track-recording)' },
  instrument: { label: 'Instrumento', color: 'var(--track-instrument)' },
}
