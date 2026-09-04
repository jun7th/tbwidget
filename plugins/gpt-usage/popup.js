const defaults = { authPath:"~/.codex/auth.json", proxyUrl:"", displayMode:"remaining", windowMode:"both", refreshSeconds:60 };
const USAGE_URL = "https://chatgpt.com/backend-api/wham/usage";
const clampPercent = value => Number.isFinite(value) ? Math.max(0, Math.min(100, Number(value))) : null;
const shownPercent = (remaining, mode) => { const value = clampPercent(remaining); return value == null ? null : mode === "used" ? 100 - value : value; };
const normalizeResetMs = value => { const n=Number(value); return Number.isFinite(n)&&n>0 ? (n<10_000_000_000?n*1000:n) : null; };
const numberValue = value => { const n=Number(value); return Number.isFinite(n) ? n : null; };
function selectRateLimit(payload) { if(payload?.rate_limit&&typeof payload.rate_limit==="object")return payload.rate_limit; const additional=payload?.additional_rate_limits; const items=Array.isArray(additional)?additional:additional&&typeof additional==="object"?Object.entries(additional).map(([name,item])=>({name,item})):[]; for(const entry of items){const item=entry?.item||entry;const name=String(item?.limit_name||item?.metered_feature||entry?.name||"").toLowerCase();if(name.includes("codex"))return item?.rate_limit||item;} return null; }
function remainingPercent(raw){const used=numberValue(raw);if(used==null)return null;const normalized=used<=1?used*100:used;return Math.round(100-Math.max(0,Math.min(100,normalized)));}
function resetAt(windowInfo){if(!windowInfo||typeof windowInfo!=="object")return null;for(const key of ["reset_at","reset_at_epoch","reset_at_timestamp"]){const raw=numberValue(windowInfo[key]);if(raw!=null&&raw>0)return Math.round(raw>10_000_000_000?raw/1000:raw);}const after=numberValue(windowInfo.reset_after_seconds);return after!=null&&after>=0?Math.round(Date.now()/1000+after):null;}
async function accessToken(path){const payload=JSON.parse(await host.fs.readText(path));const raw=String(payload?.tokens?.access_token||"").trim();const token=/^bearer\s+/i.test(raw)?raw.replace(/^bearer\s+/i,"").trim():raw;if(!token)throw new Error("auth.json 中没有 access_token，请运行 codex login");return token;}
async function fetchUsage(settings){const token=await accessToken(settings.authPath||defaults.authPath);const response=await host.http.request({url:USAGE_URL,method:"GET",proxyUrl:settings.proxyUrl||"",headers:{Accept:"*/*",Authorization:`Bearer ${token}`,"Cache-Control":"no-cache",Pragma:"no-cache",Referer:"https://chatgpt.com/codex/cloud/settings/analytics","oai-language":"en-US","x-openai-target-path":"/backend-api/wham/usage","x-openai-target-route":"/backend-api/wham/usage"}});if(response.status===401||response.status===403)throw new Error("Codex CLI 登录令牌已失效，请运行 codex login");if(!response.ok)throw new Error(`Codex 用量接口请求失败，HTTP ${response.status}`);const payload=JSON.parse(response.body||"{}");const rateLimit=selectRateLimit(payload);if(!rateLimit)throw new Error("Codex 用量响应中没有可识别的限额信息");return{primary_remaining_percent:remainingPercent(rateLimit?.primary_window?.used_percent),weekly_remaining_percent:remainingPercent(rateLimit?.secondary_window?.used_percent),primary_reset_at:resetAt(rateLimit?.primary_window),weekly_reset_at:resetAt(rateLimit?.secondary_window)};}
window.tbPlugin = {
  data() {
    return {
      settings: { ...defaults }, primaryPercent: null, weeklyPercent: null,
      primaryResetAt: null, weeklyResetAt: null, lastUsage: null, lastFetchedAt: null,
      meta: "", error: "", refreshing: false, timer: null, now: Date.now(),
      displayOptions: [{label:"余量",value:"remaining"},{label:"用量",value:"used"}],
      windowOptions: [{label:"5 小时",value:"primary"},{label:"1 周",value:"weekly"},{label:"两者",value:"both"}],
      refreshOptions: [{label:"1 分钟",value:"60"},{label:"5 分钟",value:"300"}]
    };
  },
  methods: {
    formatPercent(value) { return value == null ? "--" : `${Math.round(value)}%`; },
    timeText(value) { return new Date(value).toLocaleTimeString([], { hour:"2-digit", minute:"2-digit", second:"2-digit" }); },
    durationText(resetMs) {
      if (!resetMs) return "重置 --";
      let seconds=Math.max(0,Math.ceil((resetMs-this.now)/1000)); if(seconds<=0)return "即将重置";
      const days=Math.floor(seconds/86400);seconds%=86400;const hours=Math.floor(seconds/3600);seconds%=3600;const minutes=Math.floor(seconds/60);seconds%=60;
      const parts=[];if(days)parts.push(`${days}天`);if(hours||days)parts.push(`${hours}小时`);if(minutes||hours||days)parts.push(`${minutes}分`);if(!days)parts.push(`${seconds}秒`);return `重置 ${parts.join(" ")}`;
    },
    renderUsage(usage, fetchedAt) {
      this.lastUsage=usage||null; this.lastFetchedAt=fetchedAt||null;
      this.primaryPercent=shownPercent(usage?.primary_remaining_percent,this.settings.displayMode);
      this.weeklyPercent=shownPercent(usage?.weekly_remaining_percent,this.settings.displayMode);
      this.primaryResetAt=normalizeResetMs(usage?.primary_reset_at); this.weeklyResetAt=normalizeResetMs(usage?.weekly_reset_at);
      this.meta=fetchedAt?`更新时间：${this.timeText(fetchedAt)}`:"暂无缓存";
    },
    async savePopupSettings(patch) {
      const next={...this.settings,...patch};
      next.displayMode=next.displayMode==="used"?"used":"remaining";
      next.windowMode=["primary","weekly","both"].includes(next.windowMode)?next.windowMode:"both";
      next.refreshSeconds=Number(next.refreshSeconds)===300?300:60;
      this.settings=next;
      await host.storage.set("settings",next);
      if(this.lastUsage)this.renderUsage(this.lastUsage,this.lastFetchedAt);
    },
    resetUsage() { this.meta="重置用量属于插件业务，当前插件尚未实现。"; },
    async refresh() {
      this.refreshing=true; this.error="";
      try {
        const latest=await host.storage.get("settings",{}); this.settings={...defaults,...this.settings,...latest};
        const usage=await fetchUsage(this.settings); const fetchedAt=Date.now();
        await host.storage.set("usageCache",{usage,fetchedAt}); this.renderUsage(usage,fetchedAt);
      } catch(error) { this.error=String(error); }
      finally { this.refreshing=false; }
    }
  },
  async mounted() {
    const stored=await host.storage.get("settings",{});
    this.settings={...defaults,...stored,authPath:String(stored.authPath||defaults.authPath)};
    this.settings.displayMode=this.settings.displayMode==="used"?"used":"remaining";
    this.settings.windowMode=["primary","weekly","both"].includes(this.settings.windowMode)?this.settings.windowMode:"both";
    this.settings.refreshSeconds=Number(this.settings.refreshSeconds)===300?300:60;
    const cached=await host.storage.get("usageCache",null);
    if(cached?.usage)this.renderUsage(cached.usage,Number(cached.fetchedAt)||null); else this.meta="暂无缓存，可以手动刷新。";
    this.timer=setInterval(() => { this.now=Date.now(); },1000);
  },
  beforeUnmount() { if(this.timer)clearInterval(this.timer); }
};
