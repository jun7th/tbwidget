import type { PluginBundle } from "./types";

export type PluginBridgeSurface = "panel" | "popup" | "settings";

function escapeScript(source: string) {
  return source.replace(/<\/script/gi, "<\\/script");
}

/**
 * 生成插件 iframe 内的统一 Host Bridge。
 * panel / popup / settings 共用 request、storage、http、fs、wasm 等基础能力，
 * 仅在生命周期、尺寸测量和 settings handler 上按 surface 分支。
 */
export function buildPluginBridgeScript(bundle: PluginBundle, surface: PluginBridgeSurface) {
  const pluginId = JSON.stringify(bundle.id);
  const wasmBytes = JSON.stringify(bundle.wasmBytes ?? []);
  const surfaceName = JSON.stringify(surface);

  return `
(() => {
  const pluginId = ${pluginId};
  const surface = ${surfaceName};
  const wasmBytes = ${wasmBytes};
  let requestId = 0;
  let wasmExportsPromise;
  let submitHandler = null;
  let actionHandler = null;
  let sizeFrame = 0;
  let lastSize = "";
  const pending = new Map();

  function request(method, args = []) {
    return new Promise((resolve, reject) => {
      const id = ++requestId;
      pending.set(id, { resolve, reject });
      parent.postMessage({ tbwidgetPlugin: true, pluginId, requestId: id, method, args }, "*");
    });
  }

  async function wasmExports() {
    if (!wasmBytes.length) throw new Error("插件未提供 plugin.wasm");
    if (!wasmExportsPromise) {
      wasmExportsPromise = WebAssembly.instantiate(new Uint8Array(wasmBytes)).then(result => result.instance.exports);
    }
    return wasmExportsPromise;
  }

  function measureContent() {
    const body = document.body;
    const root = document.getElementById("plugin-root");
    const nodes = [];
    if (root) nodes.push(root);
    if (body) {
      for (const node of body.children) {
        if (["STYLE", "SCRIPT", "LINK"].includes(node.tagName) || node === root) continue;
        nodes.push(node);
      }
    }
    if (!nodes.length) nodes.push(body || document.documentElement);

    let width = 1;
    let height = 1;
    for (const node of nodes) {
      if (!node || !node.getBoundingClientRect) continue;
      const rect = node.getBoundingClientRect();
      if (rect.width <= 0 && rect.height <= 0) continue;
      width = Math.max(width, rect.right, rect.width, node.scrollWidth || 0);
      height = Math.max(height, rect.bottom, rect.height, node.scrollHeight || 0);
    }
    return { width: Math.ceil(width), height: Math.ceil(height) };
  }

  function reportSize(force = false) {
    if (surface === "settings") return;
    if (force) lastSize = "";
    if (sizeFrame) cancelAnimationFrame(sizeFrame);
    sizeFrame = requestAnimationFrame(() => {
      sizeFrame = 0;
      const measured = measureContent();
      const width = surface === "panel" ? Math.max(24, measured.width) : measured.width;
      const height = surface === "panel" ? Math.max(24, measured.height) : measured.height;
      const key = width + "x" + height;
      if (key === lastSize) return;
      lastSize = key;
      parent.postMessage({ tbwidgetPlugin: true, pluginId, method: "host.resize", width, height }, "*");
    });
  }

  window.addEventListener("message", async event => {
    const data = event.data || {};

    if (surface === "popup" && data.tbwidgetHostControl && data.pluginId === pluginId && data.action === "measure") {
      reportSize(true);
      return;
    }

    if (data.tbwidgetHost && data.pluginId === pluginId && data.requestId) {
      const item = pending.get(data.requestId);
      if (!item) return;
      pending.delete(data.requestId);
      if (data.ok) item.resolve(data.value);
      else item.reject(new Error(data.error || "宿主调用失败"));
      return;
    }

    if (surface !== "settings" || !data.tbwidgetSettingsInvoke || data.pluginId !== pluginId || !data.callId) return;
    try {
      let result = { ok: true };
      if (data.kind === "submit" && typeof submitHandler === "function") result = await submitHandler(data.values || {});
      if (data.kind === "action" && typeof actionHandler === "function") result = await actionHandler(data.action, data.values || {});
      parent.postMessage({ tbwidgetSettingsResult: true, pluginId, callId: data.callId, ok: true, value: result || { ok: true } }, "*");
    } catch (error) {
      parent.postMessage({ tbwidgetSettingsResult: true, pluginId, callId: data.callId, ok: false, error: String(error) }, "*");
    }
  });

  document.addEventListener("contextmenu", event => {
    event.preventDefault();
    event.stopPropagation();
  }, true);

  if (surface === "popup") {
    document.addEventListener("keydown", event => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      event.stopPropagation();
      void request("popup.hide");
    }, true);
    const selectionStyle = document.createElement("style");
    selectionStyle.textContent = '*:not(input):not(textarea):not([contenteditable="true"]) { -webkit-user-select: none !important; user-select: none !important; } input, textarea, [contenteditable="true"] { -webkit-user-select: text !important; user-select: text !important; }';
    (document.head || document.documentElement).appendChild(selectionStyle);
  }

  const removeHoverTips = root => {
    if (!root) return;
    if (root.nodeType === 1 && root.hasAttribute && root.hasAttribute("title")) root.removeAttribute("title");
    if (root.querySelectorAll) root.querySelectorAll("[title]").forEach(node => node.removeAttribute("title"));
  };

  if (surface === "panel") removeHoverTips(document);

  window.host = {
    enabled: true,
    get ui() { return window.kui || {}; },
    storage: {
      get: (key, fallback = null) => request("storage.get", [key, fallback]),
      set: (key, value) => request("storage.set", [key, value]),
      remove: key => request("storage.remove", [key]),
      clear: () => request("storage.clear")
    },
    http: { request: options => request("http.request", [options || {}]) },
    fs: {
      readText: path => request("fs.readText", [path]),
      readBytes: path => request("fs.readBytes", [path])
    },
    system: { metrics: () => request("system.metrics") },
    wasm: {
      exports: () => wasmExports(),
      instantiate: async (imports = {}) => {
        if (!wasmBytes.length) throw new Error("插件未提供 plugin.wasm");
        const result = await WebAssembly.instantiate(new Uint8Array(wasmBytes), imports || {});
        return result.instance.exports;
      },
      call: async (name, ...args) => {
        const exports = await wasmExports();
        const fn = exports[name];
        if (typeof fn !== "function") throw new Error("WASM 未导出函数：" + name);
        return fn(...args.map(Number));
      }
    },
    settings: surface === "settings" ? {
      define: definition => parent.postMessage({ tbwidgetPluginSettings: true, pluginId, definition }, "*"),
      onSubmit: callback => { submitHandler = callback; },
      onAction: callback => { actionHandler = callback; },
      notify: message => parent.postMessage({ tbwidgetPluginSettings: true, pluginId, notice: String(message || "") }, "*")
    } : {
      open: () => request("settings.open"),
      define: () => undefined,
      onSubmit: () => undefined,
      onAction: () => undefined,
      notify: () => undefined
    },
    popup: { hide: () => surface === "popup" ? request("popup.hide") : undefined },
    ready: () => surface === "settings" ? undefined : reportSize()
  };

  if (surface !== "settings") {
    const sizeObserver = new ResizeObserver(reportSize);
    sizeObserver.observe(document.documentElement);
    if (document.body) sizeObserver.observe(document.body);

    const observeContent = records => {
      if (surface === "panel") {
        for (const record of records) {
          if (record.type === "attributes") removeHoverTips(record.target);
          if (record.addedNodes) record.addedNodes.forEach(removeHoverTips);
        }
      }
      const root = document.getElementById("plugin-root");
      if (root) sizeObserver.observe(root);
      reportSize();
    };

    const mutationOptions = { subtree: true, childList: true };
    if (surface === "panel") {
      mutationOptions.attributes = true;
      mutationOptions.attributeFilter = ["title"];
    }
    new MutationObserver(observeContent).observe(document.documentElement, mutationOptions);
    window.addEventListener("load", () => reportSize());
    if (document.fonts && document.fonts.ready) document.fonts.ready.then(() => reportSize()).catch(() => undefined);
  }
})();`;
}

export function buildSettingsRunnerDocument(bundle: PluginBundle) {
  const bridge = escapeScript(buildPluginBridgeScript(bundle, "settings"));
  const pluginSource = escapeScript(bundle.settingsJs ?? "");
  return `<!doctype html><html><body><script>${bridge}<\/script><script>${pluginSource}<\/script></body></html>`;
}
