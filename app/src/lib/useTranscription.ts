import { useCallback, useEffect, useRef, useState } from 'react'
import { api, errorMessage, events, type InstrumentReport, type NotesMode, type PitchCurve, type PitchEngine, type Transcription } from './api'

export type TranscriptionKind = 'notes' | 'pitch' | 'instruments'

export interface TranscriptionJob {
  kind: TranscriptionKind
  trackId: string | null
  jobId: string | null
  progress: number
  message: string
}

export const JOB_LABEL: Record<TranscriptionKind, string> = {
  notes: 'Transcribiendo notas',
  pitch: 'Analizando la afinación',
  instruments: 'Detectando instrumentos',
}

export interface TranscriptionHandlers {
  onNotes: (t: Transcription) => void
  onPitch: (c: PitchCurve) => void
  onInstruments: (r: InstrumentReport) => void
}

/** Sigue el trabajo de transcripción en curso (uno a la vez; vive en App). */
export function useTranscription(handlers: TranscriptionHandlers, onError: (m: string) => void) {
  const [job, setJob] = useState<TranscriptionJob | null>(null)
  const jobRef = useRef<TranscriptionJob | null>(null)
  const handlersRef = useRef(handlers)
  useEffect(() => {
    jobRef.current = job
    handlersRef.current = handlers
  })

  useEffect(() => {
    const un = events.worker((ev) => {
      const current = jobRef.current
      if (!current?.jobId || !('job_id' in ev.data) || ev.data.job_id !== current.jobId) return
      switch (ev.event) {
        case 'job.progress':
          setJob({ ...current, progress: ev.data.progress, message: ev.data.message })
          break
        case 'job.done': {
          const h = handlersRef.current
          const apply =
            current.kind === 'notes'
              ? api.notesApply(current.trackId!).then(h.onNotes)
              : current.kind === 'pitch'
                ? api.pitchApply(current.trackId!).then(h.onPitch)
                : api.instrumentsApply().then(h.onInstruments)
          apply.catch((e) => onError(errorMessage(e))).finally(() => setJob(null))
          break
        }
        case 'job.failed':
          onError(`${JOB_LABEL[current.kind]}: falló (${ev.data.error})`)
          setJob(null)
          break
        case 'job.cancelled':
          setJob(null)
          break
      }
    })
    return () => {
      un.then((f) => f())
    }
  }, [onError])

  const run = useCallback(
    async (kind: TranscriptionKind, trackId: string | null, start: () => Promise<{ job_id: string }>) => {
      if (jobRef.current) {
        onError('Ya hay una transcripción en curso')
        return
      }
      const pending = { kind, trackId, jobId: null, progress: 0, message: 'preparando audio…' }
      jobRef.current = pending
      setJob(pending)
      try {
        const { job_id } = await start()
        setJob((j) => j && { ...j, jobId: job_id })
      } catch (e) {
        jobRef.current = null
        setJob(null)
        onError(errorMessage(e))
      }
    },
    [onError],
  )

  const startNotes = useCallback(
    (trackId: string, mode: NotesMode, sensitivity: number, device: string) =>
      run('notes', trackId, () => api.notesStart(trackId, mode, sensitivity, device)),
    [run],
  )
  const startPitch = useCallback(
    (trackId: string, engine: PitchEngine) => run('pitch', trackId, () => api.pitchStart(trackId, engine)),
    [run],
  )
  const startInstruments = useCallback(
    (modelId: string | undefined, device: string) => run('instruments', null, () => api.instrumentsStart(modelId, device)),
    [run],
  )
  const cancel = useCallback(() => {
    const id = jobRef.current?.jobId
    if (id) api.worker('jobs.cancel', { job_id: id }).catch((e) => onError(errorMessage(e)))
  }, [onError])

  return { job, startNotes, startPitch, startInstruments, cancel }
}

export type TranscriptionJobs = ReturnType<typeof useTranscription>
