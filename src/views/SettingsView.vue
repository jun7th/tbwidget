<script setup lang="ts">
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { onMounted, onUnmounted, ref } from "vue";
import PluginManager from "../components/PluginManager.vue";
import { useNativeAutoResize } from "../composables/useNativeAutoResize";
import { useWidgetConfig } from "../composables/useWidgetConfig";

const root = ref<HTMLElement | null>(null);
const startupEnabled = ref(false);
const startupBusy = ref(false);
const logPath = ref("save/log/YYYYMMDD.log");
const positionOptions = [
  { label: "左侧", value: "left" },
  { label: "右侧", value: "right" },
];
const { config, load: loadConfig, update: updateConfig, dispose } = useWidgetConfig();
const autoResize = useNativeAutoResize(root, {
  command: "resize_settings_window",
  measure: element => ({ width: element.scrollWidth, height: element.scrollHeight }),
});
const unlisteners: UnlistenFn[] = [];
let unlistenClose: (() => void) | undefined;

async function setStartup(enabled: boolean) {
  startupBusy.value = true;
  try {
    startupEnabled.value = await invoke<boolean>("set_startup_enabled", { enabled });
  } finally {
    startupBusy.value = false;
  }
}

async function setPosition(value: string | number) {
  if (value === "left" || value === "right") await updateConfig({ position: value });
}

function openLog() { void invoke("open_widget_log"); }
function hideSettings() { void invoke("hide_settings_window"); }
async function startWindowDrag(event: MouseEvent) {
  if (event.button !== 0) return;
  await getCurrentWindow().startDragging().catch(() => undefined);
}

onMounted(async () => {
  await loadConfig();
  startupEnabled.value = await invoke<boolean>("get_startup_enabled").catch(() => false);
  logPath.value = await invoke<string>("get_widget_log_path").catch(() => "save/log/YYYYMMDD.log");
  const win = getCurrentWindow();
  await win.setAlwaysOnTop(false).catch(() => undefined);
  unlistenClose = await win.onCloseRequested(event => { event.preventDefault(); void invoke("hide_settings_window"); });
  unlisteners.push(await listen("settings-resize-requested", () => autoResize.schedule(true)));
  autoResize.start();
});

onUnmounted(() => {
  unlisteners.forEach(unlisten => unlisten());
  unlistenClose?.();
  dispose();
});
</script>

<template>
  <main ref="root" class="settings-page">
    <header class="settings-titlebar" @mousedown="startWindowDrag">
      <h1 class="settings-title">
        <img src="/icon.png" width="24" />
        TB Widget 设置
      </h1>
      <k-button class="settings-close" type="text" aria-label="关闭设置" @mousedown.stop @click="hideSettings">×</k-button>
    </header>

    <div class="settings-content">
    <section class="ui-section settings-first-section">
      <k-card class="ui-card" theme="fill"  bordered size="small">
        <div class="ui-list">
          <div class="ui-row">
            <span class="ui-label">开机启动</span>
            <k-switch :model-value="startupEnabled" :loading="startupBusy" @change="setStartup" size="small"/>
          </div>
          <div class="ui-row">
            <span class="ui-label">位置</span>
            <k-radio-group
              :model-value="config.position"
              :options="positionOptions"
              type="button"
              theme="fill"
              @change="setPosition"
            />
          </div>
          <div class="ui-row">
            <span class="ui-label">日志</span>
            <k-button :title="logPath" aria-label="打开日志文件" @click="openLog">打开日志</k-button>
          </div>
        </div>
      </k-card>
    </section>

    <PluginManager />
    </div>
  </main>
</template>

<style scoped>
.settings-page { width: 520px; min-height: 100%; overflow-y: auto; background: var(--kui-color-bg, #141414); }
.settings-titlebar { height: 44px; display:flex; align-items:center; justify-content:space-between; padding:0 8px 0 16px; border-bottom:1px solid var(--kui-color-border, rgba(255,255,255,.1)); cursor:default; }
.settings-title { margin:0; font-size:14px; line-height:1; font-weight:600; display: flex; align-items: center; gap: 8px; }
.settings-close { flex:0 0 auto; font-size:20px; line-height:1; }
.settings-content { padding: 0 16px 16px; }
.settings-first-section { margin-top: 16px; }
.log-row { min-height: 42px; }
.log-label { flex:0 0 auto; } 
</style>
