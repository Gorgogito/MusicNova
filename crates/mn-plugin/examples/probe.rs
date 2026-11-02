//! Sonda de compatibilidad con plugins reales.
//!
//! `cargo run -p mn-plugin --example probe -- <carpeta o plugin>... [--no-gui]`
//!
//! Reproduce lo que hace la aplicación: el hilo principal del proceso atiende
//! las tareas de los plugins y bombea los mensajes de Windows (sus interfaces
//! los necesitan), el audio se procesa en otro hilo mientras se abre la
//! interfaz y se mueven parámetros, y el estado se guarda y restaura en otra
//! instancia. Imprime un informe por plugin y termina con código 1 si algo falla.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use mn_fx::Processor;
use mn_plugin::{find_candidates, scan_file, set_main_thread_executor, Instance, PluginDescriptor, PluginProcessor};

type Task = Box<dyn FnOnce() + Send>;

const RATE: u32 = 48_000;

/// Latidos del bucle principal, para el vigilante.
static MAIN_BEATS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// Plugin que se está probando (para decir cuál bloqueó el hilo principal).
static CURRENT_PLUGIN: Mutex<String> = Mutex::new(String::new());
/// Segundos sin vueltas del bucle principal que se consideran un bloqueo.
const STALL_SECS: u32 = 10;
/// Mensaje de Windows que se está despachando (para el vigilante).
static CURRENT_MSG: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Señal de prueba estéreo: seno de 220 Hz a la izquierda, 330 Hz a la derecha.
fn signal(start: usize, frames: usize) -> Vec<f32> {
    let mut v = Vec::with_capacity(frames * 2);
    for i in start..start + frames {
        let t = i as f32 / RATE as f32;
        v.push(0.3 * (std::f32::consts::TAU * 220.0 * t).sin());
        v.push(0.3 * (std::f32::consts::TAU * 330.0 * t).sin());
    }
    v
}

fn rms(v: &[f32]) -> f32 {
    (v.iter().map(|x| x * x).sum::<f32>() / v.len().max(1) as f32).sqrt()
}

/// Procesa `seconds` de señal en bloques de `block` frames; devuelve la salida.
fn render(p: &mut PluginProcessor, seconds: f32, block: usize) -> Vec<f32> {
    let total = (seconds * RATE as f32) as usize;
    let mut out = Vec::with_capacity(total * 2);
    let mut pos = 0;
    while pos < total {
        let n = block.min(total - pos);
        let mut buf = signal(pos, n);
        p.process(&mut buf);
        out.extend_from_slice(&buf);
        pos += n;
    }
    out
}

struct Report {
    name: String,
    lines: Vec<String>,
    failures: Vec<String>,
}

impl Report {
    fn info(&mut self, s: impl Into<String>) {
        let s = s.into();
        if std::env::var_os("MN_PROBE_VERBOSE").is_some() {
            eprintln!("· {s}");
        }
        self.lines.push(s);
    }
    fn fail(&mut self, s: impl Into<String>) {
        let s = s.into();
        self.lines.push(format!("FALLO: {s}"));
        self.failures.push(s);
    }
    fn check_audio(&mut self, what: &str, out: &[f32]) {
        let bad = out.iter().filter(|x| !x.is_finite()).count();
        let peak = out.iter().fold(0f32, |m, x| m.max(x.abs()));
        if bad > 0 {
            self.fail(format!("{what}: {bad} muestras no finitas"));
        } else if peak > 20.0 {
            self.fail(format!("{what}: pico desmesurado {peak:.1}"));
        } else {
            self.info(format!("{what}: rms entrada {:.3} → salida {:.3}, pico {peak:.3}", rms(&signal(0, out.len() / 2)), rms(out)));
        }
    }
}

