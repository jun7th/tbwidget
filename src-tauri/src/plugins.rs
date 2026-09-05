use std::{
    collections::BTreeMap,
    fs,
    path::{Component, Path, PathBuf},
    sync::atomic::Ordering,
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Emitter};

use crate::{append_log, debug_logging_enabled, WidgetState};

pub(crate) mod host;
pub(crate) mod windows;

pub(crate) fn ensure_plugin_system_ready(state: &WidgetState) -> Result<(), String> {
    if state.plugins_ready.load(Ordering::Acquire) {
        Ok(())
    } else {
        Err("插件系统初始化中".to_string())
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct PluginManifest {
    manifest_version: u32,
    id: String,
    name: String,
    #[serde(default)]
    description: String,
    version: String,
    panel: Option<String>,
    popup: Option<String>,
    settings: Option<String>,
    #[serde(default)]
    permissions: Vec<String>,
    #[serde(default)]
    builtin: bool,
}

#[derive(Clone, Default, Deserialize, Serialize)]
struct PluginEntryState {
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    order: i32,
    #[serde(default)]
    directory: String,
}

#[derive(Default, Deserialize, Serialize)]
struct PluginRuntimeConfig {
    #[serde(default)]
    plugins: BTreeMap<String, PluginEntryState>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_scan_adds_new_plugins_disabled_and_preserves_existing_state() {
        let mut runtime = PluginRuntimeConfig::default();
        runtime.plugins.insert(
            "existing".to_string(),
            PluginEntryState {
                enabled: true,
                order: 4,
                directory: "old-directory".to_string(),
            },
        );

        let added = reconcile_scanned_plugins(
            &mut runtime,
            [
                ("existing".to_string(), "existing-directory".to_string()),
                ("new-plugin".to_string(), "new-directory".to_string()),
            ],
        );

        assert_eq!(added, 1);
        let existing = runtime.plugins.get("existing").unwrap();
        assert!(existing.enabled);
        assert_eq!(existing.order, 4);
        assert_eq!(existing.directory, "existing-directory");

        let new_plugin = runtime.plugins.get("new-plugin").unwrap();
        assert!(!new_plugin.enabled);
        assert_eq!(new_plugin.order, 5);
        assert_eq!(new_plugin.directory, "new-directory");
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PluginDescriptor {
    id: String,
    name: String,
    description: String,
    version: String,
    manifest_version: u32,
    permissions: Vec<String>,
    enabled: bool,
    order: i32,
    panel: Option<String>,
    popup: Option<String>,
    settings: Option<String>,
    builtin: bool,
}

fn read_config_root(path: &Path) -> Result<Value, String> {
    if !path.is_file() {
        let value: Value = serde_json::from_str(include_str!("../default-config.json"))
            .map_err(|error| format!("内置默认配置无效：{error}"))?;
        write_config_root(path, &value)?;
        return Ok(value);
    }
    let content = fs::read_to_string(path)
        .map_err(|error| format!("无法读取主配置 {}：{error}", path.display()))?;
    let value: Value = serde_json::from_str(&content)
        .map_err(|error| format!("主配置 JSON {} 已损坏：{error}", path.display()))?;
    if !value.is_object() {
        return Err(format!("主配置 JSON {} 根节点必须是对象", path.display()));
    }
    Ok(value)
}

fn write_config_root(path: &Path, value: &Value) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("无法创建主配置目录 {}：{error}", parent.display()))?;
    }
    let content = serde_json::to_string_pretty(value)
        .map_err(|error| format!("无法序列化主配置：{error}"))?;
    fs::write(path, format!("{content}\n"))
        .map_err(|error| format!("无法写入主配置 {}：{error}", path.display()))
}

fn load_runtime_config(path: &Path) -> Result<PluginRuntimeConfig, String> {
    let root = read_config_root(path)?;
    let plugins = root.get("plugins").cloned().unwrap_or_else(|| serde_json::json!({}));
    let plugins = serde_json::from_value::<BTreeMap<String, PluginEntryState>>(plugins)
        .map_err(|error| format!("插件配置无效：{error}"))?;
    Ok(PluginRuntimeConfig { plugins })
}

fn save_runtime_config(path: &Path, config: &PluginRuntimeConfig) -> Result<(), String> {
    let mut root = read_config_root(path)?;
    root["plugins"] = serde_json::to_value(&config.plugins)
        .map_err(|error| format!("无法序列化插件配置：{error}"))?;
    write_config_root(path, &root)
}

fn plugin_config_path_from_runtime(
    state: &WidgetState,
    runtime: &PluginRuntimeConfig,
    plugin_id: &str,
) -> Result<PathBuf, String> {
    let entry = runtime
        .plugins
        .get(plugin_id)
        .ok_or_else(|| format!("主配置中没有插件：{plugin_id}"))?;
    let directory = entry.directory.trim();
    let mut components = Path::new(directory).components();
    if directory.is_empty()
        || !matches!(components.next(), Some(Component::Normal(_)))
        || components.next().is_some()
    {
        return Err(format!("插件 {plugin_id} 的目录配置无效：{}", entry.directory));
    }
    let base = state
        .config_path
        .parent()
        .ok_or_else(|| "无法确定 save/config 目录".to_string())?;
    Ok(base.join("plugin").join(directory).join("config.json"))
}

fn read_plugin_config(path: &Path) -> Result<BTreeMap<String, Value>, String> {
    if !path.is_file() {
        write_plugin_config(path, &BTreeMap::new())?;
        return Ok(BTreeMap::new());
    }
    let content = fs::read_to_string(path)
        .map_err(|error| format!("无法读取插件配置 {}：{error}", path.display()))?;
    serde_json::from_str::<BTreeMap<String, Value>>(&content)
        .map_err(|error| format!("插件配置 JSON {} 已损坏：{error}", path.display()))
}

fn write_plugin_config(path: &Path, storage: &BTreeMap<String, Value>) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("无法创建插件配置目录 {}：{error}", parent.display()))?;
    }
    let content = serde_json::to_string_pretty(storage)
        .map_err(|error| format!("无法序列化插件配置：{error}"))?;
    fs::write(path, format!("{content}\n"))
        .map_err(|error| format!("无法写入插件配置 {}：{error}", path.display()))
}

