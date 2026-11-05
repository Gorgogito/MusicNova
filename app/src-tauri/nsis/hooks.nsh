; Ganchos del instalador NSIS de MusicNova.

!macro NSIS_HOOK_PREINSTALL
  ; La carpeta por defecto de Tauri en modo «por usuario» ($LOCALAPPDATA\MusicNova)
  ; es también la de datos de la app (modelos de IA, entornos de Python). El
  ; programa va aparte, en $LOCALAPPDATA\Programs\MusicNova, como las demás apps
  ; por usuario: actualizar o desinstalar nunca toca los datos.
  ${If} $INSTDIR == "$LOCALAPPDATA\${PRODUCTNAME}"
    StrCpy $INSTDIR "$LOCALAPPDATA\Programs\${PRODUCTNAME}"
    CreateDirectory "$INSTDIR"
    SetOutPath "$INSTDIR"
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ; Python y el código del proceso de IA son solo del programa: se quitan enteros
  ; (incluidas cachés que Python haya escrito). Los datos ($LOCALAPPDATA\MusicNova:
  ; modelos, entornos) no se tocan.
  RMDir /r "$INSTDIR\python"
  RMDir /r "$INSTDIR\ai-worker"
  RMDir "$INSTDIR"
!macroend