/// Lo cambiado en la interfaz debe haber llegado al procesador: una instancia
/// nueva con el estado del procesador (en VST3, solo el del componente, sin el
/// del controlador) debe tener los mismos valores.
fn check_processor_got(r: &mut Report, inst: &Arc<Instance>, plugin: &mn_fx::PluginRef, before: &[mn_plugin::ParamInfo], after: &[mn_plugin::ParamInfo]) {
    use base64::Engine as _;
    let b64 = base64::engine::general_purpose::STANDARD;
    let Some(state) = inst.state_base64() else { return r.fail("sin estado tras el arrastre") };
    let mut state = b64.decode(state).unwrap();
    if plugin.format == "vst3" {
        let n = u32::from_le_bytes(state[..4].try_into().unwrap()) as usize;
        state.truncate(4 + n);
    }
    let mut p = plugin.clone();
    p.state = Some(b64.encode(state));
    let fresh = match Instance::load(&p, RATE) {
        Ok(f) => f.params(),
        Err(e) => return r.fail(format!("no carga con el estado: {e}")),
    };
    for (a, b) in before.iter().zip(after) {
        if a.value == b.value {
            continue;
        }
        match fresh.iter().find(|f| f.id == b.id) {
            Some(f) if (f.value - b.value).abs() <= 1e-4 * (b.max - b.min) => r.info(format!("el procesador recibió «{}»", b.name)),
            Some(f) => r.fail(format!("«{}» cambió en la interfaz ({}) pero el procesador tiene {}", b.name, b.text, f.text)),
            None => r.fail(format!("«{}» no está en la instancia restaurada", b.name)),
        }
    }
}

fn trace(s: &str) {
    if std::env::var_os("MN_PROBE_VERBOSE").is_some() {
        eprintln!("  > {s}");
    }
}

fn timed<R>(r: &mut Report, what: &str, f: impl FnOnce() -> R) -> R {
    let t = Instant::now();
    let v = f();
    r.info(format!("{what}: {} ms", t.elapsed().as_millis()));
    v
}