pub(crate) fn ensure_registered_plugin_configs(state: &WidgetState) -> Result<(), String> {
    let runtime = load_runtime_config(&state.config_path)?;
    for plugin_id in runtime.plugins.keys() {
        let path = plugin_config_path_from_runtime(state, &runtime, plugin_id)?;
        if !path.is_file() {
            let directory = runtime
                .plugins
                .get(plugin_id)
                .map(|entry| entry.directory.as_str())
                .unwrap_or_default();
            let default_path = state.plugins_dir.join(directory).join("default-config.json");
            let storage = if default_path.is_file() {
                read_plugin_config(&default_path)?
            } else {
                BTreeMap::new()
            };
            write_plugin_config(&path, &storage)?;
            append_log(
                &state.log_path,
                "FLOW",
                "plugin-config",
                &format!("插件 {} 配置不存在，已创建 {}", plugin_id, path.display()),
            );
        }
        let storage = read_plugin_config(&path)?;
        append_log(
            &state.log_path,
            "FLOW",
            "plugin-config",
            &format!(
                "插件 {} 配置加载完成 file={} keys={}",
                plugin_id,
                path.display(),
                storage.len()
            ),
        );
    }
    Ok(())
}

/// 将上一版主 config.json 内嵌的 storage 拆分到每个插件自己的 config.json。
pub(crate) fn migrate_embedded_plugin_storage(state: &WidgetState) -> Result<(), String> {
    let mut root = read_config_root(&state.config_path)?;
    let Some(plugins) = root.get_mut("plugins").and_then(Value::as_object_mut) else {
        return Ok(());
    };
    let runtime = serde_json::from_value::<BTreeMap<String, PluginEntryState>>(Value::Object(plugins.clone()))
        .map(|plugins| PluginRuntimeConfig { plugins })
        .map_err(|error| format!("插件登记配置无效：{error}"))?;
    let mut changed = false;
    for (plugin_id, entry_value) in plugins.iter_mut() {
        let Some(entry_object) = entry_value.as_object_mut() else { continue; };
        let Some(storage_value) = entry_object.remove("storage") else { continue; };
        let Some(storage_object) = storage_value.as_object() else { continue; };
        let path = plugin_config_path_from_runtime(state, &runtime, plugin_id)?;
        let mut target = read_plugin_config(&path)?;
        for (key, value) in storage_object {
            target.entry(key.clone()).or_insert_with(|| value.clone());
        }
        write_plugin_config(&path, &target)?;
        append_log(
            &state.log_path,
            "FLOW",
            "migration",
            &format!("插件 {} 内嵌配置已拆分到 {}", plugin_id, path.display()),
        );
        changed = true;
    }
    if changed {
        write_config_root(&state.config_path, &root)?;
    }
    Ok(())
}

