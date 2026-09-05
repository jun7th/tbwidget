<script setup lang="ts">
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { nextTick, onMounted, onUnmounted, ref } from "vue";
import PluginSurface from "./PluginSurface.vue";
import type { PluginDescriptor } from "../plugins/types";

type NativeTaskbarPoint = { x: number; y: number };
type NativeTaskbarClick = NativeTaskbarPoint & { regionId: string };
type NativeTaskbarPointerPoll = {
  cursor: NativeTaskbarPoint | null;
  cursorRegionId?: string | null;
  leftButtonDown: boolean;
  leftDowns: NativeTaskbarClick[];
  rightClicks: NativeTaskbarClick[];
};

const plugins = ref<PluginDescriptor[]>([]);
const frameWidths = ref<Record<string, number>>({});
const panelRevisions = ref<Record<string, number>>({});
const hoveredPluginId = ref("");
const draggedPluginId = ref("");
const slots = new Map<string, HTMLElement>();
const unlisteners: UnlistenFn[] = [];
let nativeClickTimer: number | undefined;
let interactiveRegionFrame: number | undefined;
let startupRetryTimer: number | undefined;
let startupRetryCount = 0;
let initialPluginLoadComplete = false;
let nativeClickPolling = false;
let lastPopupPluginId = "";
let lastPopupAt = 0;
let lastPointerZone = "__init__";
let suppressedHoverPluginId = "";
let dragMoved = false;
const INPUT_DEBUG_LOGS = false;
let debugLogsEnabled = import.meta.env.DEV;

type PluginDiagState = {
  mainTs?: string;
  appCreate?: boolean;
  appMounted?: boolean;
  panelsSetup?: boolean;
  panelsMounted?: boolean;
  loadBegin?: number;
  invokeBegin?: number;
  invokeDone?: number;
  invokeError?: string;
  descriptorCount?: number;
  enabledCount?: number;
  renderedPluginCount?: number;
  domSlotCount?: number;
  slotRefCount?: number;
  lastStep?: string;
};

function pluginDiag(): PluginDiagState {
  const host = window as Window & { __TBWIDGET_PLUGIN_DIAG__?: PluginDiagState };
  return (host.__TBWIDGET_PLUGIN_DIAG__ ??= {});
}

function markDiag(step: string, values: Partial<PluginDiagState> = {}) {
  const diag = pluginDiag();
  Object.assign(diag, values, { lastStep: step });
}

markDiag("PluginPanels setup entered", { panelsSetup: true });

function errorText(error: unknown) {
  if (error instanceof Error) return `${error.name}: ${error.message}${error.stack ? ` | stack=${error.stack.replace(/\s+/g, " ")}` : ""}`;
  try { return JSON.stringify(error); } catch { return String(error); }
}

async function uiDebug(message: string) {
  if (!debugLogsEnabled) return;
  console.info(`[plugins-ui] ${message}`);
  try {
    await invoke("log_taskbar_ui_debug", { message: `[plugins-ui] ${message}` });
  } catch (error) {
    console.error("[plugins-ui] log_taskbar_ui_debug failed", error);
  }
}

async function openPluginPopup(plugin: PluginDescriptor, rect: DOMRect) {
  if (!plugin.popup) return;
  const now = performance.now();
  if (plugin.id === lastPopupPluginId && now - lastPopupAt < 300) return;
  lastPopupPluginId = plugin.id;
  lastPopupAt = now;
  try {
    await invoke("show_plugin_popup", { pluginId: plugin.id, anchorX: rect.left, anchorWidth: rect.width });
  } catch (error) {
    console.error(`打开插件 ${plugin.id} 弹窗失败`, error);
  }
}

function pluginByRegionId(regionId?: string | null, requirePopup = false) {
  if (!regionId?.startsWith("plugin:")) return null;
  const pluginId = regionId.slice("plugin:".length);
  return plugins.value.find(item => item.id === pluginId && item.enabled && (!requirePopup || !!item.popup)) ?? null;
}

function pluginAtPoint(point: NativeTaskbarPoint, requirePopup = false) {
  for (const plugin of plugins.value.filter(item => item.enabled && (!requirePopup || !!item.popup))) {
    const slot = slots.get(plugin.id);
    if (!slot) continue;
    const rect = slot.getBoundingClientRect();
    if (point.x >= rect.left && point.x < rect.right && point.y >= rect.top && point.y < rect.bottom) return { plugin, rect };
  }
  return null;
}

