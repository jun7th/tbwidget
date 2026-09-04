const DEFAULTS = {
  authPath: "~/.codex/auth.json",
  proxyUrl: "",
  displayMode: "remaining",
  windowMode: "both",
  refreshSeconds: 60
};
const USAGE_URL = "https://chatgpt.com/backend-api/wham/usage";
const $ = id => document.getElementById(id);
const panel = $("panel");
const output = { primary: $("primaryValue"), weekly: $("weeklyValue") };

const number = value => Number.isFinite(Number(value)) ? Number(value) : null;
const clamp = value => Math.max(0, Math.min(100, value));
const format = value => value == null ? "--" : `${Math.round(value)}%`;

function normalize(raw = {}) {
  return {
    ...DEFAULTS,
    ...raw,
    authPath: String(raw.authPath || DEFAULTS.authPath).trim() || DEFAULTS.authPath,
    proxyUrl: String(raw.proxyUrl || "").trim(),
    displayMode: raw.displayMode === "used" ? "used" : "remaining",
    windowMode: ["primary", "weekly", "both"].includes(raw.windowMode) ? raw.windowMode : "both",
    refreshSeconds: Number(raw.refreshSeconds) === 300 ? 300 : 60
  };
}

function findRateLimit(payload) {
  if (payload?.rate_limit) return payload.rate_limit;
  const extra = payload?.additional_rate_limits;
  const items = Array.isArray(extra)
    ? extra
    : Object.entries(extra || {}).map(([name, item]) => ({ name, item }));

  for (const entry of items) {
    const item = entry?.item || entry;
    const name = String(item?.limit_name || item?.metered_feature || entry?.name || "").toLowerCase();
    if (name.includes("codex")) return item?.rate_limit || item;
  }
  return null;
}

function remaining(usedPercent) {
  const used = number(usedPercent);
  return used == null ? null : Math.round(100 - clamp(used <= 1 ? used * 100 : used));
}

function resetAt(info) {
  for (const key of ["reset_at", "reset_at_epoch", "reset_at_timestamp"]) {
    const value = number(info?.[key]);
    if (value != null && value > 0) return Math.round(value > 10_000_000_000 ? value / 1000 : value);
  }
  const after = number(info?.reset_after_seconds);
  return after != null && after >= 0 ? Math.round(Date.now() / 1000 + after) : null;
}

async function fetchUsage(config) {
  const auth = JSON.parse(await host.fs.readText(config.authPath));
  const token = String(auth?.tokens?.access_token || "").replace(/^bearer\s+/i, "").trim();
  if (!token) throw new Error("auth.json 中没有 access_token，请运行 codex login");

  const response = await host.http.request({
    url: USAGE_URL,
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
      "x-openai-target-route": "/backend-api/wham/usage"
    }
  });

  if ([401, 403].includes(response.status)) throw new Error("Codex CLI 登录令牌已失效，请运行 codex login");
  if (!response.ok) throw new Error(`Codex 用量接口请求失败，HTTP ${response.status}`);

  const limit = findRateLimit(JSON.parse(response.body || "{}"));
  if (!limit) throw new Error("Codex 用量响应中没有可识别的限额信息");

  return {
    primary_remaining_percent: remaining(limit.primary_window?.used_percent),
    weekly_remaining_percent: remaining(limit.secondary_window?.used_percent),
    primary_reset_at: resetAt(limit.primary_window),
    weekly_reset_at: resetAt(limit.secondary_window)
  };
}

function render(usage, config) {
  const shown = value => value == null
    ? null
    : config.displayMode === "used" ? 100 - clamp(value) : clamp(value);

  panel.dataset.window = config.windowMode;
  panel.dataset.mode = config.displayMode;
  output.primary.textContent = format(shown(usage?.primary_remaining_percent));
  output.weekly.textContent = format(shown(usage?.weekly_remaining_percent));
}

(async () => {
  let usage = null;
  let fetchedAt = 0;
  let busy = false;
  let requestConfigKey = "";

  const cached = await host.storage.get("usageCache", null);
  if (cached?.usage) {
    usage = cached.usage;
    fetchedAt = Number(cached.fetchedAt) || 0;
  }

  async function tick() {
    const config = normalize(await host.storage.get("settings", {}));
    render(usage, config);

    const configKey = `${config.authPath}\n${config.proxyUrl}`;
    const sourceChanged = requestConfigKey && requestConfigKey !== configKey;
    requestConfigKey = configKey;

    const expired = !usage || sourceChanged || Date.now() - fetchedAt >= config.refreshSeconds * 1000;
    if (busy || !expired) return;

    busy = true;
    try {
      usage = await fetchUsage(config);
      fetchedAt = Date.now();
      await host.storage.set("usageCache", { usage, fetchedAt });
      render(usage, normalize(await host.storage.get("settings", {})));
    } catch (_) {
      // 请求失败时保留最后一次成功数据。
    } finally {
      busy = false;
    }
  }

  await tick();
  setInterval(() => void tick(), 1000);
  host.ready();
})();
