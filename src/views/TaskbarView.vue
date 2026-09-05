<script setup lang="ts">
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { nextTick, onMounted, onUnmounted, ref } from "vue";
import PluginPanels from "../components/PluginPanels.vue";
import { useWidgetConfig } from "../composables/useWidgetConfig";

const root = ref<HTMLElement | null>(null);
const rightReserved = ref(0);
const debugMode = ref(false);
const { config, load: loadConfig, dispose: disposeConfig } = useWidgetConfig();
const unlisteners: UnlistenFn[] = [];
let resizeObserver: ResizeObserver | undefined;
let lastWidth = 0;

async function syncInteractiveRegions() {
  const elements = [...document.querySelectorAll<HTMLElement>("[data-taskbar-interactive]")]
    .filter(element => { const rect = element.getBoundingClientRect(); return rect.width > 0 && rect.height > 0; });
  const regions = elements.map((element, index) => {
    const rect = element.getBoundingClientRect();
    return {
      id: element.dataset.taskbarRegionId || `dom:${index}`,
      x: rect.left,
      y: rect.top,
      width: rect.width,
      height: rect.height,
    };
  });
  await invoke("set_taskbar_interactive_regions", { regions, scaleFactor: window.devicePixelRatio || 1 }).catch(() => undefined);
}

async function syncWindow() {
  if (config.value.position === "right") {
    rightReserved.value = Math.max(0, await invoke<number>("get_taskbar_right_reserved_width").catch(() => 0));
  } else {
    rightReserved.value = 0;
  }
  await nextTick();
  const width = Math.max(1, window.innerWidth);
  if (width !== lastWidth) {
    await invoke("set_widget_width", { width }).catch(() => undefined);
    lastWidth = width;
  }
  await syncInteractiveRegions();
}

function onRegionsChanged() { void syncWindow(); }

function pluginDiagState() {
  try {
    const diag = (window as Window & { __TBWIDGET_PLUGIN_DIAG__?: Record<string, unknown> }).__TBWIDGET_PLUGIN_DIAG__;
    return diag ? JSON.stringify(diag) : "<missing>";
  } catch (error) {
    return `<serialize-failed:${String(error)}>`;
  }
}

function elementState(name: string, element: Element | null) {
  if (!(element instanceof HTMLElement)) return `${name}=<missing>`;
  const rect = element.getBoundingClientRect();
  const style = getComputedStyle(element);
  return `${name}[rect=${rect.left.toFixed(1)},${rect.top.toFixed(1)},${rect.width.toFixed(1)}x${rect.height.toFixed(1)} display=${style.display} visibility=${style.visibility} opacity=${style.opacity} bg=${style.backgroundColor} overflow=${style.overflow}]`;
}

// function installProductionVisualProbe() {
//   if (!import.meta.env.PROD || document.getElementById("__taskbar_visual_probe__")) return;

//   const probe = document.createElement("div");
//   probe.id = "__taskbar_visual_probe__";
//   probe.textContent = "TASKBAR WEBVIEW TEST";
//   probe.style.cssText = [
//     "position:fixed!important",
//     "left:100px!important",
//     "top:4px!important",
//     "width:300px!important",
//     "height:40px!important",
//     "background:#ff0000!important",
//     "color:#ffffff!important",
//     "z-index:2147483647!important",
//     "display:flex!important",
//     "align-items:center!important",
//     "justify-content:center!important",
//     "font:600 16px/1 sans-serif!important",
//     "opacity:1!important",
//     "visibility:visible!important",
//     "pointer-events:none!important",
//   ].join(";");
//   document.body.appendChild(probe);
// }

async function logDisplayState(stage: string) {
  const container = document.querySelector<HTMLElement>(".taskbar-plugin-container");
  const slots = document.querySelectorAll(".plugin-slot");
  const message = [
    `stage=${stage}`,
    `env=${import.meta.env.MODE}/${import.meta.env.PROD ? "prod" : "dev"}`,
    `ready=${document.readyState}`,
    `href=${location.protocol}//${location.host}${location.pathname}${location.search}`,
    `viewport=${window.innerWidth}x${window.innerHeight}`,
    `dpr=${window.devicePixelRatio}`,
    `slots=${slots.length}`,
    `pluginDiag=${pluginDiagState()}`,
    elementState("html", document.documentElement),
    elementState("body", document.body),
    elementState("app", document.getElementById("app")),
    elementState("root", root.value),
    elementState("container", container),
    elementState("probe", document.getElementById("__taskbar_visual_probe__")),
  ].join(" | ");
  await invoke("log_taskbar_ui_debug", { message }).catch(() => undefined);
}

onMounted(async () => {
  // installProductionVisualProbe();
  await loadConfig();
  debugMode.value = await invoke<boolean>("get_debug_mode").catch(() => false);
  document.documentElement.classList.add("widget-window");
  resizeObserver = new ResizeObserver(() => void syncWindow());
  if (root.value) resizeObserver.observe(root.value);
  window.addEventListener("tbwidget-interactive-regions-changed", onRegionsChanged);
  unlisteners.push(await listen("display-environment-changed", () => void syncWindow()));
  await syncWindow();
  if (import.meta.env.DEV || debugMode.value) {
    await nextTick();
    void logDisplayState("mounted-after-sync");
    requestAnimationFrame(() => void logDisplayState("raf"));
    window.setTimeout(() => void logDisplayState("delay-250ms"), 250);
    window.setTimeout(() => void logDisplayState("delay-1000ms"), 1000);
  }
});

onUnmounted(() => {
  resizeObserver?.disconnect();
  window.removeEventListener("tbwidget-interactive-regions-changed", onRegionsChanged);
  unlisteners.forEach(unlisten => unlisten());
  disposeConfig();
});
</script>

<template>
  <main
    ref="root"
    class="taskbar-view"
    :class="`taskbar-view--${config.position}`"
    :style="config.position === 'right' ? { paddingRight: `${rightReserved + 8}px` } : undefined"
  >
    <div class="taskbar-plugin-container" :class="{ 'taskbar-plugin-container--debug': debugMode }">
      <PluginPanels />
    </div>
  </main>
</template>

<style scoped>
.taskbar-view {
  position: relative;
  width: 100vw;
  height: 100vh;
  min-height: 36px;
  display: flex;
  align-items: center;
  padding: 4px 8px;
  overflow: hidden;
  background: transparent;
  white-space: nowrap;
}
.taskbar-view--left { justify-content: flex-start; }
.taskbar-view--right { justify-content: flex-end; }
.taskbar-plugin-container { display:flex; align-items:center; height:36px; width:max-content; border-radius:8px; }
.taskbar-plugin-container--debug { min-width:48px; background:rgba(0, 0, 0, .55); box-shadow:inset 0 0 0 1px rgba(255,255,255,.16); }
.taskbar-plugin-container--debug :deep(.plugin-slot) { background:rgba(0, 0, 0, .28); }
</style>
