<script setup lang="ts">
import { invoke } from "@tauri-apps/api/core";
import { listen, type Event, type UnlistenFn } from "@tauri-apps/api/event";
import { computed, nextTick, onMounted, onUnmounted, ref } from "vue";

type WidgetPosition = "left" | "right";
type GptDisplayMode = "remaining" | "used";

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
const codexError = ref("");
const codexStale = ref(false);
const widgetElement = ref<HTMLElement | null>(null);
let systemTimer: number | undefined;
let codexTimer: number | undefined;
let resizeObserver: ResizeObserver | undefined;
let unlistenConfig: UnlistenFn | undefined;
let lastReportedWidth = 0;

const cpuText = computed(() => formatPercent(cpuPercent.value));
const memoryText = computed(() => formatPercent(memoryPercent.value));
const temperatureText = computed(() => formatTemperature(temperatureCelsius.value));
const diskText = computed(() => formatPercent(diskPercent.value));
const uploadText = computed(() => formatRate(uploadBytesPerSecond.value));
const downloadText = computed(() => formatRate(downloadBytesPerSecond.value));
const codexText = computed(() => {
  const remaining = codexUsage.value?.weekly_remaining_percent
    ?? codexUsage.value?.primary_remaining_percent
    ?? null;
  const value = remaining !== null && config.value.gpt_display_mode === "used"
    ? 100 - remaining
    : remaining;
  return formatPercent(value);
});
const codexDisplayName = computed(() =>
  config.value.gpt_display_mode === "used" ? "已用量" : "剩余额度",
);
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
  return value === null || !Number.isFinite(value)
    ? "--"
    : `${Math.round(Math.min(100, Math.max(0, value)))}%`;
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
function formatRate(value: number) {
  if (!Number.isFinite(value) || value < 0) return "--";
  if (value >= 1024 * 1024) return `${(value / 1024 / 1024).toFixed(1)}M/s`;
  if (value >= 1024) return `${(value / 1024).toFixed(1)}K/s`;
  return `${Math.round(value)}B/s`;
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
 * 从 Rust 后端刷新 Codex 的 5 小时和每周剩余额度。
 * @return 无返回值
 */
async function refreshCodexUsage() {
  try {
    codexUsage.value = await invoke<CodexUsage>("get_codex_usage");
    codexError.value = "";
    codexStale.value = false;
  } catch (error) {
    codexError.value = String(error);
    codexStale.value = codexUsage.value !== null;
    console.error("无法刷新 GPT/Codex 用量", error);
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
 * 按当前配置立即刷新 GPT 并重建定时器，隐藏时停止刷新。
 * @return 无返回值
 */
function restartCodexTimer() {
  stopCodexTimer();
  if (!config.value.gpt_visible) return;
  void refreshCodexUsage();
  codexTimer = window.setInterval(
    refreshCodexUsage,
    config.value.gpt_refresh_seconds * 1000,
  );
}

/**
 * 将 Vue 页面宽度同步给原生任务栏子窗口。
 * @return 无返回值
 */
async function syncWindowWidth() {
  const element = widgetElement.value;
  if (!element) return;

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
 * 响应后端托盘配置变更事件。
 * @param event 携带最新配置的 Tauri 事件
 * @return 无返回值
 */
function handleConfigChanged(event: Event<WidgetConfig>) {
  void applyConfig(event.payload);
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
 * 初始化配置、系统资源轮询和页面尺寸监听。
 * @return 无返回值
 */
async function initializeWidget() {
  await initializeConfig();
  await refreshSystemUsage();

  if (widgetElement.value) {
    resizeObserver = new ResizeObserver(handleWidgetResize);
    resizeObserver.observe(widgetElement.value);
  }
  systemTimer = window.setInterval(refreshSystemUsage, 1500);
}

/**
 * 清理页面卸载时的事件监听器、定时器和尺寸监听器。
 * @return 无返回值
 */
function cleanupWidget() {
  resizeObserver?.disconnect();
  unlistenConfig?.();
  if (systemTimer !== undefined) {
    window.clearInterval(systemTimer);
  }
  stopCodexTimer();
}

onMounted(initializeWidget);
onUnmounted(cleanupWidget);
</script>

<template>
  <main ref="widgetElement" class="widget" aria-label="系统资源与 GPT 用量">
    <button
      v-if="config.cpu_visible"
      type="button"
      class="item metric metric--cpu"
      title="打开任务管理器；可在任务管理器设置中将默认启动页设为性能"
      @click="openTaskManager"
    >
      <span class="item__label">CPU</span><strong>{{ cpuText }}</strong>
    </button>

    <button
      v-if="config.memory_visible"
      type="button"
      class="item metric metric--memory"
      title="打开任务管理器；可在任务管理器设置中将默认启动页设为性能"
      @click="openTaskManager"
    >
      <span class="item__label">内存</span><strong>{{ memoryText }}</strong>
    </button>

    <section v-if="config.temperature_visible" class="item metric--temperature">
      <span class="item__label">温度</span><strong>{{ temperatureText }}</strong>
    </section>

    <section v-if="config.disk_visible" class="item metric--disk">
      <span class="item__label">磁盘</span><strong>{{ diskText }}</strong>
    </section>

    <section v-if="config.upload_visible" class="item metric--upload">
      <span class="item__label">上行</span><strong>{{ uploadText }}</strong>
    </section>

    <section v-if="config.download_visible" class="item metric--download">
      <span class="item__label">下行</span><strong>{{ downloadText }}</strong>
    </section>

    <section
      v-if="config.gpt_visible"
      class="item codex"
      :class="{ 'codex--error': codexError && !codexUsage, 'codex--stale': codexStale }"
      :title="codexStale ? `Codex 刷新失败，当前显示上次成功数据：${codexError}` : codexError || `Codex ${codexDisplayName}，每 ${config.gpt_refresh_seconds} 秒刷新一次`"
      :aria-label="`GPT Codex ${codexDisplayName}`"
    >
      <span class="item__label">GPT</span><strong>{{ codexText }}</strong>
    </section>

    <span v-if="allMetricsHidden" class="empty-hint">右键托盘设置</span>
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

* {
  box-sizing: border-box;
}

html,
body,
#app {
  width: 100%;
  height: 100%;
  margin: 0;
  overflow: hidden;
  background: transparent;
}

body {
  user-select: none;
}

button {
  font: inherit;
}
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

.item + .item {
  border-left: 1px solid rgba(255, 255, 255, 0.14);
}

.metric {
  cursor: pointer;
}

.metric:hover {
  background: rgba(255, 255, 255, 0.08);
}

.metric:focus-visible {
  outline: 2px solid #69b8ff;
  outline-offset: 1px;
}

.item__label {
  color: #aab4c2;
  font-size: 10px;
  font-weight: 650;
  letter-spacing: 0.04em;
}

.item strong {
  font-size: 13px;
  font-variant-numeric: tabular-nums;
  line-height: 1;
}

.metric--cpu strong {
  color: #71e2b2;
}

.metric--memory strong {
  color: #7cc2ff;
}

.codex strong {
  color: #e8bdff;
}

.codex--error strong {
  color: #ff9090;
}

.codex--stale strong {
  color: #d8b56a;
}

.metric--temperature strong {
  color: #ffad73;
}

.metric--disk strong {
  color: #f3da78;
}

.metric--upload strong {
  color: #7ee0d1;
}

.metric--download strong {
  color: #8ebcff;
}

.empty-hint {
  padding: 0 10px;
  color: #aab4c2;
  font-size: 12px;
}

@media (prefers-reduced-motion: reduce) {
  * {
    transition: none !important;
  }
}
</style>
