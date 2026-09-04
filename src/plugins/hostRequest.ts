import { invoke } from "@tauri-apps/api/core";
import type { PluginBundle } from "./types";

export type PluginRequestContext = {
  hidePopup?: () => Promise<void>;
};

function hasPermission(plugin: PluginBundle, permission: string) {
  return plugin.permissions.includes(permission);
}

function requirePermission(plugin: PluginBundle, permission: string) {
  if (!hasPermission(plugin, permission)) throw new Error(`插件缺少 ${permission} 权限`);
}

function storagePrefix(pluginId: string) {
  return `tbwidget.plugin.${pluginId}.`;
}

async function storageGet(pluginId: string, key: string, fallback: unknown) {
  const nativeValue = await invoke<unknown | null>("plugin_storage_get", { pluginId, key });
  if (nativeValue !== null) return nativeValue;

  const legacyKey = storagePrefix(pluginId) + key;
  try {
    const legacyText = localStorage.getItem(legacyKey);
    if (legacyText != null) {
      const legacyValue = JSON.parse(legacyText);
      await invoke("plugin_storage_set", { pluginId, key, value: legacyValue });
      localStorage.removeItem(legacyKey);
      return legacyValue;
    }
  } catch {
    // 旧 localStorage 损坏时使用 fallback。
  }
  return fallback;
}

async function storageClear(pluginId: string) {
  await invoke("plugin_storage_clear", { pluginId });
  const prefix = storagePrefix(pluginId);
  const keys: string[] = [];
  for (let index = 0; index < localStorage.length; index += 1) {
    const key = localStorage.key(index);
    if (key?.startsWith(prefix)) keys.push(key);
  }
  keys.forEach(key => localStorage.removeItem(key));
}

/**
 * 普通插件 Host API 只提供通用基础设施：网络、只读文件、插件私有存储。
 * system.metrics 是唯一的内置特殊能力，仅供 builtin/system-monitor 使用。
 * 其余业务逻辑与 WASM 均由插件自身负责。
 */
export async function handlePluginRequest(
  plugin: PluginBundle,
  method: string,
  args: unknown[],
  context: PluginRequestContext = {},
) {
  switch (method) {
    case "storage.get":
      requirePermission(plugin, "storage");
      return storageGet(plugin.id, String(args[0] ?? ""), args[1]);
    case "storage.set":
      requirePermission(plugin, "storage");
      await invoke("plugin_storage_set", { pluginId: plugin.id, key: String(args[0] ?? ""), value: args[1] });
      return null;
    case "storage.remove":
      requirePermission(plugin, "storage");
      await invoke("plugin_storage_remove", { pluginId: plugin.id, key: String(args[0] ?? "") });
      localStorage.removeItem(storagePrefix(plugin.id) + String(args[0] ?? ""));
      return null;
    case "storage.clear":
      requirePermission(plugin, "storage");
      await storageClear(plugin.id);
      return null;

    case "http.request":
      requirePermission(plugin, "http.request");
      return invoke("plugin_http_request", { pluginId: plugin.id, request: args[0] ?? {} });

    case "fs.readText":
      requirePermission(plugin, "fs.read");
      return invoke("plugin_fs_read_text", { pluginId: plugin.id, path: String(args[0] ?? "") });
    case "fs.readBytes":
      requirePermission(plugin, "fs.read");
      return invoke("plugin_fs_read_bytes", { pluginId: plugin.id, path: String(args[0] ?? "") });

    case "system.metrics":
      if (plugin.id !== "system-monitor" || !plugin.builtin) throw new Error("system.metrics 仅提供给内置系统资源监视插件");
      requirePermission(plugin, "system.metrics");
      return invoke("plugin_system_metrics", { pluginId: plugin.id });

    // settings/popup 是插件容器生命周期控制，不承载业务能力。
    case "settings.open":
      return invoke("show_plugin_settings_window", { pluginId: plugin.id });
    case "popup.hide":
      if (context.hidePopup) await context.hidePopup();
      return null;
    default:
      throw new Error(`不支持的宿主接口：${method}`);
  }
}