fn probe(d: &PluginDescriptor, gui: bool) -> Report {
    let mut r = Report { name: format!("[{}] {} — {} ({})", d.format.as_str(), d.name, d.vendor, d.category), lines: vec![], failures: vec![] };
    let plugin = d.to_ref();
    let inst = match timed(&mut r, "carga", || Instance::load(&plugin, RATE)) {
        Ok(i) => i,
        Err(e) => {
            r.fail(format!("no carga: {e}"));
            return r;
        }
    };

    let params = timed(&mut r, "parámetros", || inst.params());
    r.info(format!(
        "{} parámetros: {}",
        params.len(),
        params.iter().take(6).map(|p| format!("{}={} [{}..{}]", p.name, p.text, p.min, p.max)).collect::<Vec<_>>().join(", ")
    ));
    if params.iter().any(|p| !(p.min <= p.value && p.value <= p.max) || !p.min.is_finite() || !p.max.is_finite()) {
        r.fail("algún parámetro fuera de su rango");
    }

    // Audio en bloques normales, enormes (se trocean) y diminutos.
    let mut p = PluginProcessor::new(inst.clone());
    let out = render(&mut p, 2.0, 512);
    r.check_audio("2 s en bloques de 512", &out);
    let out = render(&mut p, 0.5, 5000);
    r.check_audio("bloques de 5000", &out);
    let out = render(&mut p, 0.1, 7);
    r.check_audio("bloques de 7", &out);
    p.reset();

    // Parámetro: el primero continuo que no sea un «bypass».
    let target = params.iter().find(|p| !p.stepped && !p.name.to_lowercase().contains("bypass") && p.max > p.min);
    if let Some(t) = target {
        let v = if (t.value - t.max).abs() < (t.value - t.min).abs() { t.min + 0.25 * (t.max - t.min) } else { t.min + 0.75 * (t.max - t.min) };
        inst.set_param(t.id, v);
        render(&mut p, 0.05, 512);
        let now = inst.params().into_iter().find(|x| x.id == t.id).map(|x| (x.value, x.text));
        match now {
            Some((nv, text)) if (nv - v).abs() <= 1e-3 * (t.max - t.min).max(1e-9) => r.info(format!("«{}» → {v:.3} ({text})", t.name)),
            Some((nv, _)) => r.fail(format!("«{}»: se fijó {v} y el plugin informa {nv}", t.name)),
            None => r.fail(format!("«{}» desapareció", t.name)),
        }
    }

    // Interfaz abierta mientras el audio corre en otro hilo.
    let stop = Arc::new(AtomicBool::new(false));
    let audio = {
        let stop = stop.clone();
        let mut p = PluginProcessor::new(inst.clone());
        std::thread::spawn(move || {
            let mut pos = 0;
            let mut bad = 0usize;
            while !stop.load(Ordering::Relaxed) {
                let mut buf = signal(pos, 512);
                p.process(&mut buf);
                bad += buf.iter().filter(|x| !x.is_finite()).count();
                pos += 512;
                std::thread::sleep(Duration::from_millis(10));
            }
            bad
        })
    };
    if gui {
        match timed(&mut r, "abrir interfaz", || inst.open_editor()) {
            Ok(()) => {
                trace("interfaz abierta; esperando");
                std::thread::sleep(Duration::from_millis(1500));
                #[cfg(windows)]
                if let Some(dir) = std::env::var_os("MN_PROBE_SHOTS") {
                    let file = PathBuf::from(dir).join(format!("{}-{}.bmp", d.name.replace([' ', '/', '\\', ':'], "_"), d.format.as_str()));
                    match shot::capture(&format!("{} · MusicNova", d.name), &file) {
                        Ok(info) => r.info(format!("captura {}: {info}", file.display())),
                        Err(e) => r.fail(format!("captura: {e}")),
                    }
                }
                // `MN_PROBE_DRAG=x,y,dy`: arrastra el ratón dentro de la interfaz
                // (como el usuario moviendo un control) y mira qué parámetro cambió.
                #[cfg(windows)]
                if let Ok(spec) = std::env::var("MN_PROBE_DRAG") {
                    let v: Vec<i32> = spec.split(',').filter_map(|s| s.trim().parse().ok()).collect();
                    if let [x, y, dy] = v[..] {
                        let before = inst.params();
                        match shot::drag(&format!("{} · MusicNova", d.name), x, y, dy) {
                            Err(e) => r.fail(format!("arrastre: {e}")),
                            Ok(()) => {
                                std::thread::sleep(Duration::from_millis(400));
                                let after = inst.params();
                                let changed: Vec<String> = before
                                    .iter()
                                    .zip(&after)
                                    .filter(|(a, b)| a.id == b.id && (a.value != b.value || a.name != b.name))
                                    .map(|(a, b)| if a.name == b.name { format!("{}: {} → {}", a.name, a.text, b.text) } else { format!("«{}» ahora es «{}» ({})", a.name, b.name, b.text) })
                                    .collect();
                                if changed.is_empty() {
                                    r.fail("arrastrar en la interfaz no cambió ningún parámetro");
                                } else {
                                    r.info(format!("arrastre en la interfaz: {}", changed.join("; ")));
                                    check_processor_got(&mut r, &inst, &plugin, &before, &after);
                                }
                            }
                        }
                    }
                }
                if let Some(t) = target {
                    trace("parámetro con la interfaz abierta");
                    inst.set_param(t.id, t.default); // con la interfaz abierta
                    trace("parámetro fijado");
                }
                std::thread::sleep(Duration::from_millis(500));
                if !inst.editor_open() {
                    r.fail("la ventana se cerró sola");
                }
                trace("cerrando interfaz");
                timed(&mut r, "cerrar interfaz", || inst.close_editor());
                // Reabrir: muchos plugins fallan la segunda vez.
                match inst.open_editor() {
                    Ok(()) => {
                        std::thread::sleep(Duration::from_millis(500));
                        inst.close_editor();
                        r.info("interfaz reabierta y cerrada");
                    }
                    Err(e) => r.fail(format!("no se pudo reabrir la interfaz: {e}")),
                }
            }
            Err(e) => r.info(format!("sin interfaz: {e}")),
        }
    } else {
        std::thread::sleep(Duration::from_millis(300));
    }
    stop.store(true, Ordering::Relaxed);
    let bad = audio.join().unwrap_or(usize::MAX);
    if bad > 0 {
        r.fail(format!("audio en paralelo: {bad} muestras no finitas"));
    }

    // Estado: otra instancia con el estado guardado debe quedar igual.
    if let Some(t) = target {
        let v = t.min + 0.6 * (t.max - t.min);
        inst.set_param(t.id, v);
        render(&mut p, 0.05, 512);
    }
    let mut saved = plugin.clone();
    saved.state = timed(&mut r, "guardar estado", || inst.state_base64());
    match &saved.state {
        None => r.fail("no devuelve estado"),
        Some(s) => {
            r.info(format!("estado: {} bytes en base64", s.len()));
            match Instance::load(&saved, 44_100) {
                Err(e) => r.fail(format!("no carga con su estado: {e}")),
                Ok(other) => {
                    let a = inst.params();
                    let b = other.params();
                    let diff: Vec<String> = a
                        .iter()
                        .zip(&b)
                        .filter(|(x, y)| x.id == y.id && (x.value - y.value).abs() > 1e-4 * (x.max - x.min).max(1e-9))
                        .map(|(x, y)| format!("{} {} ≠ {}", x.name, x.value, y.value))
                        .take(5)
                        .collect();
                    if a.len() != b.len() {
                        r.fail(format!("restaurado con {} parámetros en vez de {}", b.len(), a.len()));
                    } else if !diff.is_empty() {
                        r.fail(format!("estado restaurado distinto: {}", diff.join("; ")));
                    } else {
                        r.info("estado restaurado idéntico");
                    }
                    let mut q = PluginProcessor::new(other.clone());
                    let out = render(&mut q, 0.5, 441);
                    r.check_audio("instancia restaurada a 44,1 kHz", &out);
                }
            }
        }
    }
    let t = Instant::now();
    drop(p);
    drop(inst);
    std::thread::sleep(Duration::from_millis(200));
    r.info(format!("liberado ({} ms)", t.elapsed().as_millis()));

    // Recarga tras descargar la biblioteca, y varias instancias en paralelo
    // (cada una en su hilo, como pistas distintas), soltadas en desorden.
    let mut many = Vec::new();
    for _ in 0..4 {
        match Instance::load(&plugin, RATE) {
            Ok(i) => many.push(i),
            Err(e) => {
                r.fail(format!("no recarga tras descargarse: {e}"));
                return r;
            }
        }
    }
    let handles: Vec<_> = many
        .iter()
        .map(|i| {
            let mut p = PluginProcessor::new(i.clone());
            std::thread::spawn(move || render(&mut p, 1.0, 256).iter().all(|x| x.is_finite()))
        })
        .collect();
    let all_ok = handles.into_iter().all(|h| h.join().unwrap_or(false));
    if all_ok {
        r.info("4 instancias en paralelo correctas");
    } else {
        r.fail("instancias en paralelo: audio no finito o pánico");
    }
    for i in [1, 3, 0, 2] {
        many[i].params();
    }
    many.swap(0, 3);
    drop(many);
    std::thread::sleep(Duration::from_millis(200));
    r
}

