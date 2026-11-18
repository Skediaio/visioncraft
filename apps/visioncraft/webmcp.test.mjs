// Tests webmcp.js with a mock `document.modelContext` and a mock `vcRpc`:
//   node --test apps/visioncraft/webmcp.test.mjs
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import test from 'node:test';

const { createWebMcp } = createRequire(import.meta.url)('./webmcp.js');

const tool = (name, route, extra = {}) => ({
  name,
  title: name,
  description: name,
  inputSchema: { type: 'object', properties: {}, additionalProperties: true },
  annotations: { readOnlyHint: false, consequentialHint: false },
  route,
  ...extra,
});

const payload = {
  tools: [
    tool('visioncraft_view_set', { kind: 'command', id: 'view.set' }),
    tool('visioncraft_film_remove', { kind: 'command', id: 'film.remove' }, { destructive: true }),
    tool('visioncraft_film_render', { kind: 'command', id: 'film.render' }),
    tool('visioncraft_run_command', { kind: 'run_command' }),
    tool('visioncraft_camera_set', { kind: 'method', name: 'ui.set', wrap: 'camera' }),
    tool('visioncraft_list', { kind: 'section', sections: { plantings: 'generators' } }),
    tool('visioncraft_snapshot', { kind: 'snapshot' }),
  ],
  destructiveCommands: ['film.remove', 'shape.remove'],
};

function setup() {
  const registered = new Map();
  const signals = [];
  const calls = [];
  let release = null;
  const modelContext = {
    registerTool(def, options) {
      registered.set(def.name, def);
      if (options?.signal) {
        signals.push(options.signal);
        options.signal.addEventListener('abort', () => registered.delete(def.name));
      }
    },
  };
  const rpc = async (method, params) => {
    calls.push({ method, params });
    if (params?.command === 'film.render') await new Promise((r) => (release = r));
    if (method === 'engine.commands') return { ok: true, result: { commands: [] } };
    if (params?.command === 'document.inspect') return { ok: true, result: { generators: [{ id: 'g1' }] } };
    if (params?.command === 'bad.command') return { ok: false, error: 'unknown command `bad.command`' };
    return { ok: true, result: null };
  };
  const frames = [];
  const pixel = new Uint8ClampedArray(4 * 4 * 4).fill(255);
  const canvas = { width: 2048, height: 1024 };
  const createCanvas = (w, h) => ({
    width: w,
    height: h,
    getContext: () => ({ drawImage() {}, getImageData: () => ({ data: pixel }) }),
    toDataURL: (mime) => `data:${mime};base64,QUJD`,
  });
  const api = createWebMcp({
    document: { modelContext },
    navigator: {},
    rpc,
    wake: () => {},
    canvas: () => canvas,
    raf: (f) => { frames.push(f); queueMicrotask(f); },
    createCanvas,
  });
  return { api, registered, signals, calls, release: () => release && release() };
}

test('nothing happens without a modelContext', () => {
  const api = createWebMcp({ document: {}, navigator: {}, rpc: async () => ({ ok: true }) });
  assert.equal(api.supported(), false);
  assert.equal(api.register(payload), 0);
});

test('the deprecated navigator.modelContext is used when the document has none', () => {
  const seen = [];
  const api = createWebMcp({
    document: {},
    navigator: { modelContext: { registerTool: (d) => seen.push(d.name) } },
    rpc: async () => ({ ok: true }),
  });
  assert.equal(api.supported(), true);
  assert.equal(api.register(payload), payload.tools.length);
  assert.equal(seen.length, payload.tools.length);
});

test('registers every tool once and unregisters through the abort signal', () => {
  const { api, registered, signals } = setup();
  assert.equal(api.register(JSON.stringify(payload)), payload.tools.length);
  assert.equal(registered.size, payload.tools.length);
  api.register(payload); // registering again replaces, not duplicates
  assert.equal(registered.size, payload.tools.length);
  assert.equal(signals[0].aborted, true);
  api.unregister();
  assert.equal(registered.size, 0);
  assert.equal(api.status().registered, 0);
});