function movePluginNearTarget(dragId: string, targetId: string, cursorX: number) {
  if (!dragId || !targetId || dragId === targetId) return false;
  const targetSlot = slots.get(targetId);
  if (!targetSlot) return false;
  const targetRect = targetSlot.getBoundingClientRect();
  const current = [...plugins.value];
  const from = current.findIndex(item => item.id === dragId);
  if (from < 0) return false;
  const [moved] = current.splice(from, 1);
  const targetIndex = current.findIndex(item => item.id === targetId);
  if (targetIndex < 0) return false;
  const insertIndex = cursorX < targetRect.left + targetRect.width / 2 ? targetIndex : targetIndex + 1;
  current.splice(insertIndex, 0, moved);
  if (current.every((item, index) => item.id === plugins.value[index]?.id)) return false;
  plugins.value = current;
  void nextTick().then(requestInteractiveRegionSync);
  return true;
}

async function finishDrag() {
  if (!draggedPluginId.value) return;
  const shouldSaveOrder = dragMoved;
  draggedPluginId.value = "";
  dragMoved = false;
  if (!shouldSaveOrder) return;
  await invoke("set_plugin_order", { pluginIds: plugins.value.map(item => item.id) }).catch(error => {
    console.error("保存插件排序失败", error);
    void loadPlugins();
  });
}

async function pollNativePointer() {
  if (nativeClickPolling) return;
  nativeClickPolling = true;
  try {
    const pointer = await invoke<NativeTaskbarPointerPoll>("poll_taskbar_native_pointer");
    const nativeHoveredPlugin = pluginByRegionId(pointer.cursorRegionId);
    const domHoveredPlugin = !nativeHoveredPlugin && pointer.cursor ? pluginAtPoint(pointer.cursor)?.plugin ?? null : null;
    const rawHoveredPluginId = nativeHoveredPlugin?.id ?? domHoveredPlugin?.id ?? "";
    if (suppressedHoverPluginId && rawHoveredPluginId !== suppressedHoverPluginId) suppressedHoverPluginId = "";
    const nextHoveredPluginId = rawHoveredPluginId === suppressedHoverPluginId ? "" : rawHoveredPluginId;

    if (INPUT_DEBUG_LOGS) {
      const pointerZone = pointer.cursor ? (pointer.cursorRegionId || (nextHoveredPluginId ? `plugin:${nextHoveredPluginId}` : "taskbar-empty")) : "outside";
      if (pointerZone !== lastPointerZone) {
        lastPointerZone = pointerZone;
        const cursor = pointer.cursor ? `(${pointer.cursor.x.toFixed(1)},${pointer.cursor.y.toFixed(1)})` : "<outside>";
        void invoke("log_taskbar_input_debug", { message: `pointer-zone ${pointerZone} cursor=${cursor}` }).catch(() => undefined);
      }
    }

    hoveredPluginId.value = nextHoveredPluginId;

    // 右键只触发 TBWidget 自己的插件 popup，不使用 WebView/Windows 默认上下文菜单。
    for (const click of pointer.rightClicks) {
      const popupPlugin = pluginByRegionId(click.regionId, true);
      const popupSlot = popupPlugin ? slots.get(popupPlugin.id) : null;
      if (popupPlugin && popupSlot) {
        suppressedHoverPluginId = popupPlugin.id;
        hoveredPluginId.value = "";
        void openPluginPopup(popupPlugin, popupSlot.getBoundingClientRect());
      }
    }

    for (const down of pointer.leftDowns) {
      const plugin = pluginByRegionId(down.regionId);
      if (plugin) {
        draggedPluginId.value = plugin.id;
        dragMoved = false;
      }
    }

    if (draggedPluginId.value && pointer.leftButtonDown) {
      const target = pluginByRegionId(pointer.cursorRegionId);
      if (target && target.id !== draggedPluginId.value && pointer.cursor) {
        dragMoved = movePluginNearTarget(draggedPluginId.value, target.id, pointer.cursor.x) || dragMoved;
      }
    } else if (draggedPluginId.value && !pointer.leftButtonDown) {
      const releasedPluginId = draggedPluginId.value;
      const popupPlugin = !dragMoved ? pluginByRegionId(pointer.cursorRegionId, true) : null;
      const popupSlot = popupPlugin && popupPlugin.id === releasedPluginId ? slots.get(popupPlugin.id) : null;
      await finishDrag();
      if (popupPlugin && popupSlot) {
        suppressedHoverPluginId = popupPlugin.id;
        hoveredPluginId.value = "";
        void openPluginPopup(popupPlugin, popupSlot.getBoundingClientRect());
      }
    }
  } catch (error) {
    console.error("读取任务栏原生指针失败", error);
  } finally {
    nativeClickPolling = false;
  }
}

