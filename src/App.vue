<script setup lang="ts">
import { invoke } from "@tauri-apps/api/core";
import { listen, type Event, type UnlistenFn } from "@tauri-apps/api/event";
import { computed, nextTick, onMounted, onUnmounted, ref } from "vue";

type WidgetPosition = "left" | "right";
type GptDisplayMode = "remaining" | "used";
type HistoryRange = "minute" | "hour";
type HistoryMetric = "cpu" | "memory" | "temperature" | "disk" | "network" | "gpt";

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

type ResetCredit = {
  status: string;
  title: string;
  description: string;
  expires_at: string | null;
  is_supported_by_plan: boolean;
};

type CodexResetCredits = {
  available_count: number;
  total_earned_count: number;
  credits: ResetCredit[];
};

type GptResetAlertState = {
  pending: boolean;
};

type HistoryPoint = {
  timestamp: number;
  value: number;
  secondary_value: number | null;
};

type ChartHoverPoint = {
  metric: HistoryMetric;
  index: number;
};

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
};

const config = ref<WidgetConfig>({ ...defaultConfig });
const cpuPercent = ref<number | null>(null);
const memoryPercent = ref<number | null>(null);
const temperatureCelsius = ref<number | null>(null);
const diskPercent = ref<number | null>(null);
const uploadBytesPerSecond = ref(0);
const downloadBytesPerSecond = ref(0);
const codexUsage = ref<CodexUsage | null>(null);
const codexResetCredits = ref<CodexResetCredits | null>(null);
const codexError = ref("");
const resetError = ref("");
const codexStale = ref(false);
const codexRefreshing = ref(false);
const codexLastUpdatedAt = ref<number | null>(null);
const clockTick = ref(0);
const gptResetAlertPending = ref(false);
const trayFlashVisible = ref(true);
const chartTooltip = ref("");
const chartTooltipMetric = ref<HistoryMetric | null>(null);
const chartHoverPoint = ref<ChartHoverPoint | null>(null);
const widgetElement = ref<HTMLElement | null>(null);
const settingsElement = ref<HTMLElement | null>(null);
const historyRange = ref<HistoryRange>("minute");
const historyPoints = ref<Record<HistoryMetric, HistoryPoint[]>>({
  cpu: [],
  memory: [],
  temperature: [],
  disk: [],
  network: [],
  gpt: [],
});
let systemTimer: number | undefined;
let codexTimer: number | undefined;
let historyTimer: number | undefined;
let alertTimer: number | undefined;
let clockTimer: number | undefined;
let resizeObserver: ResizeObserver | undefined;
let unlistenConfig: UnlistenFn | undefined;
let lastReportedWidth = 0;
let lastSettingsLayoutKey = "";

const cpuText = computed(() => formatPercent(cpuPercent.value));
const memoryText = computed(() => formatPercent(memoryPercent.value));
const temperatureText = computed(() => formatTemperature(temperatureCelsius.value));
const diskText = computed(() => formatPercent(diskPercent.value));
const uploadText = computed(() => formatRate(uploadBytesPerSecond.value));
const downloadText = computed(() => formatRate(downloadBytesPerSecond.value));
const networkVisible = computed(() => config.value.upload_visible || config.value.download_visible);
const codexRemaining = computed(() => codexUsage.value?.weekly_remaining_percent ?? codexUsage.value?.primary_remaining_percent ?? null);
const codexText = computed(() => {
  const remaining = codexRemaining.value;
  const value = remaining !== null && config.value.gpt_display_mode === "used" ? 100 - remaining : remaining;
  return formatPercent(value);
});
const codexDisplayName = computed(() => config.value.gpt_display_mode === "used" ? "已用量" : "剩余额度");
const codexUsedText = computed(() => codexRemaining.value === null ? "--" : formatPercent(100 - codexRemaining.value));
const codexChartText = computed(() => config.value.gpt_display_mode === "used" ? codexUsedText.value : codexText.value);
const gptChartUseRemaining = computed(() => config.value.gpt_display_mode === "remaining");
const codexLastUpdatedText = computed(() => {
  clockTick.value;
  if (codexLastUpdatedAt.value === null) return "未更新";
  const minutes = Math.max(0, Math.floor((Date.now() - codexLastUpdatedAt.value) / 60000));
  if (minutes < 1) return "1分钟以内";
  if (minutes === 1) return "1分钟前";
  if (minutes < 60) return `${minutes}分钟前`;
  return `${Math.floor(minutes / 60)}小时前`;
});
const allMetricsHidden = computed(() =>
  !config.value.cpu_visible
  && !config.value.memory_visible
  && !config.value.gpt_visible
  && !config.value.temperature_visible
  && !config.value.disk_visible
  && !config.value.upload_visible
  && !config.value.download_visible,
);

/**
 * 将百分比格式化为任务栏显示文本。
 * @param value 百分比数值
 * @return 格式化后的百分比或占位符
 */
function formatPercent(value: number | null) {
  return value === null || !Number.isFinite(value) ? "--" : `${Math.round(Math.min(100, Math.max(0, value)))}%`;
}

/**
 * 将温度格式化为摄氏度文本。
 * @param value 摄氏温度
 * @return 格式化后的温度或 N/A
 */
function formatTemperature(value: number | null) {
  return value === null || !Number.isFinite(value) ? "N/A" : `${Math.round(value)}°C`;
}

/**
 * 将每秒字节数格式化为紧凑传输速率。
 * @param value 每秒字节数
 * @return 格式化后的传输速率
 */
function formatRate(value: number | null) {
  if (value === null || !Number.isFinite(value) || value < 0) return "--";
  if (value >= 1024 * 1024) return `${(value / 1024 / 1024).toFixed(1)}M`;
  if (value >= 1024) return `${(value / 1024).toFixed(1)}K`;
  return `${Math.round(value)}B/s`;
}

