# 插件能力说明

## Host API

底座仅向插件暴露通用基础能力：

| 权限 | API | 用途 |
|---|---|---|
| `storage` | `host.storage.get/set/remove/clear` | 插件隔离存储 |
| `http.request` | `host.http.request(options)` | 通用 HTTP/HTTPS 请求 |
| `fs.read` | `host.fs.readText/readBytes(path)` | 文件读取 |
| - | `host.wasm.exports/call` | 调用插件自己的 `plugin.wasm` |

没有 `gpt.*`、`system.*`、`taskbar.*` 等业务或专用插件 API。

## WASM

`plugin.wasm` 在插件 iframe 内实例化，插件可以把解析、计算、协议处理等业务放进 WASM。网络、文件等 I/O 仍通过通用 Host API 完成。

## Popup

Popup 使用内容实际尺寸自动调整宿主窗口：

- 不再硬限制 220–480px 宽度。
- 内容变大/变小时都会重新测量。
- 上限为当前显示器可用区域。
- 插件可用 CSS 指定期望宽度，也可以让内容使用 intrinsic / fit-content 布局。
