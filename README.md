# TB Widget

Windows 11 任务栏插件宿主，技术栈：Tauri 2 + Rust + Vue 3 + TypeScript。

## 运行目录

打包后以 **当前 EXE 所在目录** 为运行根目录：

```text
TBWidget.exe
plugins/                         # 插件目录
save/
  config/
    config.json                  # 主设置、插件启停/排序/目录
    plugin/
      <plugin-dir>/config.json   # 插件自己的设置与缓存
  log/
    YYYYMMDD.log                 # 当天日志，只追加
```

`save/config/config.json` 不存在时会自动创建。首次创建会登记扫描到的插件；插件配置文件不存在时会从插件目录的 `default-config.json` 创建，没有默认文件则创建空 JSON。

## 插件扫描

程序启动会扫描 `plugins/` 目录并更新插件登记信息。

- 启动时会读取 `save/config/config.json` 中已登记的插件，并把新发现的插件登记为默认关闭。
- 新增、删除或替换插件后，也可以在设置页点击“刷新”按钮手动重新扫描目录。
- 发布版直接读取 EXE 同目录的 `plugins/`。


## 调试模式

在 `save/config/config.json` 根节点设置：

```json
{
  "debug": true
}
```

重启后任务栏容器使用黑色半透明背景，并在当天日志中追加插件目录、Tauri 资源目录、已加载插件描述数量以及各插件的 panel/popup/settings 入口。排查完成后改回 `false`。

## SQLite

SQLite 功能目前默认屏蔽，但源码保留。

正常构建：

```bat
npm run tauri -- build
```

如以后需要重新启用 SQLite：

```bat
npm run tauri -- build --features sqlite-storage
```

默认构建不会创建、读取或迁移 `widget-history.sqlite`。

## 开发与构建

要求：Node.js 20.19+、Rust stable、Windows WebView2。

```bat
npm install
npm run tauri -- dev
```

发布：

```bat
npm run tauri -- build
```

`npm install` / `prebuild` 会执行 `sync:kui`，把插件可选使用的 Vue/KUI 运行时同步到 `public/vendor/`，运行时不依赖 CDN。

## 插件开发

插件使用 Manifest v2：Panel / Popup / Settings 都是普通 HTML Surface，通过 `tbplugin://` 按需加载；HTML 可自行引用独立 CSS/JS。宿主只提供插件私有存储、HTTP、文件读取与窗口生命周期，当前版本不实现 WASM。`system.metrics` 是唯一内置特殊能力，只允许内置 `system-monitor` 使用。

插件格式、权限、Host API、配置存储、Popup/Settings 规则见 [`docs/PLUGIN-SDK.md`](docs/PLUGIN-SDK.md)。
