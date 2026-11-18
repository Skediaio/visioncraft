// WebMCP bridge (experimental): registers VisionCraft's curated tools with the browser's own AI
// agent through `document.modelContext` (Chrome's WebMCP; older builds `navigator.modelContext`).
// The Rust side (apps/visioncraft/src/webmcp.rs) builds the tool list from the command specs and
// hands it to `vcWebMcp.register(json)`; every tool runs through the page's `vcRpc`, so the app's
// own Session answers it: one undo step per change, shown as "agent: <command>".
//
// Kept apart from the app so it can be tested with mocks (webmcp.test.mjs): `createWebMcp(env)`
// takes the page's pieces as `env` ({document, navigator, rpc, wake, canvas, raf, createCanvas}).
(function (root) {
  "use strict";

  const CONFIRM_TIMEOUT_MS = 120000;
  const MAX_TEXT = 200000;

  function createWebMcp(env) {
    const context = () =>
      (env.document && env.document.modelContext) ||
      (env.navigator && env.navigator.modelContext) ||
      null;

    let enabled = false;
    let controller = null;
    let names = [];
    let destructive = new Set();
    let lastError = null;
    let filmRenderBusy = false;
    let nextConfirm = 1;
    const confirms = new Map(); // id -> {text, shown, resolve, timer}

    // ---- results

    function text(value) {
      let s = typeof value === "string" ? value : JSON.stringify(value === undefined ? null : value);
      if (s.length > MAX_TEXT) s = s.slice(0, MAX_TEXT) + "\n[truncated: ask for less at a time]";
      return { content: [{ type: "text", text: s }] };
    }

    async function call(method, params) {
      const reply = await env.rpc(method, params);
      if (!reply || !reply.ok) throw new Error((reply && reply.error) || method + " failed");
      return reply.result;
    }

    // ---- confirmation (the app shows the question in egui; vcWebMcp.answer replies)

    function confirm(question) {
      return new Promise((resolve) => {
        const id = nextConfirm++;
        const timer = setTimeout(() => settle(id, false), CONFIRM_TIMEOUT_MS);
        confirms.set(id, { text: question, shown: false, resolve, timer });
        if (env.wake) env.wake();
      });
    }

    function settle(id, allow) {
      const c = confirms.get(id);
      if (!c) return;
      confirms.delete(id);
      clearTimeout(c.timer);
      c.resolve(!!allow);
    }

    function describeCall(id, params) {
      let p = "";
      try { p = JSON.stringify(params || {}); } catch (e) { p = ""; }
      if (p.length > 240) p = p.slice(0, 240) + "...";
      return "The browser AI agent wants to run " + id + " " + p + ". Allow? (Ctrl+Z undoes it afterwards.)";
    }

    // ---- running a command

    async function runCommand(id, params, intent) {
      if (typeof id !== "string" || !id) throw new Error("`command` must be a command id");
      if (destructive.has(id)) {
        if (!(await confirm(describeCall(id, params)))) {
          throw new Error("The person did not allow " + id + ". Nothing changed; ask them, or do it another way.");
        }
      }
      if (id === "film.render") {
        if (filmRenderBusy) throw new Error("A film render is already running; wait for it to finish.");
        filmRenderBusy = true;
      }
      try {
        const req = { command: id, params: params || {} };
        if (intent) req.intent = String(intent).slice(0, 300);
        return await call("engine.execute", req);
      } finally {
        if (id === "film.render") filmRenderBusy = false;
      }
    }

    // ---- snapshot

    function nextFrame() {
      return new Promise((resolve) => {
        if (env.wake) env.wake();
        env.raf(() => resolve());
      });
    }

    function drawScaled(maxWidth) {
      const src = env.canvas();
      if (!src || !src.width || !src.height) throw new Error("the 3D view has no size yet");
      const scale = Math.min(1, maxWidth / src.width);
      const w = Math.max(1, Math.round(src.width * scale));
      const h = Math.max(1, Math.round(src.height * scale));
      const out = env.createCanvas(w, h);
      const g = out.getContext("2d");
      g.drawImage(src, 0, 0, w, h);
      return { out, g, w, h };
    }

    function blank(g, w, h) {
      // Pixels read back as transparent when the GPU canvas was already presented.
      const probe = g.getImageData(0, 0, w, h).data;
      for (let i = 3; i < probe.length; i += 4 * 97) if (probe[i] !== 0) return false;
      return true;
    }

    async function snapshot(input) {
      const maxWidth = Math.min(2048, Math.max(64, Math.round(Number(input.max_width) || 1024)));
      const png = input.format === "png";
      // The canvas can only be read in the frame that drew it: ask for a frame, read after it.
      let shot = null;
      for (let attempt = 0; attempt < 3 && !shot; attempt++) {
        await nextFrame();
        const s = drawScaled(maxWidth);
        if (!blank(s.g, s.w, s.h)) shot = s;
      }
      if (!shot) throw new Error("the 3D view could not be read back; try again");
      const mime = png ? "image/png" : "image/jpeg";
      const url = shot.out.toDataURL(mime, 0.85);
      const note = "VisionCraft view, " + shot.w + "x" + shot.h + " " + (png ? "PNG" : "JPEG");
      if (input.as_text) return text(note + "\n" + url);
      return {
        content: [
          { type: "image", data: url.slice(url.indexOf(",") + 1), mimeType: mime },
          { type: "text", text: note },
        ],
      };
    }

    // ---- tools

    async function execute(tool, input, extra) {
      if (!enabled) throw new Error("The WebMCP tools are switched off in VisionCraft");
      if (extra && extra.signal && extra.signal.aborted) throw new Error("cancelled");
      input = input && typeof input === "object" ? input : {};
      const route = tool.route || {};
      const intent = "WebMCP: " + tool.name;
      switch (route.kind) {
        case "command":
          return text(await runCommand(route.id, input, intent));
        case "method": {
          const params = route.wrap ? { [route.wrap]: input } : input;
          const reply = await call(route.name, params);
          return text(route.pick && reply ? reply[route.pick] : reply);
        }
        case "section": {
          const key = route.sections && route.sections[input.what];
          if (!key) throw new Error("`what` must be one of " + Object.keys(route.sections || {}).join(", "));
          const all = await call("engine.execute", {
            command: "document.inspect",
            params: { faces: false, measures: input.what === "plantings" || input.what === "shapes" },
          });
          return text(all[key]);
        }
        case "run_command":
          return text(await runCommand(input.command, input.params, input.intent || intent));
        case "list_commands": {
          const p = Object.assign({ brief: true }, input);
          return text(await call("engine.commands", p));
        }
        case "snapshot":
          return snapshot(input);
        default:
          throw new Error("unknown tool route");
      }
    }

    function register(json) {
      unregister();
      const payload = typeof json === "string" ? JSON.parse(json) : json;
      const ctx = context();
      if (!ctx) { lastError = "no modelContext"; return 0; }
      destructive = new Set(payload.destructiveCommands || []);
      enabled = true;
      controller = typeof AbortController !== "undefined" ? new AbortController() : null;
      for (const tool of payload.tools) {
        const def = {
          name: tool.name,
          title: tool.title,
          description: tool.description,
          inputSchema: tool.inputSchema,
          annotations: tool.annotations,
          execute: (input, extra) => execute(tool, input, extra),
        };
        try {
          const r = ctx.registerTool(def, controller ? { signal: controller.signal } : undefined);
          if (r && typeof r.catch === "function") r.catch((e) => { lastError = String(e); });
          names.push(tool.name);
        } catch (e) {
          lastError = String(e);
        }
      }
      return names.length;
    }

    function unregister() {
      enabled = false;
      const ctx = context();
      if (controller) controller.abort();
      else if (ctx && typeof ctx.unregisterTool === "function") {
        for (const n of names) { try { ctx.unregisterTool(n); } catch (e) { /* already gone */ } }
      }
      controller = null;
      names = [];
      for (const id of [...confirms.keys()]) settle(id, false);
    }

    return {
      supported: () => !!context(),
      register,
      unregister,
      status: () => ({ supported: !!context(), enabled, registered: names.length, error: lastError }),
      // Questions waiting for the person, once each, as JSON [[id, text], ...].
      takeConfirms() {
        const out = [];
        for (const [id, c] of confirms) if (!c.shown) { c.shown = true; out.push([id, c.text]); }
        return JSON.stringify(out);
      },
      answer: settle,
    };
  }

  if (typeof module !== "undefined" && module.exports) module.exports = { createWebMcp };
  if (root.document && root.requestAnimationFrame) {
    root.vcWebMcp = createWebMcp({
      document: root.document,
      navigator: root.navigator,
      rpc: (method, params) => root.vcRpc(method, params),
      wake: () => { if (root.__vcRpcWake) root.__vcRpcWake(); },
      canvas: () => root.document.getElementById("visioncraft_canvas"),
      raf: (f) => root.requestAnimationFrame(f),
      createCanvas: (w, h) => { const c = root.document.createElement("canvas"); c.width = w; c.height = h; return c; },
    });
  }
})(typeof window !== "undefined" ? window : globalThis);
