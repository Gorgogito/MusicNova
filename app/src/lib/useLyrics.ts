import { useCallback, useEffect, useRef, useState } from 'react'
import { api, errorMessage, events, type Lyrics } from './api'
import { tr } from '../i18n'

export interface LyricsJob {
  jobId: string | null
  progress: number
  message: string
}

/** Sigue un trabajo de transcripción de letras (vive en App, como la separación). */
export function useLyrics(onApplied: (lyrics: Lyrics) => void, onError: (m: string) => void) {
  const [job, setJob] = useState<LyricsJob | null>(null)
  const jobRef = useRef<LyricsJob | null>(null)
  useEffect(() => {
    jobRef.current = job
  }, [job])

  useEffect(() => {
    const un = events.worker((ev) => {
      const current = jobRef.current
      if (!current?.jobId || !('job_id' in ev.data) || ev.data.job_id !== current.jobId) return
      switch (ev.event) {
        case 'job.progress':
          setJob({ ...current, progress: ev.data.progress, message: ev.data.message })
          break
        case 'job.done':
          api
            .lyricsApply()
            .then(onApplied)
            .catch((e) => onError(errorMessage(e)))
            .finally(() => setJob(null))
          break
        case 'job.failed':
          onError(tr('La transcripción falló: {error}', { error: ev.data.error }))
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
  }, [onApplied, onError])

  const start = useCallback(
    async (modelId: string | undefined, language: string, device: string) => {
      setJob({ jobId: null, progress: 0, message: tr('preparando audio…') })
      try {
        const { job_id } = await api.lyricsStart(modelId, language, device)
        setJob((j) => j && { ...j, jobId: job_id })
      } catch (e) {
        setJob(null)
        onError(errorMessage(e))
      }
    },
    [onError],
  )

  const cancel = useCallback(() => {
    const id = jobRef.current?.jobId
    if (id) api.worker('jobs.cancel', { job_id: id }).catch((e) => onError(errorMessage(e)))
  }, [onError])

  return { job, start, cancel }
}
