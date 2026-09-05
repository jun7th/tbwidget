# TB Widget Plugin SDK

## 1. 插件目录

插件从 Manifest 指定的 HTML 页面直接加载。宿主不再自动查找同名 `panel.js` / `popup.js`，也不再提供 WASM 运行时。

推荐结构：

```text
plugins/<plugin-directory>/
  plugin.json             # 必需
  default-config.json     # 可选，首次创建插件配置时使用
  panel.html              # 可选，任务栏内容
  popup.html              # 可选，点击后的弹窗
  settings.html           # 可选，插件设置页
  common.js               # 可选，多个页面共享业务逻辑
  common.css              # 可选，多个页面共享样式
```

HTML、CSS、JS 如何进一步拆分由插件自己决定，也可以全部写在一个 HTML 中。插件内部的图片、JSON、额外 JS/CSS 等资源都可使用相对路径加载。

程序启动时会扫描已登记插件；新增插件目录后，在 TB Widget 设置页点击“刷新”完成登记。新发现插件默认关闭。

## 2. Manifest

```json
{
  "manifestVersion": 2,
  "id": "example",
  "name": "Example",
  "description": "插件说明",
  "version": "1.0.0",
  "panel": "panel.html",
  "popup": "popup.html",
  "settings": "settings.html",
  "permissions": ["storage", "http.request", "fs.read"]
}
```

| 字段 | 说明 |
|---|---|
| `manifestVersion` | 当前为 `2` |
| `id` | 插件唯一 ID |
| `name` / `description` / `version` | 显示信息 |
| `panel` | 任务栏 HTML，可选 |
| `popup` | 弹窗 HTML，可选 |
| `settings` | 设置 HTML，可选 |
| `permissions` | 插件申请的 Host API 权限 |
| `builtin` | 仅内置插件使用 |

## 3. 页面与资源

插件页面通过内部资源协议按需加载：

```text
tbplugin://localhost/<plugin-id>/<file>
```

例如：

```text
tbplugin://localhost/gpt-usage/popup.html
tbplugin://localhost/gpt-usage/common.js
tbplugin://localhost/gpt-usage/common.css
```

插件页面仍运行在 `sandbox="allow-scripts"` 的 iframe 中。宿主会自动注入 `window.host`，插件无需手工引用 bridge 文件。

普通页面示例：

```html
<!doctype html>
<html lang="zh-CN">
<head>
  <meta charset="UTF-8">
  <link rel="stylesheet" href="./common.css">
</head>
<body>
  <div id="value">--</div>

  <script src="./common.js"></script>
  <script>
    (async () => {
      const value = await host.storage.get("value", "--");
      document.getElementById("value").textContent = value;
      host.ready();
    })();
  </script>
</body>
</html>
```

宿主对插件资源路径执行目录穿越和符号链接越界检查，插件只能加载自己的目录内容。

## 4. Vue / KUI（可选）

插件不是必须使用 Vue。需要保持与宿主一致的 KUI 视觉时，可显式加载宿主提供的运行时：

```html
<link rel="stylesheet" href="/_host/vendor/kui-vue.css">

<div id="plugin-root">
  <k-button type="primary" @click="save">保存</k-button>
</div>

<script src="/_host/vendor/vue.global.prod.js"></script>
<script src="/_host/vendor/dayjs.min.js"></script>
<script src="/_host/vendor/kui-vue.umd.js"></script>
<script>
window.tbPlugin = {
  methods: {
    async save() {
      await host.storage.set("value", "ok");
    }
  }
};
</script>
<script src="/_host/plugin-vue-runtime.js"></script>
```

`plugin-vue-runtime.js` 只负责把页面中显式声明的 `window.tbPlugin` 挂载到 `#plugin-root`；不再使用 `new Function()` 执行插件源代码。

## 5. 配置与缓存

权限：`storage`

```js
await host.storage.get("key", fallback);
await host.storage.set("key", value);
await host.storage.remove("key");
await host.storage.clear();
```

实际文件：

```text
save/config/plugin/<plugin-directory>/config.json
```

插件自己的设置和缓存都保存在自己的配置文件中，不写入主 `config.json`。

## 6. HTTP

权限：`http.request`

```js
const response = await host.http.request({
  url: "https://example.com/api",
  method: "GET",
  headers: {},
  timeoutMs: 15000,
  proxyUrl: "",
  responseType: "text"
});
```

返回包含 `status`、`ok`、`headers`、`body` / `bytes`。

- 只允许 `http://` / `https://`。
- 默认超时 15 秒，宿主限制范围 100 ms ~ 120 s。
- 单次响应最大 16 MiB。
- `responseType: "bytes"` 返回字节数组。
- 请求由 Rust 发起，不受 WebView CORS 限制，并支持代理。

## 7. 文件读取

权限：`fs.read`

```js
const text = await host.fs.readText("~/.example/config.json");
const bytes = await host.fs.readBytes("C:/data/file.bin");
```

`~` 会展开到当前用户目录。该权限可读取当前用户有权限访问的文件，只应授予可信插件。

## 8. 内置系统监控能力

`system.metrics` 是当前唯一特殊能力：

```js
const metrics = await host.system.metrics();
```

限制：

- 只允许 `id = "system-monitor"`。
- manifest 必须是 `builtin: true`。
- 必须声明 `system.metrics` 权限。

普通插件不能申请该接口。

## 9. Popup / Settings 生命周期

Popup 和 Settings 与 Panel 使用相同的 HTML Surface 模型。窗口仍由宿主管理，插件不会为每个页面创建新的 Tauri Window。

```js
await host.settings.open();
await host.settings.close();
await host.popup.hide();
```

页面内容变化后宿主会自动测量尺寸。页面初始化完成时仍建议调用：

```js
host.ready();
```

Popup 固定内容宽度可直接使用 CSS：

```css
.popup {
  width: 440px;
  box-sizing: border-box;
}
```

Settings 直接由 `settings.html` 自己渲染和保存，不再使用 `host.settings.define/onSubmit/onAction` schema 流程。

## 10. 开发约束

- 业务协议、解析和规则放在插件，不放进宿主。
- 普通插件只申请实际需要的权限。
- 插件私有数据只通过 `host.storage` 保存。
- Panel / Popup / Settings 都按普通 HTML 页面开发。
- 公共业务逻辑优先放 `common.js`，公共样式优先放 `common.css`。
- 网络访问优先通过 `host.http`，不要依赖页面直接跨域请求。
