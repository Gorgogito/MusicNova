// Controla la interfaz de MusicNova desde fuera, con eventos de ratón y teclado
// reales del navegador (Chrome DevTools Protocol de WebView2), para probarla.
//
// Arranque: WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222 npx tauri dev
// Uso:      node scripts/ui-driver.mjs <orden> [args…]
//   shot <archivo.png>                 captura de la ventana
//   eval <js>                          evalúa JS en la página y muestra el resultado
//   click <objetivo> [n]               clic en el centro del n-ésimo elemento que coincide
//   dblclick <objetivo>                doble clic
//   hover <objetivo>
//   drag <objetivo|x,y> <dx> <dy>      arrastra con el botón pulsado (en pasos)
//   clickxy <x> <y>                    clic en coordenadas de la página
//   type <texto>                       escribe en el elemento con el foco
//   key <Tecla> [modificadores]        pulsa una tecla (p. ej. «key s», «key Delete», «key z ctrl»)
//   keydown <Tecla> / keyup <Tecla>    mantiene pulsada / suelta una tecla
//   select <objetivo> <valor>          elige una opción de un <select> (como haría el usuario)
//   wait <objetivo> [segundos]         espera a que aparezca
//   text [objetivo]                    texto visible (de la página o de un elemento)
// Objetivo: «texto=Analizar» (botón/enlace/etiqueta con ese texto), «titulo=…» (atributo
// title) o un selector CSS.

const PORT = process.env.MN_CDP_PORT || 9222

async function connect() {
  const targets = await (await fetch(`http://127.0.0.1:${PORT}/json/list`)).json()
  const page = targets.find((t) => t.type === 'page')
  if (!page) throw new Error('no hay ninguna página de WebView2 abierta')
  const ws = new WebSocket(page.webSocketDebuggerUrl)
  await new Promise((ok, err) => {
    ws.onopen = ok
    ws.onerror = err
  })
  let id = 0
  const pending = new Map()
  ws.onmessage = (m) => {
    const msg = JSON.parse(m.data)
    if (msg.id && pending.has(msg.id)) {
      const { ok, err } = pending.get(msg.id)
      pending.delete(msg.id)
      if (msg.error) err(new Error(JSON.stringify(msg.error)))
      else ok(msg.result)
    }
  }
  const send = (method, params = {}) =>
    new Promise((ok, err) => {
      const n = ++id
      pending.set(n, { ok, err })
      ws.send(JSON.stringify({ id: n, method, params }))
    })
  return { send, close: () => ws.close() }
}

const FIND = `(sel, n) => {
  const visible = (e) => { const r = e.getBoundingClientRect(); return r.width > 0 && r.height > 0 && getComputedStyle(e).visibility !== 'hidden' }
  let list
  if (sel.startsWith('texto=')) {
    const t = sel.slice(6).trim().toLowerCase()
    const all = [...document.querySelectorAll('button, a, label, option, [role=button], summary, span, strong, div, li, h3')]
    const exact = all.filter((e) => visible(e) && e.textContent.trim().toLowerCase() === t)
    list = exact.length ? exact : all.filter((e) => visible(e) && e.children.length < 4 && e.textContent.trim().toLowerCase().includes(t))
    // El más interno primero (el botón, no su contenedor).
    list = list.filter((e) => !list.some((o) => o !== e && e.contains(o)))
  } else if (sel.startsWith('titulo=')) {
    const t = sel.slice(7).toLowerCase()
    list = [...document.querySelectorAll('[title]')].filter((e) => visible(e) && e.title.toLowerCase().includes(t))
  } else list = [...document.querySelectorAll(sel)].filter(visible)
  const e = list[n || 0]
  if (!e) return null
  e.scrollIntoView({ block: 'center', inline: 'center' })
  const r = e.getBoundingClientRect()
  return { x: r.left + r.width / 2, y: r.top + r.height / 2, w: r.width, h: r.height, tag: e.tagName, text: e.textContent.trim().slice(0, 60), count: list.length }
}`

