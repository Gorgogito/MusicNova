# Pruebas de la interfaz con ratón y canciones reales

La interfaz se prueba desde fuera con eventos de ratón y teclado reales del
navegador: WebView2 expone el Chrome DevTools Protocol si se arranca con
`--remote-debugging-port`, y `app/scripts/ui-driver.mjs` lo usa para hacer
clic, arrastrar, escribir, pulsar teclas y capturar la ventana. Los diálogos
nativos de abrir y guardar se rellenan con `app/scripts/file-dialog.ps1`, que
busca la ventana de diálogo del proceso de MusicNova y le envía la ruta y la
orden «Abrir/Guardar» por su manejador (sin teclado global: no escribe nunca en
otra ventana).

```
cd app
set WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222
npx tauri dev
node scripts/ui-driver.mjs shot captura.png
node scripts/ui-driver.mjs click "texto=Importar audio o vídeo"
powershell -File scripts/file-dialog.ps1 -Path C:\musica\cancion.wav
node scripts/ui-driver.mjs drag 900,660 60 0        (arrastra un clip 60 px)
node scripts/ui-driver.mjs key z ctrl               (deshacer)
```

## Recorrido del 2026-11-02

Canción real: «Britpop» de MedleyDB (MusicDelta, 37 s, con voz). Todo con el
ratón y el teclado sobre la aplicación en marcha:

| Paso | Resultado |
|---|---|
| Importar con el diálogo nativo | Proyecto creado |
| Separar stems (HTDemucs) | 4 stems — tras corregir el bloqueo del proceso de IA (abajo) |
| Analizar | Sol mayor, 110 BPM, 4/4 con Beat This!; acordes BTC G C Cm G Em Bm… |
| Reproducir, pausar con Espacio | El tiempo avanza y se detiene |
| Doble clic en un acorde, escribir «Am7», Intro | Se corrige y se guarda |
| Arrastrar un clip, Ctrl+Z | Se mueve 2,72 s (ajustado al pulso) y se deshace |
| Clic en la pista y S | El clip se divide en el cabezal |
| Arrastrar el volumen, doble clic | −27 dB y vuelta a 0 dB |
| Letra (Whisper) | Inglés, 8 versos, karaoke palabra a palabra |
| Audio a MIDI del bajo, piano roll, arrastrar una nota | 45 notas; la nota sube 2 semitonos; Ctrl+Z |
| Limpiar la voz (DeepFilterNet3) | Pista «voz limpia» y original silenciada |
| Silenciar la batería y generar otra (ACE-Step) | Pista generada al nivel de la original |
| Exportar la mezcla a MP3 con el diálogo de guardar | MP3 de 36,8 s; aviso correcto de saturación |
| Práctica: −5 % y bucle arrastrando sobre la regla | 95 % y bucle 7,64–14,34 s ajustado a pulsos |

## Fallos encontrados y corregidos

- **El proceso de IA se quedaba parado** (grave). En Windows, mientras el hilo
  principal espera en `ReadFile` sobre la tubería de stdin, `GetFileType` sobre
  ese manejador se bloquea, y la carga de DLL como la de numpy lo consulta: un
  trabajo que importaba numpy en su hilo no avanzaba hasta la siguiente petición
  de la aplicación. El protocolo se lee ahora de un duplicado de stdin y el stdin
  del proceso apunta a `NUL`. Prueba de regresión en `test_worker.py`.
- La barra de herramientas del estudio no cabía (botones cortados y barra de
  desplazamiento horizontal en toda la página): ahora pasa a dos filas.
- El texto de los botones M/S/FX/A se partía (el relleno del botón no cabía en
  26 px); los botones de las pistas no quedaban en la misma columna cuando una
  pista tenía además el de piano roll.
- Los botones principales se veían grises (como desactivados) al pasar el ratón:
  la regla genérica de `:hover` les quitaba el degradado.
- El doble clic en el acorde grande lo seleccionaba como texto.
- Arrastrar una nota en vertical la movía también en el tiempo (el ajuste a la
  rejilla se aplicaba aunque no hubiera desplazamiento horizontal).
- Las curvas de tono del bajo tenían saltos falsos de hasta ±2 semitonos al
  principio y al final de las notas (la ventana de análisis mezcla la nota
  vecina): en instrumentos pulsados se ignoran los bordes (notas con más de 100
  cents: de 10 a 3). En la voz no, porque sus portamentos son reales.
- La batería generada entraba 4,7 dB más fuerte que la original y la mezcla
  saturaba: ahora se iguala su sonoridad a la de la pista que sustituye.
- Dos generaciones del mismo tipo tenían el mismo nombre: ahora se numeran.
- El tiempo transcurrido de una generación solo avanzaba con cada aviso de
  progreso.
- «Sistema» decía «No se detectó GPU NVIDIA» mientras aún estaba consultando el
  equipo.

Pendiente de probar con el ratón: grabación (necesita un micrófono), plugins con
interfaz propia y masterización por referencia.

## Uso real, 2026-11-05 (versión 0.1.0 instalada)

Dos problemas de uso detectados al trabajar con la aplicación:

- **Los deslizadores de los efectos eran imprecisos.** Tenían 1000 posiciones en
  unos 100 píxeles, y en frecuencias cada píxel saltaba 10–20 Hz: volver a 120 Hz
  después de subir a 1 kHz era casi imposible. Todos los parámetros (efectos,
  plugins, volumen y panorama de pista, ganancia del máster) usan ahora un control
  común (`ParamControl`):
  - clic en el valor para escribirlo: «120», «1.5k», «1,2 kHz», «-3 dB», «25 %»,
    «1.5 s», «apagado», «I30» o «D50» en el panorama, o el texto que entienda el
    plugin («-6 dB»), mediante `text_to_value` de CLAP y `getParamValueByString`
    de VST3;
  - botones − / + (no aparecen en las cabeceras de pista, por falta de sitio).
    En frecuencias el paso es un semitono (con Shift, una octava); en otras
    escalas logarítmicas, el 1 % del recorrido; en las lineales, su paso. Si se
    mantienen pulsados, se repiten cada vez más rápido;
  - Shift al arrastrar: diez veces más lento. Doble clic: valor por defecto;
  - flechas, Re Pág / Av Pág, Inicio / Fin con el foco. Ya no mueven además el cabezal;
  - Ctrl + rueda: paso a paso. Sin Ctrl, la rueda desplaza el panel.
- **El bucle creado al arrastrar sobre la regla no se podía quitar** (solo
  desactivar en Práctica). Ahora se quita con la × de la regla, con Esc en el
  estudio (si no hay ningún clip seleccionado), con doble clic dentro del bucle en
  la regla o con «Quitar» en Práctica.

Comprobado en la aplicación: escribir 1k y 120 en «Graves · frec.», − / + y
Ctrl + rueda por semitonos, flechas sin mover el cabezal, doble clic a 120 Hz, texto
no válido marcado en rojo y Esc para cancelar, volumen de pista escrito a mano, y
las cuatro formas de quitar el bucle.