fn run(paths: Vec<PathBuf>, gui: bool) -> bool {
    let files = find_candidates(&paths);
    println!("{} archivos de plugin", files.len());
    let mut ok = true;
    for f in files {
        let t = Instant::now();
        let found = match scan_file(&f) {
            Ok(v) => v,
            Err(e) => {
                println!("\n{}: FALLO al escanear: {e}", f.display());
                ok = false;
                continue;
            }
        };
        println!("\n{}: {} efecto(s), escaneo {} ms", f.display(), found.len(), t.elapsed().as_millis());
        for d in &found {
            if let Ok(mut c) = CURRENT_PLUGIN.lock() {
                *c = format!("[{}] {}", d.format.as_str(), d.name);
            }
            let rep = probe(d, gui);
            println!("  {}", rep.name);
            for l in &rep.lines {
                println!("    {l}");
            }
            ok &= rep.failures.is_empty();
        }
    }
    ok
}

/// Volcado de la pila de otro hilo (con dbghelp, incluido en Windows): sirve
/// para ver en qué se queda bloqueado el hilo principal dentro de un plugin.
#[cfg(all(windows, target_arch = "x86_64"))]
mod stack {
    use std::ffi::c_void;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Diagnostics::Debug::*;
    use windows_sys::Win32::System::LibraryLoader::GetModuleFileNameW;
    use windows_sys::Win32::System::SystemInformation::IMAGE_FILE_MACHINE_AMD64;
    use windows_sys::Win32::System::Threading::*;

