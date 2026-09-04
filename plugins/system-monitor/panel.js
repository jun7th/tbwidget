const DEFAULTS = {
  cpu: true,
  temperature: true,
  frequency: true,
  logical: true,
  memory: true,
  diskUsage: true,
  network: true,
  disk: true,
  fan: true,
  refreshMs: 1000
};
const TOGGLES = ["cpu", "temperature", "frequency", "logical", "memory", "diskUsage", "network", "disk", "fan"];
const $ = id => document.getElementById(id);

const groups = Object.fromEntries(TOGGLES.map(key => [key, $("m" + key[0].toUpperCase() + key.slice(1))]));
const number = value => Number.isFinite(Number(value)) ? Number(value) : null;
const pct = value => number(value) == null ? "N/A" : `${Math.round(number(value))}%`;
const temp = value => number(value) == null ? "N/A" : `${Math.round(number(value))} °C`;

function normalize(raw = {}) {
  const config = {
    ...DEFAULTS,
    refreshMs: [1000, 5000, 10000].includes(Number(raw.refreshMs)) ? Number(raw.refreshMs) : DEFAULTS.refreshMs
  };
  for (const key of TOGGLES) if (typeof raw[key] === "boolean") config[key] = raw[key];
  return config;
}

function frequency(value) {
  const n = number(value);
  if (n == null || n <= 0) return "N/A";
  return n >= 1000 ? `${(n / 1000).toFixed(2)} GHz` : `${Math.round(n)} MHz`;
}

function rate(value) {
  const n = Math.max(0, number(value) || 0);
  const units = ["B/s", "KB/s", "MB/s", "GB/s"];
  let index = 0;

  // 每档最多显示 999；达到 1000 时提前近似进到下一档，换算仍按 1024。
  while (index < units.length - 1 && n >= 1000 * (1024 ** index)) index++;

  const scaled = n / (1024 ** index);
  const amount = scaled >= 100 ? Math.floor(scaled)
    : scaled >= 10 ? Math.round(scaled)
    : scaled.toFixed(1).replace(/\.0$/, "");
  return `${amount} ${units[index]}`;
}

function render(snapshot, config) {
  for (const key of TOGGLES) groups[key].hidden = !config[key];

  const text = {
    cpu: pct(snapshot?.cpu?.usagePercent),
    temperature: temp(snapshot?.cpu?.temperatureC),
    frequency: frequency(snapshot?.cpu?.frequencyMhz),
    logical: number(snapshot?.cpu?.logicalProcessors) ?? "--",
    memory: pct(snapshot?.memory?.usagePercent),
    diskUsage: pct(snapshot?.disk?.usagePercent),
    down: rate(snapshot?.network?.downBytesPerSec),
    up: rate(snapshot?.network?.upBytesPerSec),
    read: rate(snapshot?.disk?.readBytesPerSec),
    write: rate(snapshot?.disk?.writeBytesPerSec),
    fan: (() => {
      const rpm = number(snapshot?.fans?.[0]?.rpm);
      return rpm == null ? "N/A" : `${Math.round(rpm)} RPM`;
    })()
  };

  for (const [id, value] of Object.entries(text)) $(id).textContent = value;
}

(async () => {
  let snapshot = null;
  let lastRefresh = 0;
  let busy = false;

  async function tick() {
    if (busy) return;
    busy = true;
    try {
      const config = normalize(await host.storage.get("settings", {}));
      if (!snapshot || Date.now() - lastRefresh >= config.refreshMs) {
        snapshot = await host.system.metrics();
        lastRefresh = Date.now();
      }
      render(snapshot, config);
    } catch (_) {
      // 保留最后一次成功数据。
    } finally {
      busy = false;
    }
  }

  await tick();
  setInterval(() => void tick(), 1000);
  host.ready();
})();
