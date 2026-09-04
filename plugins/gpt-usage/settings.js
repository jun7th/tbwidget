const defaults = { authPath: "~/.codex/auth.json", proxyUrl: "", displayMode: "remaining", windowMode: "both", refreshSeconds: 60 };
const USAGE_URL = "https://chatgpt.com/backend-api/wham/usage";
const clampPercent = v => Number.isFinite(v) ? Math.max(0, Math.min(100, Number(v))) : null;
const formatPercent = v => v == null ? "--" : `${Math.round(v)}%`;
const numberValue = value => { const n = Number(value); return Number.isFinite(n) ? n : null; };
function selectRateLimit(payload){if(payload?.rate_limit&&typeof payload.rate_limit==="object")return payload.rate_limit;const additional=payload?.additional_rate_limits;const items=Array.isArray(additional)?additional:additional&&typeof additional==="object"?Object.entries(additional).map(([name,item])=>({name,item})):[];for(const entry of items){const item=entry?.item||entry;const name=String(item?.limit_name||item?.metered_feature||entry?.name||"").toLowerCase();if(name.includes("codex"))return item?.rate_limit||item;}return null;}
function remainingPercent(raw){const used=numberValue(raw);if(used==null)return null;const normalized=used<=1?used*100:used;return Math.round(100-Math.max(0,Math.min(100,normalized)));}
async function accessToken(path){const payload=JSON.parse(await host.fs.readText(path));const raw=String(payload?.tokens?.access_token||"").trim();const token=/^bearer\s+/i.test(raw)?raw.replace(/^bearer\s+/i,"").trim():raw;if(!token)throw new Error("auth.json 中没有 access_token，请运行 codex login");return token;}
async function fetchUsage(settings){const token=await accessToken(settings.authPath||defaults.authPath);const response=await host.http.request({url:USAGE_URL,method:"GET",proxyUrl:settings.proxyUrl||"",headers:{Accept:"*/*",Authorization:`Bearer ${token}`,"Cache-Control":"no-cache",Pragma:"no-cache",Referer:"https://chatgpt.com/codex/cloud/settings/analytics","oai-language":"en-US","x-openai-target-path":"/backend-api/wham/usage","x-openai-target-route":"/backend-api/wham/usage"}});const status=Number(response.status)||0;if(status===401||status===403)throw new Error(`HTTP ${status} · Codex CLI 登录令牌已失效，请运行 codex login`);if(!response.ok)throw new Error(`HTTP ${status} · Codex 用量接口请求失败`);const payload=JSON.parse(response.body||"{}");const rateLimit=selectRateLimit(payload);if(!rateLimit)throw new Error(`HTTP ${status} · Codex 用量响应中没有可识别的限额信息`);return{httpStatus:status,primary_remaining_percent:remainingPercent(rateLimit?.primary_window?.used_percent),weekly_remaining_percent:remainingPercent(rateLimit?.secondary_window?.used_percent)};}

(async () => {
  const stored = await host.storage.get("settings", {});
  const values = {
    authPath: String(stored.authPath || defaults.authPath),
    proxyUrl: String(stored.proxyUrl || ""),
    displayMode: stored.displayMode === "used" ? "used" : "remaining",
    windowMode: ["primary", "weekly", "both"].includes(stored.windowMode) ? stored.windowMode : "both",
    refreshSeconds: Number(stored.refreshSeconds) === 300 ? 300 : 60
  };
  host.settings.define({
    title: "GPT用量 设置",
    fields: [
      { key: "authPath", label: "auth.json 路径", type: "text", placeholder: "~/.codex/auth.json", help: "路径由插件定义；~ 会由通用文件接口展开为当前用户目录。" },
      { key: "proxyUrl", label: "代理", type: "text", placeholder: "例如 http://127.0.0.1:7890" },
      { key: "displayMode", label: "数值显示", type: "radio", options: [
        { label: "余量", value: "remaining" }, { label: "用量", value: "used" }
      ] },
      { key: "windowMode", label: "任务栏限额", type: "radio", options: [
        { label: "5 小时", value: "primary" }, { label: "1 周", value: "weekly" }, { label: "两者", value: "both" }
      ] },
      { key: "refreshSeconds", label: "刷新时间", type: "radio", options: [
        { label: "1 分钟", value: 60 }, { label: "5 分钟", value: 300 }
      ] }
    ],
    values,
    actions: [
      { id: "defaultPath", label: "使用默认路径" },
      { id: "test", label: "测试接口", kind: "primary" }
    ]
  });

  host.settings.onSubmit(async next => {
    const current = await host.storage.get("settings", {});
    const authPath = String(next.authPath || defaults.authPath).trim() || defaults.authPath;
    try { await accessToken(authPath); }
    catch (error) { return { ok: false, type: "error", message: `auth.json 不可用：${String(error)}` }; }
    const saved = {
      ...defaults,
      ...current,
      authPath,
      proxyUrl: String(next.proxyUrl || "").trim(),
      displayMode: next.displayMode === "used" ? "used" : "remaining",
      windowMode: ["primary", "weekly", "both"].includes(next.windowMode) ? next.windowMode : "both",
      refreshSeconds: Number(next.refreshSeconds) === 300 ? 300 : 60
    };
    await host.storage.set("settings", saved);
    return { ok: true, type: "success", message: `设置已保存：${authPath}`, values: saved };
  });

  host.settings.onAction(async (action, current) => {
    if (action === "defaultPath") {
      return { ok: true, type: "success", message: `已切换为默认路径：${defaults.authPath}`, values: { ...current, authPath: defaults.authPath } };
    }
    if (action !== "test") return { ok: true };
    const settings = { ...defaults, ...current, authPath: String(current.authPath || defaults.authPath).trim() || defaults.authPath, proxyUrl: String(current.proxyUrl || "").trim() };
    try {
      const usage = await fetchUsage(settings);
      return { ok: true, type: "success", message: `HTTP ${usage.httpStatus} · 接口正常：5 小时余量 ${formatPercent(clampPercent(usage.primary_remaining_percent))}，1 周余量 ${formatPercent(clampPercent(usage.weekly_remaining_percent))}` };
    } catch (error) {
      const text = String(error);
      return { ok: false, type: "error", message: /HTTP\s+\d+/i.test(text) ? `接口测试失败：${text}` : `HTTP -- · 接口测试失败：${text}` };
    }
  });
  host.ready();
})();