    /// Hilos de este proceso.
    pub fn threads() -> Vec<u32> {
        use windows_sys::Win32::System::Diagnostics::ToolHelp::*;
        let mut out = Vec::new();
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
            let mut e: THREADENTRY32 = std::mem::zeroed();
            e.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
            let pid = GetCurrentProcessId();
            if Thread32First(snap, &mut e) != 0 {
                loop {
                    if e.th32OwnerProcessID == pid {
                        out.push(e.th32ThreadID);
                    }
                    if Thread32Next(snap, &mut e) == 0 {
                        break;
                    }
                }
            }
            CloseHandle(snap);
        }
        out
    }

    pub fn dump(tid: u32) {
        unsafe {
            let process = GetCurrentProcess();
            SymSetOptions(SYMOPT_UNDNAME | SYMOPT_DEFERRED_LOADS);
            SymInitialize(process, std::ptr::null(), 1);
            let th = OpenThread(THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT | THREAD_QUERY_INFORMATION, 0, tid);
            if th.is_null() {
                eprintln!("    (no se pudo abrir el hilo)");
                return;
            }
            SuspendThread(th);
            let mut ctx: CONTEXT = std::mem::zeroed();
            ctx.ContextFlags = CONTEXT_FULL_AMD64;
            let mut pcs = Vec::new();
            if GetThreadContext(th, &mut ctx) != 0 {
                let mut frame: STACKFRAME64 = std::mem::zeroed();
                frame.AddrPC.Offset = ctx.Rip;
                frame.AddrPC.Mode = AddrModeFlat;
                frame.AddrFrame.Offset = ctx.Rbp;
                frame.AddrFrame.Mode = AddrModeFlat;
                frame.AddrStack.Offset = ctx.Rsp;
                frame.AddrStack.Mode = AddrModeFlat;
                for _ in 0..48 {
                    let ok = StackWalk64(
                        IMAGE_FILE_MACHINE_AMD64 as u32,
                        process,
                        th,
                        &mut frame,
                        &mut ctx as *mut CONTEXT as *mut c_void,
                        None,
                        Some(SymFunctionTableAccess64),
                        Some(SymGetModuleBase64),
                        None,
                    );
                    if ok == 0 || frame.AddrPC.Offset == 0 {
                        break;
                    }
                    pcs.push(frame.AddrPC.Offset);
                }
            }
            ResumeThread(th);
            CloseHandle(th);
            for pc in pcs {
                let base = SymGetModuleBase64(process, pc);
                let mut name = [0u16; 260];
                let n = GetModuleFileNameW(base as _, name.as_mut_ptr(), 260) as usize;
                let module = String::from_utf16_lossy(&name[..n]);
                let module = module.rsplit('\\').next().unwrap_or("?").to_string();
                let mut buf = [0u8; std::mem::size_of::<SYMBOL_INFO>() + 256];
                let sym = buf.as_mut_ptr() as *mut SYMBOL_INFO;
                (*sym).SizeOfStruct = std::mem::size_of::<SYMBOL_INFO>() as u32;
                (*sym).MaxNameLen = 255;
                let mut disp = 0u64;
                let name = if SymFromAddr(process, pc, &mut disp, sym) != 0 {
                    let p = (*sym).Name.as_ptr() as *const u8;
                    let s = std::slice::from_raw_parts(p, (*sym).NameLen as usize);
                    format!("{}+0x{disp:x}", String::from_utf8_lossy(s))
                } else {
                    format!("0x{:x}", pc - base)
                };
                eprintln!("    {module}!{name}");
            }
        }
    }
}