/**
 * 将 ISO 时间格式化为本地显示文本。
 * @param value ISO 时间字符串
 * @return 本地时间文本
 */
function formatIsoTime(value: string | null) {
  if (!value) return "--";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return date.toLocaleString();
}

/**
 * 从 Rust 后端刷新全部系统资源指标。
 * @return 无返回值
 */
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

/**
 * 请求 Rust 后端打开 Windows 任务管理器。
 * @return 无返回值
 */
async function openTaskManager() {
  try {
    await invoke("open_task_manager");
  } catch (error) {
    console.error("无法打开 Windows 任务管理器", error);
  }
}

/**
 * 从 Rust 后端刷新 Codex 的剩余额度。
 * @return 无返回值
 */
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

/**
 * 从 Rust 后端刷新 Codex 重置额度信息。
 * @return 无返回值
 */
async function refreshCodexResetCredits() {
  try {
    codexResetCredits.value = await invoke<CodexResetCredits>("get_codex_reset_credits");
    resetError.value = "";
  } catch (error) {
    resetError.value = String(error);
    console.error("无法刷新 GPT/Codex 重置额度", error);
  }
}

/**
 * 查询后端 GPT 重置提醒状态。
 * @return 无返回值
 */
async function refreshGptResetAlertState() {
  try {
    const state = await invoke<GptResetAlertState>("get_gpt_reset_alert_state");
    gptResetAlertPending.value = state.pending;
  } catch (error) {
    console.error("无法读取 GPT 重置提醒状态", error);
  }
}

/**
 * 执行一次 GPT 重置提醒闪烁。
 * @return 无返回值
 */
async function pulseTrayFlash() {
  if (!gptResetAlertPending.value) {
    if (!trayFlashVisible.value) {
      trayFlashVisible.value = true;
      await invoke("set_tray_flash_visible", { visible: true });
    }
    return;
  }
  trayFlashVisible.value = !trayFlashVisible.value;
  try {
    await invoke("set_tray_flash_visible", { visible: trayFlashVisible.value });
  } catch (error) {
    console.error("无法闪烁托盘图标", error);
  }
}

/**
 * 停止 GPT 用量刷新定时器。
 * @return 无返回值
 */
function stopCodexTimer() {
  if (codexTimer !== undefined) {
    window.clearInterval(codexTimer);
    codexTimer = undefined;
  }
}

/**
 * 启动 GPT 重置提醒轮询。
 * @return 无返回值
 */
function startAlertTimer() {
  if (alertTimer !== undefined) window.clearInterval(alertTimer);
  void refreshGptResetAlertState();
  alertTimer = window.setInterval(() => {
    void refreshGptResetAlertState();
    void pulseTrayFlash();
  }, 1000);
}

/**
 * 启动更新时间文本刷新计时器。
 * @return 无返回值
 */
function startClockTimer() {
  if (clockTimer !== undefined) window.clearInterval(clockTimer);
  clockTimer = window.setInterval(() => {
    clockTick.value += 1;
  }, 30000);
}

/**
 * 按当前配置立即刷新 GPT 并重建定时器，隐藏时停止刷新。
 * @return 无返回值
 */
function restartCodexTimer() {
  stopCodexTimer();
  if (!config.value.gpt_visible && !isSettingsWindow) return;
  void refreshCodexUsage();
  if (isSettingsWindow) void refreshCodexResetCredits();
  codexTimer = window.setInterval(() => {
    void refreshCodexUsage();
    if (isSettingsWindow) void refreshCodexResetCredits();
  }, config.value.gpt_refresh_seconds * 1000);
}

/**
 * 将 Vue 页面宽度同步给原生任务栏子窗口。
 * @return 无返回值
 */
async function syncWindowWidth() {
  const element = widgetElement.value;
  if (!element || isSettingsWindow) return;
  const width = Math.ceil(element.scrollWidth);
  if (width === lastReportedWidth) return;
  try {
    await invoke("set_widget_width", { width });
    lastReportedWidth = width;
  } catch (error) {
    if (!String(error).includes("__TAURI_INTERNALS__")) console.error("无法同步任务栏组件宽度", error);
  }
}

/**
 * 响应 Vue 根元素尺寸变化并同步原生窗口宽度。
 * @return 无返回值
 */
function handleWidgetResize() {
  void syncWindowWidth();
}

/**
 * 应用后端下发配置并更新 GPT 定时器和窗口宽度。
 * @param nextConfig 最新组件配置
 * @return 无返回值
 */
async function applyConfig(nextConfig: WidgetConfig) {
  config.value = nextConfig;
  restartCodexTimer();
  await nextTick();
  lastReportedWidth = 0;
  await syncWindowWidth();
}

/**
 * 从表单事件中读取复选框状态。
 * @param event 表单变更事件
 * @return 复选框是否选中
 */
function checkedFromEvent(event: globalThis.Event) {
  return event.target instanceof HTMLInputElement && event.target.checked;
}

/**
 * 从表单事件中读取选择框数值。
 * @param event 表单变更事件
 * @return 选择框数值
 */
function numberFromEvent(event: globalThis.Event) {
  return event.target instanceof HTMLSelectElement ? Number(event.target.value) : config.value.gpt_refresh_seconds;
}

/**
 * 从表单事件中读取输入框文本。
 * @param event 表单变更事件
 * @return 输入框文本
 */
function textFromEvent(event: globalThis.Event) {
  return event.target instanceof HTMLInputElement ? event.target.value : "";
}

/**
 * 保存设置窗口修改后的配置。
 * @param patch 局部配置变更
 * @return 无返回值
 */
