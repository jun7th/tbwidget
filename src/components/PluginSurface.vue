<script setup lang="ts">
import { computed, onUnmounted, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { handlePluginRequest, type PluginRequestContext } from "../plugins/hostRequest";
import { pluginSurfaceUrl } from "../plugins/surfaceUrl";
import type { PluginDescriptor, PluginSurfaceName } from "../plugins/types";

type PluginMessage = {
  tbwidgetPlugin?: boolean;
  pluginId?: string;
  requestId?: number;
  method?: string;
  args?: unknown[];
  width?: number;
  height?: number;
  level?: string;
  message?: string;
};

const props = withDefaults(defineProps<{
  plugin: PluginDescriptor;
  surface: PluginSurfaceName;
  revision?: number;
  requestContext?: PluginRequestContext;
}>(), {
  revision: 0,
  requestContext: () => ({}),
});

const emit = defineEmits<{
  resize: [width: number, height: number];
  load: [];
}>();

const frame = ref<HTMLIFrameElement | null>(null);
let runtimeReady = false;
let runtimeWatchdog: number | undefined;
const src = computed(() => pluginSurfaceUrl(props.plugin, props.surface, props.revision));

function clearRuntimeWatchdog() {
  if (runtimeWatchdog === undefined) return;
  window.clearTimeout(runtimeWatchdog);
  runtimeWatchdog = undefined;
}

watch(src, () => {
  runtimeReady = false;
  clearRuntimeWatchdog();
}, { immediate: true });

function postControl(action: string) {
  frame.value?.contentWindow?.postMessage({
    tbwidgetHostControl: true,
    pluginId: props.plugin.id,
    action,
  }, "*");
}

function measure() {
  postControl("measure");
}

async function onMessage(event: MessageEvent<PluginMessage>) {
  const message = event.data;
  if (!message?.tbwidgetPlugin || message.pluginId !== props.plugin.id || !message.method) return;

  // iframe 从自定义协议读取本地 HTML，启动速度可能快于 Vue template ref 建立。
  // listener 在 setup 阶段已经安装；frame ref 可用后再额外校验 source。
  const targetWindow = frame.value?.contentWindow;
  if (targetWindow && event.source !== targetWindow) return;
  const source = event.source as WindowProxy | null;
  if (!source) return;

  if (message.method === "host.log") {
    void invoke("log_plugin_runtime", {
      pluginId: props.plugin.id,
      surface: props.surface,
      level: String(message.level || "info"),
      message: String(message.message || ""),
    }).catch(error => console.error("[plugin-surface] log_plugin_runtime failed", error));
    return;
  }

  if (message.method === "host.runtimeReady") {
    runtimeReady = true;
    clearRuntimeWatchdog();
    void invoke("log_plugin_runtime", {
      pluginId: props.plugin.id,
      surface: props.surface,
      level: "lifecycle",
      message: "插件运行时启动完成，宿主握手成功",
    }).catch(() => undefined);
    source.postMessage({
      tbwidgetHostControl: true,
      pluginId: props.plugin.id,
      action: "runtimeAck",
    }, "*");
    return;
  }

  if (message.method === "host.resize") {
    emit("resize", Math.max(1, Math.ceil(Number(message.width) || 1)), Math.max(1, Math.ceil(Number(message.height) || 1)));
    return;
  }

  if (!message.requestId) return;
  try {
    const value = await handlePluginRequest(
      props.plugin,
      message.method,
      Array.isArray(message.args) ? message.args : [],
      props.requestContext,
    );
    source.postMessage({
      tbwidgetHost: true,
      pluginId: props.plugin.id,
      requestId: message.requestId,
      ok: true,
      value,
    }, "*");
  } catch (error) {
    source.postMessage({
      tbwidgetHost: true,
      pluginId: props.plugin.id,
      requestId: message.requestId,
      ok: false,
      error: String(error),
    }, "*");
  }
}

function onLoad() {
  clearRuntimeWatchdog();
  void invoke("log_plugin_runtime", {
    pluginId: props.plugin.id,
    surface: props.surface,
    level: "lifecycle",
    message: `插件页面加载完成 url=${src.value}`,
  }).catch(() => undefined);

  // 首次 navigation 可能早于父页面 mount 生命周期。要求 runtime 重新握手，
  // 配合 runtimeAck 后再放行插件 RPC，避免第一个 storage/system 请求丢失。
  postControl("handshake");
  postControl("measure");

  if (!runtimeReady) runtimeWatchdog = window.setTimeout(() => {
    runtimeWatchdog = undefined;
    if (runtimeReady) return;
    const message = `插件页面运行时未启动 plugin=${props.plugin.id} surface=${props.surface} url=${src.value}`;
    console.error(`[plugin-surface] ${message}`);
    void invoke("log_frontend_error", { message }).catch(() => undefined);
  }, 1500);
  emit("load");
  window.requestAnimationFrame(measure);
}

// 必须在 iframe 创建前监听。放到 onMounted 会与极速本地导航产生竞态。
window.addEventListener("message", onMessage);
onUnmounted(() => {
  window.removeEventListener("message", onMessage);
  clearRuntimeWatchdog();
});

defineExpose({ frame, measure });
</script>

<template>
  <iframe
    ref="frame"
    :src="src"
    sandbox="allow-scripts"
    @load="onLoad"
  />
</template>
