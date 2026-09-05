<script setup lang="ts">
import { Settings } from "kui-icons";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { nextTick, onMounted, onUnmounted, ref } from "vue";
import PluginSurface from "./PluginSurface.vue";
import type { PluginDescriptor } from "../plugins/types";

const plugin = ref<PluginDescriptor | null>(null);
const shell = ref<HTMLElement | null>(null);
const surface = ref<InstanceType<typeof PluginSurface> | null>(null);
const unlisteners: UnlistenFn[] = [];
let hiding = false;

async function hidePopup() {
  if (hiding) return;
  hiding = true;
  try {
    await invoke("hide_plugin_popup");
  } finally {
    hiding = false;
  }
}

async function onResize(requestedWidth: number, requestedHeight: number) {
  const maxWidth = Math.max(120, Math.floor((window.screen?.availWidth || 1920) - 24));
  const maxHeight = Math.max(48, Math.floor((window.screen?.availHeight || 1080) - 64));
  const width = Math.max(120, Math.min(maxWidth, requestedWidth));
  const height = Math.max(48, Math.min(maxHeight, requestedHeight));
  await invoke("resize_plugin_popup", { width, height: height + 40 });
}

async function openPluginSettings() {
  const item = plugin.value;
  if (!item?.settings) return;
  try {
    await invoke("show_plugin_settings_window", { pluginId: item.id });
    await hidePopup();
  } catch (error) {
    console.error("无法打开插件设置", error);
  }
}

function playEnterAnimation() {
  const target = shell.value;
  if (!target) return;
  target.classList.remove("plugin-popup-shell--enter");
  void target.offsetWidth;
  target.classList.add("plugin-popup-shell--enter");
}

function finishEnterAnimation() {
  shell.value?.classList.remove("plugin-popup-shell--enter");
}

async function loadActivePlugin() {
  const activeId = await invoke<string | null>("get_active_plugin_popup_id");
  if (!activeId) {
    plugin.value = null;
    return;
  }
  const plugins = await invoke<PluginDescriptor[]>("list_plugins");
  plugin.value = plugins.find(item => item.id === activeId && item.enabled && !!item.popup) ?? null;
  await nextTick();
  surface.value?.measure();
}

function beginPopupTransition() {
  plugin.value = null;
  void loadActivePlugin();
  void nextTick().then(playEnterAnimation);
}

function handleKeydown(event: KeyboardEvent) {
  if (event.key === "Escape") void hidePopup();
}

onMounted(async () => {
  window.addEventListener("keydown", handleKeydown);
  unlisteners.push(await listen("plugin-popup-changed", beginPopupTransition));
  await loadActivePlugin();
});

onUnmounted(() => {
  window.removeEventListener("keydown", handleKeydown);
  unlisteners.forEach(unlisten => unlisten());
});
</script>

<template>
  <main ref="shell" class="plugin-popup-shell" @animationend="finishEnterAnimation">
    <header v-if="plugin" class="plugin-popup-titlebar">
      <span class="plugin-popup-title">{{ plugin.name }}</span>
      <k-button
        v-if="plugin.settings"
        class="plugin-popup-settings"
        type="text"
        aria-label="插件设置"
        @click="openPluginSettings"
      >
        <k-icon :type="Settings" size="16"/>
      </k-button>
    </header>
    <PluginSurface
      v-if="plugin"
      ref="surface"
      class="plugin-popup-frame"
      :aria-label="`${plugin.name} ${plugin.version}`"
      :plugin="plugin"
      surface="popup"
      :request-context="{ hidePopup }"
      @resize="onResize"
    />
  </main>
</template>

<style scoped>
.plugin-popup-shell { width:100vw; height:100vh; overflow:hidden; border:0; border-radius:12px; background:var(--kui-color-bg, #141414); box-shadow:inset 0 0 0 1px var(--kui-color-border, rgba(255,255,255,.12)); }
.plugin-popup-shell--enter { animation:plugin-popup-enter .5s cubic-bezier(.22,.61,.36,1) both; will-change:transform; }
@keyframes plugin-popup-enter {
  from { transform:translate3d(0, 100%, 0); }
  to { transform:translate3d(0, 0, 0); }
}
.plugin-popup-titlebar { position:relative; height:40px; min-height:40px; display:flex; align-items:center; justify-content:center; padding:0 8px; border-bottom:1px solid var(--kui-color-border, rgba(255,255,255,.1)); background:var(--kui-color-bg, #141414); }
.plugin-popup-title { max-width:calc(100% - 84px); overflow:hidden; text-overflow:ellipsis; white-space:nowrap; font-size:13px; font-weight:600; }
.plugin-popup-settings { position:absolute; right:8px; top:6px; }
.plugin-popup-frame { display:block; width:100%; max-width:100%; height:calc(100vh - 40px); margin:0; border:0; background:transparent; }
</style>
