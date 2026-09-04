# Tailwind

TB Widget 默认不依赖 Tailwind。UI 基础组件统一使用 `kui-vue`，布局使用普通 CSS。

如果某个业务页面确实需要 Tailwind，可自行安装并在该页面启用；不要用 Tailwind 再封装一套与 KUI 重复的 Button、Switch、Select、Card 等基础组件。

插件 Popup 默认由宿主提供本地 Vue + KUI runtime，不建议插件自行打包 Tailwind 或从 CDN 加载 UI 框架。