function onPanelResize(pluginId: string, requestedWidth: number) {
  const width = Math.max(24, Math.min(800, Math.ceil(Number(requestedWidth) || 0)));
  if ((frameWidths.value[pluginId] || 48) === width) return;
  frameWidths.value = { ...frameWidths.value, [pluginId]: width };
  void nextTick().then(requestInteractiveRegionSync);
}


function requestInteractiveRegionSync() {
  if (interactiveRegionFrame !== undefined) return;
  interactiveRegionFrame = window.requestAnimationFrame(async () => {
    interactiveRegionFrame = undefined;
    await invoke("set_widget_width", { width: Math.max(1, window.innerWidth) }).catch(() => undefined);
    window.dispatchEvent(new CustomEvent("tbwidget-interactive-regions-changed"));
  });
}

function setSlotRef(pluginId: string, element: unknown) {
  if (element instanceof HTMLElement) {
    if (slots.get(pluginId) === element) return;
    slots.set(pluginId, element);
  } else if (!slots.delete(pluginId)) {
    return;
  }
  markDiag("setSlotRef", { slotRefCount: slots.size, domSlotCount: document.querySelectorAll(".plugin-slot").length });
  requestInteractiveRegionSync();
}

function clearStartupRetry() {
  if (startupRetryTimer === undefined) return;
  window.clearTimeout(startupRetryTimer);
  startupRetryTimer = undefined;
}

function scheduleStartupRetry() {
  if (initialPluginLoadComplete || startupRetryTimer !== undefined || startupRetryCount >= 20) return;
  const delay = Math.min(500, 60 + startupRetryCount * 40);
  startupRetryCount += 1;
  startupRetryTimer = window.setTimeout(() => {
    startupRetryTimer = undefined;
    void loadPlugins();
  }, delay);
}

async function loadPlugins() {
  if (draggedPluginId.value) {
    markDiag("loadPlugins skipped: dragging");
    await uiDebug(`loadPlugins skipped dragged=${draggedPluginId.value}`);
    return;
  }

  const diag = pluginDiag();
  markDiag("loadPlugins entered", { loadBegin: (diag.loadBegin ?? 0) + 1, invokeError: "" });
  await uiDebug(`loadPlugins begin env=${import.meta.env.MODE}/${import.meta.env.PROD ? "prod" : "dev"} href=${location.href}`);

  try {
    const beforeInvoke = pluginDiag();
    markDiag("before invoke(list_plugins)", { invokeBegin: (beforeInvoke.invokeBegin ?? 0) + 1 });
    await uiDebug("before invoke(list_plugins)");

    const descriptors = await invoke<PluginDescriptor[]>("list_plugins");

    const descriptorCount = Array.isArray(descriptors) ? descriptors.length : -1;
    const enabledCount = Array.isArray(descriptors) ? descriptors.filter(item => item.enabled).length : -1;
    const afterInvoke = pluginDiag();
    markDiag("after invoke(list_plugins)", {
      invokeDone: (afterInvoke.invokeDone ?? 0) + 1,
      descriptorCount,
      enabledCount,
    });
    await uiDebug(`list_plugins returned isArray=${Array.isArray(descriptors)} count=${descriptorCount} enabled=${enabledCount}`);

    for (const plugin of Array.isArray(descriptors) ? descriptors : []) {
      await uiDebug(`plugin id=${plugin.id} enabled=${plugin.enabled} panel=${plugin.panel ?? "-"} popup=${plugin.popup ?? "-"}`);
    }

    markDiag("before plugins.value assignment");
    plugins.value = Array.isArray(descriptors) ? descriptors : [];
    initialPluginLoadComplete = true;
    clearStartupRetry();
    markDiag("after plugins.value assignment", { renderedPluginCount: plugins.value.length });
    await nextTick();

    const rendered = [...document.querySelectorAll<HTMLElement>(".plugin-slot")].map(slot => {
      const rect = slot.getBoundingClientRect();
      return `${slot.dataset.taskbarRegionId ?? "?"}:${rect.width.toFixed(1)}x${rect.height.toFixed(1)}`;
    });
    markDiag("after plugin DOM render", {
      renderedPluginCount: plugins.value.length,
      slotRefCount: slots.size,
      domSlotCount: rendered.length,
    });
    await uiDebug(`render complete plugins=${plugins.value.length} slots=${slots.size} domSlots=${rendered.length} [${rendered.join(", ")}]`);
    requestInteractiveRegionSync();
  } catch (error) {
    const text = errorText(error);
    plugins.value = [];
    markDiag("loadPlugins FAILED", {
      invokeError: text,
      renderedPluginCount: 0,
      domSlotCount: document.querySelectorAll(".plugin-slot").length,
      slotRefCount: slots.size,
    });
    await uiDebug(`loadPlugins FAILED ${text}`);
    console.error("加载任务栏插件失败", error);
    scheduleStartupRetry();
  }
}

