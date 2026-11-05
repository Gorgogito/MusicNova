# MusicNova: análisis y plan de desarrollo

## Decisiones confirmadas (31/10/2026)

| Tema | Decisión |
|---|---|
| Stack | Tauri 2 + Rust (núcleo y motor de audio) + React/TypeScript + proceso de IA en Python |
| Licencia | GPL-3.0-or-later (permite Rubber Band, Matchering y Chordino) |
| Plataforma inicial | Windows |
| IA generativa | Incluida (ACE-Step / YuE para stems de acompañamiento) |

## Inventario frente a Moises

| Área | Moises | MusicNova |
|---|---|---|
| Separación | Voz, coros, batería, bajo, guitarra, piano, cuerdas, vientos, otros; diálogo/música/efectos | Igual, eligiendo el modelo; batería por piezas y combinaciones de modelos |
| Análisis | Acordes, tonalidad, BPM, compases, secciones | Igual, más escala/modo, mapa de tempo y edición manual |
| Práctica | Velocidad, tono, bucles, metrónomo, cuenta previa, diagramas de acordes | Igual, más entrenador de velocidad, cejilla y transposición de acordes |
| Letras | Transcripción sincronizada en más de 30 idiomas | Igual, con editor y exportación LRC/SRT |
| Producción | Masterización, AI Studio, Voice Studio | Masterización por referencia, LUFS, generación de stems |
| Edición | Mezcla básica | Multipista, grabación, efectos, automatización, plugins CLAP/VST3 |
| Transcripción | Limitada | Audio a MIDI, piano roll, afinación vocal, detección de instrumentos |
| Exportación | Stems y mezcla | WAV/FLAC/MP3/OGG, MIDI, LRC, formato de proyecto abierto |

## Modelos por función (verificar la licencia de cada checkpoint)

| Función | Principal | Licencia |
|---|---|---|
| Separación | HTDemucs / htdemucs_6s / htdemucs_ft | MIT |
| Voz de máxima calidad | BS-RoFormer / Mel-Band RoFormer | Código MIT; pesos variables |
| Pulsos, BPM y secciones | Beat This! (código y pesos MIT, verificado); secciones propias | MIT |
| Acordes | HMM propio sobre croma CQT con plantillas armónicas (fase 3); BTC opcional | Propio / por verificar |
| Tonalidad | Perfiles Krumhansl/Temperley y CNN propio | Propio |
| Letras | faster-whisper (CTranslate2) con marcas por palabra propias; stable-ts no hizo falta | MIT |
| Instrumentos | PANNs, CLAP | MIT / Apache |
| Audio a MIDI | Basic Pitch | Apache 2.0 |
| Afinación vocal | torchcrepe / RMVPE | MIT |
| Masterización | Matchering + EBU R128 | GPL-3 / MIT |
| Reducción de ruido | DeepFilterNet | MIT / Apache |
| Generación | ACE-Step, YuE | Apache 2.0 |
| Tiempo y tono | Signalsmith Stretch, Rubber Band | MIT / GPL |

## Fases

