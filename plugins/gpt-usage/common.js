(() => {
  const defaults = {
    authPath: "~/.codex/auth.json",
    proxyUrl: "",
    displayMode: "remaining",
    windowMode: "both",
    refreshSeconds: 60,
  };
  const usageUrl = "https://chatgpt.com/backend-api/wham/usage";
  const numberValue = value => {
    const n = Number(value);
    return Number.isFinite(n) ? n : null;
  };
  const clampPercent = value => Number.isFinite(Number(value)) ? Math.max(0, Math.min(100, Number(value))) : null;

  function normalizeSettings(raw = {}) {
    return {
      ...defaults,
      ...raw,
      authPath: String(raw.authPath || defaults.authPath).trim() || defaults.authPath,
      proxyUrl: String(raw.proxyUrl || "").trim(),
      displayMode: raw.displayMode === "used" ? "used" : "remaining",
      windowMode: ["primary", "weekly", "both"].includes(raw.windowMode) ? raw.windowMode : "both",
      refreshSeconds: Number(raw.refreshSeconds) === 300 ? 300 : 60,
    };
  }

  function selectRateLimit(payload) {
    if (payload?.rate_limit && typeof payload.rate_limit === "object") return payload.rate_limit;
    const additional = payload?.additional_rate_limits;
    const items = Array.isArray(additional)
      ? additional
      : additional && typeof additional === "object"
        ? Object.entries(additional).map(([name, item]) => ({ name, item }))
        : [];
    for (const entry of items) {
      const item = entry?.item || entry;
      const name = String(item?.limit_name || item?.metered_feature || entry?.name || "").toLowerCase();
      if (name.includes("codex")) return item?.rate_limit || item;
    }
    return null;
  }

  function remainingPercent(raw) {
    const used = numberValue(raw);
    if (used == null) return null;
    const normalized = used <= 1 ? used * 100 : used;
    return Math.round(100 - Math.max(0, Math.min(100, normalized)));
  }

  function resetAt(windowInfo) {
    if (!windowInfo || typeof windowInfo !== "object") return null;
    for (const key of ["reset_at", "reset_at_epoch", "reset_at_timestamp"]) {
      const raw = numberValue(windowInfo[key]);
      if (raw != null && raw > 0) return Math.round(raw > 10_000_000_000 ? raw / 1000 : raw);
    }
    const after = numberValue(windowInfo.reset_after_seconds);
    return after != null && after >= 0 ? Math.round(Date.now() / 1000 + after) : null;
  }

  async function accessToken(path) {
    const payload = JSON.parse(await host.fs.readText(path));
    const raw = String(payload?.tokens?.access_token || "").trim();
    const token = /^bearer\s+/i.test(raw) ? raw.replace(/^bearer\s+/i, "").trim() : raw;
    if (!token) throw new Error("auth.json 中没有 access_token，请运行 codex login");
    return token;
  }

  async function fetchUsage(settings) {
    const config = normalizeSettings(settings);
    const token = await accessToken(config.authPath);
    const response = await host.http.request({
      url: usageUrl,
      method: "GET",
      proxyUrl: config.proxyUrl,
      headers: {
        Accept: "*/*",
        Authorization: `Bearer ${token}`,
        "Cache-Control": "no-cache",
        Pragma: "no-cache",
        Referer: "https://chatgpt.com/codex/cloud/settings/analytics",
        "oai-language": "en-US",
        "x-openai-target-path": "/backend-api/wham/usage",
        "x-openai-target-route": "/backend-api/wham/usage",
      },
    });
    const status = Number(response.status) || 0;
    if (status === 401 || status === 403) throw new Error(`HTTP ${status} · Codex CLI 登录令牌已失效，请运行 codex login`);
    if (!response.ok) throw new Error(`HTTP ${status} · Codex 用量接口请求失败`);
    const rateLimit = selectRateLimit(JSON.parse(response.body || "{}"));
    if (!rateLimit) throw new Error(`HTTP ${status} · Codex 用量响应中没有可识别的限额信息`);
    return {
      httpStatus: status,
      primary_remaining_percent: remainingPercent(rateLimit?.primary_window?.used_percent),
      weekly_remaining_percent: remainingPercent(rateLimit?.secondary_window?.used_percent),
      primary_reset_at: resetAt(rateLimit?.primary_window),
      weekly_reset_at: resetAt(rateLimit?.secondary_window),
    };
  }

  function shownPercent(remaining, mode) {
    const value = clampPercent(remaining);
    return value == null ? null : mode === "used" ? 100 - value : value;
  }

  function normalizeResetMs(value) {
    const n = Number(value);
    return Number.isFinite(n) && n > 0 ? (n < 10_000_000_000 ? n * 1000 : n) : null;
  }

  function formatPercent(value) {
    return value == null ? "--" : `${Math.round(value)}%`;
  }

  window.GptUsage = {
    defaults,
    normalizeSettings,
    clampPercent,
    shownPercent,
    normalizeResetMs,
    formatPercent,
    accessToken,
    fetchUsage,
  };
})();
