#!/usr/bin/env node
// Browser smoke check: serves a trunk build and the asset library, opens VisionCraft in headless
// Chromium on WebGPU, drives it through the page's `vcRpc` (index.html, the JSON-lines protocol
// of `visioncraft-cli serve`) and fails on console errors, WebGPU validation messages, panics or
// a black picture. Node 22 or later (built-in WebSocket), no packages.
//
//   node apps/visioncraft/browser-smoke/smoke.mjs --dist apps/visioncraft/dist --assets assets \
//     [--chrome /usr/bin/chromium-browser] [--out smoke-out] [--quality medium|auto|low|...]
//     [--examples garden,estate,beach-house,coast] [--views] [--size 1600x900]
//     [--interact [--model some.3dcraft] [--object library-asset-id]]
//
// --views flies to every viewpoint of each example; --interact walks, zooms out, draws, plants,
// places, paints, reloads (browser storage), saves (download) and opens (upload) in the first.
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
// Walk, zoom out, draw, plant, place, paint, save and reopen in the first example.
const INTERACT = flag('interact');
const MODEL = opt('model', '') && path.resolve(opt('model', ''));
const OBJECT_ASSET = opt('object', 'painted_wooden_bench');
const STEPS = opt('steps', '');
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
// The start card's rows, top to bottom (lib.rs, the empty project's card), 39 px apart.
const START_ROWS = ['draw', '3dcraft', 'garden', 'estate', 'beach-house', 'coast'];
const click = async (x, y) => {
  for (const type of ['mouseMoved', 'mousePressed', 'mouseReleased']) {
    await cdp('Input.dispatchMouseEvent', { type, x, y, button: 'left', clickCount: 1 });
    await sleep(60);
  }
};
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
    calm = p.plants === 0 && !(p.models > 0) && !p.library && !p.skyVolume ? calm + 1 : 0;
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

const appUp = async () => {
  const since = Date.now();
  while ((Date.now() - since) / 1000 < 120) {
    if (await evaluate(`(document.getElementById('vc-status')?.hidden ?? false) && !!window.__vcRpcWake`).catch(() => false)) return (Date.now() - since) / 1000;
    await sleep(100);
  }
  throw new Error('the app did not come up within 120 s');
};
const names = async () => {
  const d = await exec('document.inspect', { faces: false, measures: false, placements: true });
  return JSON.stringify(d);
};
// The top bar's buttons, from the left (1:1 pixels).
const OPEN_BUTTON = [45, 32], SAVE_BUTTON = [98, 32];
const chooseFile = async (button, file) => {
  await cdp('Page.setInterceptFileChooserDialog', { enabled: true });
  const opened = new Promise(res => listeners.push(m => { if (m.method === 'Page.fileChooserOpened') res(m.params); }));
  await click(...button);
  const chooser = await Promise.race([opened, sleep(10000).then(() => null)]);
  if (!chooser) throw new Error('no file chooser opened');
  await cdp('DOM.setFileInputFiles', { files: [file], backendNodeId: chooser.backendNodeId });
  await cdp('Page.setInterceptFileChooserDialog', { enabled: false });
};

