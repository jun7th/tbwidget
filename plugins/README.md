# TB Widget Plugins

插件目录：

```text
plugins/<plugin-id>/
  plugin.json
  panel.html        # 可选
  panel.js          # 可选
  popup.html        # 可选
  popup.js          # 可选
  settings.js       # 可选
  plugin.wasm       # 可选
```

## 权限与基础 API

示例 manifest：

```json
{
  "manifestVersion": 1,
  "id": "example",
  "name": "example",
  "description": "插件说明，会显示在底座设置页。",
  "version": "1.0.0",
  "wasm": "plugin.wasm",
  "permissions": ["http.request", "fs.read", "storage"]
}
```

```js
const response = await host.http.request({
  url: "https://example.com/api",
  method: "GET",
  responseType: "text"
});

const text = await host.fs.readText("~/data.json");
await host.storage.set("cache", { text });

const wasm = await host.wasm.exports();

// 插件需要自定义 WebAssembly imports 时：
const importedWasm = await host.wasm.instantiate({
  env: { now_ms: () => performance.now() }
});

// 或仅对数字参数/返回值使用快捷调用：
const value = await host.wasm.call("calculate", 1, 2);
```

支持 CORS 的接口也可在插件网页中直接使用浏览器 `fetch()`；需要绕过 WebView CORS 限制、代理或统一宿主请求时使用 `host.http.request()`。

## UI

Popup 统一使用 Vue 3 + `kui-vue`，默认 dark theme。底座不提供 `tb-*` 自定义组件。

```html
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

## Popup 尺寸

宿主会持续测量 `#plugin-root` 的实际内容尺寸并自动调整原生 Popup。

插件不需要声明窗口宽高。需要视觉上的期望宽度时，直接写内容 CSS，例如：

```css
.popup {
  width: 440px;
  box-sizing: border-box;
}
```

完全由内容决定宽度时可不写 `width`，或使用 `width: fit-content`。宿主不再把 Popup 卡在 480px 内。

## WASM 与系统资源

`plugin.wasm` 在 WebView 的浏览器 WebAssembly 沙箱内运行，适合算法、解析和数据处理，不能直接调用 Windows API/WMI/PDH。

底座常规插件能力仍以 `http.request`、`fs.read`、`storage` 为主。`system.metrics` 是唯一的内置特殊能力，只允许 `builtin: true` 的 `system-monitor` 插件使用，由 Rust 侧读取整机 CPU、内存、网络、磁盘和可用硬件传感器。普通第三方插件不能申请该接口。
