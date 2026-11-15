#!/usr/bin/env node
// Browser smoke check: serves a trunk build and the asset library, opens VisionCraft in headless
// Chromium on WebGPU, drives it through the page's `vcRpc` (index.html, the JSON-lines protocol
// of `visioncraft-cli serve`) and fails on console errors, WebGPU validation messages, panics or
// a black picture. Node 22 or later (built-in WebSocket), no packages.
//
//   node apps/visioncraft/browser-smoke/smoke.mjs --dist apps/visioncraft/dist --assets assets \
//     [--chrome /usr/bin/chromium-browser] [--out smoke-out] [--quality medium|auto|low|...]
//     [--examples garden,estate,beach-house,coast] [--views] [--size 1600x900]
//
// Writes <out>/<example>-<step>.png and <out>/report.json; exits 1 when a check failed.
import { spawn } from 'node:child_process';
import fs from 'node:fs';
import http from 'node:http';
import path from 'node:path';

const args = process.argv.slice(2);
const opt = (name, dflt) => {
  const i = args.indexOf(`--${name}`);
  return i < 0 ? dflt : args[i + 1];
};
const flag = name => args.includes(`--${name}`);
const DIST = path.resolve(opt('dist', 'apps/visioncraft/dist'));
const ASSETS = path.resolve(opt('assets', 'assets'));
const CHROME = opt('chrome', '/usr/bin/chromium-browser');
const OUT = path.resolve(opt('out', 'smoke-out'));
const QUALITY = opt('quality', 'medium');
const EXAMPLES = opt('examples', 'garden,estate,beach-house,coast').split(',').filter(Boolean);
const VIEWS = flag('views');
const [W, H] = opt('size', '1600x900').split('x').map(Number);
const LOAD_TIMEOUT_S = Number(opt('timeout', '240'));
fs.mkdirSync(OUT, { recursive: true });

const sleep = ms => new Promise(r => setTimeout(r, ms));
const t0 = Date.now();
const log = (...m) => console.log(`[${((Date.now() - t0) / 1000).toFixed(1)}s]`, ...m);

// --- static server: the build at /, the library at /assets/ ---
const TYPES = { '.html': 'text/html', '.js': 'text/javascript', '.wasm': 'application/wasm', '.json': 'application/json',
  '.png': 'image/png', '.jpg': 'image/jpeg', '.jpeg': 'image/jpeg', '.svg': 'image/svg+xml', '.ktx2': 'image/ktx2' };
const server = http.createServer((req, res) => {
  const url = decodeURIComponent(new URL(req.url, 'http://x').pathname);
  const file = url.startsWith('/assets/')
    ? path.join(ASSETS, url.slice('/assets/'.length))
    : path.join(DIST, url === '/' ? 'index.html' : url);
  if (!file.startsWith(ASSETS) && !file.startsWith(DIST)) { res.writeHead(403).end(); return; }
  fs.stat(file, (err, st) => {
    if (err || !st.isFile()) { res.writeHead(404).end(); return; }
    res.writeHead(200, { 'content-type': TYPES[path.extname(file)] ?? 'application/octet-stream', 'content-length': st.size });
    fs.createReadStream(file).pipe(res);
  });
});
await new Promise(r => server.listen(0, '127.0.0.1', r));
const PAGE = `http://127.0.0.1:${server.address().port}/`;

// --- Chromium over the DevTools protocol ---
const profile = fs.mkdtempSync(path.join(OUT, 'profile-'));
const PORT = 9300 + Math.floor(Math.random() * 600);
const chrome = spawn(CHROME, ['--headless=new', '--no-sandbox', `--user-data-dir=${profile}`, `--remote-debugging-port=${PORT}`,
  '--enable-unsafe-webgpu', '--enable-features=Vulkan', '--use-angle=vulkan', '--ignore-gpu-blocklist',
  `--window-size=${W},${H}`, '--no-first-run', 'about:blank'], { stdio: 'ignore' });
