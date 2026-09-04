# TB Widget UI / Plugin SDK

## UI

宿主与组件化插件统一使用 Vue 3 + `kui-vue`，默认 dark theme。

```html
<html theme-mode="dark">
```

不再提供 `tb-button`、`tb-card`、`tb-switch` 等自定义 Web Components。

Popup 的 `popup.html` 只写模板和样式，`popup.js` 设置 `window.tbPlugin`：

```html
<style>
.popup { display:flex; flex-direction:column; gap:12px; }
</style>
<div class="popup">
  <k-card>
    <k-switch :model-value="enabled" @change="enabled = $event" />
    <k-button type="primary" @click="save">保存</k-button>
  </k-card>
</div>
```

```js
window.tbPlugin = {
  data: () => ({ enabled: false }),
  methods: {
    async save() {
      await host.storage.set("enabled", this.enabled);
    }
  },
  async mounted() {
    this.enabled = await host.storage.get("enabled", false);
  }
};
```

宿主自动从本地 `public/vendor/` 加载 Vue、dayjs、KUI 和 KUI CSS，不依赖 CDN。

## Host API 原则

底座不理解任何业务。没有 GPT、天气、系统监控等专用 API。

### Storage

权限：`storage`

```js
await host.storage.get("key", fallback);
await host.storage.set("key", value);
await host.storage.remove("key");
await host.storage.clear();
```

数据按插件 ID 隔离保存。

### HTTP

权限：`http.request`

```js
const response = await host.http.request({
  url: "https://example.com/api",
  method: "POST",
  headers: {
    "content-type": "application/json"
  },
  body: JSON.stringify({ value: 1 }),
  timeoutMs: 15000,
  responseType: "text"
});

console.log(response.status, response.ok, response.headers, response.body);
```

二进制响应：

```js
const response = await host.http.request({
  url: "https://example.com/file.bin",
  responseType: "bytes"
});
console.log(response.bytes);
```

也可以直接使用浏览器 `fetch()`。直接 `fetch()` 受 WebView/CORS 规则约束；`host.http.request()` 由 Rust 发起，不受网页 CORS 限制，并支持 `proxyUrl`。

### File read

权限：`fs.read`

```js
const text = await host.fs.readText("~/.example/config.json");
const bytes = await host.fs.readBytes("C:/data/file.bin");
```

`~` 会展开到当前用户目录。当前权限是通用读取权限，插件声明后可以请求读取用户有权限访问的文件，因此只应授予可信插件。

### WASM

插件目录可包含 `plugin.wasm`。WASM 在插件 iframe 内实例化，不承载宿主业务命令。

```js
const exports = await host.wasm.exports();
const result = exports.my_function(1, 2);
```

对纯数字参数/返回值可使用快捷方式：

```js
const result = await host.wasm.call("my_function", 1, 2);
```

复杂字符串/对象需要插件自己设计 WASM 内存 ABI。WASM 不能直接访问网络、文件或 Windows API；需要 I/O 时由插件 JS 调用上述通用 Host API，再把数据交给 WASM 处理。

## Popup 自动尺寸

Popup 不需要在 manifest 声明窗口大小。运行时持续观察 `#plugin-root` 的真实内容尺寸：

1. Vue/KUI 挂载完成后立即测量。
2. `ResizeObserver` 监听内容变化。
3. 字体加载和动态 DOM 变化后重新测量。
4. 宿主窗口同步放大或缩小。
5. 不再使用固定 220–480px 宽度上限；最终尺寸只限制在当前显示器工作区内。

插件可以选择固定“内容期望宽度”：

```css
.popup {
  width: 440px;
  box-sizing: border-box;
}
```

也可以完全自适应内容：

```css
.popup {
  width: fit-content;
  min-width: 0;
}
```

建议所有有显式宽度且带 padding 的根容器使用 `box-sizing: border-box`，避免 `width + padding` 导致实际窗口比预期更宽。

## Settings / Popup 生命周期

这些接口只是容器控制，不是业务能力：

```js
await host.settings.open();
await host.popup.hide();
```
