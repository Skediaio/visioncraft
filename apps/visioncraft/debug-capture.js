// Debug capture: with ?debug=1 in the URL a small button appears that records a few seconds of
// the canvas (JPEG frames at reduced size) together with the window's state (ui.get,
// document.inspect, ui.load, per-frame CPU/GPU times from window.__vcFrame) and downloads it as
// one JSON file, for a developer to see what the person saw while things move.
// Also callable from the console: `await vcCapture(seconds, fps)`.
(() => {
  async function vcCapture(seconds = 4, fps = 8) {
    const rpc = async (m, p = {}) => { const r = await vcRpc(m, p); return r.ok ? r.result : { error: r.error }; };
    const canvas = [...document.querySelectorAll('canvas')].sort((a, b) => b.width * b.height - a.width * a.height)[0];
    const track = canvas.captureStream(fps * 2).getVideoTracks()[0];
    const grab = new ImageCapture(track);
    const out = { when: new Date().toISOString(), url: location.href, ua: navigator.userAgent,
      canvas: [canvas.width, canvas.height], frames: [] };
    out.ui = await rpc('ui.get');
    out.doc = await rpc('engine.execute', { command: 'document.inspect', params: { faces: false, measures: false } });
    const t0 = performance.now();
    let last = t0;
    while (performance.now() - t0 < seconds * 1000) {
      try {
        const bmp = await grab.grabFrame();
        const w = Math.min(1280, bmp.width), h = Math.round(bmp.height * w / bmp.width);
        const oc = new OffscreenCanvas(w, h);
        oc.getContext('2d').drawImage(bmp, 0, 0, w, h);
        const blob = await oc.convertToBlob({ type: 'image/jpeg', quality: 0.85 });
        const jpg = await new Promise(r => { const fr = new FileReader(); fr.onload = () => r(fr.result); fr.readAsDataURL(blob); });
        const now = performance.now();
        out.frames.push({ t: Math.round(now - t0), dt: Math.round(now - last), stats: JSON.parse(window.__vcFrame || 'null'), jpg });
        last = now;
      } catch (e) {
        out.frames.push({ error: String(e) });
      }
      await new Promise(r => setTimeout(r, 1000 / fps));
    }
    track.stop();
    out.load = await rpc('ui.load');
    out.ui_after = await rpc('ui.get');
    out.doc_after = await rpc('engine.execute', { command: 'document.inspect', params: { faces: false, measures: false } });
    const a = document.createElement('a');
    a.href = URL.createObjectURL(new Blob([JSON.stringify(out)], { type: 'application/json' }));
    a.download = `vc-capture-${Date.now()}.json`;
    a.click();
    return `saved ${out.frames.length} frames as ${a.download}`;
  }
  window.vcCapture = vcCapture;

  if (new URLSearchParams(location.search).get('debug') !== '1') return;
  const button = document.createElement('button');
  button.textContent = 'Debug capture (6 s)';
  button.title = 'Records 6 seconds of the view and the app state, then downloads a JSON file. Start moving right after clicking.';
  Object.assign(button.style, { position: 'fixed', left: '50%', bottom: '64px', transform: 'translateX(-50%)', zIndex: 10,
    padding: '6px 12px', font: '13px system-ui, sans-serif', color: '#fff', background: '#b3261e',
    border: 'none', borderRadius: '6px', cursor: 'pointer', opacity: 0.9 });
  button.onclick = async () => {
    button.disabled = true;
    button.textContent = 'Recording...';
    try {
      button.textContent = await vcCapture(6, 8);
    } catch (e) {
      button.textContent = `Capture failed: ${e}`;
    }
    setTimeout(() => { button.disabled = false; button.textContent = 'Debug capture (6 s)'; }, 3000);
  };
  document.body.appendChild(button);
})();