| Fase | Contenido | Estado |
|---|---|---|
| 0. Bases | Workspace, motor de audio, formato de proyecto, worker de IA con JSON-RPC, trabajos y gestor de modelos, interfaz base | **Completada** |
| 1a. Separación y mezclador | Demucs 4/6 stems y htdemucs_ft en GPU/CPU, stems como pistas, exportación WAV de mezcla y stems | **Completada** |
| 1b. Ampliación | Mel-Band RoFormer para voz (MIT verificada en la fuente), exportación FLAC/MP3, importar vídeo con FFmpeg LGPL bajo demanda | **Completada** |
| 2. Práctica | Tempo (25–200 %) y tono (±12 st) en tiempo real con Signalsmith Stretch, bucle A-B, metrónomo con tap tempo, cuenta previa, entrenador de velocidad, marcadores, atajos | **Completada** (el metrónomo usa BPM manual hasta la fase 3) |
| 3. Análisis musical | Pulsos y compases con Beat This! (respaldo librosa), BPM y mapa de tempo, tonalidad (Krumhansl + encaje diatónico), acordes por HMM sobre croma de stems armónicos (7 tipos), secciones por novedad de Foote, metrónomo sobre pulsos reales, diagramas de guitarra/piano, cejilla, notación Do-Re-Mi, corrección manual | **Completada**. Ampliada después: acordes con BTC (MIT, 14 tipos × 12 raíces, decodificados por pulso) cuando el modelo está instalado; con GuitarSet acierta el tipo exacto un 61,9 % del tiempo frente al 40,5 % de las plantillas |
| 4. Letras | Whisper (faster-whisper: large-v3 turbo, large-v3, small) sobre el stem de voz con VAD y filtro de alucinaciones, 99 idiomas con detección automática, marcas por palabra, reparto en versos, karaoke con relleno palabra a palabra, editor (texto, sincronía por línea y global), exportación LRC, LRC por palabra, SRT y TXT | **Completada** |
| 5a. Efectos y bus máster | Crate mn-fx: EQ paramétrica de 6 bandas, compresor con rodilla, reverb Freeverb, delay ping-pong, limitador con anticipación; cadenas por pista y en el máster sin bloqueos y con cambios de parámetros en caliente; mismo DSP al exportar; LUFS momentáneo/corto plazo en directo e integrado, LRA y pico real al exportar (validado con EBU Tech 3341 y pyloudnorm) | **Completada** |
| 5b. Masterización | Limitador de pico real (interpolación ×4 de 32 tomas, ventana de mínimo ampliada); masterización automática a un objetivo LUFS (-14, -16, -11, -9, -23) con techo en dBTP por búsqueda de secante sobre la premezcla; master por referencia con Matchering como pista nueva con comparación A/B | **Completada** |
| 5c. Grabación | Entrada de audio (dispositivo y canales mono/estéreo), medidor de entrada, monitorización por software con control de deriva, tomas alineadas con las marcas de tiempo de captura y reproducción (ancla de salida filtrada), calibración de latencia por barridos y correlación cruzada (error medido ≤ 0,1 ms tras compensar), cuenta previa y metrónomo, grabación más allá del final, desfase de pista en el formato de proyecto, proyecto vacío y borrado de tomas | **Completada** (la latencia varía unos ms entre arranques de la app: se recomienda calibrar una vez por sesión) |
| 5d. Edición de clips | Clips no destructivos en el formato de proyecto (tramo del archivo, posición, fundidos sen²/cos², ganancia) con migración automática de proyectos anteriores; renderizado común para motor, mezcla, masterización y stems; mover, recortar por los bordes, dividir (S), borrar, fundidos con tiradores, deshacer/rehacer, ajuste a pulsos (Alt para desactivar) y zoom horizontal con desplazamiento y seguimiento del cabezal | **Completada** (los clips no pasan de una pista a otra) |
| 5e. Automatización | Carriles por pista y en el máster: volumen, panorama y cualquier parámetro continuo de los efectos (escala log para frecuencias y tiempos); interpolación lineal, efectos actualizados por bloque y volumen/panorama en rampa por muestra; edición de puntos con ajuste a pulsos, deshacer/rehacer compartido con los clips; el fader queda bloqueado mientras su curva actúa; el motor y la exportación coinciden (< 1e-5) | **Completada** |
| 5f. Plugins | Crate mn-plugin: alojamiento CLAP (clap-sys) y VST3 (vst3-rs, SDK 3.8 MIT) como efectos en cualquier cadena; escaneo aislado en un proceso aparte con tiempo límite y caché; interfaz nativa en ventana propia (redimensionable si el plugin lo permite); parámetros genéricos; estado guardado en el proyecto (componente + controlador en VST3); la instancia del motor se reutiliza al reconstruir la cadena y la exportación usa instancias propias con el estado actual; plugins ausentes suenan sin procesar con aviso; pruebas con un plugin propio que exporta ambos formatos y validación con plugins libres reales (Surge XT Effects, Airwindows Consolidated, Dragonfly Reverb) mediante una sonda de compatibilidad, ver [PLUGINS.md](PLUGINS.md) | **Completada**. Ampliada después: compensación de latencia (PDC) de plugins y efectos en el motor y en la exportación, sidechain en el compresor (clave = clips crudos de otra pista) y pistas de instrumento (ver fila 5g). cada plugin en su propio proceso (si se cae o se cuelga, su efecto pasa sin procesar, la app sigue y se puede reiniciar). automatización de parámetros de plugins (ver [PLUGINS.md](PLUGINS.md)) |
| 5g. Instrumentos MIDI | Tipo de pista «instrumento»: sus notas (piano roll) suenan con el sintetizador integrado (piano eléctrico, colchón, melodía, cuerda pulsada, bajo, batería GM) o con un plugin instrumento CLAP/VST3 (eventos de nota, sin entrada de audio); el audio se genera al editar y se cambia en el motor sin recargar, así que mezcla, efectos y exportación funcionan igual; importación de MIDI (formatos 0 y 1, mapa de tempo, pitch bend) con una pista por parte y timbre según el programa GM; validado con Surge XT 1.3.4 (CLAP y VST3). Ampliada: tocar en vivo con un teclado MIDI o el del ordenador (sintetizador integrado o plugin en su proceso, por los efectos de la pista) y grabar lo tocado como notas en la posición que se oía | **Completada** |
| 6. Transcripción | Audio a MIDI con Basic Pitch (ONNX, polifónico o monofónico, sensibilidad ajustable) por pista; piano roll bajo cada pista con curva de tono superpuesta y edición (mover, alargar, añadir, borrar, flechas) integrada en deshacer/rehacer; exportación MIDI de formato 1 con mapa de tempo de los pulsos detectados y compases alineados; síntesis de las notas como pista nueva para escucharlas; afinación vocal: curva de tono con pYIN, medidas respecto a la escala (afinado/alto/bajo, desviación media) y voz afinada con Signalsmith Stretch conservando formantes, de natural (centra cada nota y conserva el vibrato) a robótica; detección de instrumentos con PANNs (21 grupos con tramos temporales) | **Completada**. Ampliada después: tono vocal con CREPE (torchcrepe, con decodificación fina propia; pYIN queda como alternativa), curvas de tono por nota exportadas como pitch bend, batería a MIDI (bombo, caja y charles con NMF semiadaptativa), detección de instrumentos sobre los stems separados y medidas con datos reales en `docs/TRANSCRIPCION.md` |
| 7. IA generativa | Limpieza de voz con DeepFilterNet3 (ONNX con procesado de libDF portado a numpy, por trozos, límite de atenuación); reducción de ruido constante en cualquier pista (Wiener con SNR a priori dirigida por decisión, perfil del bucle A-B o de los tramos silenciosos); acompañamientos con ACE-Step 1.5 base (tarea «lego»: 12 tipos de pista sobre lo que suena o la mezcla original, con tempo, compás y tonalidad del análisis, calidad y semilla) en un entorno de Python aparte instalable desde la aplicación; resultados como pistas nuevas; medidas con audio real en `docs/GENERACION.md` | **Completada** (YuE descartado: necesita ~24 GB de GPU) |
| 8. Distribución | Instalador NSIS con Python embebido, actualizaciones, ES/EN | **Completada**: interfaz en español e inglés hecha (catálogos al estilo gettext, mensajes de Rust y del proceso de IA traducidos en la interfaz, nombres de datos en el idioma elegido, comprobación con `npm run i18n:check`; ver [IDIOMAS.md](IDIOMAS.md)). Instalador NSIS por usuario hecho (17 MB con Python 3.12 incluido; dependencias de cada función y PyTorch CUDA/CPU según la GPU instalados la primera vez; ver [INSTALADOR.md](INSTALADOR.md)). Actualizaciones desde GitHub Releases hechas (firmadas, verificadas e instaladas en modo pasivo; ver [ACTUALIZACIONES.md](ACTUALIZACIONES.md)). Sin firma de código de Windows por ahora |

## Requisitos de hardware

| Perfil | CPU | RAM | GPU | Disco |
|---|---|---|---|---|
| Mínimo | 4 núcleos AVX2 | 8 GB | No necesaria | 10 GB |
| Recomendado | 8 núcleos | 16 GB | 6–8 GB VRAM (CUDA o DirectML) | 25 GB SSD |
| Generación | 8+ núcleos | 32 GB | 12–24 GB VRAM | 50 GB+ |

Equipo de desarrollo actual: RTX 3050 de 8 GB. Es suficiente para separar y analizar; para generar
habrá que usar modelos cuantizados o fragmentos cortos.
