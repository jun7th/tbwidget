<script setup lang="ts">
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { computed, nextTick, onMounted, onUnmounted, ref } from "vue";
import { useNativeAutoResize } from "../composables/useNativeAutoResize";
import { buildSettingsRunnerDocument } from "../plugins/bridge";
import { handlePluginRequest } from "../plugins/hostRequest";
import type { PluginBundle } from "../plugins/types";

type SettingOption = { label: string; value: string | number | boolean };
type SettingField = {
  key: string;
  label: string;
  type: "switch" | "text" | "number" | "select" | "radio" | string;
  placeholder?: string;
  help?: string;
  min?: number;
  max?: number;
  step?: number;
  options?: SettingOption[];
};

type SettingAction = { id: string; label: string; kind?: string };
type SettingsDefinition = {
  title?: string;
  description?: string;
  fields?: SettingField[];
  values?: Record<string, unknown>;
  actions?: SettingAction[];
};

type SettingsActionResult = {
  ok?: boolean;
  type?: string;
  message?: string;
  values?: Record<string, unknown>;
};

type SettingsBridgeMessage = {
  tbwidgetPluginSettings?: boolean;
  tbwidgetPlugin?: boolean;
  tbwidgetSettingsResult?: boolean;
  pluginId?: string;
  definition?: SettingsDefinition;
  requestId?: number;
  method?: string;
  args?: unknown[];
  callId?: number;
  ok?: boolean;
  value?: unknown;
  error?: string;
};

const plugin = ref<PluginBundle | null>(null);
const definition = ref<SettingsDefinition | null>(null);
const values = ref<Record<string, unknown>>({});
const statusText = ref("");
const runner = ref<HTMLIFrameElement | null>(null);
const windowContent = ref<HTMLElement | null>(null);
const runnerKey = ref(0);
const unlisteners: UnlistenFn[] = [];
let settingsCallId = 0;
const pendingSettingsCalls = new Map<number, { resolve: (value: SettingsActionResult) => void; reject: (error: Error) => void }>();

const title = computed(() => definition.value?.title || plugin.value?.name || "插件设置");
const fields = computed(() => definition.value?.fields ?? []);
const actions = computed(() => definition.value?.actions ?? []);
const autoResize = useNativeAutoResize(windowContent, {
  command: "resize_plugin_settings_window",
});

function onMessage(event: MessageEvent<SettingsBridgeMessage>) {
  const message = event.data;
  const bundle = plugin.value;
  const target = runner.value;
  if (!bundle || !target || event.source !== target.contentWindow || message.pluginId !== bundle.id) return;

  if (message.tbwidgetPluginSettings && message.definition) {
    definition.value = message.definition;
    values.value = { ...(message.definition.values ?? {}) };
    return;
  }

  if (message.tbwidgetSettingsResult && message.callId) {
    const pending = pendingSettingsCalls.get(message.callId);
    if (!pending) return;
    pendingSettingsCalls.delete(message.callId);
    if (message.ok) pending.resolve((message.value ?? { ok: true }) as SettingsActionResult);
    else pending.reject(new Error(message.error || "插件设置操作失败"));
    return;
  }

  if (message.tbwidgetPlugin && message.requestId && message.method) {
    void handlePluginRequest(bundle, message.method, Array.isArray(message.args) ? message.args : [])
      .then(value => target.contentWindow?.postMessage({ tbwidgetHost: true, pluginId: bundle.id, requestId: message.requestId, ok: true, value }, "*"))
      .catch(error => target.contentWindow?.postMessage({ tbwidgetHost: true, pluginId: bundle.id, requestId: message.requestId, ok: false, error: String(error) }, "*"));
  }
}

