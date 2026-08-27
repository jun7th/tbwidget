<script setup lang="ts">
import "@fontsource/inter/400.css";
import "@fontsource/inter/500.css";
import "@fontsource/inter/600.css";
import "@fontsource/inter/700.css";
import "@fontsource/manrope/600.css";
import "@fontsource/manrope/700.css";
import "@fontsource/jetbrains-mono/400.css";
import "@fontsource/jetbrains-mono/500.css";

import { invoke } from "@tauri-apps/api/core";
import { listen, type Event, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { computed, nextTick, onMounted, onUnmounted, ref } from "vue";

type WidgetPosition = "left" | "right";
type GptDisplayMode = "remaining" | "used";
type TaskbarTransparencyMode = "off" | "clear";
type AppLanguage = "en" | "zh";
type HistoryMetric = "cpu" | "memory" | "temperature" | "disk" | "network";

type WidgetConfig = {
  position: WidgetPosition;
  cpu_visible: boolean;
  memory_visible: boolean;
  gpt_visible: boolean;
  temperature_visible: boolean;
  disk_visible: boolean;
  upload_visible: boolean;
  download_visible: boolean;
  gpt_refresh_seconds: number;
  gpt_display_mode: GptDisplayMode;
  gpt_proxy_url: string;
  taskbar_transparency_mode: TaskbarTransparencyMode;
  language: AppLanguage;
};

type SystemUsage = {
  cpu_percent: number;
  memory_percent: number;
  temperature_celsius: number | null;
  disk_percent: number | null;
  upload_bytes_per_second: number;
  download_bytes_per_second: number;
};

type CodexUsage = {
  primary_remaining_percent: number | null;
  weekly_remaining_percent: number | null;
};

type HistoryPoint = {
  timestamp: number;
  value: number;
  secondary_value: number | null;
};

type TaskbarRate = {
  amount: string;
  unit: string;
};

type MetricVisibilityKey = keyof Pick<
  WidgetConfig,
  "cpu_visible" | "memory_visible" | "temperature_visible" | "disk_visible" | "gpt_visible"
>;

type TimerName = "system" | "codex" | "history" | "alert" | "clock";

const SYSTEM_REFRESH_MS = 1_500;
const HISTORY_REFRESH_MS = 5_000;
const ALERT_REFRESH_MS = 1_000;
const CLOCK_REFRESH_MS = 30_000;
const SPARKLINE_WIDTH = 250;
const SPARKLINE_HEIGHT = 42;

const HISTORY_METRICS = ["cpu", "memory", "temperature", "disk", "network"] as const satisfies readonly HistoryMetric[];
const VISIBILITY_KEYS = [
  "cpu_visible",
  "memory_visible",
  "temperature_visible",
  "disk_visible",
  "upload_visible",
  "download_visible",
  "gpt_visible",
] as const satisfies readonly (keyof WidgetConfig)[];
const CODEX_TIMER_KEYS = ["gpt_visible", "gpt_refresh_seconds"] as const satisfies readonly (keyof WidgetConfig)[];
const WIDGET_LAYOUT_KEYS = [
  "position",
  ...VISIBILITY_KEYS,
  "gpt_display_mode",
] as const satisfies readonly (keyof WidgetConfig)[];
const SETTINGS_SIZE_KEYS = [...VISIBILITY_KEYS, "language"] as const satisfies readonly (keyof WidgetConfig)[];

const isSettingsWindow = new URLSearchParams(window.location.search).has("settings");

const defaultConfig: WidgetConfig = {
  position: "left",
  cpu_visible: true,
  memory_visible: true,
  gpt_visible: true,
  temperature_visible: true,
  disk_visible: true,
  upload_visible: true,
  download_visible: true,
  gpt_refresh_seconds: 60,
  gpt_display_mode: "remaining",
  gpt_proxy_url: "",
  taskbar_transparency_mode: "off",
  language: "en",
};

const text = {
  en: {
    openLog: "Open log file",
    logFile: "Log File",
    quitApp: "Quit application",
    quit: "Quit",
    switchLanguage: "Switch language",
    settingsLabel: "Taskbar widget settings",
    layoutSettings: "Widget layout settings",
    position: "POS",
    left: "Left",
    right: "Right",
    taskbar: "TASKBAR",
    transparent: "TRANSPARENT",
    enableTaskbarTransparent: "Enable taskbar transparency",
    metricsSettings: "Metric display settings",
    gptUsed: "USED",
    gptRemaining: "REMAINING",
    gptRefreshFailed: "Codex refresh failed, showing last successful data",
    refreshEvery: "refreshes every",
    seconds: "seconds",
    traySettingsHint: "Open settings from tray",
    switchGptMode: "Switch GPT display mode, current",
    clickToggleValue: "Click to toggle value",
    refreshGptUsage: "Refresh GPT usage",
    refreshGpt: "Refresh",
    widgetLabel: "System resources and GPT usage",
    taskManagerTitle: "Open Task Manager; set the default start page to Performance in Task Manager settings",
    cpuLabel: "CPU",
    memoryLabel: "MEM",
    temperatureLabel: "TMP",
    diskLabel: "DISK",
    networkLabel: "NET",
    gptLabel: "GPT",
    cpuHistoryLabel: "CPU history chart",
    memoryHistoryLabel: "Memory history chart",
    temperatureHistoryLabel: "Temperature history chart",
    networkHistoryLabel: "Network upload and download history chart",
    diskUsageLabel: "Disk usage",
    taskbarNetworkLabel: "NET",
    gptPrimaryLabel: "5 hour",
    gptWeeklyLabel: "1 week",
    gptPrimaryProgressLabel: "GPT 5-hour remaining progress",
    gptWeeklyProgressLabel: "GPT 1-week remaining progress",
    notUpdated: "Not updated",
    underOneMinute: "under 1 minute ago",
    oneMinuteAgo: "1 minute ago",
    minutesAgo: "minutes ago",
    hoursAgo: "hours ago",
  },
  zh: {
    openLog: "打开日志文件",
    logFile: "日志",
    quitApp: "退出程序",
    quit: "退出",
    switchLanguage: "切换语言",
    settingsLabel: "任务栏组件设置",
    layoutSettings: "组件布局设置",
    position: "位置",
    left: "左",
    right: "右",
    taskbar: "任务栏",
    transparent: "透明",
    enableTaskbarTransparent: "启用任务栏透明",
    metricsSettings: "指标显示设置",
    gptUsed: "已用量",
    gptRemaining: "剩余额度",
    gptRefreshFailed: "Codex 刷新失败，当前显示上次成功数据",
    refreshEvery: "每",
    seconds: "秒刷新一次",
    traySettingsHint: "托盘打开设置",
    switchGptMode: "切换 GPT 显示模式，当前为",
    clickToggleValue: "点击切换数值显示",
    refreshGptUsage: "手动刷新 GPT 用量",
    refreshGpt: "手动刷新",
    widgetLabel: "系统资源与 GPT 用量",
    taskManagerTitle: "打开任务管理器；可在任务管理器设置中将默认启动页设为性能",
    cpuLabel: "CPU",
    memoryLabel: "内存",
    temperatureLabel: "温度",
    diskLabel: "磁盘",
    networkLabel: "网络",
    gptLabel: "GPT",
    cpuHistoryLabel: "CPU 历史曲线",
    memoryHistoryLabel: "内存历史曲线",
    temperatureHistoryLabel: "温度历史曲线",
    networkHistoryLabel: "网络上传和下载历史曲线",
    diskUsageLabel: "磁盘使用率",
    taskbarNetworkLabel: "NET",
    gptPrimaryLabel: "5 小时",
    gptWeeklyLabel: "1 星期",
    gptPrimaryProgressLabel: "GPT 5小时剩余额度进度",
    gptWeeklyProgressLabel: "GPT 1周剩余额度进度",
    notUpdated: "未更新",
    underOneMinute: "1分钟以内",
    oneMinuteAgo: "1分钟前",
    minutesAgo: "分钟前",
    hoursAgo: "小时前",
  },
} as const;

// ---------- state ----------

const config = ref<WidgetConfig>({ ...defaultConfig });
const cpuPercent = ref<number | null>(null);
const memoryPercent = ref<number | null>(null);
const temperatureCelsius = ref<number | null>(null);
const diskPercent = ref<number | null>(null);
const uploadBytesPerSecond = ref(0);
const downloadBytesPerSecond = ref(0);

const codexUsage = ref<CodexUsage | null>(null);
const codexError = ref("");
const codexStale = ref(false);
const codexRefreshing = ref(false);
const codexLastUpdatedAt = ref<number | null>(null);
const clockTick = ref(0);

const gptResetAlertPending = ref(false);
const trayFlashVisible = ref(true);
const languageMenuOpen = ref(false);
const widgetElement = ref<HTMLElement | null>(null);
const settingsElement = ref<HTMLElement | null>(null);

const historyPoints = ref<Record<HistoryMetric, HistoryPoint[]>>({
  cpu: [],
  memory: [],
  temperature: [],
  disk: [],
  network: [],
});

const timers: Partial<Record<TimerName, number>> = {};
const unlisteners: UnlistenFn[] = [];
let resizeObserver: ResizeObserver | undefined;
let lastReportedWidth = 0;
let lastSettingsLayoutKey = "";

// ---------- computed ----------

const cpuText = computed(() => formatPercent(cpuPercent.value));
const memoryText = computed(() => formatPercent(memoryPercent.value));
const temperatureText = computed(() => formatTemperature(temperatureCelsius.value));
const diskText = computed(() => formatPercent(diskPercent.value));
const uploadText = computed(() => formatRate(uploadBytesPerSecond.value));
const downloadText = computed(() => formatRate(downloadBytesPerSecond.value));
const taskbarUploadRate = computed(() => formatTaskbarRate(uploadBytesPerSecond.value));
const taskbarDownloadRate = computed(() => formatTaskbarRate(downloadBytesPerSecond.value));

const networkVisible = computed(() => config.value.upload_visible || config.value.download_visible);
const allMetricsHidden = computed(() => VISIBILITY_KEYS.every((key) => !config.value[key]));
const uiText = computed(() => text[config.value.language]);

const codexPrimaryRemaining = computed(() => codexUsage.value?.primary_remaining_percent ?? null);
const codexWeeklyRemaining = computed(() => codexUsage.value?.weekly_remaining_percent ?? null);
const codexPrimaryText = computed(() => formatCodexDisplayValue(codexPrimaryRemaining.value));
const codexWeeklyText = computed(() => formatCodexDisplayValue(codexWeeklyRemaining.value));
const codexDisplayName = computed(() => config.value.gpt_display_mode === "used" ? uiText.value.gptUsed : uiText.value.gptRemaining);
const codexStackText = computed(() => `${uiText.value.gptPrimaryLabel} ${codexPrimaryText.value} · ${uiText.value.gptWeeklyLabel} ${codexWeeklyText.value}`);
const codexLastUpdatedText = computed(() => {
  clockTick.value;
  if (codexLastUpdatedAt.value === null) return uiText.value.notUpdated;

  const minutes = Math.max(0, Math.floor((Date.now() - codexLastUpdatedAt.value) / 60_000));
  if (minutes < 1) return uiText.value.underOneMinute;
  if (minutes === 1) return uiText.value.oneMinuteAgo;

  const value = minutes < 60 ? minutes : Math.floor(minutes / 60);
  const suffix = minutes < 60 ? uiText.value.minutesAgo : uiText.value.hoursAgo;
  return config.value.language === "zh" ? `${value}${suffix}` : `${value} ${suffix}`;
});

// ---------- formatting ----------

function clampPercent(value: number) {
  return Math.min(100, Math.max(0, value));
}

function isValidNumber(value: number | null): value is number {
  return value !== null && Number.isFinite(value);
}

function formatPercent(value: number | null) {
  return isValidNumber(value) ? `${Math.round(clampPercent(value))}%` : "--";
}

function formatTemperature(value: number | null) {
  return isValidNumber(value) ? `${Math.round(value)}°C` : "N/A";
}

function formatRate(value: number | null) {
  if (!isValidNumber(value) || value < 0) return "--";
  if (value >= 1024 * 1024) return `${(value / 1024 / 1024).toFixed(1)}M`;
  if (value >= 1024) return `${(value / 1024).toFixed(1)}K`;
  return `${Math.round(value)}B/s`;
}

function compactAmount(value: number) {
  return value < 10 ? value.toFixed(1) : Math.min(999, Math.round(value)).toString();
}

function formatTaskbarRate(value: number | null): TaskbarRate {
  if (!isValidNumber(value) || value < 0) return { amount: "--", unit: "" };
  if (value >= 1024 * 1024) return { amount: compactAmount(value / 1024 / 1024), unit: "M/s" };
  if (value >= 1024) return { amount: compactAmount(value / 1024), unit: "K/s" };
  return { amount: Math.min(999, Math.round(value)).toString(), unit: "B/s" };
}

function formatCodexDisplayValue(remaining: number | null) {
  const value = remaining !== null && config.value.gpt_display_mode === "used" ? 100 - remaining : remaining;
  return formatPercent(value);
}

// ---------- generic helpers ----------

function patchHasAny(patch: Partial<WidgetConfig>, keys: readonly (keyof WidgetConfig)[]) {
  return keys.some((key) => key in patch);
}

function diffConfig(previous: WidgetConfig, nextConfig: WidgetConfig): Partial<WidgetConfig> {
  return Object.fromEntries(
    (Object.keys(nextConfig) as (keyof WidgetConfig)[])
      .filter((key) => previous[key] !== nextConfig[key])
      .map((key) => [key, nextConfig[key]]),
  ) as Partial<WidgetConfig>;
}

function checkedFromEvent(event: globalThis.Event) {
  return event.target instanceof HTMLInputElement && event.target.checked;
}

function stopTimer(name: TimerName) {
  const timer = timers[name];
  if (timer === undefined) return;
  window.clearInterval(timer);
  delete timers[name];
}

function startTimer(name: TimerName, callback: () => void, interval: number) {
  stopTimer(name);
  timers[name] = window.setInterval(callback, interval);
}

async function registerUnlistener(factory: () => Promise<UnlistenFn>, errorMessage: string) {
  try {
    unlisteners.push(await factory());
  } catch (error) {
    console.error(errorMessage, error);
  }
}

async function runCommand(command: string, errorMessage: string) {
  try {
    await invoke(command);
  } catch (error) {
    console.error(errorMessage, error);
  }
}

// ---------- backend data ----------

async function refreshSystemUsage() {
  try {
    const usage = await invoke<SystemUsage>("get_system_usage");
    cpuPercent.value = usage.cpu_percent;
    memoryPercent.value = usage.memory_percent;
    temperatureCelsius.value = usage.temperature_celsius;
    diskPercent.value = usage.disk_percent;
    uploadBytesPerSecond.value = usage.upload_bytes_per_second;
    downloadBytesPerSecond.value = usage.download_bytes_per_second;
  } catch (error) {
    console.error("无法刷新系统资源使用率", error);
  }
}

async function refreshCodexUsage() {
  if (codexRefreshing.value) return;
  codexRefreshing.value = true;

  try {
    codexUsage.value = await invoke<CodexUsage>("get_codex_usage");
    codexLastUpdatedAt.value = Date.now();
    codexError.value = "";
    codexStale.value = false;
    await refreshGptResetAlertState();
  } catch (error) {
    codexError.value = String(error);
    codexStale.value = codexUsage.value !== null;
    console.error("无法刷新 GPT/Codex 用量", error);
  } finally {
    codexRefreshing.value = false;
  }
}

async function refreshGptResetAlertState() {
  try {
    gptResetAlertPending.value = (await invoke<{ pending: boolean }>("get_gpt_reset_alert_state")).pending;
  } catch (error) {
    console.error("无法读取 GPT 重置提醒状态", error);
  }
}

async function setTrayFlashVisible(visible: boolean) {
  trayFlashVisible.value = visible;
  try {
    await invoke("set_tray_flash_visible", { visible });
  } catch (error) {
    console.error("无法设置托盘闪烁状态", error);
  }
}

async function pulseTrayFlash() {
  if (!gptResetAlertPending.value) {
    if (!trayFlashVisible.value) await setTrayFlashVisible(true);
    return;
  }
  await setTrayFlashVisible(!trayFlashVisible.value);
}

function restartCodexTimer() {
  stopTimer("codex");
  if (!config.value.gpt_visible && !isSettingsWindow) return;

  void refreshCodexUsage();
  startTimer(
    "codex",
    () => void refreshCodexUsage(),
    Math.max(1, config.value.gpt_refresh_seconds) * 1_000,
  );
}

async function loadHistory(metric: HistoryMetric) {
  return invoke<HistoryPoint[]>("get_history_points", { query: { metric, range: "minute" } });
}

async function refreshHistory() {
  try {
    const entries = await Promise.all(
      HISTORY_METRICS.map(async (metric) => [metric, await loadHistory(metric)] as const),
    );
    historyPoints.value = Object.fromEntries(entries) as Record<HistoryMetric, HistoryPoint[]>;
  } catch (error) {
    console.error("无法刷新历史曲线", error);
  }
}

// ---------- configuration ----------

async function syncWindowWidth() {
  const element = widgetElement.value;
  if (!element || isSettingsWindow) return;

  const width = Math.ceil(element.scrollWidth);
  if (width === lastReportedWidth) return;

  try {
    await invoke("set_widget_width", { width });
    lastReportedWidth = width;
  } catch (error) {
    if (!String(error).includes("__TAURI_INTERNALS__")) {
      console.error("无法同步任务栏组件宽度", error);
    }
  }
}

async function applyConfig(nextConfig: WidgetConfig, patch?: Partial<WidgetConfig>) {
  config.value = nextConfig;

  if (!patch || patchHasAny(patch, CODEX_TIMER_KEYS)) restartCodexTimer();

  await nextTick();

  if (!patch || patchHasAny(patch, WIDGET_LAYOUT_KEYS)) {
    lastReportedWidth = 0;
    await syncWindowWidth();
  }

  if (patch && patchHasAny(patch, SETTINGS_SIZE_KEYS)) scheduleSettingsResize();
}

async function updateConfig(patch: Partial<WidgetConfig>) {
  const previousConfig = { ...config.value };
  const nextConfig = { ...config.value, ...patch };

  await applyConfig(nextConfig, patch);

  try {
    const savedConfig = await invoke<WidgetConfig>("save_widget_settings", { config: nextConfig });
    await applyConfig(savedConfig, diffConfig(config.value, savedConfig));
  } catch (error) {
    await applyConfig(previousConfig, diffConfig(config.value, previousConfig));
    console.error("无法保存组件配置", error);
  }
}

function setMetricVisible(key: MetricVisibilityKey, event: globalThis.Event) {
  void updateConfig({ [key]: checkedFromEvent(event) } as Partial<WidgetConfig>);
}

function setNetworkVisible(visible: boolean) {
  void updateConfig({ upload_visible: visible, download_visible: visible });
}

function settingsLayoutKey() {
  return [
    config.value.language,
    config.value.cpu_visible,
    config.value.memory_visible,
    config.value.temperature_visible,
    config.value.disk_visible,
    networkVisible.value,
    config.value.gpt_visible,
  ].join("|");
}

function handleConfigChanged(event: Event<WidgetConfig>) {
  void applyConfig(event.payload, diffConfig(config.value, event.payload));
}

async function initializeConfig() {
  await registerUnlistener(
    () => listen<WidgetConfig>("widget-config-changed", handleConfigChanged),
    "无法监听组件配置变更",
  );
  await registerUnlistener(
    () => listen("display-environment-changed", () => void handleDisplayEnvironmentChanged()),
    "无法监听显示环境变化",
  );

  try {
    await applyConfig(await invoke<WidgetConfig>("get_widget_config"));
  } catch (error) {
    console.error("无法读取组件配置，将使用默认配置", error);
    await applyConfig({ ...defaultConfig });
  }
}

// ---------- settings/window behavior ----------

async function resizeSettingsWindow(force = false) {
  const element = settingsElement.value;
  if (!isSettingsWindow || !element) return;

  const layoutKey = settingsLayoutKey();
  if (!force && layoutKey === lastSettingsLayoutKey) return;

  try {
    await invoke("resize_settings_window", {
      width: element.scrollWidth,
      height: element.scrollHeight,
    });
    lastSettingsLayoutKey = layoutKey;
  } catch (error) {
    console.error("无法调整设置窗口尺寸", error);
  }
}

function scheduleSettingsResize(force = false) {
  if (isSettingsWindow) void nextTick(() => resizeSettingsWindow(force));
}

async function handleDisplayEnvironmentChanged() {
  if (!isSettingsWindow) lastReportedWidth = 0;
  await nextTick();
  await (isSettingsWindow ? resizeSettingsWindow(true) : syncWindowWidth());
}

function handleSettingsBlur() {
  window.setTimeout(() => {
    if (!document.hasFocus()) void hideSettings();
  }, 120);
}

function preventPackagedContextMenu(event: MouseEvent) {
  event.preventDefault();
}

// ---------- charts ----------

function sparklinePoints(points: HistoryPoint[], secondary = false, percent = true) {
  if (points.length === 0) return "";

  const values = points.map((point) => secondary ? point.secondary_value ?? 0 : point.value);
  const maxValue = percent ? 100 : Math.max(1, ...values);

  return values.map((value, index) => {
    const x = points.length === 1 ? SPARKLINE_WIDTH : index / (points.length - 1) * SPARKLINE_WIDTH;
    const y = SPARKLINE_HEIGHT - Math.min(1, Math.max(0, value / maxValue)) * SPARKLINE_HEIGHT;
    return `${x.toFixed(1)},${y.toFixed(1)}`;
  }).join(" ");
}

function historyMax(metric: HistoryMetric, secondary = false) {
  const values = historyPoints.value[metric]
    .map((point) => secondary ? point.secondary_value : point.value)
    .filter(isValidNumber);
  return values.length ? Math.max(...values) : null;
}

function progressWidth(value: number | null) {
  return `${isValidNumber(value) ? clampPercent(value) : 0}%`;
}

// ---------- UI actions ----------

function toggleGptDisplayMode() {
  void updateConfig({ gpt_display_mode: config.value.gpt_display_mode === "remaining" ? "used" : "remaining" });
}

function toggleLanguageMenu() {
  languageMenuOpen.value = !languageMenuOpen.value;
}

function selectLanguage(language: AppLanguage) {
  languageMenuOpen.value = false;
  if (config.value.language !== language) void updateConfig({ language });
}

function openTaskManager() {
  return runCommand("open_task_manager", "无法打开 Windows 任务管理器");
}

function openWidgetLog() {
  return runCommand("open_widget_log", "无法打开组件日志");
}

function quitApplication() {
  return runCommand("quit_application", "无法退出组件应用");
}

function hideSettings() {
  if (!isSettingsWindow) return Promise.resolve();
  return runCommand("hide_settings_window", "无法隐藏设置窗口");
}

// ---------- lifecycle ----------

function startSharedPolling() {
  void refreshGptResetAlertState();
  startTimer("alert", () => {
    void refreshGptResetAlertState();
    void pulseTrayFlash();
  }, ALERT_REFRESH_MS);

  startTimer("clock", () => {
    clockTick.value += 1;
  }, CLOCK_REFRESH_MS);
}

async function startSystemPolling() {
  await refreshSystemUsage();
  startTimer("system", () => void refreshSystemUsage(), SYSTEM_REFRESH_MS);
}

async function initializeWidget() {
  await initializeConfig();
  startSharedPolling();
  await startSystemPolling();

  if (widgetElement.value) {
    resizeObserver = new ResizeObserver(() => void syncWindowWidth());
    resizeObserver.observe(widgetElement.value);
  }
}

async function initializeSettings() {
  await initializeConfig();

  const settingsWindow = getCurrentWindow();
  await registerUnlistener(
    () => settingsWindow.onScaleChanged(() => scheduleSettingsResize(true)),
    "无法监听设置窗口缩放变化",
  );
  await registerUnlistener(
    () => settingsWindow.listen("settings-resize-requested", () => scheduleSettingsResize(true)),
    "无法监听设置窗口重新测量请求",
  );

  startSharedPolling();
  await startSystemPolling();
  await refreshCodexUsage();
  await refreshHistory();
  scheduleSettingsResize();

  startTimer("history", () => void refreshHistory(), HISTORY_REFRESH_MS);
  window.addEventListener("blur", handleSettingsBlur);
}

function cleanupWidget() {
  resizeObserver?.disconnect();
  unlisteners.splice(0).forEach((unlisten) => unlisten());
  (Object.keys(timers) as TimerName[]).forEach(stopTimer);

  window.removeEventListener("contextmenu", preventPackagedContextMenu);
  window.removeEventListener("blur", handleSettingsBlur);
  void invoke("set_tray_flash_visible", { visible: true });
}

onMounted(() => {
  if (import.meta.env.PROD) window.addEventListener("contextmenu", preventPackagedContextMenu);
  void (isSettingsWindow ? initializeSettings() : initializeWidget());
});

onUnmounted(cleanupWidget);
</script>
<template>
  <main v-if="!isSettingsWindow" ref="widgetElement" class="widget" :aria-label="uiText.widgetLabel">
    <button v-if="config.cpu_visible" type="button" class="item metric metric--cpu" :title="uiText.taskManagerTitle" @click="openTaskManager">
      <span class="item__label">{{ uiText.cpuLabel }}</span><strong>{{ cpuText }}</strong>
    </button>
    <button v-if="config.memory_visible" type="button" class="item metric metric--memory" :title="uiText.taskManagerTitle" @click="openTaskManager">
      <span class="item__label">{{ uiText.memoryLabel }}</span><strong>{{ memoryText }}</strong>
    </button>
    <section v-if="config.temperature_visible" class="item metric--temperature"><span class="item__label">{{ uiText.temperatureLabel }}</span><strong>{{ temperatureText }}</strong></section>
    <section v-if="config.disk_visible" class="item metric--disk"><span class="item__label">{{ uiText.diskLabel }}</span><strong>{{ diskText }}</strong></section>
    <section v-if="networkVisible" class="item item--stacked metric--network"><span class="item__label">{{ uiText.taskbarNetworkLabel }}</span><strong class="item__stack item__stack--rate"><span><em>↑</em><b>{{ taskbarUploadRate.amount }}</b><i>{{ taskbarUploadRate.unit }}</i></span><span><em>↓</em><b>{{ taskbarDownloadRate.amount }}</b><i>{{ taskbarDownloadRate.unit }}</i></span></strong></section>
    <section v-if="config.gpt_visible" class="item codex" :class="{ 'codex--error': codexError && !codexUsage, 'codex--stale': codexStale }" :title="codexStale ? `${uiText.gptRefreshFailed}：${codexError}` : codexError || `Codex ${codexDisplayName} · ${uiText.refreshEvery} ${config.gpt_refresh_seconds} ${uiText.seconds}`" :aria-label="`GPT Codex ${codexDisplayName}`">
      <span class="item__label">{{ uiText.gptLabel }}</span><strong class="item__stack item__stack--gpt"><span><em>{{ uiText.gptPrimaryLabel }}</em>{{ codexPrimaryText }}</span><span><em>{{ uiText.gptWeeklyLabel }}</em>{{ codexWeeklyText }}</span></strong>
    </section>
    <span v-if="allMetricsHidden" class="empty-hint">{{ uiText.traySettingsHint }}</span>
  </main>
  <main v-else ref="settingsElement" class="settings-card" :aria-label="uiText.settingsLabel">
      <header class="settings-header">
        <div class="settings-brand">
          <img src="/app.png" width="24"/>
          <!--
          <svg aria-hidden="true" viewBox="0 0 24 24">
          <path d="M5 7l4 4-4 4M11 15h7" />
          </svg>
          !-->
          <h1>TB Widget</h1>
        </div>
        <div class="header-actions">
          <div class="language-menu">
            <button type="button" class="language-button" :aria-label="uiText.switchLanguage" :aria-expanded="languageMenuOpen" :title="uiText.switchLanguage" @click="toggleLanguageMenu">🌐</button>
            <div v-if="languageMenuOpen" class="language-menu__panel" role="menu" :aria-label="uiText.switchLanguage">
              <button type="button" role="menuitemradio" :aria-checked="config.language === 'en'" @click="selectLanguage('en')"><span aria-hidden="true">{{ config.language === "en" ? "✓" : "" }}</span>English</button>
              <button type="button" role="menuitemradio" :aria-checked="config.language === 'zh'" @click="selectLanguage('zh')"><span aria-hidden="true">{{ config.language === "zh" ? "✓" : "" }}</span>简体中文</button>
            </div>
          </div>
          <button type="button" :aria-label="uiText.openLog" @click="openWidgetLog">{{ uiText.logFile }}</button>
          <button type="button" class="quit-button" :aria-label="uiText.quitApp" @click="quitApplication">{{ uiText.quit }}</button>
        </div>
      </header>
      <div class="settings-content">
        <section class="settings-controls" :aria-label="uiText.layoutSettings">
          <fieldset>
            <legend>{{ uiText.position }}</legend>
            <label><input type="radio" name="position" :checked="config.position === 'left'" @change="updateConfig({ position: 'left' })" /> {{ uiText.left }}</label>
            <label><input type="radio" name="position" :checked="config.position === 'right'" @change="updateConfig({ position: 'right' })" /> {{ uiText.right }}</label>
          </fieldset>
          <fieldset>
            <legend>{{ uiText.taskbar }}</legend>
            <label><input type="checkbox" :aria-label="uiText.enableTaskbarTransparent" :checked="config.taskbar_transparency_mode === 'clear'" @change="updateConfig({ taskbar_transparency_mode: checkedFromEvent($event) ? 'clear' : 'off' })" /> {{ uiText.transparent }}</label>
          </fieldset>
        </section>
        <div class="settings-divider"></div>
        <section class="visibility-filters" :aria-label="uiText.metricsSettings">
          <label><input type="checkbox" :aria-label="uiText.cpuLabel" :checked="config.cpu_visible" @change="setMetricVisible('cpu_visible', $event)" /> {{ uiText.cpuLabel }}</label>
          <label><input type="checkbox" :aria-label="uiText.memoryLabel" :checked="config.memory_visible" @change="setMetricVisible('memory_visible', $event)" /> {{ uiText.memoryLabel }}</label>
          <label><input type="checkbox" :aria-label="uiText.temperatureLabel" :checked="config.temperature_visible" @change="setMetricVisible('temperature_visible', $event)" /> {{ uiText.temperatureLabel }}</label>
          <label><input type="checkbox" :aria-label="uiText.networkLabel" :checked="networkVisible" @change="setNetworkVisible(checkedFromEvent($event))" /> {{ uiText.networkLabel }}</label>
          <label><input type="checkbox" :aria-label="uiText.diskLabel" :checked="config.disk_visible" @change="setMetricVisible('disk_visible', $event)" /> {{ uiText.diskLabel }}</label>
          <label><input type="checkbox" :aria-label="uiText.gptLabel" :checked="config.gpt_visible" @change="setMetricVisible('gpt_visible', $event)" /> {{ uiText.gptLabel }}</label>
        </section>
        <section class="metric-list" aria-label="实时指标">
          <article v-if="config.cpu_visible" class="metric-card metric-card--cpu">
            <div class="metric-summary"><div class="metric-label">{{ uiText.cpuLabel }} <span class="metric-badge">MAX {{ formatPercent(historyMax('cpu')) }}</span></div><strong>{{ cpuText }}</strong></div>
            <svg class="settings-sparkline" :aria-label="uiText.cpuHistoryLabel" preserveAspectRatio="none" viewBox="0 0 250 42"><polyline :points="sparklinePoints(historyPoints.cpu)" /></svg>
          </article>
          <article v-if="config.memory_visible" class="metric-card metric-card--memory">
            <div class="metric-summary"><div class="metric-label">{{ uiText.memoryLabel }} <span class="metric-badge">MAX {{ formatPercent(historyMax('memory')) }}</span></div><strong>{{ memoryText }}</strong></div>
            <svg class="settings-sparkline" :aria-label="uiText.memoryHistoryLabel" preserveAspectRatio="none" viewBox="0 0 250 42"><polyline :points="sparklinePoints(historyPoints.memory)" /></svg>
          </article>
          <article v-if="config.temperature_visible" class="metric-card metric-card--temperature">
            <div class="metric-summary"><div class="metric-label">{{ uiText.temperatureLabel }} <span class="metric-badge">MAX {{ formatTemperature(historyMax('temperature')) }}</span></div><strong>{{ temperatureText }}</strong></div>
            <svg class="settings-sparkline" :aria-label="uiText.temperatureHistoryLabel" preserveAspectRatio="none" viewBox="0 0 250 42"><polyline :points="sparklinePoints(historyPoints.temperature)" /></svg>
          </article>
          <article v-if="networkVisible" class="metric-card metric-card--network">
            <div class="metric-summary metric-summary--network">
              <div class="metric-label">{{ uiText.networkLabel }} <span class="metric-badge">MAX ↑{{ formatRate(historyMax('network')) }} ↓{{ formatRate(historyMax('network', true)) }}</span></div>
              <strong><span>↑{{ uploadText }}</span><span>↓{{ downloadText }}</span></strong>
            </div>
            <svg class="settings-sparkline" :aria-label="uiText.networkHistoryLabel" preserveAspectRatio="none" viewBox="0 0 250 42"><polyline class="upload-line" :points="sparklinePoints(historyPoints.network, false, false)" /><polyline class="download-line" :points="sparklinePoints(historyPoints.network, true, false)" /></svg>
          </article>
          <article v-if="config.disk_visible" class="metric-card metric-card--disk">
            <div class="metric-summary"><div class="metric-label">{{ uiText.diskLabel }}</div><strong>{{ diskText }}</strong></div>
            <div class="progress-track" :aria-label="uiText.diskUsageLabel"><span :style="{ width: progressWidth(diskPercent) }"></span></div>
          </article>
          <article
            v-if="config.gpt_visible"
            class="metric-card metric-card--gpt"
            :class="{ 'metric-card--gpt-used': config.gpt_display_mode === 'used', 'metric-card--gpt-remaining': config.gpt_display_mode === 'remaining', 'metric-card--error': codexError && !codexUsage, 'metric-card--stale': codexStale }"
            role="button"
            tabindex="0"
            :aria-label="`${uiText.switchGptMode}${codexDisplayName} ${codexStackText}`"
            :title="codexError || `${uiText.clickToggleValue} · ${codexLastUpdatedText}`"
            @click="toggleGptDisplayMode"
            @keydown.enter.self="toggleGptDisplayMode"
            @keydown.space.self.prevent="toggleGptDisplayMode"
          >
            <button type="button" class="metric-action metric-action--refresh" :disabled="codexRefreshing" :aria-label="uiText.refreshGptUsage" :title="uiText.refreshGpt" @click.stop="refreshCodexUsage"><svg aria-hidden="true" viewBox="0 0 24 24"><path d="M20 6v5h-5M4 18v-5h5M18.5 9A7 7 0 006.8 6.8L4 10m16 4-2.8 3.2A7 7 0 015.5 15" /></svg></button>
            <div class="metric-summary metric-summary--gpt">
              <div class="metric-label metric-label--gpt">{{ uiText.gptLabel }} <span class="metric-mode-label">{{ codexDisplayName }}</span></div>
              <strong class="metric-stack"><span><em>{{ uiText.gptPrimaryLabel }}</em>{{ codexPrimaryText }}</span><span><em>{{ uiText.gptWeeklyLabel }}</em>{{ codexWeeklyText }}</span></strong>
            </div>
            <div class="gpt-progress-list">
              <div class="gpt-progress-row"><div class="progress-track progress-track--gpt-primary" :aria-label="uiText.gptPrimaryProgressLabel"><span :style="{ width: progressWidth(codexPrimaryRemaining) }"></span></div></div>
              <div class="gpt-progress-row"><div class="progress-track progress-track--gpt-weekly" :aria-label="uiText.gptWeeklyProgressLabel"><span :style="{ width: progressWidth(codexWeeklyRemaining) }"></span></div></div>
            </div>
          </article>
        </section>
      </div>
  </main>
</template>
<style>
:root {
  font-family: "Segoe UI Variable Text", "Microsoft YaHei UI", sans-serif;
  color: #f6f8fb;
  background: transparent;
  font-synthesis: none;
  text-rendering: geometricPrecision;
  -webkit-font-smoothing: antialiased;
}
* { box-sizing: border-box; }
html,
body,
#app {
  width: 100%;
  height: 100%;
  margin: 0;
  overflow: hidden;
  background: transparent;
}
body { user-select: none; }
button, input, select { font: inherit; }
</style>
<style scoped>
.widget {
  width: max-content;
  height: 100vh;
  min-height: 36px;
  display: inline-flex;
  align-items: center;
  padding: 4px 8px;
  overflow: hidden;
  border: 0;
  background: transparent;
  box-shadow: none;
  white-space: nowrap;
}
.item {
  min-width: 0;
  height: 28px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 5px;
  padding: 0 9px;
  border: 0;
  color: inherit;
  background: transparent;
  white-space: nowrap;
}
.item + .item { border-left: 1px solid rgba(255, 255, 255, 0.14); }
.metric { cursor: pointer; }
.metric:hover { background: rgba(255, 255, 255, 0.08); }
.metric:focus-visible { outline: 2px solid #69b8ff; outline-offset: 1px; }
.item--stacked .item__label { justify-content: center; min-width: 24px; text-align: center; }
.item__label { display: inline-flex; align-items: center; height: 16px; color: #aab4c2; font-size: 10px; font-weight: 650; line-height: 16px; letter-spacing: 0.04em; }
.item strong { display: inline-flex; align-items: center; height: 16px; font-size: 13px; font-variant-numeric: tabular-nums; line-height: 16px; }
.item--stacked { --stack-font-size: 8px; --stack-line-height: 11px; --stack-row-height: 11px; }
.item strong.item__stack { display: flex; flex-direction: column; align-items: flex-start; height: auto; gap: 2px; font-family: "JetBrains Mono", monospace; font-size: 8px; font-weight: 500; line-height: var(--stack-line-height); }
.item strong.item__stack span { display: grid; grid-template-columns: 8px 4ch 18px; align-items: center; column-gap: 2px; height: var(--stack-row-height); }
.item strong.item__stack em { color: #aab4c2; font: inherit; font-style: normal; text-align: center; }
.item strong.item__stack b { font: inherit; font-variant-numeric: tabular-nums; text-align: right; }
.item strong.item__stack i { font: inherit; color: #aab4c2; font-style: normal; text-align: left; }
.item strong.item__stack--gpt span { grid-template-columns: 30px 4ch; column-gap: 4px; }
.metric--cpu strong { color: #60A5FA; }
.metric--memory strong { color: #A78BFA; }
.codex strong { color: #E879F9; }
.codex--error strong { color: #ff9090; }
.codex--stale strong { color: #d8b56a; }
.metric--temperature strong { color: #F87171; }
.metric--disk strong { color: #34D399; }
.metric--network strong { color: #22D3EE; }
.empty-hint { padding: 0 10px; color: #aab4c2; font-size: 12px; }
.settings-card {
  width: min(384px, 100vw);
  max-height: 100vh;
  margin: 0;
  overflow-x: hidden;
  overflow-y: auto;
  color: #e0e2ed;
  background: #1c2028;
  box-shadow: 0 24px 60px rgba(0, 0, 0, 0.38);
  font-family: "Inter", "Microsoft YaHei UI", sans-serif;
  font-size: 12px;
  scrollbar-width: thin;
  scrollbar-color: rgba(139, 144, 160, 0.5) transparent;
}
.settings-header {
  width: 100%;
  height: 48px;
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 0 16px;
  border-bottom: 1px solid rgba(255, 255, 255, 0.1);
  background: rgba(28, 32, 40, 0.3);
}
.settings-brand,
.header-actions,
.settings-controls,
.settings-controls fieldset,
.settings-controls label,
.visibility-filters,
.visibility-filters label,
.metric-card,
.metric-label,
.metric-label--gpt,
.metric-summary--network strong {
  display: flex;
  align-items: center;
}
.settings-brand { gap: 8px; }
.settings-brand svg { width: 20px; height: 20px; fill: none; stroke: #adc6ff; stroke-width: 2; stroke-linecap: round; stroke-linejoin: round; }
.settings-brand h1 { margin: 0; color: #e0e2ed; font-family: "Manrope", sans-serif; font-size: 20px; font-weight: 700; line-height: 26px; letter-spacing: -0.01em; }
.header-actions { gap: 2px; }
.header-actions button { padding: 5px 7px; border: 0; border-radius: 4px; color: #e0e2ed; font-size: 11px; font-weight: 600; line-height: 16px; letter-spacing: 0.06em; background: transparent; cursor: pointer; }
.header-actions button:hover { background: rgba(255, 255, 255, 0.1); }
.header-actions .quit-button { color: #ffb4ab; }
.language-menu { position: relative; }
.language-menu__panel { position: absolute; top: calc(100% + 6px); left: 0; z-index: 10; display: flex; flex-direction: column; min-width: 108px; padding: 4px; border: 1px solid rgba(255, 255, 255, 0.12); border-radius: 8px; background: rgba(23, 27, 35, 0.96); box-shadow: 0 10px 24px rgba(0, 0, 0, 0.3); }
.language-menu__panel button { display: flex; gap: 6px; justify-content: flex-start; width: 100%; padding: 6px 8px; text-align: left; white-space: nowrap; }
.language-menu__panel span { width: 10px; color: #8bd6a0; }
.settings-content { display: flex; flex-direction: column; gap: 16px; padding: 16px; }
.settings-controls { justify-content: space-between; gap: 0px; padding: 0 4px; }
.settings-controls fieldset { gap: 8px; min-width: 0; margin: 0; padding: 0; border: 0; }
.settings-controls legend { float: left; margin: 0 2px 0 0; padding: 0; color: #8b90a0; font-size: 11px; font-weight: 600; line-height: 16px; letter-spacing: 0.06em; }
.settings-controls label,
.visibility-filters label { gap: 5px; color: #c1c6d7; font-size: 11px; font-weight: 600; line-height: 16px; letter-spacing: 0.06em; cursor: pointer; }
.settings-controls input,
.visibility-filters input { width: 14px; height: 14px; margin: 0; accent-color: #4b8eff; cursor: pointer; }
.settings-divider { height: 1px; background: rgba(255, 255, 255, 0.1); }
.visibility-filters { flex-wrap: nowrap; gap: 8px 8px; padding: 0 4px; }
.metric-list { display: flex; flex-direction: column; gap: 8px; }
.metric-card {
  position: relative;
  width: 100%;
  height: 72px;
  justify-content: space-between;
  gap: 6px;
  padding: 12px;
  border: 1px solid rgba(255, 255, 255, 0.1);
  border-radius: 12px;
  background: rgba(255, 255, 255, 0.05);
}
.metric-summary { width: 92px; min-width: 92px; display: flex; flex-direction: column; justify-content: flex-start; gap: 6px; }
.metric-label { gap: 6px; color: #c1c6d7; font-family: "JetBrains Mono", monospace; font-size: 12px; font-weight: 500; line-height: 16px; letter-spacing: 0.02em; white-space: nowrap; }
.metric-badge {
  position: absolute;
  top: 8px;
  right: 10px;
  z-index: 2;
  max-width: calc(100% - 20px);
  overflow: hidden;
  padding: 2px 4px;
  border-radius: 4px;
  color: currentColor;
  font-family: "Inter", sans-serif;
  font-size: 8px;
  font-weight: 700;
  line-height: 1;
  text-overflow: ellipsis;
  white-space: nowrap;
  background: color-mix(in srgb, currentColor 18%, transparent);
  pointer-events: none;
}
.metric-summary strong { color: currentColor; font-family: "Manrope", sans-serif; font-size: 20px; font-weight: 600; font-variant-numeric: tabular-nums; line-height: 22px; }
.settings-sparkline { min-width: 0; height: 42px; flex: 1; overflow: visible; color: currentColor; }
.settings-sparkline polyline { fill: none; stroke: currentColor; stroke-width: 1.5; stroke-linecap: round; stroke-linejoin: round; vector-effect: non-scaling-stroke; filter: drop-shadow(0 0 4px color-mix(in srgb, currentColor 60%, transparent)); }
.metric-card--cpu { color: #4b8eff; }
.metric-card--memory { color: #a855f7; }
.metric-card--temperature { color: #ef6719; }
.metric-card--network { color: #4b8eff; }
.metric-card--disk { color: #34d399; }
.metric-card--gpt { height: 86px; color: #ec4899; cursor: pointer; }
.metric-card--gpt-remaining .metric-summary strong { color: #34d399; }
.metric-card--gpt-used .metric-summary strong { color: #f87171; }
.metric-card--error { color: #ffb4ab; }
.metric-card--stale { color: #d8b56a; }
.metric-summary--network { gap: 2px; }
.metric-summary--network .metric-label { gap: 4px; }
.metric-summary--network strong { flex-direction: column; align-items: flex-start; gap: 0; font-family: "JetBrains Mono", monospace; font-size: 11px; font-weight: 500; line-height: 15px; }
.metric-summary--gpt { gap: 4px; }
.metric-stack { flex-direction: column; align-items: flex-start; gap: 0; font-family: "JetBrains Mono", monospace !important; font-size: 10px !important; line-height: 13px !important; }
.metric-stack span { display: flex; gap: 6px; }
.metric-stack em { min-width: 34px; color: #8b90a0; font-style: normal; font-size: 9px; }
.gpt-progress-list { flex: 1; display: flex; flex-direction: column; gap: 8px; min-width: 0; margin-right: 28px; padding-top: 23px; }
.gpt-progress-row { display: flex; align-items: center; gap: 8px; min-width: 0; height: 6px; color: currentColor; }
.gpt-progress-row .progress-track { margin: 0; }
.settings-sparkline .upload-line { color: #4b8eff; }
.settings-sparkline .download-line { color: #ef6719; }
.progress-track { height: 6px; flex: 1; margin: 0; overflow: hidden; border-radius: 999px; background: #31353d; }
.progress-track span { display: block; height: 100%; border-radius: inherit; background: currentColor; box-shadow: 0 0 8px color-mix(in srgb, currentColor 50%, transparent); transition: width 180ms ease; }
.progress-track--gpt-primary span { background: rgba(96, 165, 250, 0.7); box-shadow: 0 0 8px rgba(96, 165, 250, 0.35); }
.progress-track--gpt-weekly span { background: #34d399; box-shadow: 0 0 8px color-mix(in srgb, #34d399 50%, transparent); }
.metric-mode-label { color: #8b90a0; font-family: "Inter", sans-serif; font-size: 8px; font-weight: 700; line-height: 1; }
.metric-action {
  position: absolute;
  top: 7px;
  right: 8px;
  z-index: 3;
  width: 22px;
  height: 22px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  padding: 3px;
  border: 0;
  border-radius: 5px;
  color: #c1c6d7;
  background: rgba(28, 32, 40, 0.75);
  cursor: pointer;
}
.metric-action:hover { background: rgba(255, 255, 255, 0.1); }
.metric-action:disabled { opacity: 0.45; cursor: wait; }
.metric-action svg { width: 14px; height: 14px; fill: none; stroke: currentColor; stroke-width: 1.8; stroke-linecap: round; stroke-linejoin: round; }
.settings-card button:focus-visible,
.settings-card input:focus-visible,
.metric-card--gpt:focus-visible { outline: 2px solid #adc6ff; outline-offset: 2px; }
@media (max-width: 359px) {
  .settings-header { height: auto; min-height: 48px; flex-wrap: wrap; gap: 4px 8px; padding: 8px 16px; }
  .header-actions, .settings-controls fieldset { flex-wrap: wrap; }
  .settings-controls { align-items: flex-start; flex-direction: column; }
}
@media (prefers-reduced-motion: reduce) {
  * { transition: none !important; }
}
</style>