async function updateConfig(patch: Partial<WidgetConfig>) {
  const nextConfig = { ...config.value, ...patch };
  try {
    await applyConfig(await invoke<WidgetConfig>("save_widget_settings", { config: nextConfig }));
    if (isVisibilityPatch(patch)) scheduleSettingsResize();
  } catch (error) {
    console.error("无法保存组件配置", error);
  }
}

/**
 * 更新单个指标显示开关。
 * @param key 配置中的显示开关键
 * @param event 表单变更事件
 * @return 无返回值
 */
function setMetricVisible(key: keyof Pick<WidgetConfig, "cpu_visible" | "memory_visible" | "temperature_visible" | "disk_visible" | "gpt_visible">, event: globalThis.Event) {
  const visible = checkedFromEvent(event);
  switch (key) {
    case "cpu_visible":
      void updateConfig({ cpu_visible: visible });
      break;
    case "memory_visible":
      void updateConfig({ memory_visible: visible });
      break;
    case "temperature_visible":
      void updateConfig({ temperature_visible: visible });
      break;
    case "disk_visible":
      void updateConfig({ disk_visible: visible });
      break;
    case "gpt_visible":
      void updateConfig({ gpt_visible: visible });
      break;
  }
}

/**
 * 切换网络显示开关。
 * @param visible 是否显示网络
 * @return 无返回值
 */
function setNetworkVisible(visible: boolean) {
  void updateConfig({ upload_visible: visible, download_visible: visible });
}

/**
 * 判断配置变更是否会改变曲线卡片数量。
 * @param patch 局部配置变更
 * @return 是否需要重新计算设置窗口尺寸
 */
function isVisibilityPatch(patch: Partial<WidgetConfig>) {
  return "cpu_visible" in patch || "memory_visible" in patch || "temperature_visible" in patch || "disk_visible" in patch || "upload_visible" in patch || "download_visible" in patch || "gpt_visible" in patch;
}

/**
 * 生成当前设置面板可见内容的结构指纹。
 * @return 可见内容指纹
 */
function settingsLayoutKey() {
  return [config.value.cpu_visible, config.value.memory_visible, config.value.temperature_visible, config.value.disk_visible, networkVisible.value, config.value.gpt_visible].join("|");
}

/**
 * 响应后端托盘配置变更事件。
 * @param event 携带最新配置的 Tauri 事件
 * @return 无返回值
 */
function handleConfigChanged(event: Event<WidgetConfig>) {
  void applyConfig(event.payload).then(() => scheduleSettingsResize());
}

/**
 * 获取初始配置并监听后端托盘配置变更事件。
 * @return 无返回值
 */
async function initializeConfig() {
  try {
    unlistenConfig = await listen<WidgetConfig>("widget-config-changed", handleConfigChanged);
  } catch (error) {
    console.error("无法监听组件配置变更", error);
  }
  try {
    await applyConfig(await invoke<WidgetConfig>("get_widget_config"));
  } catch (error) {
    console.error("无法读取组件配置，将使用默认配置", error);
    await applyConfig({ ...defaultConfig });
  }
}

/**
 * 从后端读取单个指标的历史曲线。
 * @param metric 指标名称
 * @return 历史点数组
 */
async function loadHistory(metric: HistoryMetric) {
  const range = metric === "gpt" ? historyRange.value : "minute";
  return invoke<HistoryPoint[]>("get_history_points", { query: { metric, range } });
}

/**
 * 刷新设置窗口中的全部历史曲线。
 * @return 无返回值
 */
async function refreshHistory() {
  try {
    const [cpu, memory, temperature, disk, network, gpt] = await Promise.all([loadHistory("cpu"), loadHistory("memory"), loadHistory("temperature"), loadHistory("disk"), loadHistory("network"), loadHistory("gpt")]);
    historyPoints.value = { cpu, memory, temperature, disk, network, gpt };
  } catch (error) {
    console.error("无法刷新历史曲线", error);
  }
}

/**
 * 切换历史曲线时间范围。
 * @param range 新时间范围
 * @return 无返回值
 */
function setHistoryRange(range: HistoryRange) {
  historyRange.value = range;
  void refreshHistory();
}

/**
 * 根据设置面板内容高度请求后端调整窗口。
 * @return 无返回值
 */
async function resizeSettingsWindow() {
  if (!isSettingsWindow || !settingsElement.value) return;
  const layoutKey = settingsLayoutKey();
  if (layoutKey === lastSettingsLayoutKey) return;
  lastSettingsLayoutKey = layoutKey;
  try {
    await invoke("resize_settings_window", { height: settingsElement.value.scrollHeight + 10 });
  } catch (error) {
    console.error("无法调整设置窗口尺寸", error);
  }
}

/**
 * 延迟到 DOM 更新后再调整设置窗口尺寸。
 * @return 无返回值
 */
function scheduleSettingsResize() {
  if (!isSettingsWindow) return;
  void nextTick(() => resizeSettingsWindow());
}

/**
 * 将历史点转换为 SVG 折线坐标。
 * @param points 历史点
 * @param secondary 是否使用第二条数据
 * @param percent 是否按百分比缩放
 * @return SVG points 属性文本
 */
function sparklinePoints(points: HistoryPoint[], secondary = false, percent = true) {
  if (points.length === 0) return "";
  const width = 250;
  const height = 42;
  const values = points.map((point) => secondary ? point.secondary_value ?? 0 : point.value);
  const maxValue = percent ? 100 : Math.max(1, ...values);
  return values.map((value, index) => {
    const x = points.length === 1 ? width : index / (points.length - 1) * width;
    const y = height - Math.min(1, Math.max(0, value / maxValue)) * height;
    return `${x.toFixed(1)},${y.toFixed(1)}`;
  }).join(" ");
}