let tabs;
for (let i = 0; i < 80 && !tabs; i++) {
  try { tabs = await (await fetch(`http://127.0.0.1:${PORT}/json`)).json(); } catch { await sleep(250); }
}
const ws = new WebSocket(tabs.find(t => t.type === 'page').webSocketDebuggerUrl);
await new Promise(r => { ws.onopen = r; });
let nextId = 0;
const pending = new Map();
const listeners = [];
ws.onmessage = e => {
  const m = JSON.parse(e.data);
  if (m.id && pending.has(m.id)) { pending.get(m.id)(m); pending.delete(m.id); } else if (m.method) listeners.forEach(l => l(m));
};
const cdp = (method, params = {}) => new Promise((resolve, reject) => {
  pending.set(++nextId, m => (m.error ? reject(new Error(`${method}: ${m.error.message}`)) : resolve(m.result)));
  ws.send(JSON.stringify({ id: nextId, method, params }));
});
const evaluate = async (expression, timeoutMs = 120000) => {
  const r = await Promise.race([
    cdp('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true }),
    sleep(timeoutMs).then(() => ({ exceptionDetails: { text: `timed out after ${timeoutMs} ms: the page did not answer` } })),
  ]);
  if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description ?? r.exceptionDetails.text);
  return r.result.value;
};

// --- what the page says ---
const consoleLines = [];
const network = { bytes: 0, byType: {}, requests: 0, failed: [] };
const reqType = new Map();
listeners.push(m => {
  if (m.method === 'Runtime.consoleAPICalled') {
    const text = m.params.args.map(a => a.value ?? a.description ?? '').join(' ');
    consoleLines.push({ t: Date.now() - t0, level: m.params.type, text });
  } else if (m.method === 'Runtime.exceptionThrown') {
    consoleLines.push({ t: Date.now() - t0, level: 'exception', text: m.params.exceptionDetails.exception?.description ?? m.params.exceptionDetails.text });
  } else if (m.method === 'Log.entryAdded') {
    consoleLines.push({ t: Date.now() - t0, level: m.params.entry.level, text: `[${m.params.entry.source}] ${m.params.entry.text}` });
  } else if (m.method === 'Network.responseReceived') {
    const u = new URL(m.params.response.url);
    reqType.set(m.params.requestId, path.extname(u.pathname) || 'other');
    if (m.params.response.status >= 400) network.failed.push(`${m.params.response.status} ${u.pathname}`);
  } else if (m.method === 'Network.loadingFinished') {
    const ty = reqType.get(m.params.requestId) ?? 'other';
    network.bytes += m.params.encodedDataLength;
    network.byType[ty] = (network.byType[ty] ?? 0) + m.params.encodedDataLength;
    network.requests++;
  }
});
// Errors: anything the page logs as an error, a panic, or WebGPU complaining.
const BAD = /panicked|RuntimeError|unreachable|Validation Error|validation|GPUValidationError|lost|out of memory|OperationError|CONTEXT_LOST/i;
const isBad = l => l.level === 'error' || l.level === 'exception' || BAD.test(l.text);

await cdp('Runtime.enable');
await cdp('Log.enable');
await cdp('Page.enable');
await cdp('Network.enable');
await cdp('Emulation.setDeviceMetricsOverride', { width: W, height: H, deviceScaleFactor: 1, mobile: false });
// Frame times: each presented frame takes one canvas texture.
await cdp('Page.addScriptToEvaluateOnNewDocument', { source: `
  window.__vcFrames = [];
  const get = GPUCanvasContext.prototype.getCurrentTexture;
  GPUCanvasContext.prototype.getCurrentTexture = function () {
    window.__vcFrames.push(performance.now());
    if (window.__vcFrames.length > 4000) window.__vcFrames.splice(0, 2000);
    return get.call(this);
  };
  // Main-thread tasks over 50 ms (the page does not answer meanwhile).
  window.__vcLong = [];
  new PerformanceObserver(l => l.getEntries().forEach(e => window.__vcLong.push([e.startTime, e.duration])))
    .observe({ type: 'longtask', buffered: true });` });

const checks = [];
const check = (name, ok, detail = '') => { checks.push({ name, ok, detail }); log(ok ? 'ok  ' : 'FAIL', name, detail); };
const rpc = async (method, params = {}, timeoutMs = 120000) => {
  const r = await evaluate(`vcRpc(${JSON.stringify(method)}, ${JSON.stringify(params)})`, timeoutMs);
  if (!r?.ok) throw new Error(`${method} ${JSON.stringify(params)}: ${r?.error ?? JSON.stringify(r)}`);
  return r.result;
};
const exec = (command, params = {}) => rpc('engine.execute', { command, params });
const uiGet = () => rpc('ui.get');