function callSettingsHandler(kind: "submit" | "action", action?: string) {
  const bundle = plugin.value;
  const target = runner.value;
  if (!bundle || !target?.contentWindow) return Promise.reject(new Error("插件设置运行器尚未就绪"));
  const callId = ++settingsCallId;
  // values.value 是 Vue 的响应式 Proxy，不能直接传给 window.postMessage。
  // 插件设置本身要求是 JSON 可持久化数据，因此先转成纯 JSON 对象再跨 iframe 发送。
  const messageValues = JSON.parse(JSON.stringify(values.value)) as Record<string, unknown>;
  return new Promise<SettingsActionResult>((resolve, reject) => {
    pendingSettingsCalls.set(callId, { resolve, reject });
    target.contentWindow?.postMessage({
      tbwidgetSettingsInvoke: true,
      pluginId: bundle.id,
      callId,
      kind,
      action,
      values: messageValues,
    }, "*");
    window.setTimeout(() => {
      const pending = pendingSettingsCalls.get(callId);
      if (!pending) return;
      pendingSettingsCalls.delete(callId);
      pending.reject(new Error("插件设置操作超时"));
    }, 10000);
  });
}

async function loadActivePlugin() {
  const activeId = await invoke<string | null>("get_active_plugin_settings_id");
  if (!activeId) {
    plugin.value = null;
    definition.value = null;
    values.value = {};
    return;
  }
  const bundles = await invoke<PluginBundle[]>("list_plugins");
  const nextPlugin = bundles.find(item => item.id === activeId && item.enabled && item.settingsJs) ?? null;
  // 设置窗口会被隐藏后复用；同一个插件第二次打开时必须强制重建 runner，
  // 否则旧 iframe 不会再次执行 settings.js，definition 会一直保持为空。
  plugin.value = null;
  definition.value = null;
  values.value = {};
  statusText.value = "";
  runnerKey.value += 1;
  await nextTick();
  plugin.value = nextPlugin;
  await nextTick();
}

async function closeWindow() {
  await invoke("hide_plugin_settings_window");
}

async function startWindowDrag(event: MouseEvent) {
  if (event.button !== 0) return;
  await getCurrentWindow().startDragging().catch(() => undefined);
}

function updateField(key: string, value: unknown) {
  values.value = { ...values.value, [key]: value };
  statusText.value = "";
}

function inputValue(field: SettingField) {
  return values.value[field.key] ?? "";
}

function uiOptions(field: SettingField) {
  return (field.options ?? []).map(option => ({ label: option.label, value: String(option.value) }));
}

function updateOption(field: SettingField, value: string | number) {
  const option = (field.options ?? []).find(item => String(item.value) === String(value));
  updateField(field.key, option ? option.value : value);
}

async function saveSettings() {
  statusText.value = "正在保存…";
  try {
    const result = await callSettingsHandler("submit");
    if (result.values) values.value = { ...result.values };
    statusText.value = result.message || (result.ok === false ? "保存失败" : "设置已保存");
  } catch (error) {
    statusText.value = String(error);
  }
}

async function runAction(action: SettingAction) {
  statusText.value = `正在执行“${action.label}”…`;
  try {
    const result = await callSettingsHandler("action", action.id);
    if (result.values) values.value = { ...result.values };
    statusText.value = result.message || (result.ok === false ? "操作失败" : "操作完成");
  } catch (error) {
    statusText.value = String(error);
  }
}

function handleKeydown(event: KeyboardEvent) {
  if (event.key === "Escape") void closeWindow();
}

onMounted(async () => {
  window.addEventListener("message", onMessage);
  window.addEventListener("keydown", handleKeydown);
  unlisteners.push(await listen("plugin-settings-changed", () => void loadActivePlugin()));
  await loadActivePlugin();
  autoResize.start();
  await getCurrentWindow().setFocus();
});

onUnmounted(() => {
  window.removeEventListener("message", onMessage);
  window.removeEventListener("keydown", handleKeydown);
  pendingSettingsCalls.forEach(item => item.reject(new Error("插件设置窗口已关闭")));
  pendingSettingsCalls.clear();
  unlisteners.forEach(unlisten => unlisten());
});
</script>

