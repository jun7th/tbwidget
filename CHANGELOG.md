# 更新日志

## [plugin_v1.0] - 2026-09-05

### 新增

- 新增 Manifest v2 HTML Surface 插件架构，统一支持 Panel、Popup 和 Settings 页面。
- 新增 `tbplugin://` 内部资源协议，支持插件 HTML、CSS、JS、图片和 JSON 等相对资源安全加载。
- 新增统一的插件 iframe Host Runtime，支持 RPC 握手、请求排队、日志转发和内容尺寸测量。
- 新增插件私有配置文件及 `default-config.json` 初始化机制。
- 新增内置插件资源完整性检查，并在开发和构建前自动执行。
- 新增按日期追加的结构化日志、运行时 Debug 模式和任务栏显示诊断。

### 变更

- 插件描述接口从内嵌 HTML/JS/WASM 内容改为返回 Panel、Popup 和 Settings 入口路径。
- 将插件 Manifest、Host API 和窗口生命周期拆分为独立 Rust 模块。
- 将应用配置统一迁移到 `save/config/config.json`，插件配置迁移到 `save/config/plugin/<plugin-directory>/config.json`。
- 将运行日志迁移到 `save/log/YYYYMMDD.log`。
- 插件 HTTP、文件和存储接口现在会在 Rust 端验证插件启用状态及 Manifest 权限。
- Popup 和 Settings 改为加载插件自己的 HTML 页面，并根据内容自动调整窗口尺寸。
- 任务栏插件交互改为结合原生鼠标命中区域，支持左键拖动排序及右键打开 Popup。
- 系统监控采样器改为懒加载，降低应用启动阻塞。
- SQLite 存储改为可选的 `sqlite-storage` Cargo feature，默认构建不再访问 SQLite。
- GPT 用量和系统监控插件迁移到 Manifest v2，并使用共享 CSS、JS 和独立默认配置。
- README 中的插件扫描说明已同步为当前实现：启动时扫描插件目录，新发现插件默认关闭。

### 移除

- 移除 Manifest v1 插件加载流程。
- 移除宿主拼装插件文档和通过 `new Function()` 执行插件代码的机制。
- 移除插件 WASM 运行时及相关描述字段。
- 移除旧的 Settings schema/submit/action 表单协议。
- 移除旧插件 Bridge、Document Builder、Runtime 和独立 panel/popup/settings 脚本。
- 清理旧插件文档、Tailwind 配置、示例配置和默认占位资源。

### 不兼容变更

- 插件必须升级到 `manifestVersion: 2`。
- Panel、Popup 和 Settings 必须由 Manifest 指向实际 HTML 文件。
- Manifest v1、内嵌脚本、WASM 和旧 Settings schema 插件不再兼容。
- GPT 用量插件 ID 已从 `gpt` 更改为 `gpt-usage`。