async function find(cdp, sel, n = 0) {
  const r = await cdp.send('Runtime.evaluate', { expression: `(${FIND})(${JSON.stringify(sel)}, ${n})`, returnByValue: true })
  return r.result.value
}

async function mouse(cdp, type, x, y, extra = {}) {
  await cdp.send('Input.dispatchMouseEvent', { type, x, y, button: 'left', buttons: type === 'mouseReleased' ? 0 : 1, clickCount: 1, ...extra })
}

async function clickAt(cdp, x, y, count = 1) {
  await cdp.send('Input.dispatchMouseEvent', { type: 'mouseMoved', x, y, buttons: 0 })
  for (let c = 1; c <= count; c++) {
    await mouse(cdp, 'mousePressed', x, y, { clickCount: c })
    await mouse(cdp, 'mouseReleased', x, y, { clickCount: c })
  }
}

const MODS = { alt: 1, ctrl: 2, meta: 4, shift: 8 }
const sleep = (ms) => new Promise((r) => setTimeout(r, ms))

async function main() {
  const [cmd, ...args] = process.argv.slice(2)
  const cdp = await connect()
  try {
    switch (cmd) {
      case 'shot': {
        const { data } = await cdp.send('Page.captureScreenshot', { format: 'png' })
        const fs = await import('node:fs')
        fs.writeFileSync(args[0], Buffer.from(data, 'base64'))
        console.log('captura:', args[0])
        break
      }
      case 'eval': {
        const r = await cdp.send('Runtime.evaluate', { expression: args.join(' '), returnByValue: true, awaitPromise: true })
        console.log(JSON.stringify(r.result.value ?? r.exceptionDetails ?? r.result, null, 1))
        break
      }
      case 'click':
      case 'dblclick':
      case 'hover': {
        const e = await find(cdp, args[0], Number(args[1] || 0))
        if (!e) throw new Error(`no encuentro «${args[0]}»`)
        if (cmd === 'hover') await cdp.send('Input.dispatchMouseEvent', { type: 'mouseMoved', x: e.x, y: e.y, buttons: 0 })
        else await clickAt(cdp, e.x, e.y, cmd === 'dblclick' ? 2 : 1)
        console.log(`${cmd} ${e.tag} «${e.text}» en (${Math.round(e.x)}, ${Math.round(e.y)}) [${e.count} coincidencias]`)
        break
      }
      case 'clickxy':
        await clickAt(cdp, Number(args[0]), Number(args[1]))
        console.log('clic en', args[0], args[1])
        break
      case 'drag': {
        let x, y
        if (/^-?\d+(\.\d+)?,-?\d+(\.\d+)?$/.test(args[0])) [x, y] = args[0].split(',').map(Number)
        else {
          const e = await find(cdp, args[0])
          if (!e) throw new Error(`no encuentro «${args[0]}»`)
          ;({ x, y } = e)
        }
        const [dx, dy] = [Number(args[1]), Number(args[2])]
        const mods = MODS[args[3]] || 0
        await cdp.send('Input.dispatchMouseEvent', { type: 'mouseMoved', x, y, buttons: 0 })
        await mouse(cdp, 'mousePressed', x, y, { modifiers: mods })
        const steps = 12
        for (let k = 1; k <= steps; k++) {
          await mouse(cdp, 'mouseMoved', x + (dx * k) / steps, y + (dy * k) / steps, { modifiers: mods })
          await sleep(16)
        }
        await mouse(cdp, 'mouseReleased', x + dx, y + dy, { modifiers: mods })
        console.log(`arrastre de (${Math.round(x)}, ${Math.round(y)}) a (${Math.round(x + dx)}, ${Math.round(y + dy)})`)
        break
      }
      case 'type':
        await cdp.send('Input.insertText', { text: args.join(' ') })
        console.log('escrito:', args.join(' '))
        break
      case 'key': {
        const key = args[0]
        const modifiers = args.slice(1).reduce((m, k) => m | (MODS[k] || 0), 0)
        const code = key.length === 1 ? `Key${key.toUpperCase()}` : key
        const vk = key.length === 1 ? key.toUpperCase().charCodeAt(0) : { Delete: 46, Backspace: 8, Escape: 27, Enter: 13, ArrowUp: 38, ArrowDown: 40, ArrowLeft: 37, ArrowRight: 39, Space: 32 }[key]
        const text = key.length === 1 && !(modifiers & 2) ? key : key === 'Space' ? ' ' : undefined
        await cdp.send('Input.dispatchKeyEvent', { type: 'keyDown', key: key === 'Space' ? ' ' : key, code, windowsVirtualKeyCode: vk, modifiers, text })
        await cdp.send('Input.dispatchKeyEvent', { type: 'keyUp', key: key === 'Space' ? ' ' : key, code, windowsVirtualKeyCode: vk, modifiers })
        console.log('tecla', args.join('+'))
        break
      }
      case 'keydown':
      case 'keyup': {
        // Mantener una tecla pulsada (teclado del ordenador como piano): «keydown a» … «keyup a».
        const key = args[0]
        const code = key.length === 1 ? `Key${key.toUpperCase()}` : key
        const vk = key.length === 1 ? key.toUpperCase().charCodeAt(0) : undefined
        const type = cmd === 'keydown' ? 'keyDown' : 'keyUp'
        await cdp.send('Input.dispatchKeyEvent', { type, key, code, windowsVirtualKeyCode: vk, text: type === 'keyDown' && key.length === 1 ? key : undefined })
        console.log(cmd, key)
        break
      }
      case 'select': {
        const e = await find(cdp, args[0])
        if (!e) throw new Error(`no encuentro «${args[0]}»`)
        await clickAt(cdp, e.x, e.y)
        // Los desplegables nativos no se pueden manejar con eventos de ratón: se
        // fija el valor y se lanza el mismo evento que produce el usuario.
        const r = await cdp.send('Runtime.evaluate', {
          returnByValue: true,
          expression: `(() => { const el = document.elementFromPoint(${e.x}, ${e.y}); const s = el.closest('select') || el.querySelector('select') || el;
            const opt = [...s.options].find((o) => o.value === ${JSON.stringify(args[1])} || o.textContent.trim().toLowerCase().includes(${JSON.stringify(args[1].toLowerCase())}));
            if (!opt) return 'sin opción: ' + [...s.options].map((o) => o.textContent.trim()).join(' | ');
            const set = Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, 'value').set; set.call(s, opt.value);
            s.dispatchEvent(new Event('change', { bubbles: true })); return 'elegido: ' + opt.textContent.trim() })()`,
        })
        await cdp.send('Input.dispatchKeyEvent', { type: 'keyDown', key: 'Escape', code: 'Escape', windowsVirtualKeyCode: 27 })
        console.log(r.result.value)
        break
      }
      case 'wait': {
        const limit = Date.now() + Number(args[1] || 30) * 1000
        let e = null
        while (Date.now() < limit && !(e = await find(cdp, args[0]))) await sleep(500)
        if (!e) throw new Error(`«${args[0]}» no apareció`)
        console.log('apareció', e.tag, `«${e.text}»`)
        break
      }
      case 'text': {
        const expr = args[0]
          ? `(() => { const r = (${FIND})(${JSON.stringify(args[0])}, 0); if (!r) return null; return document.elementFromPoint(r.x, r.y)?.closest('section,div,.panel')?.innerText })()`
          : 'document.body.innerText'
        const r = await cdp.send('Runtime.evaluate', { expression: expr, returnByValue: true })
        console.log(r.result.value)
        break
      }
      default:
        throw new Error(`orden desconocida: ${cmd}`)
    }
  } finally {
    cdp.close()
  }
}

main().catch((e) => {
  console.error('ERROR:', e.message)
  process.exit(1)
})