fn read_manifest(root: &Path) -> Result<PluginManifest, String> {
    let manifest_path = root.join("plugin.json");
    let content = fs::read_to_string(&manifest_path)
        .map_err(|error| format!("无法读取 {}：{error}", manifest_path.display()))?;
    let manifest: PluginManifest = serde_json::from_str(&content)
        .map_err(|error| format!("插件清单 {} 无效：{error}", manifest_path.display()))?;
    if manifest.manifest_version != 2 {
        return Err(format!(
            "插件 {} 使用不支持的 manifestVersion={}，当前仅支持 2",
            manifest.id, manifest.manifest_version
        ));
    }
    if manifest.id.trim().is_empty()
        || manifest.id.contains('/')
        || manifest.id.contains('\\')
        || manifest.id == "."
        || manifest.id == ".."
    {
        return Err(format!("插件目录 {} 的 id 无效", root.display()));
    }
    Ok(manifest)
}

fn load_registered_manifests(state: &WidgetState) -> Result<Vec<(PathBuf, PluginManifest)>, String> {
    let runtime = load_runtime_config(&state.config_path)?;
    let mut manifests = Vec::new();
    for (plugin_id, entry) in runtime.plugins {
        let directory = entry.directory.trim();
        if directory.is_empty() {
            append_log(
                &state.log_path,
                "ERROR",
                "plugins",
                &format!("插件 {} 尚未登记目录；请点击刷新按钮扫描", plugin_id),
            );
            continue;
        }
        let mut components = Path::new(directory).components();
        if !matches!(components.next(), Some(Component::Normal(_))) || components.next().is_some() {
            append_log(
                &state.log_path,
                "ERROR",
                "plugins",
                &format!("插件 {} 的目录配置不安全：{}", plugin_id, directory),
            );
            continue;
        }
        let root = state.plugins_dir.join(directory);
        if !root.is_dir() {
            append_log(
                &state.log_path,
                "ERROR",
                "plugins",
                &format!("已登记插件 {} 的目录不存在：{}", plugin_id, root.display()),
            );
            continue;
        }
        let manifest = match read_manifest(&root) {
            Ok(manifest) => manifest,
            Err(error) => {
                append_log(&state.log_path, "ERROR", "plugins", &error);
                continue;
            }
        };
        if manifest.id != plugin_id {
            append_log(
                &state.log_path,
                "ERROR",
                "plugins",
                &format!("插件目录 {} 的 id={} 与配置 id={} 不一致", root.display(), manifest.id, plugin_id),
            );
            continue;
        }
        manifests.push((root, manifest));
    }
    manifests.sort_by(|left, right| left.1.id.cmp(&right.1.id));
    Ok(manifests)
}

pub(crate) fn log_registered_plugins(state: &WidgetState) -> Result<usize, String> {
    let manifests = load_registered_manifests(state)?;
    let runtime = load_runtime_config(&state.config_path)?;
    for (root, manifest) in &manifests {
        let entry = runtime.plugins.get(&manifest.id);
        append_log(
            &state.log_path,
            "DEBUG",
            "plugins",
            &format!(
                "已登记插件 id={} name={} version={} enabled={} order={} path={}",
                manifest.id,
                manifest.name,
                manifest.version,
                entry.map(|item| item.enabled).unwrap_or(false),
                entry.map(|item| item.order).unwrap_or(-1),
                root.display()
            ),
        );
    }
    append_log(
        &state.log_path,
        "FLOW",
        "plugins",
        &format!("已登记可加载插件数量={}", manifests.len()),
    );
    Ok(manifests.len())
}

fn safe_plugin_path(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let relative_path = Path::new(relative);
    if relative_path.is_absolute()
        || relative_path
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::RootDir | Component::Prefix(_)))
    {
        return Err(format!("插件文件路径不安全：{relative}"));
    }
    Ok(root.join(relative_path))
}