/// Captura de la ventana del plugin (píxeles de pantalla) en BMP, y cuántos
/// colores distintos tiene: una interfaz que no se dibuja sale casi lisa.
#[cfg(windows)]
mod shot {
    use std::collections::HashSet;
    use std::path::Path;
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::Graphics::Gdi::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowW, GetClientRect, SetForegroundWindow};

    /// Arrastre vertical con el botón izquierdo en el punto `x,y` del área cliente.
    /// Mueve el cursor real (muchas interfaces leen su posición) y envía los mensajes.
    pub fn drag(title: &str, x: i32, y: i32, dy: i32) -> Result<(), String> {
        use windows_sys::Win32::Foundation::POINT;
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
            SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEINPUT,
        };
        use windows_sys::Win32::UI::WindowsAndMessaging::{GetCursorPos, SetCursorPos};
        let wide: Vec<u16> = title.encode_utf16().chain(Some(0)).collect();
        unsafe {
            let hwnd = FindWindowW(std::ptr::null(), wide.as_ptr());
            if hwnd.is_null() {
                return Err("no se encuentra la ventana".into());
            }
            SetForegroundWindow(hwnd);
            std::thread::sleep(std::time::Duration::from_millis(200));
            let mut saved = POINT { x: 0, y: 0 };
            GetCursorPos(&mut saved);
            let mut p = POINT { x, y };
            ClientToScreen(hwnd, &mut p);
            let click = |flags| {
                let input = INPUT {
                    r#type: INPUT_MOUSE,
                    Anonymous: INPUT_0 { mi: MOUSEINPUT { dx: 0, dy: 0, mouseData: 0, dwFlags: flags, time: 0, dwExtraInfo: 0 } },
                };
                SendInput(1, &input, std::mem::size_of::<INPUT>() as i32);
            };
            SetCursorPos(p.x, p.y);
            std::thread::sleep(std::time::Duration::from_millis(100));
            click(MOUSEEVENTF_LEFTDOWN);
            let steps = 20;
            for i in 1..=steps {
                std::thread::sleep(std::time::Duration::from_millis(20));
                SetCursorPos(p.x, p.y + dy * i / steps);
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
            click(MOUSEEVENTF_LEFTUP);
            SetCursorPos(saved.x, saved.y);
        }
        Ok(())
    }

    pub fn capture(title: &str, file: &Path) -> Result<String, String> {
        let wide: Vec<u16> = title.encode_utf16().chain(Some(0)).collect();
        unsafe {
            let hwnd = FindWindowW(std::ptr::null(), wide.as_ptr());
            if hwnd.is_null() {
                return Err("no se encuentra la ventana".into());
            }
            SetForegroundWindow(hwnd);
            std::thread::sleep(std::time::Duration::from_millis(300));
            let mut rc: RECT = std::mem::zeroed();
            GetClientRect(hwnd, &mut rc);
            let (w, h) = (rc.right - rc.left, rc.bottom - rc.top);
            let mut origin = windows_sys::Win32::Foundation::POINT { x: 0, y: 0 };
            ClientToScreen(hwnd, &mut origin);
            let screen = GetDC(std::ptr::null_mut());
            let mem = CreateCompatibleDC(screen);
            let bmp = CreateCompatibleBitmap(screen, w, h);
            let old = SelectObject(mem, bmp);
            BitBlt(mem, 0, 0, w, h, screen, origin.x, origin.y, SRCCOPY);
            let mut info: BITMAPINFO = std::mem::zeroed();
            info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
            info.bmiHeader.biWidth = w;
            info.bmiHeader.biHeight = h;
            info.bmiHeader.biPlanes = 1;
            info.bmiHeader.biBitCount = 32;
            let mut px = vec![0u8; (w * h * 4) as usize];
            GetDIBits(mem, bmp, 0, h as u32, px.as_mut_ptr().cast(), &mut info, DIB_RGB_COLORS);
            SelectObject(mem, old);
            DeleteObject(bmp);
            DeleteDC(mem);
            ReleaseDC(std::ptr::null_mut(), screen);

            let colors: HashSet<[u8; 3]> = px.as_chunks::<4>().0.iter().map(|c| [c[0], c[1], c[2]]).collect();
            let mut out = Vec::with_capacity(54 + px.len());
            out.extend_from_slice(b"BM");
            out.extend_from_slice(&(54 + px.len() as u32).to_le_bytes());
            out.extend_from_slice(&[0; 4]);
            out.extend_from_slice(&54u32.to_le_bytes());
            out.extend_from_slice(&40u32.to_le_bytes());
            out.extend_from_slice(&w.to_le_bytes());
            out.extend_from_slice(&h.to_le_bytes());
            out.extend_from_slice(&1u16.to_le_bytes());
            out.extend_from_slice(&32u16.to_le_bytes());
            out.extend_from_slice(&[0; 24]);
            out.extend_from_slice(&px);
            std::fs::write(file, out).map_err(|e| e.to_string())?;
            Ok(format!("{w}×{h}, {} colores", colors.len()))
        }
    }
}