test('execute round-trips view.set through the page protocol', async () => {
  const { api, registered, calls } = setup();
  api.register(payload);
  const out = await registered.get('visioncraft_view_set').execute({ rain: 0.4 });
  assert.deepEqual(calls[0], {
    method: 'engine.execute',
    params: { command: 'view.set', params: { rain: 0.4 }, intent: 'WebMCP: visioncraft_view_set' },
  });
  assert.equal(out.content[0].type, 'text');
});

test('a failed command rejects with the command message', async () => {
  const { api, registered } = setup();
  api.register(payload);
  await assert.rejects(
    registered.get('visioncraft_run_command').execute({ command: 'bad.command' }),
    /unknown command/,
  );
});

test('window methods wrap their input', async () => {
  const { api, registered, calls } = setup();
  api.register(payload);
  await registered.get('visioncraft_camera_set').execute({ position: [1, 2, 3], target: [0, 0, 0] });
  assert.deepEqual(calls[0], {
    method: 'ui.set',
    params: { camera: { position: [1, 2, 3], target: [0, 0, 0] } },
  });
});

test('destructive tools and run_command ask first; a denial changes nothing', async () => {
  const { api, registered, calls } = setup();
  api.register(payload);
  const pending = registered.get('visioncraft_film_remove').execute({ id: 'f1' });
  const [[id, question]] = JSON.parse(api.takeConfirms());
  assert.match(question, /film\.remove/);
  assert.equal(JSON.parse(api.takeConfirms()).length, 0, 'asked once');
  api.answer(id, false);
  await assert.rejects(pending, /did not allow/);
  assert.equal(calls.length, 0);

  const generic = registered.get('visioncraft_run_command').execute({ command: 'shape.remove', params: { id: 'S1' } });
  const [[id2]] = JSON.parse(api.takeConfirms());
  api.answer(id2, true);
  await generic;
  assert.equal(calls.length, 1);
  assert.equal(calls[0].params.command, 'shape.remove');

  // Not destructive: no question.
  await registered.get('visioncraft_run_command').execute({ command: 'view.set', params: {} });
  assert.equal(JSON.parse(api.takeConfirms()).length, 0);
});

test('only one film render at a time', async () => {
  const { api, registered, release } = setup();
  api.register(payload);
  const first = registered.get('visioncraft_film_render').execute({ id: 'f1' });
  await new Promise((r) => setTimeout(r, 5));
  await assert.rejects(registered.get('visioncraft_film_render').execute({ id: 'f1' }), /already running/);
  release();
  await first;
  const again = registered.get('visioncraft_film_render').execute({ id: 'f1' });
  await new Promise((r) => setTimeout(r, 5));
  release();
  await again;
});

test('visioncraft_list picks one section of document.inspect', async () => {
  const { api, registered } = setup();
  api.register(payload);
  const out = await registered.get('visioncraft_list').execute({ what: 'plantings' });
  assert.deepEqual(JSON.parse(out.content[0].text), [{ id: 'g1' }]);
});

test('snapshot returns an image block, or a data URL as text', async () => {
  const { api, registered } = setup();
  api.register(payload);
  const img = await registered.get('visioncraft_snapshot').execute({});
  assert.equal(img.content[0].type, 'image');
  assert.equal(img.content[0].mimeType, 'image/jpeg');
  assert.equal(img.content[0].data, 'QUJD');
  assert.match(img.content[1].text, /1024x512/);
  const txt = await registered.get('visioncraft_snapshot').execute({ as_text: true, format: 'png' });
  assert.match(txt.content[0].text, /data:image\/png;base64,QUJD/);
});

test('tools refuse to run once switched off', async () => {
  const { api, registered } = setup();
  api.register(payload);
  const run = registered.get('visioncraft_view_set');
  api.unregister();
  await assert.rejects(run.execute({}), /switched off/);
});