// What a person does in a project: walk, zoom out, draw, plant, place, paint, save, reopen.
const interact = async ex => {
  const out = {};
  // Walking a few metres (the W key held).
  const before = (await uiGet()).camera.position;
  await rpc('ui.set', { walk: true });
  await sleep(500);
  await cdp('Input.dispatchKeyEvent', { type: 'keyDown', key: 'w', code: 'KeyW', windowsVirtualKeyCode: 87 });
  out.walk_frames = await (async () => { const s = await evaluate('performance.now()'); await sleep(2500); const f = await evaluate(`window.__vcFrames.filter(x => x > ${s})`); return f.length; })();
  await cdp('Input.dispatchKeyEvent', { type: 'keyUp', key: 'w', code: 'KeyW', windowsVirtualKeyCode: 87 });
  const walked = (await uiGet()).camera;
  out.walked_m = +Math.hypot(walked.position[0] - before[0], walked.position[2] - before[2]).toFixed(1);
  out.walk_view = walked.view;
  out.pictures = [await picture(`${ex}-walk`)];
  check(`${ex}: walking moves`, walked.view === 'walk' && out.walk_frames > 10, `${out.walk_frames} frames in 2.5 s, ${out.walked_m} m from the orbit eye`);
  await rpc('ui.set', { walk: false });
  // Orbit far out.
  await rpc('ui.set', { camera: { position: [600, 900, 1400], target: [0, 0, 0] } });
  await settle(60);
  out.far = await frameTimes(3);
  out.pictures.push(await picture(`${ex}-far`));
  check(`${ex}: far out is not black`, out.pictures.at(-1).mean > 12, JSON.stringify(out.far));
  // Draw a lawn and a path, plant a tree, place an object, change a material.
  const lawn = await exec('shape.add_area', { name: 'Smoke lawn', outline: [[-30, -30], [-18, -30], [-18, -20], [-30, -20]], material: 'lawn' });
  await exec('shape.add_area', { name: 'Smoke path', outline: [[-18, -26], [-10, -26], [-10, -25], [-18, -25]], material: 'gravel' });
  await exec('planting.add_single', { name: 'Smoke birch', asset: 'flora:betula-pendula', position: [-24, 0, -25] });
  await exec('planting.add_single', { name: 'Smoke bench', asset: OBJECT_ASSET, position: [-20, 0, -22] });
  await exec('material.assign', { target: { type: 'owner', owner: lawn.id }, material: 'paving' });
  await rpc('ui.set', { camera: { position: [-14, 9, -10], target: [-22, 0, -25] } });
  const s = await settle(120);
  out.pictures.push(await picture(`${ex}-edited`));
  check(`${ex}: drawn, planted and placed things arrive`, s.done, JSON.stringify(s.ui.pending));
  // Browser storage: a reload brings the project back.
  await cdp('Page.reload');
  out.reload_s = await appUp();
  const restored = await names();
  check(`${ex}: the project survives a reload (browser storage)`, restored.includes('Smoke lawn') && restored.includes('Smoke bench'), `${out.reload_s.toFixed(1)} s`);
  // Download (Save) and upload (Open).
  const dl = fs.mkdtempSync(path.join(OUT, 'download-'));
  await cdp('Browser.setDownloadBehavior', { behavior: 'allow', downloadPath: dl });
  await click(...SAVE_BUTTON);
  let saved;
  for (let i = 0; i < 60 && !saved; i++) { await sleep(500); saved = fs.readdirSync(dl).find(f => !f.endsWith('.crdownload')); }
  check(`${ex}: Save downloads the project`, !!saved, saved ?? 'nothing downloaded in 30 s');
  if (saved) {
    out.saved_bytes = fs.statSync(path.join(dl, saved)).size;
    await exec('file.new');
    await chooseFile(OPEN_BUTTON, path.join(dl, saved));
    let reopened = '';
    for (let i = 0; i < 40 && !reopened.includes('Smoke lawn'); i++) { await sleep(500); reopened = await names(); }
    check(`${ex}: Open reads the downloaded project back`, reopened.includes('Smoke lawn'), `${saved}, ${out.saved_bytes} bytes`);
  }
  // A 3dcraft model file.
  if (MODEL) {
    await exec('file.new');
    await sleep(500);
    await chooseFile(OPEN_BUTTON, MODEL);
    let models = 0;
    for (let i = 0; i < 120 && !models; i++) { await sleep(500); models = (await exec('document.inspect', { faces: false, measures: false })).models?.length ?? 0; }
    const ms = await settle(120);
    out.pictures.push(await picture(`${ex}-model`));
    check(`${ex}: a .3dcraft model opens`, models > 0 && out.pictures.at(-1).mean > 12, `${path.basename(MODEL)}: ${models} model(s), settled ${ms.done}`);
  }
  return out;
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

  // --steps FILE: a module whose default export runs instead of the examples (for chasing one
  // problem), given the helpers.
  if (STEPS) {
    const steps = (await import(path.resolve(STEPS))).default;
    report.steps = await steps({ rpc, exec, uiGet, picture, settle, frameTimes, memory, sleep, cdp, evaluate, click, check, log, consoleLines });
    EXAMPLES.length = 0;
  }
  for (const ex of EXAMPLES) {
    const r = { pictures: [] };
    report.examples[ex] = r;
    const before = consoleLines.length, bytesBefore = network.bytes;
    try {
      await exec('file.new');
      await evaluate('window.__vcLong.length = 0');
      const t = Date.now();
      // As a person: the start card's button (centred; one row per example, `START_ROWS`).
      await sleep(500);
      const row = START_ROWS.indexOf(ex);
      await click(W / 2, H / 2 - 86 + 39 * row);
      await sleep(1500);
      const opened = await exec('document.inspect', { faces: false, measures: false });
      // The right example: each has its own number of viewpoints.
      r.opened_by_click = (opened.viewpoints ?? []).length === { garden: 6, estate: 3, 'beach-house': 10, coast: 15 }[ex];
      check(`${ex}: the start screen button opens it`, r.opened_by_click);
      if (!r.opened_by_click) await exec('project.load_example', { name: ex });
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
    if (INTERACT && ex === EXAMPLES[0]) {
      try { r.interact = await interact(ex); } catch (e) { check(`${ex}: interaction`, false, String(e.message ?? e)); }
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