/**
 * 判断当前指标是否有可显示的悬浮点。
 * @param metric 指标名称
 * @param points 历史点
 * @return 是否显示悬浮点
 */
function hasHoverPoint(metric: HistoryMetric, points: HistoryPoint[]) {
  return chartHoverPoint.value?.metric === metric && chartHoverPoint.value.index >= 0 && chartHoverPoint.value.index < points.length;
}

/**
 * 计算悬浮点的 SVG 横坐标。
 * @param points 历史点
 * @return SVG 横坐标
 */
function hoverX(points: HistoryPoint[]) {
  const index = chartHoverPoint.value?.index ?? 0;
  if (points.length <= 1) return 250;
  return index / (points.length - 1) * 250;
}

/**
 * 计算悬浮点的 SVG 纵坐标。
 * @param points 历史点
 * @param secondary 是否使用第二条数据
 * @param percent 是否按百分比缩放
 * @return SVG 纵坐标
 */
function hoverY(points: HistoryPoint[], secondary = false, percent = true) {
  const index = chartHoverPoint.value?.index ?? 0;
  const point = points[index];
  if (!point) return 42;
  const values = points.map((item) => secondary ? item.secondary_value ?? 0 : item.value);
  const maxValue = percent ? 100 : Math.max(1, ...values);
  const value = secondary ? point.secondary_value ?? 0 : point.value;
  return 42 - Math.min(1, Math.max(0, value / maxValue)) * 42;
}

/**
 * 读取 GPT 当前显示模式对应的历史值。
 * @param point GPT 历史点
 * @return 当前显示模式下的值
 */
function gptHistoryValue(point: HistoryPoint) {
  return gptChartUseRemaining.value ? point.secondary_value ?? null : point.value;
}

/**
 * 将 GPT 历史点转换为当前显示模式下的曲线点。
 * @return GPT 曲线点
 */
function gptDisplayHistoryPoints() {
  return historyPoints.value.gpt.map((point) => ({
    timestamp: point.timestamp,
    value: gptHistoryValue(point) ?? 0,
    secondary_value: null,
  }));
}

/**
 * 生成曲线边上的刻度文本。
 * @param points 历史点
 * @param formatter 数值格式化函数
 * @param secondary 是否使用第二条数据
 * @param fallback 无历史时显示的当前值
 * @return 刻度文本
 */
function scaleText(points: HistoryPoint[], formatter: (value: number | null) => string, secondary = false, fallback: number | null = null) {
  const values = points.map((point) => secondary ? point.secondary_value ?? null : point.value).filter((value): value is number => value !== null && Number.isFinite(value));
  if (values.length === 0) return formatter(fallback);
  return formatter(Math.max(...values));
}

/**
 * 格式化历史点时间。
 * @param timestamp Unix 秒级时间戳
 * @return 本地时间文本
 */
function formatPointTime(timestamp: number) {
  const date = new Date(timestamp * 1000);
  if (Number.isNaN(date.getTime())) return "时间未知";
  return date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit" });
}

/**
 * 根据鼠标所在 SVG 位置更新图表提示。
 * @param event 鼠标事件
 * @param metric 指标名称
 * @param points 历史点
 * @param formatter 数值格式化函数
 * @param secondary 是否使用第二条数据
 * @return 无返回值
 */
function updateChartTooltip(event: MouseEvent, metric: HistoryMetric, points: HistoryPoint[], formatter: (value: number | null) => string, secondary = false) {
  chartTooltipMetric.value = metric;
  const hover = pointFromMouse(event, points);
  if (!hover) {
    chartHoverPoint.value = null;
    chartTooltip.value = "暂无历史数据";
    return;
  }
  chartHoverPoint.value = { metric, index: hover.index };
  const point = hover.point;
  const value = secondary ? point.secondary_value ?? null : point.value;
  chartTooltip.value = `${formatPointTime(point.timestamp)} · ${formatter(value)}`;
}

/**
 * 根据鼠标所在 SVG 位置读取最近历史点。
 * @param event 鼠标事件
 * @param points 历史点
 * @return 最近历史点或空
 */
function pointFromMouse(event: MouseEvent, points: HistoryPoint[]) {
  if (points.length === 0) return null;
  const rect = (event.currentTarget as SVGSVGElement).getBoundingClientRect();
  const ratio = rect.width <= 0 ? 1 : Math.min(1, Math.max(0, (event.clientX - rect.left) / rect.width));
  const index = Math.min(points.length - 1, Math.max(0, Math.round(ratio * (points.length - 1))));
  return { index, point: points[index] };
}

/**
 * 根据鼠标所在 SVG 位置更新网络图表提示。
 * @param event 鼠标事件
 * @return 无返回值
 */
function updateNetworkTooltip(event: MouseEvent) {
  chartTooltipMetric.value = "network";
  const hover = pointFromMouse(event, historyPoints.value.network);
  if (!hover) {
    chartHoverPoint.value = null;
    chartTooltip.value = "暂无历史数据";
    return;
  }
  chartHoverPoint.value = { metric: "network", index: hover.index };
  const point = hover.point;
  chartTooltip.value = `${formatPointTime(point.timestamp)} · ↑${formatRate(point.value)} ↓${formatRate(point.secondary_value)}`;
}

/**
 * 清空图表提示。
 * @return 无返回值
 */
function clearChartTooltip() {
  chartTooltip.value = "";
  chartTooltipMetric.value = null;
  chartHoverPoint.value = null;
}

/**
 * 打开组件日志文件。
 * @return 无返回值
 */
async function openWidgetLog() {
  try {
    await invoke("open_widget_log");
  } catch (error) {
    console.error("无法打开组件日志", error);
  }
}

/**
 * 退出组件应用。
 * @return 无返回值
 */