/// 为 tbplugin:// 资源协议解析已启用插件中的文件，并阻止目录穿越/符号链接逃逸。
pub(crate) fn resolve_plugin_asset(
    state: &WidgetState,
    plugin_id: &str,
    relative: &str,
) -> Result<PathBuf, String> {
    ensure_plugin_system_ready(state)?;
    let runtime = load_runtime_config(&state.config_path)?;
    let entry = runtime
        .plugins
        .get(plugin_id)
        .ok_or_else(|| format!("找不到已登记插件：{plugin_id}"))?;
    if !entry.enabled {
        return Err(format!("插件 {plugin_id} 未启用"));
    }

    let directory = entry.directory.trim();
    let mut components = Path::new(directory).components();
    if directory.is_empty()
        || !matches!(components.next(), Some(Component::Normal(_)))
        || components.next().is_some()
    {
        return Err(format!("插件 {plugin_id} 的目录配置无效：{}", entry.directory));
    }

    let root = state.plugins_dir.join(directory);
    let manifest = read_manifest(&root)?;
    if manifest.id != plugin_id {
        return Err(format!("插件目录中的 id={} 与请求 id={} 不一致", manifest.id, plugin_id));
    }

    let candidate = safe_plugin_path(&root, relative)?;
    let canonical_root = root
        .canonicalize()
        .map_err(|error| format!("无法解析插件目录 {}：{error}", root.display()))?;
    let canonical_file = candidate
        .canonicalize()
        .map_err(|error| format!("无法读取插件资源 {}：{error}", candidate.display()))?;
    if !canonical_file.starts_with(&canonical_root) || !canonical_file.is_file() {
        return Err(format!("插件资源路径越界或不是文件：{relative}"));
    }
    Ok(canonical_file)
}

fn scan_manifests(plugins_dir: &Path) -> Result<Vec<(PathBuf, PluginManifest)>, String> {
    fs::create_dir_all(plugins_dir)
        .map_err(|error| format!("无法创建插件目录 {}：{error}", plugins_dir.display()))?;

    let mut manifests = Vec::new();
    let entries = fs::read_dir(plugins_dir)
        .map_err(|error| format!("无法扫描插件目录 {}：{error}", plugins_dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("无法读取插件目录项：{error}"))?;
        let file_type = entry
            .file_type()
            .map_err(|error| format!("无法读取插件目录项类型：{error}"))?;
        if !file_type.is_dir() {
            continue;
        }
        let root = entry.path();
        let manifest_path = root.join("plugin.json");
        if !manifest_path.is_file() {
            continue;
        }
        let content = fs::read_to_string(&manifest_path)
            .map_err(|error| format!("无法读取 {}：{error}", manifest_path.display()))?;
        let manifest: PluginManifest = serde_json::from_str(&content)
            .map_err(|error| format!("插件清单 {} 无效：{error}", manifest_path.display()))?;
        if manifest.manifest_version != 2 {
            return Err(format!(
                "插件 {} 使用不支持的 manifestVersion={}，当前仅支持 2",
                manifest.id, manifest.manifest_version
            ));
        }
        if manifest.id.trim().is_empty()
            || manifest.id.contains('/')
            || manifest.id.contains('\\')
            || manifest.id == "."
            || manifest.id == ".."
        {
            return Err(format!("插件目录 {} 的 id 无效", root.display()));
        }
        manifests.push((root, manifest));
    }
    manifests.sort_by(|left, right| left.1.id.cmp(&right.1.id));
    Ok(manifests)
}

fn reconcile_scanned_plugins(
    runtime: &mut PluginRuntimeConfig,
    discovered: impl IntoIterator<Item = (String, String)>,
) -> usize {
    let mut next_order = runtime
        .plugins
        .values()
        .map(|entry| entry.order)
        .max()
        .unwrap_or(-1)
        + 1;
    let mut added = 0;

    for (plugin_id, directory) in discovered {
        let entry = runtime.plugins.entry(plugin_id).or_insert_with(|| {
            added += 1;
            let entry = PluginEntryState {
                enabled: false,
                order: next_order,
                directory: directory.clone(),
            };
            next_order += 1;
            entry
        });
        entry.directory = directory;
    }

    added
}

fn discovered_plugin_directories(
    manifests: &[(PathBuf, PluginManifest)],
) -> Result<Vec<(String, String)>, String> {
    manifests
        .iter()
        .map(|(root, manifest)| {
            let directory = root
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| format!("插件目录名称无效：{}", root.display()))?
                .to_string();
            Ok((manifest.id.clone(), directory))
        })
        .collect()
}

pub(crate) fn scan_plugins_on_startup(state: &WidgetState) -> Result<usize, String> {
    let manifests = scan_manifests(&state.plugins_dir)?;
    let discovered = discovered_plugin_directories(&manifests)?;
    let _guard = state
        .config_file_lock
        .lock()
        .map_err(|error| format!("无法锁定主配置文件：{error}"))?;
    let mut runtime = load_runtime_config(&state.config_path)?;
    let added = reconcile_scanned_plugins(&mut runtime, discovered);
    save_runtime_config(&state.config_path, &runtime)?;
    append_log(
        &state.log_path,
        "FLOW",
        "plugins",
        &format!(
            "启动插件扫描完成，发现 {} 个插件，新增 {} 个（默认关闭）",
            manifests.len(),
            added
        ),
    );
    Ok(added)
}

