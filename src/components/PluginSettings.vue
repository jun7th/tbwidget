<script setup lang="ts">
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { nextTick, onMounted, onUnmounted, ref } from "vue";
import PluginSurface from "./PluginSurface.vue";
import type { PluginDescriptor } from "../plugins/types";

const plugin = ref<PluginDescriptor | null>(null);
const surface = ref<InstanceType<typeof PluginSurface> | null>(null);
const revision = ref(0);
const unlisteners: UnlistenFn[] = [];
let presented = false;

async function presentWindow() {
  if (presented) return;
  presented = true;
  await invoke("present_plugin_settings_window");
}

async function onResize(requestedWidth: number, requestedHeight: number) {
  const width = Math.max(400, Math.min(720, requestedWidth));
  const height = Math.max(196, Math.min(716, requestedHeight));
  await invoke("resize_plugin_settings_window", { width, height: height + 44 });
  await presentWindow();
}

async function loadActivePlugin() {
  const activeId = await invoke<string | null>("get_active_plugin_settings_id");
  presented = false;
  if (!activeId) {
    plugin.value = null;
    return;
  }
  const plugins = await invoke<PluginDescriptor[]>("list_plugins");
  const nextPlugin = plugins.find(item => item.id === activeId && item.enabled && !!item.settings) ?? null;
  plugin.value = null;
  revision.value += 1;
  await nextTick();
  plugin.value = nextPlugin;
  await nextTick();
  surface.value?.measure();
}

async function closeWindow() {
  await invoke("hide_plugin_settings_window");
}

async function startWindowDrag(event: MouseEvent) {
  if (event.button !== 0) return;
  await getCurrentWindow().startDragging().catch(() => undefined);
}

function handleKeydown(event: KeyboardEvent) {
  if (event.key === "Escape") void closeWindow();
}

onMounted(async () => {
  window.addEventListener("keydown", handleKeydown);
  unlisteners.push(await listen("plugin-settings-changed", () => void loadActivePlugin()));
  await loadActivePlugin();
});

onUnmounted(() => {
  window.removeEventListener("keydown", handleKeydown);
  unlisteners.forEach(unlisten => unlisten());
});
</script>

<template>
  <main class="plugin-settings-shell">
    <div class="plugin-settings-window">
      <header class="plugin-settings-titlebar" @mousedown="startWindowDrag">
        <span class="plugin-settings-title">{{ plugin ? `${plugin.name} 设置` : "插件设置" }}</span>
        <k-button type="text" class="plugin-settings-close" aria-label="关闭插件设置" @mousedown.stop @click="closeWindow">×</k-button>
      </header>

      <PluginSurface
        v-if="plugin"
        ref="surface"
        class="plugin-settings-frame"
        :plugin="plugin"
        surface="settings"
        :revision="revision"
        @resize="onResize"
      />
      <div v-else class="plugin-settings-empty">当前没有可用的插件设置。</div>
    </div>
  </main>
</template>

<style scoped>
.plugin-settings-shell { width:100vw; height:100vh; overflow:hidden; background:var(--kui-color-bg,#141414); }
.plugin-settings-window { width:100%; height:100%; background:var(--kui-color-bg,#141414); }
.plugin-settings-titlebar { height:44px; display:flex; align-items:center; justify-content:space-between; padding:0 8px 0 16px; border-bottom:1px solid var(--kui-color-border,rgba(255,255,255,.1)); }
.plugin-settings-title { min-width:0; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; font-size:14px; font-weight:600; }
.plugin-settings-close { flex:0 0 auto; font-size:20px; line-height:1; }
.plugin-settings-frame { display:block; width:100%; height:calc(100vh - 44px); margin:0; border:0; background:transparent; }
.plugin-settings-empty { padding:20px; color:var(--kui-color-text-description,#999); font-size:13px; }
</style>