#[cfg(windows)]
fn main() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, MsgWaitForMultipleObjects, PeekMessageW, TranslateMessage, MSG, PM_REMOVE, QS_ALLINPUT,
    };

    let args: Vec<String> = std::env::args().skip(1).collect();
    let gui = !args.iter().any(|a| a == "--no-gui");
    let paths: Vec<PathBuf> = args.iter().filter(|a| !a.starts_with("--")).map(PathBuf::from).collect();

    // Como la aplicación (Tauri declara conciencia de PPP por monitor).
    if std::env::var_os("MN_PROBE_DPI_UNAWARE").is_none() {
        unsafe {
            windows_sys::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(windows_sys::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        }
    }
    let (tx, rx) = mpsc::channel::<Task>();
    let tx = Mutex::new(tx);
    set_main_thread_executor(move |f| {
        let _ = tx.lock().unwrap().send(f);
    });
    mn_plugin::install();

    let done = Arc::new(AtomicBool::new(false));
    let worker = {
        let done = done.clone();
        std::thread::spawn(move || {
            let ok = run(paths, gui);
            done.store(true, Ordering::SeqCst);
            ok
        })
    };
    // Bucle del «hilo principal»: mensajes de ventanas y tareas de los plugins.
    let mut msg: MSG = unsafe { std::mem::zeroed() };
    // Vigilante: si el hilo principal deja de dar vueltas (un plugin lo
    // bloquea), se informa de qué plugin era, con su pila, y se termina.
    let main_tid = unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() };
    std::thread::spawn(move || {
        let mut last = MAIN_BEATS.load(Ordering::Relaxed);
        let mut stalled = 0;
        loop {
            std::thread::sleep(Duration::from_secs(1));
            let now = MAIN_BEATS.load(Ordering::Relaxed);
            stalled = if now == last { stalled + 1 } else { 0 };
            last = now;
            if stalled == STALL_SECS {
                let plugin = CURRENT_PLUGIN.lock().map(|p| p.clone()).unwrap_or_default();
                println!(
                    "\nFALLO: el hilo principal lleva {STALL_SECS} s bloqueado dentro de «{plugin}» (mensaje de Windows 0x{:04x})",
                    CURRENT_MSG.load(Ordering::Relaxed)
                );
                #[cfg(target_arch = "x86_64")]
                {
                    eprintln!("pila del hilo principal:");
                    stack::dump(main_tid);
                    if std::env::var_os("MN_PROBE_ALL_THREADS").is_some() {
                        let me = unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() };
                        for t in stack::threads().into_iter().filter(|&t| t != main_tid && t != me) {
                            eprintln!("hilo {t}:");
                            stack::dump(t);
                        }
                    }
                }
                println!("HAY FALLOS");
                std::process::exit(3);
            }
        }
    });
    while !done.load(Ordering::SeqCst) {
        MAIN_BEATS.fetch_add(1, Ordering::Relaxed);
        unsafe {
            MsgWaitForMultipleObjects(0, std::ptr::null(), 0, 5, QS_ALLINPUT);
            let mut n = 0;
            while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&msg);
                CURRENT_MSG.store(msg.message, Ordering::Relaxed);
                DispatchMessageW(&msg);
                CURRENT_MSG.store(0, Ordering::Relaxed);
                n += 1;
                if n == 10_000 {
                    trace(&format!("bucle de mensajes saturado: último 0x{:04x} hwnd {:?}", msg.message, msg.hwnd));
                }
            }
        }
        while let Ok(f) = rx.try_recv() {
            let t = Instant::now();
            f();
            if t.elapsed() > Duration::from_millis(500) {
                trace(&format!("tarea del hilo principal: {} ms", t.elapsed().as_millis()));
            }
        }
    }
    // Apagados pendientes.
    let end = Instant::now();
    while end.elapsed() < Duration::from_millis(500) {
        while let Ok(f) = rx.try_recv() {
            f();
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let ok = worker.join().unwrap_or(false);
    println!("\n{}", if ok { "TODO CORRECTO" } else { "HAY FALLOS" });
    std::process::exit(if ok { 0 } else { 1 });
}

#[cfg(not(windows))]
fn main() {
    eprintln!("la sonda solo funciona en Windows por ahora");
}
