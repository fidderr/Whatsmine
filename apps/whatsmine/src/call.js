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
//   Rust -> JS   {t:'start', call, video}    call connected: open mic (+camera)
//                {t:'stop'}  {t:'mute', on}  {t:'cam', on}  {t:'peerVideo', on}
//                {t:'a', ts, d}  {t:'v', ts, key, d}   packets from the peer
//                {t:'ring', kind}            'in' | 'out' | null
//
// Codecs are WebCodecs (Opus 48 kHz mono, VP8), played through
// MediaStreamTrackGenerator, so no WebRTC and no third-party servers.
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
    const remote = document.createElement('video')
    remote.className = 'wm-remote'
    remote.autoplay = true
    remote.muted = true
    remote.playsInline = true
    const local = document.createElement('video')
    local.className = 'wm-local'
    local.autoplay = true
    local.muted = true
    local.playsInline = true
    const audio = document.createElement('audio')
    audio.autoplay = true
    box.append(remote, local, audio)
    document.body.append(box)
    els = { box, remote, local, audio }
    return els
  }

  // ---- audio ----
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

    r.adec = new AudioDecoder({
      output: ad => {
        stats.aDec++
        r.agenW.write(ad).catch(() => { try { ad.close() } catch (_) {} })
      },
      error: e => fail('audio-decoder', e)
    })
    r.adec.configure({ codec: 'opus', sampleRate: 48000, numberOfChannels: 1 })
    r.agen = new MediaStreamTrackGenerator({ kind: 'audio' })
    r.agenW = r.agen.writable.getWriter()
    r.aIdx = 0
    const e = ensureEls()
    e.audio.srcObject = new MediaStream([r.agen])
    e.audio.play().catch(err => note('audio-play', err))
  }

  function onAudio (m) {
    const r = run
    if (!r || !r.adec || r.adec.state !== 'configured') return
    stats.aRecv++
    // The network may reorder or drop packets; the playback clock is ours.
    try {
      r.adec.decode(new EncodedAudioChunk({ type: 'key', timestamp: (r.aIdx++) * 20000, data: unb64(m.d) }))
    } catch (e) { note('audio-decode', e) }
  }

  // ---- video ----
  function initVideoEncoder (r, w, h) {
    if (r.venc) { try { r.venc.close() } catch (_) {} }
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
      try { r.camReader && r.camReader.cancel() } catch (_) {}
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

  function ensureVideoDecoder (r) {
    if (r.vdec && r.vdec.state === 'configured') return
    r.vdec = new VideoDecoder({
      output: f => {
        stats.vDec++
        r.vgenW.write(f).catch(() => { try { f.close() } catch (_) {} })
      },
      error: e => { note('video-decoder', e); r.vdec = null }
    })
    r.vdec.configure({ codec: 'vp8' })
  }

  function onVideo (m) {
    const r = run
    if (!r) return
    stats.vRecv++
    // A delta frame is useless until a key frame has been seen.
    if (!r.vdec || r.vdec.state !== 'configured') {
      if (!m.key) { stats.vKeyWait++; return }
      ensureVideoDecoder(r)
    }
    try {
      r.vdec.decode(new EncodedVideoChunk({ type: m.key ? 'key' : 'delta', timestamp: (r.vIdx = (r.vIdx || 0) + 1) * 66000, data: unb64(m.d) }))
    } catch (e) { note('video-decode', e); r.vdec = null }
  }

  // ---- call lifecycle ----
  async function start (m) {
    await stop()
    const e = ensureEls()
    const r = (run = { id: m.call, closed: false, muted: false, camOn: false })
    try {
      r.mic = await navigator.mediaDevices.getUserMedia({
        audio: { echoCancellation: true, noiseSuppression: true, autoGainControl: true, channelCount: 1, sampleRate: 48000 }
      })
    } catch (err) {
      fail('microphone', err)
      return
    }
    if (r.closed) { r.mic.getTracks().forEach(t => t.stop()); return }
    r.vgen = new MediaStreamTrackGenerator({ kind: 'video' })
    r.vgenW = r.vgen.writable.getWriter()
    e.remote.srcObject = new MediaStream([r.vgen])
    e.remote.play().catch(() => {})
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
    try { r.micReader && r.micReader.cancel() } catch (_) {}
    try { r.camReader && r.camReader.cancel() } catch (_) {}
    ;[r.mic, r.cam].forEach(s => s && s.getTracks().forEach(t => t.stop()))
    for (const c of [r.aenc, r.adec, r.venc, r.vdec]) { try { c && c.close() } catch (_) {} }
    for (const w of [r.agenW, r.vgenW]) { try { w && w.close() } catch (_) {} }
    if (els) {
      els.audio.srcObject = null
      els.remote.srcObject = null
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
      case 'peerVideo':
        if (els) els.box.classList.toggle('has-remote', !!m.on)
        return
      case 'ring': return ring(m.kind)
    }
  }

  window.wmCall = {
    stats: () => JSON.parse(JSON.stringify(stats)),
    state: () => run && { id: run.id, camOn: run.camOn, muted: run.muted },
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