// The 3D view's brightness in the middle of the screenshot (panels sit at the edges).
const picture = async name => {
  const shot = await cdp('Page.captureScreenshot', { format: 'png' });
  fs.writeFileSync(path.join(OUT, `${name}.png`), Buffer.from(shot.data, 'base64'));
  const s = await evaluate(`(async () => {
    const img = new Image(); img.src = 'data:image/png;base64,${shot.data}'; await img.decode();
    const c = new OffscreenCanvas(img.width, img.height), g = c.getContext('2d'); g.drawImage(img, 0, 0);
    const x0 = img.width * 0.25 | 0, y0 = img.height * 0.2 | 0, w = img.width * 0.5 | 0, h = img.height * 0.6 | 0;
    const d = g.getImageData(x0, y0, w, h).data; let sum = 0, sum2 = 0, n = 0;
    for (let i = 0; i < d.length; i += 16) { const l = 0.2126 * d[i] + 0.7152 * d[i + 1] + 0.0722 * d[i + 2]; sum += l; sum2 += l * l; n++; }
    const mean = sum / n; return { mean, sd: Math.sqrt(Math.max(0, sum2 / n - mean * mean)) };
  })()`);
  return { name, mean: +s.mean.toFixed(1), sd: +s.sd.toFixed(1) };
};

// Waits until nothing is on its way (plants, downloads, sky volume) or the timeout.
const settle = async (timeoutS = LOAD_TIMEOUT_S) => {
  const start = Date.now();
  let calm = 0, last;
  while ((Date.now() - start) / 1000 < timeoutS) {
    last = await uiGet();
    const p = last.pending;
    calm = p.plants === 0 && !p.library && !p.skyVolume ? calm + 1 : 0;
    if (calm >= 3) return { s: (Date.now() - start) / 1000, done: true, ui: last };
    await sleep(500);
  }
  return { s: (Date.now() - start) / 1000, done: false, ui: last };
};

// Frame times while the camera circles a little (every request is a repaint).
const frameTimes = async (seconds = 4) => {
  const ui = await uiGet();
  const [p, t] = [ui.camera.position, ui.camera.target];
  const r = Math.hypot(p[0] - t[0], p[2] - t[2]), a0 = Math.atan2(p[2] - t[2], p[0] - t[0]);
  const start = await evaluate('performance.now()');
  const end = Date.now() + seconds * 1000;
  for (let i = 0; Date.now() < end; i++) {
    const a = a0 + 0.004 * i;
    await rpc('ui.set', { camera: { position: [t[0] + r * Math.cos(a), p[1], t[2] + r * Math.sin(a)], target: t } });
  }
  await rpc('ui.set', { camera: { position: p, target: t } });
  const f = await evaluate(`window.__vcFrames.filter(x => x > ${start})`);
  const dt = f.slice(1).map((x, i) => x - f[i]).sort((a, b) => a - b);
  const q = k => +(dt[Math.min(dt.length - 1, Math.floor(dt.length * k))] ?? NaN).toFixed(1);
  return { frames: dt.length, median_ms: q(0.5), p90_ms: q(0.9), fps: +(1000 / q(0.5)).toFixed(1) };
};

const memory = async () => {
  const m = await cdp('Performance.getMetrics').catch(() => null);
  const get = k => m?.metrics.find(x => x.name === k)?.value ?? 0;
  const stats = await evaluate('window.__vcStats ? JSON.parse(window.__vcStats) : null');
  const long = await evaluate('window.__vcLong.splice(0).map(e => e[1])');
  return { long_tasks: long.length, longest_task_ms: Math.round(Math.max(0, ...long)), long_task_total_s: +(long.reduce((a, b) => a + b, 0) / 1000).toFixed(1), js_heap_mb: +(get('JSHeapUsedSize') / 1048576).toFixed(1), texture_mb: stats ? +(stats.texture_bytes / 1048576).toFixed(1) : null, peak_texture_mb: stats ? +(stats.peak_texture_bytes / 1048576).toFixed(1) : null };
};

