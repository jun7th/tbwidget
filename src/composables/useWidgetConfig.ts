import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { ref } from "vue";
import { defaultWidgetConfig, type WidgetConfig } from "../types/widget";

export function useWidgetConfig() {
  const config = ref<WidgetConfig>({ ...defaultWidgetConfig });
  let unlisten: UnlistenFn | undefined;

  async function load() {
    config.value = await invoke<WidgetConfig>("get_widget_config").catch(() => ({ ...defaultWidgetConfig }));
    if (!unlisten) {
      unlisten = await listen<WidgetConfig>("widget-config-changed", event => {
        config.value = event.payload;
      });
    }
    return config.value;
  }

  async function update(patch: Partial<WidgetConfig>) {
    const previous = config.value;
    const next = { ...previous, ...patch };
    config.value = next;
    try {
      config.value = await invoke<WidgetConfig>("save_widget_settings", { config: next });
    } catch (error) {
      config.value = previous;
      throw error;
    }
  }

  function dispose() {
    unlisten?.();
    unlisten = undefined;
  }

  return { config, load, update, dispose };
}