fn validate_manifest_entry(root: &Path, plugin_id: &str, kind: &str, entry: &Option<String>) -> Result<(), String> {
    let Some(relative) = entry.as_deref() else { return Ok(()); };
    let path = safe_plugin_path(root, relative)?;
    if !path.is_file() {
        return Err(format!("插件 {plugin_id} 的 {kind} 文件不存在：{}", path.display()));
    }
    Ok(())
}

fn build_descriptors(
    manifests: Vec<(PathBuf, PluginManifest)>,
    runtime: &PluginRuntimeConfig,
) -> Result<Vec<PluginDescriptor>, String> {
    let mut plugins = Vec::with_capacity(manifests.len());
    for (root, manifest) in manifests {
        validate_manifest_entry(&root, &manifest.id, "panel", &manifest.panel)?;
        validate_manifest_entry(&root, &manifest.id, "popup", &manifest.popup)?;
        validate_manifest_entry(&root, &manifest.id, "settings", &manifest.settings)?;
        let entry = runtime.plugins.get(&manifest.id).cloned().unwrap_or_default();
        plugins.push(PluginDescriptor {
            id: manifest.id,
            name: manifest.name,
            description: manifest.description,
            version: manifest.version,
            manifest_version: manifest.manifest_version,
            permissions: manifest.permissions,
            enabled: entry.enabled,
            order: entry.order,
            panel: manifest.panel,
            popup: manifest.popup,
            settings: manifest.settings,
            builtin: manifest.builtin,
        });
    }
    plugins.sort_by_key(|plugin| plugin.order);
    Ok(plugins)
}

#[tauri::command]
pub(crate) fn list_plugins(
    state: tauri::State<'_, WidgetState>,
) -> Result<Vec<PluginDescriptor>, String> {
    ensure_plugin_system_ready(state.inner())?;
    let manifests = load_registered_manifests(state.inner())?;
    let runtime = load_runtime_config(&state.config_path)?;
    let plugins = build_descriptors(manifests, &runtime)?;

    if debug_logging_enabled() {
        append_log(
            &state.log_path,
            "DEBUG",
            "plugins-debug",
            &format!(
                "list_plugins descriptors={} plugins_dir={} exists={}",
                plugins.len(),
                state.plugins_dir.display(),
                state.plugins_dir.is_dir()
            ),
        );
        for plugin in &plugins {
            append_log(
                &state.log_path,
                "DEBUG",
                "plugins-debug",
                &format!(
                    "plugin id={} builtin={} enabled={} order={} panel={} popup={} settings={}",
                    plugin.id,
                    plugin.builtin,
                    plugin.enabled,
                    plugin.order,
                    plugin.panel.as_deref().unwrap_or("-"),
                    plugin.popup.as_deref().unwrap_or("-"),
                    plugin.settings.as_deref().unwrap_or("-"),
                ),
            );
        }
    }

    Ok(plugins)
}

#[tauri::command]
pub(crate) fn refresh_plugins(
    app: AppHandle,
    state: tauri::State<'_, WidgetState>,
) -> Result<Vec<PluginDescriptor>, String> {
    ensure_plugin_system_ready(state.inner())?;
    append_log(
        &state.log_path,
        "ACTION",
        "plugins",
        &format!("手动扫描插件目录 {}", state.plugins_dir.display()),
    );
    let manifests = scan_manifests(&state.plugins_dir)?;
    let discovered = discovered_plugin_directories(&manifests)?;
    let _guard = state
        .config_file_lock
        .lock()
        .map_err(|error| format!("无法锁定主配置文件：{error}"))?;
    let mut runtime = load_runtime_config(&state.config_path)?;
    let added = reconcile_scanned_plugins(&mut runtime, discovered);
    for (root, manifest) in &manifests {
        append_log(
            &state.log_path,
            "DEBUG",
            "plugins",
            &format!(
                "扫描到插件 id={} name={} version={} enabled={} order={} path={}",
                manifest.id,
                manifest.name,
                manifest.version,
                runtime.plugins.get(&manifest.id).map(|entry| entry.enabled).unwrap_or(false),
                runtime.plugins.get(&manifest.id).map(|entry| entry.order).unwrap_or(-1),
                root.display()
            ),
        );
    }
    save_runtime_config(&state.config_path, &runtime)?;
    drop(_guard);
    ensure_registered_plugin_configs(state.inner())?;
    append_log(
        &state.log_path,
        "ACTION",
        "plugins",
        &format!(
            "手动插件扫描完成，发现 {} 个插件，新增 {} 个（默认关闭）；结果已写入 save/config/config.json",
            manifests.len(),
            added
        ),
    );

    let runtime = load_runtime_config(&state.config_path)?;
    let plugins = build_descriptors(load_registered_manifests(state.inner())?, &runtime)?;
    app.emit("plugins-changed", ())
        .map_err(|error| format!("发送插件刷新事件失败：{error}"))?;
    Ok(plugins)
}