const report = { page: PAGE, quality: QUALITY, size: [W, H], examples: {}, checks, console: consoleLines, network };
let exitCode = 0;
try {
  await cdp('Performance.enable');
  const navStart = Date.now();
  await cdp('Page.navigate', { url: PAGE });
  // The app is up when the loading card is gone and the bridge answers.
  for (;;) {
    const up = await evaluate(`(document.getElementById('vc-status')?.hidden ?? false) && !!window.__vcRpcWake`).catch(() => false);
    if (up || (Date.now() - navStart) / 1000 > 120 || consoleLines.some(isBad)) break;
    await sleep(100);
  }
  report.first_picture_s = (Date.now() - navStart) / 1000;
  report.adapter = await evaluate(`(async () => { const a = await navigator.gpu.requestAdapter(); const i = a.info || {}; return { vendor: i.vendor, architecture: i.architecture, description: i.description, maxSampledTexturesPerShaderStage: a.limits.maxSampledTexturesPerShaderStage, maxTextureDimension2D: a.limits.maxTextureDimension2D, maxBufferSize: a.limits.maxBufferSize, features: [...a.features] }; })()`);
  log('adapter', JSON.stringify(report.adapter));
  check('app starts', !!(await evaluate('!!window.__vcRpcWake')), `${report.first_picture_s.toFixed(1)} s`);
  report.start_screen = await picture('start');
  check('start screen is not black', report.start_screen.mean > 8, JSON.stringify(report.start_screen));
  report.download_at_start_mb = +(network.bytes / 1048576).toFixed(1);
  if (QUALITY !== 'auto') await rpc('ui.set', { quality: QUALITY });

  for (const ex of EXAMPLES) {
    const r = { pictures: [] };
    report.examples[ex] = r;
    const before = consoleLines.length, bytesBefore = network.bytes;
    try {
      await exec('file.new');
      await evaluate('window.__vcLong.length = 0');
      const t = Date.now();
      await exec('project.load_example', { name: ex });
      await rpc('ui.set', { mode: null });
      r.load_command_s = (Date.now() - t) / 1000;
      const s = await settle();
      r.settle_s = +s.s.toFixed(1);
      r.loaded = s.done;
      r.quality = s.ui.quality;
      check(`${ex}: everything arrives`, s.done, `${r.settle_s} s ${JSON.stringify(s.ui.pending)}`);
      const pic = await picture(`${ex}-open`);
      r.pictures.push(pic);
      check(`${ex}: picture is not black`, pic.mean > 12 && pic.sd > 2, JSON.stringify(pic));
      r.frames = await frameTimes();
      r.memory = await memory();
      log(ex, 'frames', JSON.stringify(r.frames), 'memory', JSON.stringify(r.memory));
      if (VIEWS) {
        const doc = await exec('document.inspect', { faces: false, measures: false });
        const vps = doc.viewpoints ?? doc.project?.viewpoints ?? [];
        for (const v of vps) {
          await rpc('ui.fly', { viewpoint: v.id });
          await sleep(2500);
          const vs = await settle(90);
          const name = `${ex}-${String(v.name).toLowerCase().replace(/[^a-z0-9]+/g, '-')}`;
          const vp = await picture(name);
          vp.settled = vs.done;
          r.pictures.push(vp);
          check(`${name}: not black`, vp.mean > 6 && vp.sd > 1.5, JSON.stringify(vp));
        }
      }
    } catch (e) {
      check(`${ex}: runs`, false, String(e.message ?? e));
    }
    r.download_mb = +((network.bytes - bytesBefore) / 1048576).toFixed(1);
    r.console = consoleLines.slice(before).map(l => `${l.level}: ${l.text}`.slice(0, 400));
    const bad = consoleLines.slice(before).filter(isBad);
    check(`${ex}: no console errors`, bad.length === 0, bad.slice(0, 3).map(l => l.text.slice(0, 300)).join(' | '));
  }
} catch (e) {
  check('smoke run', false, String(e.stack ?? e));
} finally {
  report.download_total_mb = +(network.bytes / 1048576).toFixed(1);
  report.dist_sizes = Object.fromEntries(fs.readdirSync(DIST).filter(f => /\.(wasm|js)$/.test(f)).map(f => [f, fs.statSync(path.join(DIST, f)).size]));
  fs.writeFileSync(path.join(OUT, 'report.json'), JSON.stringify(report, null, 1));
  const failed = checks.filter(c => !c.ok);
  log(`${checks.length - failed.length}/${checks.length} checks passed; report in ${path.join(OUT, 'report.json')}`);
  if (failed.length) exitCode = 1;
  chrome.kill();
  server.close();
  await sleep(500);
  fs.rmSync(profile, { recursive: true, force: true, maxRetries: 5 });
  process.exit(exitCode);
}
