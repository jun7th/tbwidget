import { convertFileSrc } from "@tauri-apps/api/core";
import type { PluginDescriptor, PluginSurfaceName } from "./types";

function encodePluginPath(path: string) {
  return path
    .replace(/\\/g, "/")
    .split("/")
    .filter(Boolean)
    .map(segment => encodeURIComponent(segment))
    .join("/");
}

function pluginProtocolBase() {
  try {
    // Tauri 在 Windows/Android 上将自定义协议映射为
    // http://<scheme>.localhost/，macOS/Linux 则使用 <scheme>://localhost/。
    // 这里只转换空路径取得平台正确的 base，实际插件路径单独拼接，
    // 这样不会把目录分隔符编码为 %2F，插件内 ./common.js 等相对路径仍可工作。
    return convertFileSrc("", "tbplugin").replace(/\/+$/, "");
  } catch {
    // 仅供非 Tauri 的前端预览/静态检查使用。
    return "tbplugin://localhost";
  }
}

export function pluginSurfaceUrl(plugin: PluginDescriptor, surface: PluginSurfaceName, revision = 0) {
  const entry = plugin[surface];
  if (!entry) return "";
  const pluginId = encodeURIComponent(plugin.id);
  const path = encodePluginPath(entry);
  const query = new URLSearchParams({ surface, v: String(revision) });
  return `${pluginProtocolBase()}/${pluginId}/${path}?${query}`;
}
