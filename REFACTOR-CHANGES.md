# 本次代码整理说明

## 已处理

- 未发现 `location.reload()` / `window.location.reload()` / `history.go(0)` 之类整页强制刷新代码。
- 将 `PluginPanels.vue`、`PluginPopup.vue`、`PluginSettings.vue` 中重复的 iframe Host Bridge 合并到 `src/plugins/bridge.ts`。
- Bridge 内统一 request / pending / storage / http / fs / wasm / settings / popup 生命周期逻辑。
- Bridge 的尺寸上报加入 `requestAnimationFrame` 合并与尺寸去重，减少重复 `host.resize`。
- Popup 打开后的多次强制测量从“立即 + 下一帧 + 50ms”收敛为一次主动测量，后续由 `ResizeObserver` 接管。
- 新增 `src/composables/useNativeAutoResize.ts`，统一设置窗口和插件设置窗口的 DOM -> 原生窗口 resize。
- `useNativeAutoResize` 会合并同一帧 ResizeObserver 回调，并跳过重复宽高，避免无意义的 Tauri `invoke`。
- 补上 `SettingsView.vue` 对 Rust `settings-resize-requested` 的监听，修复原先只 emit、前端无人监听的链路。
- GPT Popup 移除每秒 `this.$forceUpdate()`，改为响应式 `now` 时钟。
- GPT Panel 的设置/缓存检查从 1 秒一次降到 5 秒一次，并行读取 settings 与 usageCache，减少存储调用。

## 有意保留

- 插件设置窗口再次打开同一插件时仍会重建隐藏 iframe runner。这不是网页 reload，而是为了重新执行插件 `settings.js` 并重新提交设置定义。
- `PluginPanels.vue` 中原生任务栏鼠标指针仍维持 40ms polling。此部分与 Windows Hook / 拖拽 / 右键弹窗强耦合，本轮不改成事件驱动，避免引入任务栏交互回归。
- System Monitor 的刷新间隔属于插件业务刷新，不是网页强制刷新，因此保留。

## 检查结果

- GPT panel/popup JavaScript：`node --check` 通过。
- 新增共享 TypeScript：严格 `tsc` 检查通过。
- panel / popup / settings 三种 Bridge 实际生成后的 JavaScript：`node --check` 通过。
- settings runner 生成后的内联脚本：`node --check` 通过。
- 全项目再次扫描未发现 `$forceUpdate()`、`location.reload()`、`history.go(0)`。

## 未执行

完整 `npm run build` 未执行：原项目没有 `package-lock.json` / `node_modules`，当前环境安装依赖超时。源码级 TypeScript 与生成脚本语法检查均已完成。
