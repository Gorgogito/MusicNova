# Actualizaciones

MusicNova se actualiza desde las [releases de GitHub](https://github.com/Gorgogito/MusicNova/releases)
con el plugin oficial de Tauri (`tauri-plugin-updater`).

## Cómo funciona

- Cada versión publica tres archivos: el instalador `MusicNova_X.Y.Z_x64-setup.exe`, su firma
  `.sig` y `latest.json` (versión, notas, fecha, URL del instalador y su firma).
- La app consulta `https://github.com/Gorgogito/MusicNova/releases/latest/download/latest.json`
  al arrancar (8 s después, si «Buscar al arrancar» está activado en Sistema) o al pulsar «Buscar
  actualizaciones».
- Si hay una versión nueva aparece un aviso con «Instalar y reiniciar». La app guarda el estado de
  los plugins, descarga el instalador y verifica su firma con la clave pública incluida en
  `tauri.conf.json`. Una firma que no coincide se rechaza. Después ejecuta el instalador en modo
  pasivo (solo una barra de progreso) y se reinicia con la versión nueva.
- Los proyectos, los modelos y los entornos de IA no se tocan: viven en otras carpetas (ver
  [INSTALADOR.md](INSTALADOR.md)).
- Solo se aceptan direcciones `https` en las compilaciones normales. `MUSICNOVA_UPDATE_URL` apunta
  a otro `latest.json`; con `http`, eso solo funciona en compilaciones de depuración (pruebas locales).

## La clave de firma

- Privada: `%USERPROFILE%\.tauri\musicnova.key` (sin contraseña). **No se sube al repositorio y
  hay que guardar una copia de seguridad**: sin ella no se pueden firmar actualizaciones que
  acepten las copias ya instaladas. Habría que publicar una versión nueva con otra clave, que los
  usuarios tendrían que instalar a mano.
- Pública: `%USERPROFILE%\.tauri\musicnova.key.pub`, copiada en `plugins.updater.pubkey` de
  `app/src-tauri/tauri.conf.json`.

## Publicar una versión

```
cd app
node scripts/set-version.mjs 0.2.0          # tauri.conf.json, package.json, Cargo.toml, ai-worker
set TAURI_SIGNING_PRIVATE_KEY=%USERPROFILE%\.tauri\musicnova.key
set TAURI_SIGNING_PRIVATE_KEY_PASSWORD=
npx tauri build                              # instalador + .sig
node scripts/release.mjs --notes notas.md            # genera latest.json y lo comprueba
node scripts/release.mjs --notes notas.md --publish  # crea la release v0.2.0 en GitHub (gh)
```

Sin la clave, `tauri build` falla al firmar: el proyecto exige artefactos de actualización
(`createUpdaterArtifacts`).

## Verificación (noviembre de 2026)

Prueba local de punta a punta. Se instaló un cliente 0.1.0 (compilación de depuración, para poder
usar `http`) y se sirvió una 0.1.1 firmada desde `http://127.0.0.1:8765`:

- al arrancar, el aviso «MusicNova 0.1.1 está disponible (tienes la 0.1.0)»; en Sistema, las notas;
- «Instalar y reiniciar»: descarga, firma correcta, instalación pasiva y la app se reinició sola
  con la 0.1.1 (versión del ejecutable, del registro y de la app); datos intactos;
- la 0.1.1 (compilación normal) rechazó el servidor `http`.

Fallo encontrado y corregido: el selector de idioma del instalador aparecía también en las
actualizaciones y las dejaba esperando. Se quitó; NSIS elige el idioma del sistema entre español e
inglés.
