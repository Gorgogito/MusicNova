# Idiomas (español e inglés)

La interfaz está en español e inglés. El idioma se elige en «Sistema» (Idioma · Language); la
primera vez se toma el del sistema operativo. Cambiarlo recarga la ventana.

## Cómo funciona

Al estilo gettext: el texto español del código es la clave y cada idioma tiene un catálogo que lo
traduce. Si una frase no está en el catálogo se muestra en español.

- **Interfaz** (`app/src`): las frases van envueltas en `tr('…')`, con huecos con nombre
  (`tr('Marcador {n}', { n })`), y los plurales en `plural(n, '{n} pista', '{n} pistas')`. El
  catálogo inglés es `app/src/i18n/en.ts`. La función se llama `tr` y no `t` porque muchas
  variables del código se llaman `t` (pistas).
- **Mensajes del backend**: los errores de Rust, el progreso y los errores del proceso de IA y las
  descripciones de los modelos llegan en español y se traducen en la interfaz con
  `app/src/i18n/backend.ts`, en los puntos de entrada (`errorMessage()`, los eventos de trabajos,
  los avisos de plugins). Las claves con huecos `{1}`, `{2}`… son patrones; lo capturado también se
  traduce, así que funcionan los errores encadenados («no se pudo cargar X: <otro error>»). El
  código de Rust y de Python no cambia para traducir mensajes.
- **Datos que crea el backend** (nombres de pistas y proyectos: «Batería», «Toma 3», «… · sin
  ruido»): se crean en el idioma de la interfaz, que esta comunica a Rust al arrancar
  (`app_set_language`; tabla en `app/src-tauri/src/i18n.rs`). Los proyectos ya creados conservan sus
  nombres: son datos del usuario.
- Las notas en solfeo (Do Re Mi) son una notación que elige el usuario, no un idioma.

## Comprobar los catálogos

```
cd app
npm run i18n:check
```

Extrae todas las frases marcadas de la interfaz y los mensajes en español de Rust (`crates/`,
`app/src-tauri/`), del proceso de IA (`ai-worker/musicnova_ai`) y de los manifiestos de modelos, y
avisa de las que faltan o sobran en cada catálogo (código de salida 1 si falta alguna). En
desarrollo, la consola también avisa de cada frase sin traducción que se muestra.

## Añadir un idioma

1. Copiar `en.ts` y `backend.ts` con el código del idioma y traducir los valores.
2. Añadirlo a `LANGS` y a `CATALOGS` en `app/src/i18n/index.ts` (y su `locale()`).
3. Añadir sus nombres en `app/src-tauri/src/i18n.rs`.

## Verificación (noviembre de 2026)

En la app real, con la interfaz en inglés, un recorrido por todas las vistas y paneles buscando
texto en español en el DOM (texto visible, `title`, `placeholder` y opciones) solo encontró datos
del proyecto (nombres de pistas creadas en español) y el nombre del micrófono que da Windows. Un
proyecto nuevo en inglés se crea como «New project» y una pista de instrumento sin nombre como
«Instrument».
