<script setup lang="ts">
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { nextTick, onMounted, onUnmounted, ref } from "vue";
import { buildPluginBridgeScript } from "../plugins/bridge";
import { buildPluginDocument } from "../plugins/document";
import { handlePluginRequest } from "../plugins/hostRequest";
import type { PluginBundle } from "../plugins/types";

type NativeTaskbarPoint = { x: number; y: number };
type NativeTaskbarClick = NativeTaskbarPoint & { regionId: string };
type NativeTaskbarPointerPoll = {
  cursor: NativeTaskbarPoint | null;
  cursorRegionId?: string | null;
  leftButtonDown: boolean;
  leftDowns: NativeTaskbarClick[];
};

type PluginMessage = {
  tbwidgetPlugin?: boolean;
  pluginId?: string;
  requestId?: number;
  method?: string;
  args?: unknown[];
  width?: number;
  height?: number;
};

const plugins = ref<PluginBundle[]>([]);
const frameWidths = ref<Record<string, number>>({});
const panelRevisions = ref<Record<string, number>>({});
const hoveredPluginId = ref("");
const draggedPluginId = ref("");
const frames = new Map<string, HTMLIFrameElement>();
const slots = new Map<string, HTMLElement>();
const unlisteners: UnlistenFn[] = [];
let nativeClickTimer: number | undefined;
let nativeClickPolling = false;
let lastPopupPluginId = "";
let lastPopupAt = 0;
let lastPointerZone = "__init__";
let suppressedHoverPluginId = "";
let dragMoved = false;
const INPUT_DEBUG_LOGS = false;

function srcdoc(plugin: PluginBundle) {
  return buildPluginDocument("panel", plugin.panelHtml, buildPluginBridgeScript(plugin, "panel"), plugin.panelJs);
}

async function openPluginPopup(plugin: PluginBundle, rect: DOMRect) {
  if (!plugin.popupHtml || !plugin.popupJs) return;
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
  return plugins.value.find(item => item.id === pluginId && item.enabled && (!requirePopup || (item.popupHtml && item.popupJs))) ?? null;
}

function pluginAtPoint(point: NativeTaskbarPoint, requirePopup = false) {
  for (const plugin of plugins.value.filter(item => item.enabled && (!requirePopup || (item.popupHtml && item.popupJs)))) {
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
  void nextTick().then(() => window.dispatchEvent(new CustomEvent("tbwidget-interactive-regions-changed")));
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

async function onMessage(event: MessageEvent<PluginMessage>) {
  const message = event.data;
  if (!message?.tbwidgetPlugin || !message.pluginId || !message.method) return;
  const plugin = plugins.value.find(item => item.id === message.pluginId && item.enabled);
  if (!plugin) return;
  const frame = frames.get(plugin.id);
  if (!frame || event.source !== frame.contentWindow) return;

  if (message.method === "host.resize") {
    const width = Math.max(24, Math.min(800, Math.ceil(Number(message.width) || 0)));
    frameWidths.value = { ...frameWidths.value, [plugin.id]: width };
    await nextTick();
    window.dispatchEvent(new CustomEvent("tbwidget-interactive-regions-changed"));
    return;
  }

  if (!message.requestId) return;
  try {
    const value = await handlePluginRequest(plugin, message.method, Array.isArray(message.args) ? message.args : []);
    frame.contentWindow?.postMessage({ tbwidgetHost: true, pluginId: plugin.id, requestId: message.requestId, ok: true, value }, "*");
  } catch (error) {
    frame.contentWindow?.postMessage({ tbwidgetHost: true, pluginId: plugin.id, requestId: message.requestId, ok: false, error: String(error) }, "*");
  }
}

function setFrameRef(pluginId: string, element: unknown) {
  if (element instanceof HTMLIFrameElement) frames.set(pluginId, element);
  else frames.delete(pluginId);
}

function requestInteractiveRegionSync() {
  window.requestAnimationFrame(() => window.dispatchEvent(new CustomEvent("tbwidget-interactive-regions-changed")));
}

function setSlotRef(pluginId: string, element: unknown) {
  if (element instanceof HTMLElement) slots.set(pluginId, element);
  else slots.delete(pluginId);
  requestInteractiveRegionSync();
}

async function loadPlugins() {
  if (draggedPluginId.value) return;
  plugins.value = await invoke<PluginBundle[]>("list_plugins");
  await nextTick();
  window.dispatchEvent(new CustomEvent("tbwidget-interactive-regions-changed"));
}

onMounted(async () => {
  window.addEventListener("message", onMessage);
  await loadPlugins();
  unlisteners.push(await listen("plugins-changed", loadPlugins));
  unlisteners.push(await listen<{ pluginId: string; key: string }>("plugin-storage-changed", event => {
    if (event.payload.key !== "settings") return;
    const pluginId = event.payload.pluginId;
    panelRevisions.value = { ...panelRevisions.value, [pluginId]: (panelRevisions.value[pluginId] ?? 0) + 1 };
    requestInteractiveRegionSync();
  }));
  nativeClickTimer = window.setInterval(() => void pollNativePointer(), 40);
});

onUnmounted(() => {
  window.removeEventListener("message", onMessage);
  if (nativeClickTimer !== undefined) window.clearInterval(nativeClickTimer);
  unlisteners.forEach(unlisten => unlisten());
});
</script>

<template>
  <div
    v-for="plugin in plugins.filter(item => item.enabled && item.panelHtml.trim().length > 0)"
    :key="`${plugin.id}:${panelRevisions[plugin.id] ?? 0}`"
    :ref="element => setSlotRef(plugin.id, element)"
    data-taskbar-interactive
    :data-taskbar-region-id="`plugin:${plugin.id}`"
    class="plugin-slot"
    :class="{
      'plugin-slot--popup': !!plugin.popupHtml && !!plugin.popupJs,
      'plugin-slot--hovered': hoveredPluginId === plugin.id,
      'plugin-slot--dragging': draggedPluginId === plugin.id,
    }"
    :style="{ width: `${frameWidths[plugin.id] || 48}px` }"
  >
    <iframe
      :ref="element => setFrameRef(plugin.id, element)"
      class="plugin-panel"
      :srcdoc="srcdoc(plugin)"
      sandbox="allow-scripts"
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
