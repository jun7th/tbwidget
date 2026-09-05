(() => {
  const defaults = {
    cpu: true,
    temperature: true,
    frequency: true,
    logical: true,
    memory: true,
    diskUsage: true,
    network: true,
    disk: true,
    fan: true,
    refreshMs: 1000,
  };
  const toggleKeys = ["cpu", "temperature", "frequency", "logical", "memory", "diskUsage", "network", "disk", "fan"];
  const numberValue = value => {
    const n = Number(value);
    return Number.isFinite(n) ? n : null;
  };

  function normalize(raw = {}) {
    const source = raw && typeof raw === "object" ? raw : {};
    const refreshMs = [1000, 5000, 10000].includes(Number(source.refreshMs)) ? Number(source.refreshMs) : defaults.refreshMs;
    const next = { ...defaults, refreshMs };
    for (const key of toggleKeys) if (typeof source[key] === "boolean") next[key] = source[key];
    return next;
  }

  function pct(value) {
    const n = numberValue(value);
    return n == null ? "N/A" : `${Math.round(n)}%`;
  }

  function temp(value) {
    const n = numberValue(value);
    return n == null ? "N/A" : `${Math.round(n)} °C`;
  }

  function frequency(value) {
    const n = numberValue(value);
    if (n == null || n <= 0) return "N/A";
    return n >= 1000 ? `${(n / 1000).toFixed(2)} GHz` : `${Math.round(n)} MHz`;
  }

  function size(value) {
    const n = Number(value) || 0;
    if (n >= 1024 ** 3) return `${(n / 1024 ** 3).toFixed(1)} GB`;
    if (n >= 1024 ** 2) return `${(n / 1024 ** 2).toFixed(1)} MB`;
    if (n >= 1024) return `${(n / 1024).toFixed(0)} KB`;
    return `${Math.round(n)} B`;
  }

  function rate(value) {
    const n = Math.max(0, Number(value) || 0);
    const units = ["B/s", "KB/s", "MB/s", "GB/s"];
    let index = 0;
    while (index < units.length - 1 && n >= 1000 * (1024 ** index)) index++;
    const scaled = n / (1024 ** index);
    const amount = scaled >= 100 ? Math.floor(scaled) : scaled >= 10 ? Math.round(scaled) : scaled.toFixed(1).replace(/\.0$/, "");
    return `${amount} ${units[index]}`;
  }

  window.SystemMonitorPlugin = { defaults, toggleKeys, numberValue, normalize, pct, temp, frequency, size, rate };
})();