<template>
  <main class="plugin-settings-shell">
    <div ref="windowContent" class="plugin-settings-window">
      <header class="plugin-settings-titlebar" @mousedown="startWindowDrag">
        <span class="plugin-settings-title">{{ title }}</span>
        <k-button type="text" class="plugin-settings-close" aria-label="关闭插件设置" @mousedown.stop @click="closeWindow">×</k-button>
      </header>

      <iframe
        v-if="plugin"
        :key="runnerKey"
        ref="runner"
        class="settings-runner"
        title="Plugin settings definition runner"
        :srcdoc="buildSettingsRunnerDocument(plugin)"
        sandbox="allow-scripts"
      />

      <div class="plugin-settings-page">
        <div v-if="plugin && definition?.description" class="plugin-settings-description">
          <span class="ui-muted">{{ definition.description }}</span>
          <k-tag size="small">v{{ plugin.version }}</k-tag>
        </div>

        <k-card v-if="plugin && definition" class="ui-card" theme="fill">
          <div class="ui-list">
            <div
              v-for="field in fields"
              :key="field.key"
              class="ui-row"
              :class="{ 'ui-row--stacked': field.type === 'text' || field.type === 'number' }"
            >
              <div class="ui-copy field-copy">
                <span class="ui-label">{{ field.label }}</span>
                <span v-if="field.help" class="ui-muted">{{ field.help }}</span>
              </div>

              <k-switch
                v-if="field.type === 'switch'"
                :model-value="!!values[field.key]"
                @change="value => updateField(field.key, value)"
              />

              <k-radio-group
                v-else-if="field.type === 'radio'"
                :model-value="String(inputValue(field))"
                :options="uiOptions(field)"
                @change="value => updateOption(field, value)"
              />

              <k-select
                v-else-if="field.type === 'select'"
                :model-value="String(inputValue(field))"
                :options="uiOptions(field)"
                block
                @change="value => updateOption(field, value)"
              />

              <k-input-number
                v-else-if="field.type === 'number'"
                :model-value="Number(inputValue(field) || 0)"
                :placeholder="field.placeholder"
                :min="field.min"
                :max="field.max"
                :step="field.step"
                @change="value => updateField(field.key, value)"
              />

              <k-input
                v-else
                :model-value="String(inputValue(field))"
                :placeholder="field.placeholder"
                @change="value => updateField(field.key, String(value ?? ''))"
              />
            </div>
          </div>
        </k-card>

        <k-card v-else class="ui-card" theme="fill">
          <div class="ui-empty">{{ plugin ? "正在读取插件设置定义…" : "当前没有可用的插件设置。" }}</div>
        </k-card>

        <div v-if="plugin && definition && actions.length" class="ui-actions settings-actions">
          <k-button
            v-for="action in actions"
            :key="action.id"
            :type="action.kind === 'primary' ? 'primary' : undefined"
            @click="runAction(action)"
          >{{ action.label }}</k-button>
        </div>

        <div v-if="statusText" class="ui-status">{{ statusText }}</div>

        <div class="ui-actions ui-actions--end footer-actions">
          <k-button theme="plain" @click="closeWindow">取消</k-button>
          <k-button type="primary" :disabled="!definition" @click="saveSettings">保存</k-button>
        </div>
      </div>
    </div>
  </main>
</template>

<style scoped>
.plugin-settings-shell { width:100vw; height:100vh; overflow:auto; background:var(--kui-color-bg,#141414); }
.plugin-settings-window { width:460px; min-height:240px; background:var(--kui-color-bg,#141414); }
.plugin-settings-titlebar { height:44px; display:flex; align-items:center; justify-content:space-between; padding:0 8px 0 16px; border-bottom:1px solid var(--kui-color-border,rgba(255,255,255,.1)); }
.plugin-settings-title { min-width:0; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; font-size:14px; font-weight:600; }
.plugin-settings-close { flex:0 0 auto; font-size:20px; line-height:1; }
.settings-runner { position:fixed; width:1px; height:1px; left:-10000px; top:-10000px; opacity:0; pointer-events:none; }
.plugin-settings-page { width:460px; padding:16px 20px 18px; }
.plugin-settings-description { display:flex; align-items:flex-start; justify-content:space-between; gap:12px; margin-bottom:12px; }
.field-copy { flex:1 1 auto; }
.ui-row--stacked :deep(.k-input), .ui-row--stacked :deep(.k-input-wrap) { width:100%; }
.settings-actions { margin-top:12px; }
.footer-actions { margin-top:12px; }
</style>
