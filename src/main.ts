import "@fontsource/inter/400.css";
import "@fontsource/inter/500.css";
import "@fontsource/inter/600.css";
import "@fontsource/jetbrains-mono/400.css";
import { invoke } from "@tauri-apps/api/core";
import { createApp } from "vue";
import Kui from "kui-vue";
import "kui-vue/style/index.css";
import App from "./App.vue";
import "./styles/app.css";

document.documentElement.setAttribute("theme-mode", "dark");

// 任务栏插件初始化诊断状态。使用普通 window 字段保存，避免诊断本身依赖 Tauri invoke。
// TaskbarView 会周期性把这里的状态写入原生日志。
const pluginDiag = ((window as any).__TBWIDGET_PLUGIN_DIAG__ ??= {
  mainTs: "entered",
  appCreate: false,
  appMounted: false,
  panelsSetup: false,
  panelsMounted: false,
  loadBegin: 0,
  invokeBegin: 0,
  invokeDone: 0,
  invokeError: "",
  descriptorCount: -1,
  enabledCount: -1,
  renderedPluginCount: -1,
  domSlotCount: -1,
  slotRefCount: -1,
  lastStep: "main.ts entered",
});

function errorText(error: unknown) {
  if (error instanceof Error) return `${error.name}: ${error.message}${error.stack ? ` | stack=${error.stack.replace(/\s+/g, " ")}` : ""}`;
  try { return JSON.stringify(error); } catch { return String(error); }
}

function uiError(message: string) {
  void invoke("log_frontend_error", { message }).catch(() => undefined);
}

window.addEventListener("error", event => {
  uiError(`window.error message=${event.message} file=${event.filename}:${event.lineno}:${event.colno} error=${errorText(event.error)}`);
});

window.addEventListener("unhandledrejection", event => {
  uiError(`unhandledrejection reason=${errorText(event.reason)}`);
});

// 发布版禁用 WebView/Chromium 自带右键菜单。
// 任务栏插件的右键弹窗由原生鼠标 Hook + 应用逻辑处理，不依赖该菜单事件。
if (import.meta.env.PROD) {
  window.addEventListener("contextmenu", event => {
    event.preventDefault();
  }, { capture: true });
}

pluginDiag.lastStep = "before createApp";
const app = createApp(App);
pluginDiag.appCreate = true;
pluginDiag.lastStep = "after createApp";
app.config.errorHandler = (error, _instance, info) => {
  uiError(`vue.error info=${info} error=${errorText(error)}`);
  console.error("Vue error", info, error);
};

pluginDiag.lastStep = "before app.mount";
app
  .use(Kui)
  .provide("size", "small")
  .mount("#app");
pluginDiag.appMounted = true;
pluginDiag.lastStep = "after app.mount";
