(() => {
  const pathParts = decodeURIComponent(location.pathname || "")
    .split("/")
    .filter(Boolean);
  const pluginId = pathParts[0] || "";
  const surface = new URLSearchParams(location.search).get("surface") || "panel";
  let requestId = 0;
  let sizeFrame = 0;
  let lastSize = "";
  const pending = new Map();
  const queuedRequests = [];
  let parentReady = false;
  let firstConfigReadLogged = false;

  function formatLogValue(value) {
    if (typeof value === "string") return value;
    if (value instanceof Error) {
      return `${value.name}: ${value.message}${value.stack ? ` | stack=${String(value.stack).replace(/\s+/g, " ")}` : ""}`;
    }
    try {
      const json = JSON.stringify(value);
      if (json !== undefined) return json;
    } catch {
      // 循环对象等无法 JSON 序列化时退回 String。
    }
    try { return String(value); } catch { return "<unprintable>"; }
  }

  function emitHostLog(level, args) {
    const message = (Array.isArray(args) ? args : [args]).map(formatLogValue).join(" ");
    parent.postMessage({
      tbwidgetPlugin: true,
      pluginId,
      method: "host.log",
      level,
      message,
    }, "*");
  }

  for (const consoleLevel of ["debug", "info", "log", "warn", "error"]) {
    const original = console[consoleLevel]?.bind(console);
    if (!original) continue;
    try {
      console[consoleLevel] = (...args) => {
        emitHostLog(consoleLevel === "log" ? "info" : consoleLevel, args);
        original(...args);
      };
    } catch {
      // 某些 WebView 可能不允许覆盖 console 方法，不影响插件运行。
    }
  }

  window.addEventListener("error", event => {
    emitHostLog("error", [`window.error message=${event.message} file=${event.filename}:${event.lineno}:${event.colno}`, event.error || ""]);
  });

  window.addEventListener("unhandledrejection", event => {
    emitHostLog("error", ["unhandledrejection", event.reason]);
  });

  emitHostLog("lifecycle", [`插件运行时开始 surface=${surface} href=${location.href}`]);

  function sendRuntimeReady() {
    parent.postMessage({
      tbwidgetPlugin: true,
      pluginId,
      method: "host.runtimeReady",
    }, "*");
  }

  function flushQueuedRequests() {
    if (!parentReady) return;
    while (queuedRequests.length) {
      const payload = queuedRequests.shift();
      try {
        parent.postMessage(payload, "*");
      } catch (error) {
        const item = pending.get(payload?.requestId);
        if (item) {
          pending.delete(payload.requestId);
          item.reject(error);
        }
      }
    }
  }

  // 插件 API 最终由 Rust/serde_json 接收，只支持 JSON 数据。
  // Vue reactive()/ref() 暴露的是 Proxy，Proxy 不能直接通过 postMessage 的 structured clone。
  // 在 iframe 边界先生成普通 JSON 快照，同时也避免插件随后修改同一对象影响已排队请求。
  function snapshotRequestArgs(args) {
    try {
      return JSON.parse(JSON.stringify(args));
    } catch (error) {
      const detail = error instanceof Error ? error.message : String(error);
      throw new TypeError(`宿主调用参数无法序列化: ${detail}`);
    }
  }

  function request(method, args = []) {
    return new Promise((resolve, reject) => {
      const id = ++requestId;
      let payload;
      try {
        payload = {
          tbwidgetPlugin: true,
          pluginId,
          requestId: id,
          method,
          args: snapshotRequestArgs(args),
        };
      } catch (error) {
        reject(error);
        return;
      }

      pending.set(id, { resolve, reject });
      if (!parentReady) {
        queuedRequests.push(payload);
        return;
      }

      try {
        parent.postMessage(payload, "*");
      } catch (error) {
        pending.delete(id);
        reject(error);
      }
    });
  }

  async function storageGet(key, fallback = null) {
    try {
      const value = await request("storage.get", [key, fallback]);
      if (!firstConfigReadLogged) {
        firstConfigReadLogged = true;
        emitHostLog("config", [`插件配置首次读取完成 key=${formatLogValue(key)}`]);
      }
      return value;
    } catch (error) {
      emitHostLog("error", [`插件配置读取失败 key=${formatLogValue(key)}`, error]);
      throw error;
    }
  }

  function visibleContentNodes() {
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
    return nodes;
  }

  function measureContent() {
    let width = 1;
    let height = 1;
    for (const node of visibleContentNodes()) {
      if (!node?.getBoundingClientRect) continue;
      const rect = node.getBoundingClientRect();
      if (rect.width <= 0 && rect.height <= 0) continue;
      width = Math.max(width, rect.right, rect.width, node.scrollWidth || 0);
      height = Math.max(height, rect.bottom, rect.height, node.scrollHeight || 0);
    }
    return { width: Math.ceil(width), height: Math.ceil(height) };
  }

  function reportSize(force = false) {
    if (force) lastSize = "";
    if (sizeFrame) cancelAnimationFrame(sizeFrame);
    sizeFrame = requestAnimationFrame(() => {
      sizeFrame = 0;
      const measured = measureContent();
      const width = surface === "panel" ? Math.max(24, measured.width) : measured.width;
      const height = surface === "panel" ? Math.max(24, measured.height) : measured.height;
      const key = `${width}x${height}`;
      if (key === lastSize) return;
      lastSize = key;
      parent.postMessage({ tbwidgetPlugin: true, pluginId, method: "host.resize", width, height }, "*");
    });
  }

  window.addEventListener("message", event => {
    const data = event.data || {};
    if (data.tbwidgetHostControl && data.pluginId === pluginId) {
      if (data.action === "runtimeAck") {
        parentReady = true;
        flushQueuedRequests();
        return;
      }
      if (data.action === "handshake") {
        sendRuntimeReady();
        return;
      }
      if (data.action === "measure") {
        reportSize(true);
        return;
      }
    }
    if (!data.tbwidgetHost || data.pluginId !== pluginId || !data.requestId) return;
    const item = pending.get(data.requestId);
    if (!item) return;
    pending.delete(data.requestId);
    if (data.ok) item.resolve(data.value);
    else item.reject(new Error(data.error || "宿主调用失败"));
  });

  document.addEventListener("contextmenu", event => {
    event.preventDefault();
    event.stopPropagation();
  }, true);

  if (surface === "popup") {
    const style = document.createElement("style");
    style.textContent = `
html, body { width:max-content !important; min-width:0 !important; height:max-content !important; min-height:0 !important; margin:0 !important; padding:0 !important; border:0 !important; overflow:hidden !important; }
body { display:inline-block !important; }
#plugin-root { display:inline-block !important; width:max-content !important; min-width:0 !important; height:max-content !important; margin:0 !important; padding:0 !important; border:0 !important; }
*:not(input):not(textarea):not([contenteditable="true"]) { -webkit-user-select:none !important; user-select:none !important; }
input, textarea, [contenteditable="true"] { -webkit-user-select:text !important; user-select:text !important; }`;
    (document.head || document.documentElement).appendChild(style);
    document.addEventListener("keydown", event => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      event.stopPropagation();
      void request("popup.hide");
    }, true);
  }

  const removeHoverTips = root => {
    if (!root) return;
    if (root.nodeType === 1 && root.hasAttribute?.("title")) root.removeAttribute("title");
    root.querySelectorAll?.("[title]").forEach(node => node.removeAttribute("title"));
  };
  if (surface === "panel") removeHoverTips(document);

  window.host = {
    enabled: true,
    get ui() { return window.kui || {}; },
    storage: {
      get: storageGet,
      set: (key, value) => request("storage.set", [key, value]),
      remove: key => request("storage.remove", [key]),
      clear: () => request("storage.clear"),
    },
    http: { request: options => request("http.request", [options || {}]) },
    fs: {
      readText: path => request("fs.readText", [path]),
      readBytes: path => request("fs.readBytes", [path]),
    },
    system: { metrics: () => request("system.metrics") },
    settings: {
      open: () => request("settings.open"),
      close: () => request("settings.close"),
    },
    popup: { hide: () => surface === "popup" ? request("popup.hide") : undefined },
    log: {
      debug: (...args) => emitHostLog("debug", args),
      info: (...args) => emitHostLog("info", args),
      warn: (...args) => emitHostLog("warn", args),
      error: (...args) => emitHostLog("error", args),
    },
    ready: () => reportSize(true),
  };

  sendRuntimeReady();

  function beginSizeTracking() {
    emitHostLog("lifecycle", ["插件 DOM/页面初始化完成"]);
    const sizeObserver = new ResizeObserver(() => reportSize());
    sizeObserver.observe(document.documentElement);
    if (document.body) sizeObserver.observe(document.body);
    const root = document.getElementById("plugin-root");
    if (root) sizeObserver.observe(root);

    const mutationOptions = { subtree: true, childList: true };
    if (surface === "panel") {
      mutationOptions.attributes = true;
      mutationOptions.attributeFilter = ["title"];
    }
    new MutationObserver(records => {
      if (surface === "panel") {
        for (const record of records) {
          if (record.type === "attributes") removeHoverTips(record.target);
          record.addedNodes?.forEach(removeHoverTips);
        }
      }
      const pluginRoot = document.getElementById("plugin-root");
      if (pluginRoot) sizeObserver.observe(pluginRoot);
      reportSize();
    }).observe(document.documentElement, mutationOptions);

    reportSize(true);
    document.fonts?.ready?.then(() => reportSize(true)).catch(() => undefined);
  }

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", beginSizeTracking, { once: true });
  } else {
    beginSizeTracking();
  }
})();