onMounted(async () => {
  debugLogsEnabled ||= await invoke<boolean>("get_debug_mode").catch(() => false);
  markDiag("PluginPanels onMounted entered", { panelsMounted: true });
  await uiDebug("PluginPanels mounted");

  // 先监听，再做首次读取，避免启动阶段 plugins-changed 正好发生在两者之间。
  unlisteners.push(await listen("plugins-changed", loadPlugins));
  unlisteners.push(await listen<{ pluginId: string; key: string }>("plugin-storage-changed", event => {
    if (event.payload.key !== "settings") return;
    const pluginId = event.payload.pluginId;
    panelRevisions.value = { ...panelRevisions.value, [pluginId]: (panelRevisions.value[pluginId] ?? 0) + 1 };
    requestInteractiveRegionSync();
  }));

  markDiag("before initial loadPlugins");
  await loadPlugins();
  markDiag("after initial loadPlugins");
  nativeClickTimer = window.setInterval(() => void pollNativePointer(), 40);
});

onUnmounted(() => {
  if (nativeClickTimer !== undefined) window.clearInterval(nativeClickTimer);
  clearStartupRetry();
  if (interactiveRegionFrame !== undefined) window.cancelAnimationFrame(interactiveRegionFrame);
  unlisteners.forEach(unlisten => unlisten());
});
</script>

<template>
  <div
    v-for="plugin in plugins.filter(item => item.enabled && !!item.panel)"
    :key="`${plugin.id}:${panelRevisions[plugin.id] ?? 0}`"
    :ref="element => setSlotRef(plugin.id, element)"
    data-taskbar-interactive
    :data-taskbar-region-id="`plugin:${plugin.id}`"
    class="plugin-slot"
    :class="{
      'plugin-slot--popup': !!plugin.popup,
      'plugin-slot--hovered': hoveredPluginId === plugin.id,
      'plugin-slot--dragging': draggedPluginId === plugin.id,
    }"
    :style="{ width: `${frameWidths[plugin.id] || 48}px` }"
  >
    <PluginSurface
      class="plugin-panel"
      :plugin="plugin"
      surface="panel"
      :revision="panelRevisions[plugin.id] ?? 0"
      @resize="width => onPanelResize(plugin.id, width)"
    />
  </div>
</template>

<style scoped>
.plugin-slot {
  position: relative;
  height: 36px;
  flex: 0 0 auto;
  display: flex;
  align-items: stretch;
  margin: 0;
  overflow: hidden;
  border-radius: 8px;
  background: transparent;
  cursor: grab;
  transition: transform 100ms ease, opacity 100ms ease;
}
.plugin-slot::after {
  content: "";
  position: absolute;
  inset: 0;
  z-index: 2;
  border-radius: 8px;
  pointer-events: none;
  background: transparent;
  box-shadow: inset 0 0 0 1px transparent;
  transition: background 120ms ease, box-shadow 120ms ease;
}
.plugin-slot--hovered::after {
  background: rgba(255,255,255,.14);
  box-shadow: inset 0 0 0 1px rgba(255,255,255,.09);
}
.plugin-slot--dragging { cursor: grabbing; opacity: .72; transform: scale(.98); }
.plugin-slot--popup { cursor: grab; }
.plugin-panel {
  position: relative;
  z-index: 1;
  width: 100%;
  height: 36px;
  flex: 1 1 auto;
  margin: 0;
  border: 0;
  background: transparent;
  overflow: hidden;
  pointer-events: none;
}
</style>