async function quitApplication() {
  try {
    await invoke("quit_application");
  } catch (error) {
    console.error("无法退出组件应用", error);
  }
}

/**
 * 隐藏设置窗口。
 * @return 无返回值
 */
async function hideSettings() {
  if (!isSettingsWindow) return;
  try {
    await invoke("hide_settings_window");
  } catch (error) {
    console.error("无法隐藏设置窗口", error);
  }
}

/**
 * 设置窗口失去焦点后隐藏窗口。
 * @return 无返回值
 */
function handleSettingsBlur() {
  window.setTimeout(() => {
    if (!document.hasFocus()) void hideSettings();
  }, 120);
}

/**
 * 初始化任务栏组件模式。
 * @return 无返回值
 */
async function initializeWidget() {
  await initializeConfig();
  startAlertTimer();
  startClockTimer();
  await refreshSystemUsage();
  if (widgetElement.value) {
    resizeObserver = new ResizeObserver(handleWidgetResize);
    resizeObserver.observe(widgetElement.value);
  }
  systemTimer = window.setInterval(refreshSystemUsage, 1500);
}

/**
 * 初始化设置窗口模式。
 * @return 无返回值
 */
async function initializeSettings() {
  await initializeConfig();
  startAlertTimer();
  startClockTimer();
  await refreshSystemUsage();
  await refreshCodexUsage();
  await refreshCodexResetCredits();
  await refreshHistory();
  scheduleSettingsResize();
  systemTimer = window.setInterval(refreshSystemUsage, 1500);
  historyTimer = window.setInterval(refreshHistory, 5000);
  window.addEventListener("blur", handleSettingsBlur);
}

/**
 * 清理页面卸载时的事件监听器、定时器和尺寸监听器。
 * @return 无返回值
 */
function cleanupWidget() {
  resizeObserver?.disconnect();
  unlistenConfig?.();
  if (systemTimer !== undefined) window.clearInterval(systemTimer);
  if (historyTimer !== undefined) window.clearInterval(historyTimer);
  if (alertTimer !== undefined) window.clearInterval(alertTimer);
  if (clockTimer !== undefined) window.clearInterval(clockTimer);
  window.removeEventListener("blur", handleSettingsBlur);
  void invoke("set_tray_flash_visible", { visible: true });
  stopCodexTimer();
}

onMounted(() => {
  if (isSettingsWindow) {
    void initializeSettings();
  } else {
    void initializeWidget();
  }
});
onUnmounted(cleanupWidget);
</script>

