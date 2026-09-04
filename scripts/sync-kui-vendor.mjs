import { copyFile, mkdir } from "node:fs/promises";
import { resolve } from "node:path";

const root = process.cwd();
const target = resolve(root, "public/vendor");
await mkdir(target, { recursive: true });

const files = [
  ["node_modules/vue/dist/vue.global.prod.js", "vue.global.prod.js"],
  ["node_modules/dayjs/dayjs.min.js", "dayjs.min.js"],
  ["node_modules/kui-vue/dist/index.js", "kui-vue.umd.js"],
  ["node_modules/kui-vue/style/index.css", "kui-vue.css"],
];

try {
  for (const [source, output] of files) {
    await copyFile(resolve(root, source), resolve(target, output));
  }
} catch (error) {
  console.error("[TB Widget] KUI vendor 同步失败。请先执行 npm install（需要 kui-vue 5.8.0）。");
  throw error;
}
