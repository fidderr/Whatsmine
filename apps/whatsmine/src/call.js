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
//   Rust -> JS   {t:'start', call, video, group}   we are in the call
//                {t:'stop'}  {t:'mute', on}  {t:'cam', on}
//                {t:'peers', peers:[{id,name}]}    who else is in the call
//                {t:'peerVideo', from, on}
//                {t:'a', from, ts, d}  {t:'v', from, ts, key, d}   packets from a peer
//                {t:'ring', kind}            'in' | 'out' | null
//
// Every other person in the call gets their own decoders and a tile, so a
// group call is the same code as a 1:1 call. Codecs are WebCodecs (Opus
// 48 kHz mono, VP8), played through MediaStreamTrackGenerator: no WebRTC and
// no third-party servers.
(() => {
  if (window.wmCall) return
  const CFG = { aBitrate: 24000, vBitrate: 700000, fps: 15, keyEvery: 2 }
  const stats = { aSent: 0, aRecv: 0, aDec: 0, vSent: 0, vRecv: 0, vDec: 0, vKeyWait: 0, errors: [] }
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

  function setName (p, name) {
    if (!name) return
    p.label.textContent = name
    p.letter.textContent = (Array.from(name)[0] || '?').toUpperCase()
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
    p = { id, tile, video, audio, label, letter, aIdx: 0, vIdx: 0, cam: false, vdec: null }
    setName(p, name)

    p.adec = new AudioDecoder({
      output: ad => {
        stats.aDec++
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

  // ---- call lifecycle ----
  async function start (m) {
    await stop()
    const e = ensureEls()
    const r = (run = { id: m.call, closed: false, muted: false, camOn: false, peers: new Map() })
    e.box.classList.toggle('group', !!m.group)
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
    e.box.classList.add('active')
    if (m.video) await cameraOn(true)
  }

  async function stop () {
    ring(null)
    const r = run
    run = null
    if (!r) return
    r.closed = true
    quiet(() => r.micReader && r.micReader.cancel())
    quiet(() => r.camReader && r.camReader.cancel())
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
        p.cam = !!m.on
        p.tile.classList.toggle('cam', p.cam)
        layout(r)
        return
      }
      case 'ring': return ring(m.kind)
    }
  }

  window.wmCall = {
    stats: () => JSON.parse(JSON.stringify(stats)),
    state: () => run && { id: run.id, camOn: run.camOn, muted: run.muted, peers: Array.from(run.peers.keys()) },
    // Pixels in a peer's tile, for tests: true if the picture is not flat.
    tileInfo: () => run && Array.from(run.peers.values()).map(p => ({
      id: p.id, cam: p.cam, w: p.video.videoWidth, h: p.video.videoHeight, name: p.label.textContent
    })),
    attach: async d => {
      dx = d
      for (;;) {
        let m
        try { m = await dx.recv() } catch (_) { return }
        try { await handle(m) } catch (e) { note('handle', e) }
      }
    }
  }
})()
