# Rellena el diálogo nativo de abrir/guardar de MusicNova sin tocar el teclado
# global: busca la ventana de diálogo (#32770) del proceso de MusicNova, escribe
# la ruta en su cuadro de nombre con WM_SETTEXT y pulsa «Abrir/Guardar» (IDOK).
# Uso: powershell -File file-dialog.ps1 -Path C:\ruta\archivo.wav [-TimeoutSec 10]
param([Parameter(Mandatory)] [string] $Path, [int] $TimeoutSec = 10)

Add-Type @"
using System; using System.Runtime.InteropServices; using System.Text; using System.Collections.Generic;
public static class Dlg {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc f, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr p, EnumProc f, IntPtr l);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassName(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr SendMessage(IntPtr h, uint m, IntPtr w, string l);
  [DllImport("user32.dll")] public static extern IntPtr SendMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern IntPtr GetParent(IntPtr h);
  public static string Cls(IntPtr h) { var s = new StringBuilder(128); GetClassName(h, s, 128); return s.ToString(); }
  public static List<IntPtr> Top(HashSet<uint> pids) {
    var r = new List<IntPtr>();
    EnumWindows((h, l) => { uint p; GetWindowThreadProcessId(h, out p); if (pids.Contains(p) && IsWindowVisible(h) && Cls(h) == "#32770") r.Add(h); return true; }, IntPtr.Zero);
    return r;
  }
  public static List<IntPtr> Children(IntPtr parent) {
    var r = new List<IntPtr>();
    EnumChildWindows(parent, (h, l) => { r.Add(h); return true; }, IntPtr.Zero);
    return r;
  }
}
"@

$pids = New-Object 'System.Collections.Generic.HashSet[uint32]'
Get-Process | Where-Object { $_.ProcessName -like 'musicnova*' } | ForEach-Object { [void]$pids.Add([uint32]$_.Id) }
if ($pids.Count -eq 0) { throw 'MusicNova no está en marcha' }

$deadline = (Get-Date).AddSeconds($TimeoutSec)
$dlg = $null
while (-not $dlg -and (Get-Date) -lt $deadline) {
  $found = [Dlg]::Top($pids)
  if ($found.Count -gt 0) { $dlg = $found[0] } else { Start-Sleep -Milliseconds 250 }
}
if (-not $dlg) { throw 'no apareció el diálogo de archivos de MusicNova' }

# Cuadro del nombre de archivo: el Edit dentro de un ComboBox (abrir) o el Edit directo (guardar).
$edit = [Dlg]::Children($dlg) | Where-Object { [Dlg]::Cls($_) -eq 'Edit' -and ([Dlg]::Cls([Dlg]::GetParent($_)) -match 'ComboBox') } | Select-Object -First 1
if (-not $edit) { $edit = [Dlg]::Children($dlg) | Where-Object { [Dlg]::Cls($_) -eq 'Edit' } | Select-Object -First 1 }
if (-not $edit) { throw 'el diálogo no tiene cuadro de nombre' }
$WM_SETTEXT = 0x000C; $WM_COMMAND = 0x0111; $IDOK = 1
[void][Dlg]::SendMessage($edit, $WM_SETTEXT, [IntPtr]::Zero, $Path)
Start-Sleep -Milliseconds 200
[void][Dlg]::SendMessage($dlg, $WM_COMMAND, [IntPtr]$IDOK, [IntPtr]::Zero)
"diálogo completado con $Path"