#[tauri::command]
pub(crate) fn set_plugin_enabled(
    app: AppHandle,
    state: tauri::State<'_, WidgetState>,
    plugin_id: String,
    enabled: bool,
) -> Result<(), String> {
    ensure_plugin_system_ready(state.inner())?;
    let manifests = load_registered_manifests(state.inner())?;
    if !manifests.iter().any(|(_, manifest)| manifest.id == plugin_id) {
        return Err(format!("找不到已登记插件：{plugin_id}；如刚添加插件，请先点击刷新按钮扫描"));
    }

    let _guard = state
        .config_file_lock
        .lock()
        .map_err(|error| format!("无法锁定主配置文件：{error}"))?;
    let mut runtime = load_runtime_config(&state.config_path)?;
    let entry = runtime
        .plugins
        .get_mut(&plugin_id)
        .ok_or_else(|| format!("主配置中没有插件：{plugin_id}"))?;
    entry.enabled = enabled;
    if let Err(error) = save_runtime_config(&state.config_path, &runtime) {
        append_log(
            &state.log_path,
            "ERROR",
            "plugins",
            &format!("插件 {} {}失败：{error}", plugin_id, if enabled { "启用" } else { "停用" }),
        );
        return Err(error);
    }
    append_log(
        &state.log_path,
        "ACTION",
        "plugins",
        &format!("插件 {} {}成功，配置已写入 save/config/config.json", plugin_id, if enabled { "启用" } else { "停用" }),
    );

    app.emit("plugins-changed", ())
        .map_err(|error| format!("发送插件变更事件失败：{error}"))?;
    Ok(())
}

#[tauri::command]
pub(crate) fn set_plugin_order(
    app: AppHandle,
    state: tauri::State<'_, WidgetState>,
    plugin_ids: Vec<String>,
) -> Result<(), String> {
    ensure_plugin_system_ready(state.inner())?;
    let known = load_registered_manifests(state.inner())?
        .into_iter()
        .map(|(_, manifest)| manifest.id)
        .collect::<std::collections::BTreeSet<_>>();
    if plugin_ids.iter().any(|id| !known.contains(id)) {
        return Err("插件排序中包含未知或未登记插件".to_string());
    }

    let requested = plugin_ids.iter().cloned().collect::<std::collections::BTreeSet<_>>();
    let _guard = state
        .config_file_lock
        .lock()
        .map_err(|error| format!("无法锁定主配置文件：{error}"))?;
    let mut runtime = load_runtime_config(&state.config_path)?;
    let mut remaining = runtime
        .plugins
        .iter()
        .filter(|(id, _)| !requested.contains(*id))
        .map(|(id, entry)| (id.clone(), entry.order))
        .collect::<Vec<_>>();
    remaining.sort_by_key(|(_, order)| *order);

    let mut order = 0_i32;
    for plugin_id in plugin_ids {
        if let Some(entry) = runtime.plugins.get_mut(&plugin_id) {
            entry.order = order;
            order += 1;
        }
    }
    for (plugin_id, _) in remaining {
        if let Some(entry) = runtime.plugins.get_mut(&plugin_id) {
            entry.order = order;
            order += 1;
        }
    }

    if let Err(error) = save_runtime_config(&state.config_path, &runtime) {
        append_log(
            &state.log_path,
            "ERROR",
            "plugins",
            &format!("插件排序修改失败：{error}"),
        );
        return Err(error);
    }
    append_log(&state.log_path, "ACTION", "plugins", "插件排序修改成功并写入 save/config/config.json");
    app.emit("plugins-changed", ())
        .map_err(|error| format!("发送插件排序变更事件失败：{error}"))?;
    Ok(())
}

