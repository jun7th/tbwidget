<script setup lang="ts">
import { RefreshCcw } from "kui-icons";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { computed, onMounted, onUnmounted, ref } from "vue";
import type { PluginDescriptor } from "../plugins/types";

const plugins = ref<PluginDescriptor[]>([]);
const busy = ref("");
const refreshing = ref(false);
const error = ref("");
let unlisten: UnlistenFn | undefined;

const sortedPlugins = computed(() => [...plugins.value].sort((left, right) => {
  if (left.builtin !== right.builtin) return left.builtin ? -1 : 1;
  if (left.order !== right.order) return left.order - right.order;
  return left.name.localeCompare(right.name, "zh-CN");
}));

async function loadPlugins() {
  try { plugins.value = await invoke<PluginDescriptor[]>("list_plugins"); error.value = ""; }
  catch (reason) { error.value = String(reason); }
}
async function refreshPlugins() {
  refreshing.value = true;
  try { plugins.value = await invoke<PluginDescriptor[]>("refresh_plugins"); error.value = ""; }
  catch (reason) { error.value = String(reason); }
  finally { refreshing.value = false; }
}
async function toggle(plugin: PluginDescriptor, enabled: boolean) {
  busy.value = plugin.id;
  try { await invoke("set_plugin_enabled", { pluginId: plugin.id, enabled }); await loadPlugins(); }
  catch (reason) { error.value = String(reason); await loadPlugins(); }
  finally { busy.value = ""; }
}
onMounted(async () => { await loadPlugins(); unlisten = await listen("plugins-changed", loadPlugins); });
onUnmounted(() => unlisten?.());
</script>

<template>
  <section class="ui-section">
    <div v-if="error" class="ui-status ui-status--error">{{ error }}</div>

    <k-card v-if="plugins.length" class="ui-card" theme="fill" title="插件" bordered size="small">
      <template #extra>
        <k-button type="text" size="small" :loading="refreshing"  style="position: absolute; right:12px; top: 8px;" @click="refreshPlugins">
          <Icon :type="RefreshCcw" size="16" />
        </k-button>
      </template>
      <div class="ui-list plugin-list">
        <div v-for="plugin in sortedPlugins" :key="plugin.id" class="plugin-item">
          <div class="plugin-title-row">
            <div class="plugin-title-copy">
              <span class="ui-label plugin-name">{{ plugin.name }}</span>
              <k-tag v-if="plugin.builtin" size="small"style="border: 1px solid #6c0;" >内置</k-tag>
              <k-tag size="small">v{{ plugin.version }}</k-tag>
            </div>
            <k-switch size="small" :model-value="plugin.enabled" :loading="busy === plugin.id"
              @change="value => toggle(plugin, value)" />
          </div>
          <span v-if="plugin.description" class="ui-muted plugin-description">{{ plugin.description }}</span>
        </div>
      </div>
    </k-card>

    <k-card v-else class="ui-card" theme="fill" title="插件" bordered size="small">
      <template #extra>
        <k-button type="text" size="small" :loading="refreshing"  style="position: absolute; right:12px; top: 8px;" @click="refreshPlugins">
          <Icon :type="RefreshCcw" size="16" />
        </k-button>
      </template>

      <div class="ui-empty">未发现插件</div>
    </k-card>
  </section>
</template>

<style scoped>
.section-title {
  margin-bottom: 0;
}

.plugin-item {
  min-height: 62px;
  padding: 10px 2px;
  display: flex;
  flex-direction: column;
  gap: 5px;
}

.plugin-item:first-child {
  padding-top: 0px;
}

.plugin-item:last-child {
  padding-bottom: 0px;
}

.plugin-item+.plugin-item {
  border-top: 1px solid var(--kui-color-border, rgba(255, 255, 255, .1));
}

.plugin-title-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
}

.plugin-title-copy {
  min-width: 0;
  display: flex;
  align-items: center;
  gap: 7px;
  flex-wrap: wrap;
}

.plugin-name {
  flex: 0 1 auto;
}

.plugin-description {
  display: block;
  padding-right: 48px;
}

.refresh-icon {
  width: 16px;
  height: 16px;
  fill: none;
  stroke: currentColor;
  stroke-width: 1.8;
  stroke-linecap: round;
  stroke-linejoin: round;
}
</style>
