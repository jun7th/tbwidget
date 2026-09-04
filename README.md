# TB Widget

Windows 11 任务栏插件宿主，使用 Tauri 2 + Rust + Vue 3 + TypeScript。

## 架构原则

底座不包含 GPT、系统监控、天气等任何业务逻辑。插件 Host API 只提供通用基础能力：

- `host.http.request()`：通用 HTTP/HTTPS 请求。
- `host.fs.readText()` / `host.fs.readBytes()`：文件读取。
- `host.storage.*`：按插件隔离的数据存储。
- `host.wasm.*`：加载并调用插件自己的 `plugin.wasm`。
- Popup / Settings 等插件窗口生命周期由宿主管理，但不属于业务 API。

业务协议、数据解析、计算规则都应放在插件 JS/WASM 中。WASM 本身处于沙箱，不能凭空访问 Windows API；若未来需要新的原生能力，应设计通用能力接口，而不是把具体业务写进底座。

UI 统一使用 `kui-vue`，默认 KUI dark theme，不再维护 `tb-*` Web Components。

## 开发运行

```bat
npm install
npm run tauri dev
```

`npm install` 会自动执行 `sync:kui`，把插件 iframe 所需的 Vue/KUI UMD 与 CSS 同步到 `public/vendor/`，运行时不依赖 CDN。

要求：Node.js 20.19+、Rust stable、Windows WebView2。

## 插件

`plugins/` 中保留 `gpt-usage` 作为通用能力示例。GPT/Codex 的 URL、认证文件解析、Usage 数据解析全部位于插件中；底座只负责文件读取和 HTTP 请求。

插件 Popup 的宽高由内容实时测量并通知宿主窗口，自适应实际内容，不再限制为固定 220–480px。最大尺寸仅受当前显示器可用区域限制。

完整插件约定见 `docs/UI-SDK.md` 与 `plugins/README.md`。
