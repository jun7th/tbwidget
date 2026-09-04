export type PluginSurface = "panel" | "popup" | "settings";

function escapeScript(source: string) {
  return source.replace(/<\/script/gi, "<\\/script");
}

function usesKuiRuntime(html: string, pluginSource: string) {
  return /<k-[a-z0-9-]+/i.test(html) || /\b(?:window\.)?tbPlugin\b/.test(pluginSource);
}

function splitVueTemplate(html: string) {
  const styles: string[] = [];
  const template = (html || "").replace(/<style\b[^>]*>[\s\S]*?<\/style>/gi, block => {
    styles.push(block);
    return "";
  });
  return { template, styles: styles.join("\n") };
}

export function buildPluginDocument(surface: PluginSurface, html: string, bridgeSource: string, pluginSource = "") {
  const kuiRuntime = usesKuiRuntime(html, pluginSource);
  const vueDocument = kuiRuntime ? splitVueTemplate(html) : { template: html || "", styles: "" };
  const intrinsicPopupStyle = surface === "popup"
    ? `<style id="tb-popup-intrinsic-size">
html, body { width: max-content !important; min-width: 0 !important; height: max-content !important; min-height: 0 !important; margin: 0 !important; padding: 0 !important; border: 0 !important; overflow: hidden !important; }
body { display: inline-block !important; }
#plugin-root { display: inline-block !important; width: max-content !important; min-width: 0 !important; height: max-content !important; margin: 0 !important; padding: 0 !important; border: 0 !important; }
</style>`
    : "";
  const head = kuiRuntime
    ? `<link rel="stylesheet" href="/vendor/kui-vue.css" />\n${intrinsicPopupStyle}\n${vueDocument.styles}`
    : intrinsicPopupStyle;

  const body = kuiRuntime
    ? `<div id="plugin-root">${vueDocument.template}</div>
<script>${escapeScript(bridgeSource)}<\/script>
<script id="tb-plugin-source" type="text/plain">${escapeScript(pluginSource)}<\/script>
<script src="/vendor/vue.global.prod.js"><\/script>
<script src="/vendor/dayjs.min.js"><\/script>
<script src="/vendor/kui-vue.umd.js"><\/script>
<script src="/plugin-runtime.js"><\/script>`
    : `${html || ""}
<script>${escapeScript(bridgeSource)}<\/script>
<script>${escapeScript(pluginSource)}<\/script>`;

  return `<!doctype html>
<html lang="zh-CN" theme-mode="dark" data-tb-surface="${surface}">
<head>
<meta charset="UTF-8" />
<meta name="viewport" content="width=device-width,initial-scale=1" />
<meta name="color-scheme" content="dark" />
${head}
</head>
<body theme-mode="dark">
${body}
</body>
</html>`;
}
