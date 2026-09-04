<script setup lang="ts">
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { nextTick, onMounted, onUnmounted, ref } from "vue";
import PluginPanels from "../components/PluginPanels.vue";
import { useWidgetConfig } from "../composables/useWidgetConfig";

const root = ref<HTMLElement | null>(null);
const rightReserved = ref(0);
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

onMounted(async () => {
  await loadConfig();
  document.documentElement.classList.add("widget-window");
  resizeObserver = new ResizeObserver(() => void syncWindow());
  if (root.value) resizeObserver.observe(root.value);
  window.addEventListener("tbwidget-interactive-regions-changed", onRegionsChanged);
  unlisteners.push(await listen("display-environment-changed", () => void syncWindow()));
  await syncWindow();
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
    <PluginPanels />
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
</style>
