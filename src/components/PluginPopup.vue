<script setup lang="ts">
import { Settings } from "kui-icons";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { nextTick, onMounted, onUnmounted, ref } from "vue";
import { buildPluginBridgeScript } from "../plugins/bridge";
import { buildPluginDocument } from "../plugins/document";
import { handlePluginRequest } from "../plugins/hostRequest";
import type { PluginBundle } from "../plugins/types";

type PluginMessage = {
  tbwidgetPlugin?: boolean;
  pluginId?: string;
  requestId?: number;
  method?: string;
  args?: unknown[];
  width?: number;
  height?: number;
};

const plugin = ref<PluginBundle | null>(null);
const frame = ref<HTMLIFrameElement | null>(null);
const unlisteners: UnlistenFn[] = [];
let hiding = false;

function popupSrcdoc(bundle: PluginBundle) {
  return buildPluginDocument("popup", bundle.popupHtml ?? "", buildPluginBridgeScript(bundle, "popup"), bundle.popupJs ?? "");
}

async function hidePopup() {
  if (hiding) return;
  hiding = true;
  try {
    await invoke("hide_plugin_popup");
  } finally {
    hiding = false;
  }
}

async function onMessage(event: MessageEvent<PluginMessage>) {
  const message = event.data;
  const bundle = plugin.value;
  const targetFrame = frame.value;
  if (!bundle || !targetFrame || event.source !== targetFrame.contentWindow) return;
  if (!message?.tbwidgetPlugin || message.pluginId !== bundle.id || !message.method) return;

  if (message.method === "host.resize") {
    const requestedWidth = Math.max(1, Math.ceil(Number(message.width) || 1));
    const requestedHeight = Math.max(1, Math.ceil(Number(message.height) || 1));
    const maxWidth = Math.max(120, Math.floor((window.screen?.availWidth || 1920) - 24));
    const maxHeight = Math.max(48, Math.floor((window.screen?.availHeight || 1080) - 64));
    const width = Math.max(120, Math.min(maxWidth, requestedWidth));
    const height = Math.max(48, Math.min(maxHeight, requestedHeight));
    await invoke("resize_plugin_popup", { width, height: height + 40 });
    return;
  }

  if (!message.requestId) return;
  try {
    const value = await handlePluginRequest(bundle, message.method, Array.isArray(message.args) ? message.args : [], { hidePopup });
    targetFrame.contentWindow?.postMessage({ tbwidgetHost: true, pluginId: bundle.id, requestId: message.requestId, ok: true, value }, "*");
  } catch (error) {
    targetFrame.contentWindow?.postMessage({ tbwidgetHost: true, pluginId: bundle.id, requestId: message.requestId, ok: false, error: String(error) }, "*");
  }
}

async function openPluginSettings() {
  const bundle = plugin.value;
  if (!bundle) return;
  try {
    await invoke("show_plugin_settings_window", { pluginId: bundle.id });
    await hidePopup();
  } catch (error) {
    console.error("无法打开插件设置", error);
  }
}

async function loadActivePlugin() {
  const activeId = await invoke<string | null>("get_active_plugin_popup_id");
  if (!activeId) {
    plugin.value = null;
    return;
  }
  const bundles = await invoke<PluginBundle[]>("list_plugins");
  plugin.value = bundles.find(item => item.id === activeId && item.enabled && item.popupHtml && item.popupJs) ?? null;
  await nextTick();
  const target = frame.value?.contentWindow;
  if (plugin.value && target) {
    const control = { tbwidgetHostControl: true, pluginId: plugin.value.id, action: "measure" };
    target.postMessage(control, "*");
  }
}

function handleKeydown(event: KeyboardEvent) {
  if (event.key === "Escape") void hidePopup();
}

onMounted(async () => {
  window.addEventListener("message", onMessage);
  window.addEventListener("keydown", handleKeydown);
  unlisteners.push(await listen("plugin-popup-changed", () => void loadActivePlugin()));
  await loadActivePlugin();
});

onUnmounted(() => {
  window.removeEventListener("message", onMessage);
  window.removeEventListener("keydown", handleKeydown);
  unlisteners.forEach(unlisten => unlisten());
});
</script>

<template>
  <main class="plugin-popup-shell">
    <header v-if="plugin" class="plugin-popup-titlebar">
      <span class="plugin-popup-title">{{ plugin.name }}</span>
      <k-button
        v-if="plugin.settingsJs"
        class="plugin-popup-settings"
        type="text"
        aria-label="插件设置"
        @click="openPluginSettings"
      >
      <k-icon :type="Settings" size="16"/>
    </k-button>
      
    </header>
    <iframe
      v-if="plugin"
      ref="frame"
      class="plugin-popup-frame"
      :aria-label="`${plugin.name} ${plugin.version}`"
      :srcdoc="popupSrcdoc(plugin)"
      sandbox="allow-scripts"
    />
  </main>
</template>

<style scoped>
.plugin-popup-shell { width:100vw; height:100vh; overflow:hidden; border:0; border-radius:12px; background:var(--kui-color-bg, #141414); box-shadow:inset 0 0 0 1px var(--kui-color-border, rgba(255,255,255,.12)); }
.plugin-popup-titlebar { position:relative; height:40px; min-height:40px; display:flex; align-items:center; justify-content:center; padding:0 8px; border-bottom:1px solid var(--kui-color-border, rgba(255,255,255,.1)); background:var(--kui-color-bg, #141414); }
.plugin-popup-title { max-width:calc(100% - 84px); overflow:hidden; text-overflow:ellipsis; white-space:nowrap; font-size:13px; font-weight:600; }
.plugin-popup-settings { position:absolute; right:8px; top:6px; }
.plugin-popup-frame { display:block; width:100%; max-width:100%; height:calc(100vh - 40px); margin:0; border:0; background:transparent; }
</style>
