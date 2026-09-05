import { readFile, stat } from "node:fs/promises";
import { resolve } from "node:path";

const root = resolve(import.meta.dirname, "..");
const readJson = async path => JSON.parse(await readFile(path, "utf8"));
const existsFile = async path => stat(path).then(value => value.isFile()).catch(() => false);

const configPath = resolve(root, "src-tauri/default-config.json");
const tauriPath = resolve(root, "src-tauri/tauri.conf.json");
const config = await readJson(configPath);
const tauri = await readJson(tauriPath);
const entries = Object.entries(config.plugins || {});

if (!entries.length) throw new Error("default-config.json 没有登记任何内置插件");
if (tauri?.bundle?.resources?.["../plugins/"] !== "plugins/") {
  throw new Error('tauri.conf.json 必须包含 resources 映射："../plugins/": "plugins/"');
}

for (const [id, entry] of entries) {
  const directory = String(entry?.directory || "").trim();
  if (!directory) throw new Error(`内置插件 ${id} 缺少 directory`);
  const pluginRoot = resolve(root, "plugins", directory);
  const manifestPath = resolve(pluginRoot, "plugin.json");
  if (!(await existsFile(manifestPath))) throw new Error(`内置插件 ${id} 缺少 ${manifestPath}`);
  const manifest = await readJson(manifestPath);
  if (manifest.id !== id) throw new Error(`内置插件 ${id} 的 manifest.id=${manifest.id || "<empty>"} 不匹配`);
  if (manifest.manifestVersion !== 2) throw new Error(`插件 ${id} 必须使用 manifestVersion=2`);
  //if (manifest.builtin !== true) throw new Error(`内置插件 ${id} 必须设置 builtin=true`);
  if (!manifest.panel) throw new Error(`内置插件 ${id} 缺少 panel`);
  for (const surface of ["panel", "popup", "settings"]) {
    const entryFile = manifest[surface];
    if (entryFile && !(await existsFile(resolve(pluginRoot, entryFile)))) {
      throw new Error(`插件 ${id} 的 ${surface} 文件不存在：${entryFile}`);
    }
  }
}

console.log(`[check:plugins] 内置插件检查通过：${entries.map(([id]) => id).join(", ")}`);
