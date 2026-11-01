// Iconos de trazo simple dibujados a mano (sin dependencias externas).
const PATHS: Record<string, string> = {
  play: 'M7 4.5v15l12-7.5z',
  pause: 'M7 4h3.5v16H7zM13.5 4H17v16h-3.5z',
  stop: 'M6 6h12v12H6z',
  back: 'M18 5v14l-9-7zM6 5h2v14H6z',
  folder: 'M3 6.5A1.5 1.5 0 0 1 4.5 5h4l2 2h9A1.5 1.5 0 0 1 21 8.5v9a1.5 1.5 0 0 1-1.5 1.5h-15A1.5 1.5 0 0 1 3 17.5z',
  studio: 'M4 7h16M4 12h16M4 17h16M8 5v4M15 10v4M11 15v4',
  model: 'M12 3l8 4.5v9L12 21l-8-4.5v-9zM12 12l8-4.5M12 12v9M12 12L4 7.5',
  system: 'M4 5h16v11H4zM9 20h6M12 16v4',
  plus: 'M12 5v14M5 12h14',
  download: 'M12 4v11m0 0l-4.5-4.5M12 15l4.5-4.5M5 19h14',
  trash: 'M5 7h14M10 11v6M14 11v6M7 7l1 12h8l1-12M9.5 7V4.5h5V7',
  refresh: 'M19 12a7 7 0 1 1-2.05-4.95M19 4v4h-4',
  x: 'M6 6l12 12M18 6L6 18',
  music: 'M9 18V6l11-2v12M9 18a3 3 0 1 1-6 0 3 3 0 0 1 6 0zM20 16a3 3 0 1 1-6 0 3 3 0 0 1 6 0z',
  mic: 'M12 3a3 3 0 0 1 3 3v6a3 3 0 0 1-6 0V6a3 3 0 0 1 3-3zM5.5 11a6.5 6.5 0 0 0 13 0M12 17.5V21M8.5 21h7',
  pin: 'M12 3v4M12 17v4M3 12h4M17 12h4M12 9a3 3 0 1 1 0 6 3 3 0 0 1 0-6z',
  lyrics: 'M4 6h16M4 10h10M4 14h16M4 18h7M17 15v5M17 15l3-1',
  sliders: 'M6 4v16M12 4v16M18 4v16M4 9h4M10 15h4M16 7h4',
  chord: 'M5 4v16M9.5 4v16M14 4v16M18.5 4v16M5 8h13.5M5 12h13.5M9.5 10h.01M14 14h.01M18.5 10h.01',
}

const FILLED = new Set(['play', 'pause', 'stop', 'back'])

export function Icon({ name, size = 18 }: { name: keyof typeof PATHS | string; size?: number }) {
  const filled = FILLED.has(name)
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill={filled ? 'currentColor' : 'none'}
      stroke={filled ? 'none' : 'currentColor'}
      strokeWidth={1.8}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <path d={PATHS[name]} />
    </svg>
  )
}
