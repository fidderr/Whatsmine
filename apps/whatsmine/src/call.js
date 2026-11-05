// WhatsMine calls: capture, encode, decode and play, in the page.
//
// Rust owns signalling and the network; this file owns the media. Packets
// travel between the two through the Dioxus eval channel (`dioxus.send` /
// `dioxus.recv`) as small JSON objects with base64 payloads:
//
//   JS -> Rust   {t:'a', ts, d}              one Opus packet
//                {t:'v', ts, key, d}         one VP8 frame
//                {t:'cam', on}               our camera went on/off
//                {t:'error', what, msg}      microphone / camera / codec problem
//                {t:'s', ts, key, c, d}      one frame of the screen we show
//                {t:'screen', on, w, h}      showing it, or not any more
//                {t:'askKey'}                we need a whole picture of what we watch
//   Rust -> JS   {t:'start', call, video, group}   we are in the call
//                {t:'stop'}  {t:'mute', on}  {t:'cam', on}
//                {t:'peers', peers:[{id,name}]}    who else is in the call
//                {t:'peerVideo', from, on}
//                {t:'a', from, ts, d}  {t:'v', from, ts, key, d}   packets from a peer
//                {t:'ring', kind}            'in' | 'out' | null
//                {t:'share', on}             start or stop showing our screen
//                {t:'screenKey'}             someone needs a whole picture of ours
//                {t:'s', from, n, ts, key, c, d}   one frame of a screen we watch
//                {t:'peerScreen', from, on}  someone started or stopped showing theirs
//
// Every other person in the call gets their own decoders and a tile, so a
// group call is the same code as a 1:1 call. Codecs are WebCodecs (Opus
// 48 kHz mono, VP8), played through MediaStreamTrackGenerator: no WebRTC and
// no third-party servers.
(() => {
  if (window.wmCall) return
  const CFG = { aBitrate: 24000, vBitrate: 700000, fps: 15, keyEvery: 2 }
  // A screen is text more often than not: sharp over smooth, and few frames will do.
  const SCREEN = { fps: 10, keyEvery: 8, bitrate: 2500000, least: 700000 }
  const SCREEN_CODECS = [{ name: 'vp09.00.10.08', bits: 1 }, { name: 'vp8', bits: 0 }]
  const DECODE = ['vp8', 'vp09.00.10.08', 'avc1.42E01F']
  const stats = {
    aSent: 0, aRecv: 0, aDec: 0, vSent: 0, vRecv: 0, vDec: 0, vKeyWait: 0, errors: [],
    sSent: 0, sBytes: 0, sRecv: 0, sDec: 0, sGaps: 0, sKeys: 0, speaking: 0
  }
  // For tests: a painted picture instead of what is really on screen.
  let driving = null
  let paintedScreen = false
  let dx = null
  let run = null
  let els = null
  let ringTimer = null
  let ringCtx = null

  const send = o => { try { dx.send(o) } catch (_) {} }
  const note = (what, e) => {
    const msg = String((e && e.message) || e)
    stats.errors.push(what + ': ' + msg)
    if (stats.errors.length > 20) stats.errors.shift()
    return msg
  }
  const fail = (what, e) => send({ t: 'error', what, msg: note(what, e) })
  const quiet = fn => { try { fn() } catch (_) {} }

  function b64 (u8) {
    let s = ''
    for (let i = 0; i < u8.length; i += 0x8000) s += String.fromCharCode.apply(null, u8.subarray(i, i + 0x8000))
    return btoa(s)
  }
  function unb64 (s) {
    const bin = atob(s)
    const u8 = new Uint8Array(bin.length)
    for (let i = 0; i < bin.length; i++) u8[i] = bin.charCodeAt(i)
    return u8
  }

  function ensureEls () {
    if (els) return els
    const box = document.createElement('div')
    box.id = 'wm-call-media'
    const tiles = document.createElement('div')
    tiles.className = 'wm-tiles'
    const local = document.createElement('video')
    local.className = 'wm-local'
    local.autoplay = true
    local.muted = true
    local.playsInline = true
    box.append(tiles, local)
    document.body.append(box)
    els = { box, tiles, local }
    return els
  }

  // ---- one tile (and decoder pair) per other person ----
  function layout (r) {
    const e = ensureEls()
    const n = r.peers.size
    e.box.dataset.n = String(n)
    const cols = n <= 1 ? 1 : n <= 4 ? 2 : 3
    e.tiles.style.gridTemplateColumns = 'repeat(' + cols + ', 1fr)'
    let anyCam = false
    r.peers.forEach(p => { if (p.cam) anyCam = true })
    e.box.classList.toggle('peer-cam', anyCam)
  }

  function setCam (r, p, on) {
    p.cam = on
    p.tile.classList.toggle('cam', on)
    if (r) layout(r)
  }

  function setName (p, name) {
    if (!name) return
    p.label.textContent = name
    p.letter.textContent = (Array.from(name)[0] || '?').toUpperCase()
  }

  // Is someone saying something? Only looks, in a group, at a little of what came in anyway.
  function loud (ad) {
    const n = ad.numberOfFrames
    if (!n) return false
    const buf = new Float32Array(n)
    try { ad.copyTo(buf, { planeIndex: 0, format: 'f32-planar' }) } catch (_) { return false }
    let sum = 0
    let count = 0
    for (let i = 0; i < n; i += 4) {
      sum += buf[i] * buf[i]
      count++
    }
    return Math.sqrt(sum / count) > 0.015
  }

  function ensurePeer (r, id, name) {
    let p = r.peers.get(id)
    if (p) {
      setName(p, name)
      return p
    }
    const e = ensureEls()
    const tile = document.createElement('div')
    tile.className = 'wm-tile'
    const video = document.createElement('video')
    video.autoplay = true
    video.muted = true
    video.playsInline = true
    const letter = document.createElement('div')
    letter.className = 'wm-letter'
    const label = document.createElement('div')
    label.className = 'wm-label'
    tile.append(video, letter, label)
    e.tiles.append(tile)
    const audio = document.createElement('audio')
    audio.autoplay = true
    e.box.append(audio)
    p = { id, tile, video, audio, label, letter, aIdx: 0, vIdx: 0, cam: false, vdec: null, nA: 0, nV: 0 }
    setName(p, name)

    p.adec = new AudioDecoder({
      output: ad => {
        stats.aDec++
        p.nA++
        if (r.group && loud(ad)) p.lastLoud = performance.now()
        p.agenW.write(ad).catch(() => quiet(() => ad.close()))
      },
      error: err => fail('audio-decoder', err)
    })
    p.adec.configure({ codec: 'opus', sampleRate: 48000, numberOfChannels: 1 })
    p.agen = new MediaStreamTrackGenerator({ kind: 'audio' })
    p.agenW = p.agen.writable.getWriter()
    audio.srcObject = new MediaStream([p.agen])
    audio.play().catch(err => note('audio-play', err))

    p.vgen = new MediaStreamTrackGenerator({ kind: 'video' })
    p.vgenW = p.vgen.writable.getWriter()
    video.srcObject = new MediaStream([p.vgen])
    video.play().catch(() => {})

    r.peers.set(id, p)
    layout(r)
    return p
  }

  function removePeer (r, id) {
    const p = r.peers.get(id)
    if (!p) return
    closeScreen(p)
    for (const c of [p.adec, p.vdec]) quiet(() => c && c.close())
    for (const w of [p.agenW, p.vgenW]) quiet(() => w && w.close())
    p.tile.remove()
    p.audio.srcObject = null
    p.audio.remove()
    r.peers.delete(id)
    layout(r)
  }

  function onAudio (m) {
    const r = run
    if (!r || r.closed) return
    const p = ensurePeer(r, m.from)
    if (p.adec.state !== 'configured') return
    stats.aRecv++
    // The network may reorder or drop packets; the playback clock is ours.
    try {
      p.adec.decode(new EncodedAudioChunk({ type: 'key', timestamp: (p.aIdx++) * 20000, data: unb64(m.d) }))
    } catch (e) { note('audio-decode', e) }
  }

  function ensureVideoDecoder (p) {
    if (p.vdec && p.vdec.state === 'configured') return
    p.vdec = new VideoDecoder({
      output: f => {
        stats.vDec++
        p.nV++
        // Pictures are arriving, so show them (the peer may not have said so).
        p.lastV = performance.now()
        if (!p.cam) setCam(run, p, true)
        p.vgenW.write(f).catch(() => quiet(() => f.close()))
      },
      error: e => { note('video-decoder', e); p.vdec = null }
    })
    p.vdec.configure({ codec: 'vp8' })
  }

  function onVideo (m) {
    const r = run
    if (!r || r.closed) return
    const p = ensurePeer(r, m.from)
    stats.vRecv++
    // A delta frame is useless until a key frame has been seen.
    if (!p.vdec || p.vdec.state !== 'configured') {
      if (!m.key) { stats.vKeyWait++; return }
      ensureVideoDecoder(p)
    }
    try {
      p.vdec.decode(new EncodedVideoChunk({ type: m.key ? 'key' : 'delta', timestamp: (p.vIdx++) * 66000, data: unb64(m.d) }))
    } catch (e) { note('video-decode', e); p.vdec = null }
  }

  // ---- sending ----
  function startAudio (r) {
    const track = r.mic.getAudioTracks()[0]
    r.aenc = new AudioEncoder({
      output: chunk => {
        if (r.closed || r.muted) return
        const u8 = new Uint8Array(chunk.byteLength)
        chunk.copyTo(u8)
        stats.aSent++
        send({ t: 'a', ts: Math.floor(chunk.timestamp) % 4294967296, d: b64(u8) })
      },
      error: e => fail('audio-encoder', e)
    })
    r.aenc.configure({ codec: 'opus', sampleRate: 48000, numberOfChannels: 1, bitrate: CFG.aBitrate })
    const reader = new MediaStreamTrackProcessor({ track }).readable.getReader()
    r.micReader = reader
    ;(async () => {
      for (;;) {
        let res
        try { res = await reader.read() } catch (_) { break }
        if (res.done) break
        const ad = res.value
        if (r.meTile && !r.muted && loud(ad)) r.meLoud = performance.now()
        try { if (!r.closed && !r.muted) r.aenc.encode(ad) } catch (e) { note('audio-encode', e) }
        ad.close()
      }
    })()
  }

  function initVideoEncoder (r, w, h) {
    if (r.venc) quiet(() => r.venc.close())
    r.vw = w
    r.vh = h
    r.venc = new VideoEncoder({
      output: chunk => {
        if (r.closed || !r.camOn) return
        const u8 = new Uint8Array(chunk.byteLength)
        chunk.copyTo(u8)
        stats.vSent++
        send({ t: 'v', ts: Math.floor(chunk.timestamp) % 4294967296, key: chunk.type === 'key', d: b64(u8) })
      },
      error: e => fail('video-encoder', e)
    })
    r.venc.configure({
      codec: 'vp8', width: w, height: h, bitrate: CFG.vBitrate, framerate: CFG.fps, latencyMode: 'realtime'
    })
    r.forceKey = true
  }

  async function cameraOn (on) {
    const r = run
    if (!r || on === r.camOn) return
    const e = ensureEls()
    if (!on) {
      r.camOn = false
      quiet(() => r.camReader && r.camReader.cancel())
      if (r.cam) r.cam.getTracks().forEach(t => t.stop())
      r.cam = null
      e.local.srcObject = null
      e.box.classList.remove('has-local')
      send({ t: 'cam', on: false })
      return
    }
    try {
      r.cam = await navigator.mediaDevices.getUserMedia({
        video: { width: { ideal: 640 }, height: { ideal: 480 }, frameRate: { ideal: CFG.fps }, facingMode: 'user' }
      })
    } catch (err) {
      fail('camera', err)
      send({ t: 'cam', on: false })
      return
    }
    if (r.closed) { r.cam.getTracks().forEach(t => t.stop()); return }
    const track = r.cam.getVideoTracks()[0]
    e.local.srcObject = new MediaStream([track])
    e.local.play().catch(() => {})
    e.box.classList.add('has-local')
    r.camOn = true
    r.frameNo = 0
    const reader = new MediaStreamTrackProcessor({ track }).readable.getReader()
    r.camReader = reader
    send({ t: 'cam', on: true })
    ;(async () => {
      for (;;) {
        let res
        try { res = await reader.read() } catch (_) { break }
        if (res.done) break
        const frame = res.value
        try {
          if (r.closed || !r.camOn) { frame.close(); continue }
          const w = frame.displayWidth & ~1
          const h = frame.displayHeight & ~1
          if (!r.venc || r.vw !== w || r.vh !== h) initVideoEncoder(r, w, h)
          if (r.venc.encodeQueueSize > 3) { frame.close(); continue }
          const key = r.forceKey || r.frameNo % (CFG.fps * CFG.keyEvery) === 0
          r.forceKey = false
          r.frameNo++
          r.venc.encode(frame, { keyFrame: key })
        } catch (err) { note('video-encode', err) }
        frame.close()
      }
    })()
  }

  // ---- showing our screen ----
  function paintedStream () {
    const c = document.createElement('canvas')
    c.width = 1280
    c.height = 720
    const g = c.getContext('2d')
    let n = 0
    const paint = () => {
      n++
      g.fillStyle = '#f5f6f9'
      g.fillRect(0, 0, c.width, c.height)
      g.fillStyle = '#1f5fd6'
      g.fillRect(0, 0, c.width, 48)
      g.fillStyle = '#14161c'
      g.font = '32px sans-serif'
      g.fillText('A painted screen, frame ' + n, 48, 120)
      g.fillStyle = n % 2 ? '#ff8a7a' : '#6fd6a0'
      g.fillRect(48 + (n % 100) * 8, 200, 120, 120)
    }
    paint()
    const s = c.captureStream(SCREEN.fps)
    const t = setInterval(paint, 1000 / SCREEN.fps)
    s.getVideoTracks()[0].addEventListener('ended', () => clearInterval(t))
    s.stopPaint = () => clearInterval(t)
    return s
  }

  async function screenEncoder (r, w, h) {
    const share = r.share
    if (share.enc) quiet(() => share.enc.close())
    share.w = w
    share.h = h
    const viewers = Math.max(1, r.peers.size)
    const bitrate = Math.max(SCREEN.least, Math.round(SCREEN.bitrate / viewers))
    const base = { width: w, height: h, bitrate, framerate: SCREEN.fps, latencyMode: 'realtime', contentHint: 'detail' }
    let chosen = null
    for (const c of SCREEN_CODECS) {
      try {
        const answer = await VideoEncoder.isConfigSupported({ codec: c.name, ...base })
        if (answer.supported) { chosen = c; break }
      } catch (_) {}
    }
    if (!chosen) throw new Error('no encoder for a screen')
    const enc = new VideoEncoder({
      output: chunk => {
        if (r.closed || !r.share) return
        const u8 = new Uint8Array(chunk.byteLength)
        chunk.copyTo(u8)
        stats.sSent++
        stats.sBytes += u8.length
        send({ t: 's', ts: Math.floor(chunk.timestamp) % 4294967296, key: chunk.type === 'key', c: chosen.bits, d: b64(u8) })
      },
      error: e => fail('screen-encoder', e)
    })
    enc.configure({ codec: chosen.name, ...base })
    share.enc = enc
    share.forceKey = true
  }

  function stopShowing (r) {
    const share = r && r.share
    if (!share) return
    r.share = null
    quiet(() => share.reader.cancel())
    share.stream.getTracks().forEach(t => t.stop())
    quiet(() => share.stream.stopPaint && share.stream.stopPaint())
    quiet(() => share.enc && share.enc.close())
    send({ t: 'screen', on: false, w: 0, h: 0 })
  }

  async function shareOn (on) {
    const r = run
    if (!r) return
    if (!on) return stopShowing(r)
    if (r.share) return
    let stream
    try {
      stream = paintedScreen
        ? paintedStream()
        : await navigator.mediaDevices.getDisplayMedia({
          video: { frameRate: { ideal: SCREEN.fps, max: 15 }, width: { max: 1920 }, height: { max: 1080 } },
          audio: false
        })
    } catch (err) {
      // Saying no in the picker is no error.
      if (err && err.name !== 'NotAllowedError') fail('screen', err)
      send({ t: 'screen', on: false, w: 0, h: 0 })
      return
    }
    if (r.closed || r.share) { stream.getTracks().forEach(t => t.stop()); return }
    const track = stream.getVideoTracks()[0]
    track.contentHint = 'detail'
    // The system's own Stop, a window that closed.
    track.addEventListener('ended', () => stopShowing(r))
    const reader = new MediaStreamTrackProcessor({ track }).readable.getReader()
    const set = track.getSettings()
    r.share = { stream, track, reader, enc: null, w: 0, h: 0, forceKey: true, lastKey: 0, busy: false }
    send({ t: 'screen', on: true, w: set.width || 0, h: set.height || 0, label: track.label || '', surface: set.displaySurface || '' })
    ;(async () => {
      for (;;) {
        let res
        try { res = await reader.read() } catch (_) { break }
        if (res.done) break
        const frame = res.value
        const share = r.share
        try {
          if (r.closed || !share) { frame.close(); continue }
          const w = frame.displayWidth & ~1
          const h = frame.displayHeight & ~1
          if (!share.enc || share.w !== w || share.h !== h) await screenEncoder(r, w, h)
          // Behind: this one is skipped, the next one is newer anyway.
          if (share.enc.encodeQueueSize > 2) { frame.close(); continue }
          const now = performance.now()
          const key = share.forceKey || now - share.lastKey > SCREEN.keyEvery * 1000
          if (key) {
            share.forceKey = false
            share.lastKey = now
          }
          share.enc.encode(frame, { keyFrame: key })
        } catch (err) { note('screen-encode', err) }
        frame.close()
      }
    })()
  }

  // ---- watching someone's screen ----
  function zoomable (tile, video) {
    let z = 1, x = 0, y = 0
    const points = new Map()
    let spread = 0
    const show = () => {
      z = Math.min(6, Math.max(1, z))
      if (z === 1) { x = 0; y = 0 }
      video.style.transform = 'translate(' + x + 'px,' + y + 'px) scale(' + z + ')'
    }
    const apart = () => {
      const p = Array.from(points.values())
      return p.length < 2 ? 0 : Math.hypot(p[0].x - p[1].x, p[0].y - p[1].y)
    }
    tile.addEventListener('wheel', e => {
      if (driving) return
      e.preventDefault()
      z *= e.deltaY < 0 ? 1.15 : 0.87
      show()
    }, { passive: false })
    let moved = 0
    let downAt = 0
    tile.addEventListener('pointerdown', e => {
      if (driving) return
      tile.setPointerCapture(e.pointerId)
      points.set(e.pointerId, { x: e.clientX, y: e.clientY })
      spread = apart()
      moved = 0
      downAt = performance.now()
    })
    tile.addEventListener('pointermove', e => {
      const p = points.get(e.pointerId)
      if (!p || driving) return
      moved += Math.abs(e.clientX - p.x) + Math.abs(e.clientY - p.y)
      if (points.size === 1 && z > 1) { x += e.clientX - p.x; y += e.clientY - p.y }
      p.x = e.clientX
      p.y = e.clientY
      if (points.size === 2) {
        const now = apart()
        if (spread > 0) z *= now / spread
        spread = now
      }
      show()
    })
    const lift = e => { points.delete(e.pointerId); spread = apart() }
    tile.addEventListener('pointerup', e => {
      lift(e)
      if (driving) return
      // A tap, not a drag: the rest of the call screen goes away, or comes back.
      if (moved < 12 && performance.now() - downAt < 350) document.body.classList.toggle('call-chrome-off')
    })
    tile.addEventListener('pointercancel', lift)
    tile.addEventListener('dblclick', () => {
      if (driving) return
      z = z > 1 ? 1 : 2.5
      show()
    })
  }

  function screenTile (p) {
    if (p.sTile) return
    const e = ensureEls()
    const tile = document.createElement('div')
    tile.className = 'wm-screen'
    const video = document.createElement('video')
    video.autoplay = true
    video.muted = true
    video.playsInline = true
    tile.append(video)
    e.box.append(tile)
    e.box.classList.add('has-screen')
    p.sTile = tile
    p.sVideo = video
    p.sGen = new MediaStreamTrackGenerator({ kind: 'video' })
    p.sOut = p.sGen.writable.getWriter()
    video.srcObject = new MediaStream([p.sGen])
    video.play().catch(() => {})
    p.sIdx = 0
    p.sNext = null
    p.sLost = true
    zoomable(tile, video)
  }

  function closeScreen (p) {
    if (!p.sTile) return
    quiet(() => p.sDec && p.sDec.close())
    quiet(() => p.sOut && p.sOut.close())
    p.sTile.remove()
    document.body.classList.remove('call-chrome-off')
    p.sTile = null
    p.sVideo = null
    p.sDec = null
    p.sCodec = null
    if (els && !document.querySelector('.wm-screen')) els.box.classList.remove('has-screen')
  }

  function askKey (p) {
    const now = performance.now()
    if (now - (p.sAsked || 0) < 1000) return
    p.sAsked = now
    stats.sKeys++
    send({ t: 'askKey' })
  }

  function onScreen (m) {
    const r = run
    if (!r || r.closed) return
    const p = ensurePeer(r, m.from)
    // Frames come on a stream of their own, and can arrive after they said they stopped.
    if (p.sOff) return
    screenTile(p)
    stats.sRecv++
    // A frame builds on the ones before it: one missing, and the rest waits for a whole picture.
    if (p.sLost && !m.key) return askKey(p)
    if (p.sNext !== null && m.n !== p.sNext && !m.key) {
      p.sLost = true
      stats.sGaps++
      return askKey(p)
    }
    p.sNext = (m.n + 1) >>> 0
    p.sLost = false
    if (!p.sDec || p.sDec.state !== 'configured' || p.sCodec !== m.c) {
      quiet(() => p.sDec && p.sDec.close())
      p.sCodec = m.c
      p.sDec = new VideoDecoder({
        output: f => {
          stats.sDec++
          p.sOut.write(f).catch(() => quiet(() => f.close()))
        },
        error: e => { note('screen-decoder', e); p.sDec = null; p.sLost = true; askKey(p) }
      })
      p.sDec.configure({ codec: DECODE[m.c] || 'vp8' })
    }
    try {
      p.sDec.decode(new EncodedVideoChunk({ type: m.key ? 'key' : 'delta', timestamp: (p.sIdx++) * 100000, data: unb64(m.d) }))
    } catch (e) { note('screen-decode', e); p.sDec = null; p.sLost = true; askKey(p) }
  }

  // ---- controlling the screen someone shows ----
  // Keys by their place on the keyboard: [scan code, extended]. What is not in here stays here, like
  // the Windows key.
  const SCAN = (() => {
    const m = { Escape: [1, 0], IntlBackslash: [0x56, 0], F11: [0x57, 0], F12: [0x58, 0] }
    ;('Digit1 Digit2 Digit3 Digit4 Digit5 Digit6 Digit7 Digit8 Digit9 Digit0 Minus Equal Backspace Tab ' +
      'KeyQ KeyW KeyE KeyR KeyT KeyY KeyU KeyI KeyO KeyP BracketLeft BracketRight Enter ControlLeft ' +
      'KeyA KeyS KeyD KeyF KeyG KeyH KeyJ KeyK KeyL Semicolon Quote Backquote ShiftLeft Backslash ' +
      'KeyZ KeyX KeyC KeyV KeyB KeyN KeyM Comma Period Slash ShiftRight NumpadMultiply AltLeft Space CapsLock ' +
      'F1 F2 F3 F4 F5 F6 F7 F8 F9 F10 NumLock ScrollLock Numpad7 Numpad8 Numpad9 NumpadSubtract ' +
      'Numpad4 Numpad5 Numpad6 NumpadAdd Numpad1 Numpad2 Numpad3 Numpad0 NumpadDecimal')
      .split(' ').forEach((name, i) => { m[name] = [2 + i, 0] })
    const ext = {
      NumpadEnter: 0x1C, ControlRight: 0x1D, NumpadDivide: 0x35, PrintScreen: 0x37, AltRight: 0x38,
      Home: 0x47, ArrowUp: 0x48, PageUp: 0x49, ArrowLeft: 0x4B, ArrowRight: 0x4D, End: 0x4F,
      ArrowDown: 0x50, PageDown: 0x51, Insert: 0x52, Delete: 0x53, ContextMenu: 0x5D
    }
    Object.keys(ext).forEach(name => { m[name] = [ext[name], 1] })
    return m
  })()

  function driveOn (m) {
    if (!m.on) {
      if (driving) driving.stop()
      driving = null
      return
    }
    const r = run
    if (!r || driving) return
    const peer = Array.from(r.peers.values()).find(p => p.sTile)
    if (!peer) return
    const tile = peer.sTile
    const video = peer.sVideo
    // All of it, at the size it is: what is pointed at is what gets clicked.
    video.style.transform = ''
    document.body.classList.remove('call-chrome-off')
    document.body.classList.add('call-driving')
    let queue = []
    const put = e => {
      const last = queue[queue.length - 1]
      if (e[0] === 'm' && last && last[0] === 'm') queue[queue.length - 1] = e
      else queue.push(e)
    }
    const flush = setInterval(() => {
      if (queue.length) {
        send({ t: 'ctl', e: queue })
        queue = []
      }
    }, 33)
    // A place on the picture, from 0 to 65535. Nothing for the black bars around it.
    const spot = e => {
      const box = video.getBoundingClientRect()
      const vw = video.videoWidth
      const vh = video.videoHeight
      if (!vw || !vh || !box.width || !box.height) return null
      const scale = Math.min(box.width / vw, box.height / vh)
      const w = vw * scale
      const h = vh * scale
      const x = (e.clientX - box.left - (box.width - w) / 2) / w
      const y = (e.clientY - box.top - (box.height - h) / 2) / h
      if (x < 0 || x > 1 || y < 0 || y > 1) return null
      return ['m', Math.round(x * 65535), Math.round(y * 65535)]
    }
    const fingers = new Map()
    let press = null
    let scroll = 0
    const onDown = e => {
      e.preventDefault()
      quiet(() => tile.setPointerCapture(e.pointerId))
      const at = spot(e)
      if (e.pointerType !== 'touch') {
        if (!at) return
        put(at)
        put(['b', e.button, 1])
        return
      }
      fingers.set(e.pointerId, { x: e.clientX, y: e.clientY })
      if (fingers.size === 1) {
        press = { at, x: e.clientX, y: e.clientY, moved: 0, dragging: false, done: false, when: performance.now() }
        press.timer = setTimeout(() => {
          // Held still: the right button.
          if (press && !press.dragging && press.at) {
            put(press.at)
            put(['b', 2, 1])
            put(['b', 2, 0])
            press.done = true
          }
        }, 600)
      } else if (press) {
        // A second finger: scrolling, whatever the first was about to do.
        clearTimeout(press.timer)
        if (press.dragging) put(['b', 0, 0])
        press.dragging = false
        press.done = true
      }
    }
    const onMove = e => {
      e.preventDefault()
      if (e.pointerType !== 'touch') {
        const at = spot(e)
        if (at) put(at)
        return
      }
      const f = fingers.get(e.pointerId)
      if (!f) return
      const dy = e.clientY - f.y
      const dist = Math.abs(e.clientX - f.x) + Math.abs(dy)
      f.x = e.clientX
      f.y = e.clientY
      if (fingers.size >= 2) {
        scroll += dy
        while (Math.abs(scroll) >= 24) {
          put(['w', 0, scroll > 0 ? 120 : -120])
          scroll -= scroll > 0 ? 24 : -24
        }
        return
      }
      if (!press || press.done) return
      press.moved += dist
      if (!press.dragging && press.moved > 12 && press.at) {
        clearTimeout(press.timer)
        press.dragging = true
        put(press.at)
        put(['b', 0, 1])
      }
      if (press.dragging) {
        const at = spot(e)
        if (at) put(at)
      }
    }
    const onUp = e => {
      e.preventDefault()
      if (e.pointerType !== 'touch') {
        put(['b', e.button, 0])
        return
      }
      fingers.delete(e.pointerId)
      if (!press) return
      clearTimeout(press.timer)
      if (press.dragging) put(['b', 0, 0])
      else if (!press.done && press.at && performance.now() - press.when < 600) {
        put(press.at)
        put(['b', 0, 1])
        put(['b', 0, 0])
      }
      if (!fingers.size) {
        press = null
        scroll = 0
      }
    }
    const onCancel = e => {
      fingers.delete(e.pointerId)
      if (press) {
        clearTimeout(press.timer)
        if (press.dragging) put(['b', 0, 0])
        press = null
      }
    }
    let wheelX = 0
    let wheelY = 0
    const onWheel = e => {
      e.preventDefault()
      // In pixels, lines or pages: a notch of a normal wheel is about 100 pixels or 3 lines.
      const unit = e.deltaMode === 1 ? 40 : e.deltaMode === 2 ? 800 : 1
      wheelX += e.deltaX * unit
      wheelY += e.deltaY * unit
      // A notch of the real wheel at a time, however small the steps of a touchpad. On Windows
      // up and to the right count as positive.
      while (Math.abs(wheelY) >= 100) {
        put(['w', 0, wheelY > 0 ? -120 : 120])
        wheelY -= wheelY > 0 ? 100 : -100
      }
      while (Math.abs(wheelX) >= 100) {
        put(['w', wheelX > 0 ? 120 : -120, 0])
        wheelX -= wheelX > 0 ? 100 : -100
      }
    }
    const noMenu = e => e.preventDefault()
    const down = new Set()
    const onKey = e => {
      if (e.target === keys) return
      const s = SCAN[e.code]
      if (!s) return
      e.preventDefault()
      e.stopPropagation()
      const isDown = e.type === 'keydown'
      if (isDown) down.add(e.code)
      else down.delete(e.code)
      put(['k', s[0], s[1], isDown ? 1 : 0])
    }
    // Looking away: nothing stays held down over there.
    const letGo = () => {
      down.forEach(code => put(['k', SCAN[code][0], SCAN[code][1], 0]))
      down.clear()
    }
    // A phone's keyboard, in a field with one space in it: gone means Backspace was pressed.
    let keys = null
    let keysButton = null
    if (window.matchMedia && matchMedia('(pointer: coarse)').matches) {
      const tap = (code) => {
        put(['k', code, 0, 1])
        put(['k', code, 0, 0])
      }
      keys = document.createElement('input')
      keys.className = 'wm-keys-field'
      keys.setAttribute('autocapitalize', 'off')
      keys.setAttribute('autocomplete', 'off')
      keys.setAttribute('autocorrect', 'off')
      keys.value = ' '
      keys.addEventListener('input', () => {
        const v = keys.value
        if (v.length === 0) tap(0x0E)
        else if (v.length > 1) put(['t', v.slice(1)])
        keys.value = ' '
      })
      keys.addEventListener('keydown', e => {
        if (e.key === 'Enter') {
          e.preventDefault()
          tap(0x1C)
        }
      })
      keysButton = document.createElement('button')
      keysButton.className = 'wm-keys'
      keysButton.textContent = m.keys || ''
      keysButton.setAttribute('data-t', 'control-keys')
      keysButton.addEventListener('click', () => { keys.value = ' '; keys.focus() })
      document.body.append(keys, keysButton)
    }
    tile.addEventListener('pointerdown', onDown)
    tile.addEventListener('pointermove', onMove)
    tile.addEventListener('pointerup', onUp)
    tile.addEventListener('pointercancel', onCancel)
    tile.addEventListener('wheel', onWheel, { passive: false })
    tile.addEventListener('contextmenu', noMenu)
    document.addEventListener('keydown', onKey, true)
    document.addEventListener('keyup', onKey, true)
    window.addEventListener('blur', letGo)
    driving = {
      stop: () => {
        letGo()
        if (queue.length) send({ t: 'ctl', e: queue })
        clearInterval(flush)
        if (press) clearTimeout(press.timer)
        tile.removeEventListener('pointerdown', onDown)
        tile.removeEventListener('pointermove', onMove)
        tile.removeEventListener('pointerup', onUp)
        tile.removeEventListener('pointercancel', onCancel)
        tile.removeEventListener('wheel', onWheel)
        tile.removeEventListener('contextmenu', noMenu)
        document.removeEventListener('keydown', onKey, true)
        document.removeEventListener('keyup', onKey, true)
        window.removeEventListener('blur', letGo)
        if (keys) keys.remove()
        if (keysButton) keysButton.remove()
        document.body.classList.remove('call-driving')
      }
    }
  }

  // ---- call lifecycle ----
  async function start (m) {
    stop()
    const e = ensureEls()
    const r = (run = { id: m.call, closed: false, muted: false, camOn: false, peers: new Map(), group: !!m.group })
    e.box.classList.toggle('group', !!m.group)
    e.box.classList.toggle('room', !!m.room)
    if (m.room) {
      // In a room you see yourself as one of them, with who is talking outlined.
      const tile = document.createElement('div')
      tile.className = 'wm-tile me'
      const letter = document.createElement('div')
      letter.className = 'wm-letter'
      letter.textContent = (Array.from(m.initial || '')[0] || '?').toUpperCase()
      const label = document.createElement('div')
      label.className = 'wm-label'
      label.textContent = m.me || ''
      tile.append(letter, label)
      e.tiles.append(tile)
      r.meTile = tile
    }
    if (m.group) {
      r.talk = setInterval(() => {
        const now = performance.now()
        r.peers.forEach(p => {
          const on = now - (p.lastLoud || 0) < 400
          if (on) stats.speaking++
          p.tile.classList.toggle('speaking', on)
        })
        if (r.meTile) r.meTile.classList.toggle('speaking', !r.muted && now - (r.meLoud || 0) < 400)
      }, 200)
    }
    try {
      r.mic = await navigator.mediaDevices.getUserMedia({
        audio: { echoCancellation: true, noiseSuppression: true, autoGainControl: true, channelCount: 1, sampleRate: 48000 }
      })
    } catch (err) {
      fail('microphone', err)
      return
    }
    if (r.closed) { r.mic.getTracks().forEach(t => t.stop()); return }
    try { startAudio(r) } catch (err) { fail('audio', err); return }
    if (r.closed) return
    e.box.classList.add('active')
    // A peer whose pictures stop (camera off, or the message was lost) goes back to the letter.
    r.watch = setInterval(() => r.peers.forEach(p => { if (p.cam && performance.now() - (p.lastV || 0) > 3000) setCam(r, p, false) }), 1000)
    if (m.video) await cameraOn(true)
  }

  function stop () {
    ring(null)
    const r = run
    run = null
    if (!r) return
    r.closed = true
    clearInterval(r.watch)
    clearInterval(r.talk)
    if (r.meTile) r.meTile.remove()
    document.body.classList.remove('call-min')
    quiet(() => r.micReader && r.micReader.cancel())
    quiet(() => r.camReader && r.camReader.cancel())
    stopShowing(r)
    if (driving) {
      driving.stop()
      driving = null
    }
    ;[r.mic, r.cam].forEach(s => s && s.getTracks().forEach(t => t.stop()))
    for (const c of [r.aenc, r.venc]) quiet(() => c && c.close())
    for (const id of Array.from(r.peers.keys())) removePeer(r, id)
    if (els) {
      els.local.srcObject = null
      els.box.className = ''
    }
  }

  // ---- ringing ----
  function ring (kind) {
    if (ringTimer) { clearInterval(ringTimer); ringTimer = null }
    if (!kind) return
    const beep = () => {
      try {
        ringCtx = ringCtx || new (window.AudioContext || window.webkitAudioContext)()
        if (ringCtx.state === 'suspended') ringCtx.resume()
        const o = ringCtx.createOscillator()
        const g = ringCtx.createGain()
        o.frequency.value = kind === 'in' ? 520 : 425
        g.gain.setValueAtTime(0.0001, ringCtx.currentTime)
        g.gain.exponentialRampToValueAtTime(0.25, ringCtx.currentTime + 0.05)
        g.gain.exponentialRampToValueAtTime(0.0001, ringCtx.currentTime + (kind === 'in' ? 0.9 : 1.0))
        o.connect(g).connect(ringCtx.destination)
        o.start()
        o.stop(ringCtx.currentTime + 1.1)
      } catch (_) {}
    }
    beep()
    ringTimer = setInterval(beep, kind === 'in' ? 2000 : 3000)
  }

  async function handle (m) {
    switch (m.t) {
      case 'a': return onAudio(m)
      case 'v': return onVideo(m)
      case 's': return onScreen(m)
      case 'share': return shareOn(!!m.on)
      case 'control': return driveOn(m)
      case 'small': return document.body.classList.toggle('call-min', !!m.on)
      case 'hand': {
        const r = run
        if (!r) return
        ensurePeer(r, m.from).tile.classList.toggle('hand', !!m.up)
        return
      }
      case 'screenKey':
        if (run && run.share) run.share.forceKey = true
        return
      case 'peerScreen': {
        const r = run
        if (!r) return
        const p = ensurePeer(r, m.from)
        p.sOff = !m.on
        if (m.on) {
          screenTile(p)
          p.sLost = true
          askKey(p)
        } else {
          if (driving) {
            driving.stop()
            driving = null
          }
          closeScreen(p)
        }
        return
      }
      case 'start': return start(m)
      case 'stop': return stop()
      case 'mute':
        if (run) {
          run.muted = !!m.on
          run.mic && run.mic.getAudioTracks().forEach(t => { t.enabled = !m.on })
        }
        return
      case 'cam': return cameraOn(!!m.on)
      case 'peers': {
        const r = run
        if (!r) return
        const keep = new Set(m.peers.map(p => p.id))
        m.peers.forEach(p => ensurePeer(r, p.id, p.name))
        Array.from(r.peers.keys()).forEach(id => { if (!keep.has(id)) removePeer(r, id) })
        return
      }
      case 'peerVideo': {
        const r = run
        if (!r) return
        const p = ensurePeer(r, m.from)
        setCam(r, p, !!m.on)
        if (!m.on) p.lastV = 0
        return
      }
      case 'ring': return ring(m.kind)
    }
  }

  window.wmCall = {
    stats: () => JSON.parse(JSON.stringify(stats)),
    state: () => run && { id: run.id, camOn: run.camOn, muted: run.muted, peers: Array.from(run.peers.keys()), showing: !!run.share },
    usePaintedScreen: on => { paintedScreen = !!on },
    // For tests: how big the screen we watch is, and whether it has more than one colour in it.
    screenInfo: () => {
      const v = document.querySelector('.wm-screen video')
      if (!v || !v.videoWidth) return null
      const c = document.createElement('canvas')
      c.width = 64
      c.height = 36
      const g = c.getContext('2d')
      g.drawImage(v, 0, 0, 64, 36)
      const d = g.getImageData(0, 0, 64, 36).data
      const seen = new Set()
      for (let i = 0; i < d.length; i += 4) seen.add(d[i] >> 4 << 8 | d[i + 1] >> 4 << 4 | d[i + 2] >> 4)
      return { w: v.videoWidth, h: v.videoHeight, colors: seen.size }
    },
    // Pixels in a peer's tile, for tests: true if the picture is not flat.
    tileInfo: () => run && Array.from(run.peers.values()).map(p => ({
      id: p.id, cam: p.cam, w: p.video.videoWidth, h: p.video.videoHeight, name: p.label.textContent, nA: p.nA, nV: p.nV
    })),
    attach: async d => {
      dx = d
      for (;;) {
        let m
        try { m = await dx.recv() } catch (_) { return }
        // Not awaited: a hang-up must not wait behind a slow camera start.
        handle(m).catch(e => note('handle', e))
      }
    }
  }
})()
