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
| 3. Análisis musical | Pulsos y compases con Beat This! (respaldo librosa), BPM y mapa de tempo, tonalidad (Krumhansl + encaje diatónico), acordes por HMM sobre croma de stems armónicos (7 tipos), secciones por novedad de Foote, metrónomo sobre pulsos reales, diagramas de guitarra/piano, cejilla, notación Do-Re-Mi, corrección manual | **Completada** (acordes con modelo neuronal BTC pendientes de verificar licencia) |
| 4. Letras | Whisper (faster-whisper: large-v3 turbo, large-v3, small) sobre el stem de voz con VAD y filtro de alucinaciones, 99 idiomas con detección automática, marcas por palabra, reparto en versos, karaoke con relleno palabra a palabra, editor (texto, sincronía por línea y global), exportación LRC, LRC por palabra, SRT y TXT | **Completada** |
| 5a. Efectos y bus máster | Crate mn-fx: EQ paramétrica de 6 bandas, compresor con rodilla, reverb Freeverb, delay ping-pong, limitador con anticipación; cadenas por pista y en el máster sin bloqueos y con cambios de parámetros en caliente; mismo DSP al exportar; LUFS momentáneo/corto plazo en directo e integrado, LRA y pico real al exportar (validado con EBU Tech 3341 y pyloudnorm) | **Completada** |
| 5b. Masterización | Limitador de pico real (interpolación ×4 de 32 tomas, ventana de mínimo ampliada); masterización automática a un objetivo LUFS (-14, -16, -11, -9, -23) con techo en dBTP por búsqueda de secante sobre la premezcla; master por referencia con Matchering como pista nueva con comparación A/B | **Completada** |
| 5c. Grabación | Entrada de audio (dispositivo y canales mono/estéreo), medidor de entrada, monitorización por software con control de deriva, tomas alineadas con las marcas de tiempo de captura y reproducción (ancla de salida filtrada), calibración de latencia por barridos y correlación cruzada (error medido ≤ 0,1 ms tras compensar), cuenta previa y metrónomo, grabación más allá del final, desfase de pista en el formato de proyecto, proyecto vacío y borrado de tomas | **Completada** (la latencia varía unos ms entre arranques de la app: se recomienda calibrar una vez por sesión) |
| 5d. Edición de clips | Clips no destructivos en el formato de proyecto (tramo del archivo, posición, fundidos sen²/cos², ganancia) con migración automática de proyectos anteriores; renderizado común para motor, mezcla, masterización y stems; mover, recortar por los bordes, dividir (S), borrar, fundidos con tiradores, deshacer/rehacer, ajuste a pulsos (Alt para desactivar) y zoom horizontal con desplazamiento y seguimiento del cabezal | **Completada** (los clips no pasan de una pista a otra) |
| 5e. Automatización | Carriles por pista y en el máster: volumen, panorama y cualquier parámetro continuo de los efectos (escala log para frecuencias y tiempos); interpolación lineal, efectos actualizados por bloque y volumen/panorama en rampa por muestra; edición de puntos con ajuste a pulsos, deshacer/rehacer compartido con los clips; el fader queda bloqueado mientras su curva actúa; el motor y la exportación coinciden (< 1e-5) | **Completada** |
| 5f. Plugins | Crate mn-plugin: alojamiento CLAP (clap-sys) y VST3 (vst3-rs, SDK 3.8 MIT) como efectos en cualquier cadena; escaneo aislado en un proceso aparte con tiempo límite y caché; interfaz nativa en ventana propia (redimensionable si el plugin lo permite); parámetros genéricos; estado guardado en el proyecto (componente + controlador en VST3); la instancia del motor se reutiliza al reconstruir la cadena y la exportación usa instancias propias con el estado actual; plugins ausentes suenan sin procesar con aviso; pruebas con un plugin propio que exporta ambos formatos | **Completada** (pendiente: automatizar parámetros de plugins, compensar su latencia, instrumentos/MIDI y sidechain; validar con plugins comerciales) |
| 6. Transcripción | Audio a MIDI, piano roll, afinación vocal, detección de instrumentos | Pendiente |
| 7. IA generativa | Stems de acompañamiento (ACE-Step/YuE), reducción de ruido | Pendiente |
| 8. Distribución | Instalador NSIS con Python embebido, actualizaciones, ES/EN | Pendiente |

## Requisitos de hardware

| Perfil | CPU | RAM | GPU | Disco |
|---|---|---|---|---|
| Mínimo | 4 núcleos AVX2 | 8 GB | No necesaria | 10 GB |
| Recomendado | 8 núcleos | 16 GB | 6–8 GB VRAM (CUDA o DirectML) | 25 GB SSD |
| Generación | 8+ núcleos | 32 GB | 12–24 GB VRAM | 50 GB+ |

Equipo de desarrollo actual: RTX 3050 de 8 GB. Es suficiente para separar y analizar; para generar
habrá que usar modelos cuantizados o fragmentos cortos.
