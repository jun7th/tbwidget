(() => {
  const root = document.getElementById("plugin-root");
  if (!root || !window.Vue || !window.kui) {
    console.error("[TB Widget] KUI 插件运行时未就绪");
    window.host?.ready?.();
    return;
  }

  const template = root.innerHTML;
  root.innerHTML = "";
  const options = window.tbPlugin && typeof window.tbPlugin === "object" ? window.tbPlugin : {};
  const app = window.Vue.createApp({ ...options, template });
  app.config.errorHandler = error => console.error("[TB Widget] 插件 Vue 错误", error);
  app.use(window.kui.default || window.kui);
  app.provide("size", "small");
  app.mount(root);
  window.Vue.nextTick(() => window.host?.ready?.());
})();