<template>
  <main v-if="!isSettingsWindow" ref="widgetElement" class="widget" aria-label="系统资源与 GPT 用量">
    <button v-if="config.cpu_visible" type="button" class="item metric metric--cpu" title="打开任务管理器；可在任务管理器设置中将默认启动页设为性能" @click="openTaskManager">
      <span class="item__label">CPU</span><strong>{{ cpuText }}</strong>
    </button>
    <button v-if="config.memory_visible" type="button" class="item metric metric--memory" title="打开任务管理器；可在任务管理器设置中将默认启动页设为性能" @click="openTaskManager">
      <span class="item__label">内存</span><strong>{{ memoryText }}</strong>
    </button>
    <section v-if="config.temperature_visible" class="item metric--temperature"><span class="item__label">温度</span><strong>{{ temperatureText }}</strong></section>
    <section v-if="config.disk_visible" class="item metric--disk"><span class="item__label">磁盘</span><strong>{{ diskText }}</strong></section>
    <section v-if="config.upload_visible" class="item metric--upload"><span class="item__label">上行</span><strong>{{ uploadText }}</strong></section>
    <section v-if="config.download_visible" class="item metric--download"><span class="item__label">下行</span><strong>{{ downloadText }}</strong></section>
    <section v-if="config.gpt_visible" class="item codex" :class="{ 'codex--error': codexError && !codexUsage, 'codex--stale': codexStale }" :title="codexStale ? `Codex 刷新失败，当前显示上次成功数据：${codexError}` : codexError || `Codex ${codexDisplayName}，每 ${config.gpt_refresh_seconds} 秒刷新一次`" :aria-label="`GPT Codex ${codexDisplayName}`">
      <span class="item__label">GPT</span><strong>{{ codexText }}</strong>
    </section>
    <span v-if="allMetricsHidden" class="empty-hint">托盘打开设置</span>
  </main>

  <main v-else ref="settingsElement" class="settings-shell" aria-label="任务栏组件设置">
    <section class="settings-grid">
      <article class="controls-panel">
        <div class="compact-line">
          <button :class="{ active: config.position === 'left' }" @click="updateConfig({ position: 'left' })">靠左</button>
          <button :class="{ active: config.position === 'right' }" @click="updateConfig({ position: 'right' })">靠右</button>
          <div style="flex: 100;"></div>
          <a role="button" tabindex="0" @click="openWidgetLog" @keydown.enter="openWidgetLog()">打开日志</a>
          <a role="button" tabindex="0" class="danger-link" @click="quitApplication" @keydown.enter="quitApplication()">退出程序</a>
        </div>
        <div class="check-grid">
          <label><input type="checkbox" :checked="config.cpu_visible" @change="setMetricVisible('cpu_visible', $event)" /> CPU</label>
          <label><input type="checkbox" :checked="config.memory_visible" @change="setMetricVisible('memory_visible', $event)" /> 内存</label>
          <label><input type="checkbox" :checked="config.temperature_visible" @change="setMetricVisible('temperature_visible', $event)" /> 温度</label>
          <label><input type="checkbox" :checked="config.disk_visible" @change="setMetricVisible('disk_visible', $event)" /> 磁盘</label>
          <label><input type="checkbox" :checked="networkVisible" @change="setNetworkVisible(checkedFromEvent($event))" /> 网络</label>
          <label><input type="checkbox" :checked="config.gpt_visible" @change="setMetricVisible('gpt_visible', $event)" /> GPT</label>
        </div>
      </article>

      <article v-if="config.cpu_visible" class="panel chart-card accent-cpu">
        <div class="card-head"><span>CPU</span><strong>{{ cpuText }}</strong></div>
        <div class="chart-row"><svg viewBox="0 0 250 46" class="sparkline" @mousemove="updateChartTooltip($event, 'cpu', historyPoints.cpu, formatPercent)" @mouseleave="clearChartTooltip"><g class="chart-grid"><line x1="0" y1="10" x2="250" y2="10" /><line x1="0" y1="24" x2="250" y2="24" /><line x1="0" y1="38" x2="250" y2="38" /></g><polyline :points="sparklinePoints(historyPoints.cpu)" /><g v-if="hasHoverPoint('cpu', historyPoints.cpu)" class="chart-hover"><line :x1="hoverX(historyPoints.cpu)" y1="0" :x2="hoverX(historyPoints.cpu)" y2="42" /><circle :cx="hoverX(historyPoints.cpu)" :cy="hoverY(historyPoints.cpu)" r="3.4" /></g></svg><span class="scale-value">{{ scaleText(historyPoints.cpu, formatPercent, false, cpuPercent) }}</span></div>
        <div v-if="chartTooltipMetric === 'cpu'" class="chart-tooltip">{{ chartTooltip }}</div>
      </article>

      <article v-if="config.memory_visible" class="panel chart-card accent-memory">
        <div class="card-head"><span>内存</span><strong>{{ memoryText }}</strong></div>
        <div class="chart-row"><svg viewBox="0 0 250 46" class="sparkline" @mousemove="updateChartTooltip($event, 'memory', historyPoints.memory, formatPercent)" @mouseleave="clearChartTooltip"><g class="chart-grid"><line x1="0" y1="10" x2="250" y2="10" /><line x1="0" y1="24" x2="250" y2="24" /><line x1="0" y1="38" x2="250" y2="38" /></g><polyline :points="sparklinePoints(historyPoints.memory)" /><g v-if="hasHoverPoint('memory', historyPoints.memory)" class="chart-hover"><line :x1="hoverX(historyPoints.memory)" y1="0" :x2="hoverX(historyPoints.memory)" y2="42" /><circle :cx="hoverX(historyPoints.memory)" :cy="hoverY(historyPoints.memory)" r="3.4" /></g></svg><span class="scale-value">{{ scaleText(historyPoints.memory, formatPercent, false, memoryPercent) }}</span></div>
        <div v-if="chartTooltipMetric === 'memory'" class="chart-tooltip">{{ chartTooltip }}</div>
      </article>

      <article v-if="config.temperature_visible" class="panel chart-card accent-temperature">
        <div class="card-head"><span>温度</span><strong>{{ temperatureText }}</strong></div>
        <div class="chart-row"><svg viewBox="0 0 250 46" class="sparkline" @mousemove="updateChartTooltip($event, 'temperature', historyPoints.temperature, formatTemperature)" @mouseleave="clearChartTooltip"><g class="chart-grid"><line x1="0" y1="10" x2="250" y2="10" /><line x1="0" y1="24" x2="250" y2="24" /><line x1="0" y1="38" x2="250" y2="38" /></g><polyline :points="sparklinePoints(historyPoints.temperature)" /><g v-if="hasHoverPoint('temperature', historyPoints.temperature)" class="chart-hover"><line :x1="hoverX(historyPoints.temperature)" y1="0" :x2="hoverX(historyPoints.temperature)" y2="42" /><circle :cx="hoverX(historyPoints.temperature)" :cy="hoverY(historyPoints.temperature)" r="3.4" /></g></svg><span class="scale-value">{{ scaleText(historyPoints.temperature, formatTemperature, false, temperatureCelsius) }}</span></div>
        <div v-if="chartTooltipMetric === 'temperature'" class="chart-tooltip">{{ chartTooltip }}</div>
      </article>

      <article v-if="config.disk_visible" class="panel chart-card accent-disk">
        <div class="card-head"><span>磁盘</span><strong>{{ diskText }}</strong></div>
        <div class="chart-row"><svg viewBox="0 0 250 46" class="sparkline" @mousemove="updateChartTooltip($event, 'disk', historyPoints.disk, formatPercent)" @mouseleave="clearChartTooltip"><g class="chart-grid"><line x1="0" y1="10" x2="250" y2="10" /><line x1="0" y1="24" x2="250" y2="24" /><line x1="0" y1="38" x2="250" y2="38" /></g><polyline :points="sparklinePoints(historyPoints.disk)" /><g v-if="hasHoverPoint('disk', historyPoints.disk)" class="chart-hover"><line :x1="hoverX(historyPoints.disk)" y1="0" :x2="hoverX(historyPoints.disk)" y2="42" /><circle :cx="hoverX(historyPoints.disk)" :cy="hoverY(historyPoints.disk)" r="3.4" /></g></svg><span class="scale-value">{{ scaleText(historyPoints.disk, formatPercent, false, diskPercent) }}</span></div>
        <div v-if="chartTooltipMetric === 'disk'" class="chart-tooltip">{{ chartTooltip }}</div>
      </article>

      <article v-if="networkVisible" class="panel chart-card accent-network">
        <div class="card-head"><span>网络</span><strong>↑{{ uploadText }} ↓{{ downloadText }}</strong></div>
        <div class="chart-row"><svg viewBox="0 0 250 46" class="sparkline network-line" @mousemove="updateNetworkTooltip" @mouseleave="clearChartTooltip"><g class="chart-grid"><line x1="0" y1="10" x2="250" y2="10" /><line x1="0" y1="24" x2="250" y2="24" /><line x1="0" y1="38" x2="250" y2="38" /></g><polyline class="upload-line" :points="sparklinePoints(historyPoints.network, false, false)" /><polyline class="download-line" :points="sparklinePoints(historyPoints.network, true, false)" /><g v-if="hasHoverPoint('network', historyPoints.network)" class="chart-hover"><line :x1="hoverX(historyPoints.network)" y1="0" :x2="hoverX(historyPoints.network)" y2="42" /><circle class="upload-dot" :cx="hoverX(historyPoints.network)" :cy="hoverY(historyPoints.network, false, false)" r="3.4" /><circle class="download-dot" :cx="hoverX(historyPoints.network)" :cy="hoverY(historyPoints.network, true, false)" r="3.4" /></g></svg><span class="scale-value">↑{{ scaleText(historyPoints.network, formatRate, false, uploadBytesPerSecond) }}<br />↓{{ scaleText(historyPoints.network, formatRate, true, downloadBytesPerSecond) }}</span></div>
        <div v-if="chartTooltipMetric === 'network'" class="chart-tooltip">{{ chartTooltip }}</div>
      </article>

      <article v-if="config.gpt_visible" class="panel chart-card accent-gpt">
        <div class="card-head"><span>GPT / Codex</span><div class="compact-line"><button :class="{ active: historyRange === 'minute' }" @click="setHistoryRange('minute')">分钟</button><button :class="{ active: historyRange === 'hour' }" @click="setHistoryRange('hour')">小时</button></div><strong>{{ codexDisplayName }} {{ codexChartText }}</strong></div>
        <div class="chart-row"><svg viewBox="0 0 250 46" class="sparkline" @mousemove="updateChartTooltip($event, 'gpt', gptDisplayHistoryPoints(), formatPercent)" @mouseleave="clearChartTooltip"><g class="chart-grid"><line x1="0" y1="10" x2="250" y2="10" /><line x1="0" y1="24" x2="250" y2="24" /><line x1="0" y1="38" x2="250" y2="38" /></g><polyline :points="sparklinePoints(gptDisplayHistoryPoints())" /><g v-if="hasHoverPoint('gpt', gptDisplayHistoryPoints())" class="chart-hover"><line :x1="hoverX(gptDisplayHistoryPoints())" y1="0" :x2="hoverX(gptDisplayHistoryPoints())" y2="42" /><circle :cx="hoverX(gptDisplayHistoryPoints())" :cy="hoverY(gptDisplayHistoryPoints())" r="3.4" /></g></svg><span class="scale-value">{{ scaleText(gptDisplayHistoryPoints(), formatPercent, false, gptChartUseRemaining ? codexRemaining : codexRemaining === null ? null : 100 - codexRemaining) }}</span></div>
        <div v-if="chartTooltipMetric === 'gpt'" class="chart-tooltip">{{ chartTooltip }}</div>
      </article>

      <article class="panel gpt-panel">
        <h2>GPT 设置</h2>
        <div class="compact-line"><button :class="{ active: config.gpt_display_mode === 'remaining' }" @click="updateConfig({ gpt_display_mode: 'remaining' })">余量</button><button :class="{ active: config.gpt_display_mode === 'used' }" @click="updateConfig({ gpt_display_mode: 'used' })">用量</button><select :value="config.gpt_refresh_seconds" @change="updateConfig({ gpt_refresh_seconds: numberFromEvent($event) })"><option value="30">30 秒</option><option value="60">1 分钟</option><option value="180">3 分钟</option><option value="300">5 分钟</option><option value="600">10 分钟</option></select><span class="last-updated">{{ codexLastUpdatedText }}</span><button :disabled="codexRefreshing" @click="refreshCodexUsage">{{ codexRefreshing ? '刷新中' : '手动刷新' }}</button></div>
        <input :value="config.gpt_proxy_url" placeholder="代理 http://127.0.0.1:7890" @change="updateConfig({ gpt_proxy_url: textFromEvent($event) })" />
        <div class="reset-box"><span>重置 {{ codexResetCredits?.available_count ?? 0 }} 次 · 过期 {{ formatIsoTime(codexResetCredits?.credits?.[0]?.expires_at ?? null) }}</span><button disabled title="官方执行重置接口未确认，当前只保留界面">手动重置未启用</button><p v-if="resetError" class="error-text">{{ resetError }}</p></div>
      </article>
    </section>
    <footer class="settings-links"></footer>
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
.item__label { color: #aab4c2; font-size: 10px; font-weight: 650; letter-spacing: 0.04em; }
.item strong { font-size: 13px; font-variant-numeric: tabular-nums; line-height: 1; }
.metric--cpu strong { color: #60A5FA; }
.metric--memory strong { color: #A78BFA; }
.codex strong { color: #E879F9; }
.codex--error strong { color: #ff9090; }
.codex--stale strong { color: #d8b56a; }
.metric--temperature strong { color: #F87171; }
.metric--disk strong { color: #34D399; }
.metric--upload strong { color: #FB923C; }
.metric--download strong { color: #22D3EE; }
.empty-hint { padding: 0 10px; color: #aab4c2; font-size: 12px; }
.settings-shell {
  width: 100%;
  height: 100vh;
  padding: 8px;
  overflow: hidden;
  overflow-y: scroll;
  color: #ecf4ff;
  font-size: 11px;
  background:
    radial-gradient(circle at 10% 4%, rgba(76, 130, 255, 0.18), transparent 25%),
    linear-gradient(135deg, #07111d 0%, #0d1726 54%, #10131d 100%);
  scrollbar-width: thin;
  scrollbar-color: rgba(120, 160, 220, 0.45) transparent;
}

.settings-shell::-webkit-scrollbar { width: 6px; }
.settings-shell::-webkit-scrollbar-track { background: transparent; }
.settings-shell::-webkit-scrollbar-thumb { background: rgba(120, 160, 220, 0.45); border-radius: 6px; }
.settings-shell::-webkit-scrollbar-thumb:hover { background: rgba(120, 160, 220, 0.7); }
.settings-grid { display: grid; grid-template-columns: 1fr; gap: 5px; }
.panel { min-height: 0; padding: 6px 8px; border: 1px solid rgba(70, 255, 166, 0.16); border-radius: 6px; background: rgba(2, 6, 4, 0.88); box-shadow: inset 0 0 22px rgba(66, 255, 157, .045); }
.chart-card { position: relative; overflow: hidden; font-family: "Cascadia Mono", Consolas, "Microsoft YaHei UI", monospace; }
.card-head { display: flex; justify-content: space-between; align-items: baseline; gap: 8px; margin-bottom: 2px; white-space: nowrap; }
.card-head span { color: #6cffb5; font-size: 9px; font-weight: 800; letter-spacing: .05em; text-transform: uppercase; }
.card-head strong { color: #d7ffe9; font-size: 13px; font-variant-numeric: tabular-nums; }
.chart-row { display: grid; grid-template-columns: minmax(0, 1fr) 42px; gap: 6px; align-items: center; }
.sparkline { width: 100%; height: 42px; overflow: visible; cursor: crosshair; }
.sparkline polyline { fill: none; stroke: currentColor; stroke-width: 2; stroke-linecap: round; stroke-linejoin: round; filter: drop-shadow(0 0 5px currentColor); }
.chart-grid line { stroke: rgba(66, 255, 157, .14); stroke-width: 1; shape-rendering: crispEdges; }
.chart-hover line { stroke: rgba(215, 255, 233, .42); stroke-width: 1; stroke-dasharray: 2 2; shape-rendering: crispEdges; }
.chart-hover circle { fill: #d7ffe9; stroke: currentColor; stroke-width: 2; filter: drop-shadow(0 0 5px currentColor); }
.chart-hover .upload-dot { color: #FB923C; }
.chart-hover .download-dot { color: #22D3EE; }
.scale-value { color: #86dcae; font-size: 9px; line-height: 1.25; text-align: right; font-variant-numeric: tabular-nums; white-space: nowrap; }
.chart-tooltip { position: absolute; right: 50px; bottom: 6px; z-index: 2; padding: 3px 6px; border: 1px solid rgba(108, 255, 181, .28); border-radius: 4px; color: #d7ffe9; font-size: 9px; line-height: 1; font-variant-numeric: tabular-nums; white-space: nowrap; background: rgba(0, 8, 4, .88); box-shadow: 0 0 12px rgba(66, 255, 157, .1); }
.last-updated { color: #93a4ba; font-size: 9px; white-space: nowrap; }
.accent-cpu { color: #60A5FA; }
.accent-memory { color: #A78BFA; }
.accent-temperature { color: #F87171; }
.accent-disk { color: #34D399; }
.accent-network { color: #34D399; }
.accent-gpt { color: #E879F9; }
.upload-line { color: #FB923C; }
.download-line { color: #22D3EE; opacity: .9; }
.compact-line { display: flex; align-items: center; gap: 5px; margin-top: 4px; color: #aebbd0; font-size: 10px; white-space: nowrap; margin-bottom: 10px; }
.compact-line button { border: 0; border-radius: 999px; padding: 3px 7px; color: #b7c5d8; font-size: 10px; background: rgba(255,255,255,.07); cursor: pointer; }
.compact-line button.active { color: #06111d; background: #7ee0d1; }
.compact-line a { color: #8ebcff; font-size: 10px; text-decoration: underline; cursor: pointer; white-space: nowrap; }
.compact-line a.danger-link { color: #ff9090; }
.controls-panel, .gpt-panel { grid-column: auto; }
.panel h2 { margin: 0 0 5px; color: #dce8f7; font-size: 11px; white-space: nowrap; }
.check-grid { display: grid; grid-template-columns: repeat(6, auto); gap: 5px 8px; align-items: center; }
.controls-panel label { display: inline-flex; align-items: center; gap: 3px; color: #d9e5f4; font-size: 10px; white-space: nowrap; }
.controls-panel input { width: 11px; height: 11px; accent-color: #7ee0d1; }
select, input { width: 100%; min-width: 0; border: 1px solid rgba(180, 205, 230, .16); border-radius: 8px; padding: 4px 6px; color: #ecf4ff; font-size: 10px; background: rgba(4, 10, 18, .58); outline: none; }
.gpt-panel select { width: 78px; }
select:focus, input:focus { border-color: #7ee0d1; box-shadow: 0 0 0 2px rgba(126,224,209,.14); }
.reset-box { display: grid; gap: 4px; margin-top: 5px; padding: 5px 7px; border-radius: 8px; background: rgba(255,255,255,.055); color: #c2d0e2; font-size: 10px; }
.reset-box button { width: max-content; border: 0; border-radius: 999px; padding: 3px 7px; color: #76869a; font-size: 10px; background: rgba(255,255,255,.1); }
.error-text { margin: 0; color: #ff9090; font-size: 10px; }
.settings-links { display: flex; justify-content: flex-end; gap: 10px; padding: 5px 2px 0; font-size: 10px; }
.settings-links a { color: #8ebcff; text-decoration: underline; cursor: pointer; white-space: nowrap; }
.settings-links .danger-link { color: #ff9090; }

@media (prefers-reduced-motion: reduce) {
  * { transition: none !important; }
}
</style>
