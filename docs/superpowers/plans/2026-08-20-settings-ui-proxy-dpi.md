# Settings UI, Proxy, and Responsive Window Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace only the settings window with the supplied template, keep all live metric behavior, fix GPT refresh at 60 seconds, and resolve proxy configuration from the command line or `config.json`.

**Architecture:** Preserve the existing single Vue component and Rust application state to minimize changes. Convert the supplied Tailwind layout to scoped local CSS, bundle the three fonts through npm, and map existing metrics/history directly into the new cards. Resolve an immutable effective proxy during startup, and clamp the settings window in logical pixels to the current work area.

**Tech Stack:** Vue 3 Composition API, TypeScript 5.6, Vite 6, Tauri 2, Rust 2021, serde/serde_json, reqwest, `@fontsource` packages.

## Global Constraints

- `D:\User\Desktop\template.html` is the visual source of truth for the settings window.
- Do not change the taskbar horizontal metric-bar markup or styles.
- Do not load Tailwind CDN, Google Fonts, or any runtime web resource.
- GPT refresh is exactly 60 seconds; clicking the GPT card toggles remaining/used; the refresh icon remains a manual refresh action.
- Remove the reset UI and do not implement a reset action.
- Proxy priority is exactly `--proxy <URL>` then executable-directory `config.json` then no proxy.
- Preserve serialized/database fields needed for backward compatibility; do not delete or migrate SQLite columns.
- Do not use Git or Python, delete files, read `node_modules`, or read `src-tauri/target`.

## File Structure

- Create `src-tauri/src/proxy_config.rs`: CLI/config parsing, priority resolution, pure unit tests.
- Modify `src-tauri/src/lib.rs`: effective proxy state, fixed refresh normalization, proxy use, removal of frontend-only reset work from initialization.
- Modify `src/App.vue`: supplied settings structure, local class names/CSS, data mapping, GPT card behavior, responsive content measurement.
- Modify `package.json` and `package-lock.json`: local font packages only.
- Modify `src-tauri/tauri.conf.json`: responsive settings-window defaults.
- Modify `src-tauri/Cargo.toml`: only Win32 monitor features needed for work-area sizing.

---

### Task 1: Resolve Proxy and Normalize GPT Refresh

**Files:**
- Create: `src-tauri/src/proxy_config.rs`
- Modify: `src-tauri/src/lib.rs:29-36, 86-140, 205-208, 436-449, 471-510, 640-667, 909-999, 1440-1475`

**Interfaces:**
- Produces: `pub struct ProxyFile { pub proxy: String }`.
- Produces: `pub fn cli_proxy<I, S>(args: I) -> Result<Option<String>, String>`.
- Produces: `pub fn resolve_proxy(cli: Option<String>, file: Option<ProxyFile>) -> Option<String>`.
- Produces: `pub fn load_proxy_file(path: &Path) -> Result<Option<ProxyFile>, String>`.
- Produces: `fn normalize_widget_config(config: WidgetConfig) -> WidgetConfig`.
- Consumes: existing `runtime_data_dir`, `build_codex_client`, and `WidgetState`.

- [ ] **Step 1: Add failing argument/priority tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_proxy_accepts_separate_value() {
        let value = cli_proxy(["tbwidget.exe", "--proxy", "http://127.0.0.1:7890"])
            .unwrap();
        assert_eq!(value.as_deref(), Some("http://127.0.0.1:7890"));
    }

    #[test]
    fn cli_proxy_requires_value() {
        assert!(cli_proxy(["tbwidget.exe", "--proxy"]).is_err());
    }

    #[test]
    fn cli_wins_over_file() {
        let file = ProxyFile { proxy: "http://file:7890".into() };
        assert_eq!(
            resolve_proxy(Some("http://cli:7890".into()), Some(file)).as_deref(),
            Some("http://cli:7890")
        );
    }

    #[test]
    fn blank_values_mean_no_proxy() {
        assert_eq!(resolve_proxy(Some("  ".into()), Some(ProxyFile { proxy: "".into() })), None);
    }
}
```

- [ ] **Step 2: Run focused tests and confirm failure**

Run: `cargo test proxy_config::tests --lib`

Expected: FAIL because the module and functions do not exist.

- [ ] **Step 3: Implement exact CLI parsing and file deserialization**

Accept only `--proxy <URL>`; preserve unrelated Tauri arguments; reject repeated `--proxy` values with a clear Chinese error instead of silently choosing one.

```rust
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct ProxyFile {
    #[serde(default)]
    pub proxy: String,
}

pub fn resolve_proxy(cli: Option<String>, file: Option<ProxyFile>) -> Option<String> {
    cli.filter(|value| !value.trim().is_empty())
        .or_else(|| file.map(|value| value.proxy).filter(|value| !value.trim().is_empty()))
        .map(|value| value.trim().to_owned())
}
```

`load_proxy_file` returns `Ok(None)` when `config.json` does not exist. Existing but malformed JSON returns an error containing the full file path; setup logs it and continues with no file proxy.

- [ ] **Step 4: Add immutable effective proxy to application state**

Add `gpt_proxy_url: String` directly to `WidgetState`; populate it in `setup_app` from `env::args_os()` and `runtime_data_dir().join("config.json")`. Change both Codex request functions to read `state.gpt_proxy_url` instead of locking `state.config.gpt_proxy_url`.

Update the invalid-proxy error to reference `--proxy` and `config.json`, not SQLite.

- [ ] **Step 5: Add fixed-refresh normalization tests**

```rust
#[test]
fn widget_config_always_normalizes_to_sixty_seconds() {
    let mut config = WidgetConfig::default();
    config.gpt_refresh_seconds = 300;
    assert_eq!(normalize_widget_config(config).gpt_refresh_seconds, 60);
}
```

Run: `cargo test widget_config_always_normalizes_to_sixty_seconds --lib`

Expected: FAIL until normalization exists.

- [ ] **Step 6: Normalize on both load and save**

Set `const GPT_REFRESH_SECONDS: u64 = 60`; remove the multi-value allowlist. Call `normalize_widget_config` after SQLite deserialization and before replacing/saving state in `save_widget_settings`. Keep `gpt_refresh_seconds` and `gpt_proxy_url` in `WidgetConfig` for backward-compatible JSON/SQLite serialization.

- [ ] **Step 7: Run Rust tests and compile check**

Run: `cargo test proxy_config::tests --lib`

Expected: all proxy tests PASS.

Run: `cargo test widget_config_always_normalizes_to_sixty_seconds --lib`

Expected: PASS.

Run: `cargo check --lib`

Expected: PASS.

---

### Task 2: Bundle the Template Fonts Locally

**Files:**
- Modify: `package.json`
- Modify mechanically: `package-lock.json`
- Modify: `src/App.vue`

**Interfaces:**
- Produces: local families `Inter`, `Manrope`, and `JetBrains Mono` in the Vite bundle.
- Consumes: npm registry during development/build setup only; no runtime network access.

- [ ] **Step 1: Install only the three required font packages**

Run:

```powershell
npm install @fontsource/inter@5 @fontsource/manrope@5 @fontsource/jetbrains-mono@5
```

Expected: `package.json` and `package-lock.json` add the three `@fontsource` dependencies. Do not inspect `node_modules`.

- [ ] **Step 2: Import exact weights in `App.vue`**

At the top of `<script setup>` add:

```ts
import "@fontsource/inter/400.css";
import "@fontsource/inter/500.css";
import "@fontsource/inter/600.css";
import "@fontsource/inter/700.css";
import "@fontsource/manrope/600.css";
import "@fontsource/manrope/700.css";
import "@fontsource/jetbrains-mono/400.css";
import "@fontsource/jetbrains-mono/500.css";
```

- [ ] **Step 3: Verify the frontend bundle**

Run: `npm run build`

Expected: PASS; generated assets include local WOFF2 references and contain no request to `fonts.googleapis.com` or `fonts.gstatic.com`.

Run: `rg -n "cdn.tailwindcss.com|fonts.googleapis.com|fonts.gstatic.com" src package.json`

Expected: no matches.

---

### Task 3: Replace Only the Settings Window with the Supplied Template

**Files:**
- Modify: `src/App.vue:1-84, 340-840, 867-1074`

**Interfaces:**
- Consumes: existing config, live metric refs, history refs, formatting helpers, update commands, log/quit methods.
- Produces: `function historyMax(metric: HistoryMetric, secondary?: boolean): number | null`.
- Produces: `function toggleGptDisplayMode(): void`.
- Produces: `function progressWidth(value: number | null): string`.
- Preserves exactly: the existing `v-if="!isSettingsWindow"` taskbar markup and its `.widget`, `.item`, metric-color styles.

- [ ] **Step 1: Add small pure display helpers**

```ts
function historyMax(metric: HistoryMetric, secondary = false) {
  const values = historyPoints.value[metric]
    .map((point) => secondary ? point.secondary_value : point.value)
    .filter((value): value is number => value !== null && Number.isFinite(value));
  return values.length === 0 ? null : Math.max(...values);
}

function progressWidth(value: number | null) {
  return `${Math.min(100, Math.max(0, value ?? 0))}%`;
}

function toggleGptDisplayMode() {
  void updateConfig({
    gpt_display_mode: config.value.gpt_display_mode === "remaining" ? "used" : "remaining",
  });
}
```

- [ ] **Step 2: Remove settings-only reset and editable proxy state**

Remove frontend reset-credit types, refs, refresh calls, error blocks, refresh-period selector handler, and proxy-input handler. Keep backend commands untouched in this task. Ensure `restartCodexTimer` always schedules `60_000` milliseconds regardless of loaded UI state.

- [ ] **Step 3: Replace the settings `<main>` structure**

Translate the supplied template into Vue markup with these mappings:

```text
Header title       -> TB Widget
Log File           -> openWidgetLog
Quit               -> quitApplication
POS Left/Right     -> updateConfig({ position }) using mutually exclusive buttons
TASKBAR Transparent-> checked when mode == clear; change maps clear/off
CPU/MEM/TMP/NET/DSK/GPT filters -> existing visibility fields
CPU/MEM/TMP/NET    -> existing SVG history data and live/MAX labels
DSK                -> diskPercent progress bar
GPT                -> codexChartText progress bar; card click toggles mode
GPT refresh icon   -> @click.stop="refreshCodexUsage"
```

All controls must use real `<button>`/`<input>` semantics and retain visible focus styles. Use Chinese accessible labels while preserving the short visible metric labels from the template.

- [ ] **Step 4: Replace settings CSS with local equivalents**

Create scoped classes corresponding to the template's fixed visual tokens:

```css
.settings-shell {
  --surface: #10131b;
  --surface-container: #1c2028;
  --surface-highest: #31353d;
  --primary: #adc6ff;
  --primary-container: #4b8eff;
  --outline: #8b90a0;
  width: 100%;
  min-height: 100%;
  padding: 24px;
  overflow: auto;
  color: #e0e2ed;
  font-family: "Inter", "Segoe UI Variable Text", "Microsoft YaHei UI", sans-serif;
  background: radial-gradient(circle at top right, #1a1c2e, #0f1118);
}

.settings-card {
  width: min(384px, 100%);
  margin: 0 auto;
  overflow: hidden;
  border: 1px solid rgb(255 255 255 / 10%);
  border-radius: 16px;
  background: rgb(28 32 40 / 40%);
  box-shadow: 0 24px 64px rgb(0 0 0 / 35%);
  backdrop-filter: blur(30px);
}

.metric-card {
  height: 72px;
  display: flex;
  align-items: center;
  padding: 12px;
  border: 1px solid rgb(255 255 255 / 10%);
  border-radius: 12px;
  background: rgb(255 255 255 / 5%);
}
```

Implement the remaining template colors, typography, gaps, header, controls, progress bars, and hover/focus states directly. Do not add Tailwind configuration files or utility classes.

- [ ] **Step 5: Protect taskbar markup and styles**

Before and after editing, copy the `v-if="!isSettingsWindow"` block and `.widget` through `.empty-hint` styles into the review notes and compare them textually.

Run: `rg -n "v-if=\"!isSettingsWindow\"|^\.widget|^\.item|^\.empty-hint" src/App.vue`

Expected: the original taskbar block and core selectors remain present with unchanged values.

- [ ] **Step 6: Build and inspect the settings page**

Run: `npm run build`

Expected: PASS with no unused TypeScript symbols.

Run the Tauri development app and open settings.

Expected: layout matches the supplied template; all visible metrics update; filters alter card visibility; clicking GPT toggles display mode; clicking refresh does not toggle mode.

---

### Task 4: Responsive Settings Window and Work-Area Clamping

**Files:**
- Modify: `src-tauri/tauri.conf.json`
- Modify: `src-tauri/src/lib.rs:683-710, 1210-1252`
- Modify: `src-tauri/Cargo.toml`
- Modify: `src/App.vue:545-575, 793-840`

**Interfaces:**
- Changes command: `resize_settings_window(app: AppHandle, width: f64, height: f64) -> Result<(), String>`.
- Produces: `fn primary_work_area() -> Result<(RECT, u32), String>` returning physical work area and monitor DPI.
- Consumes: settings root `scrollWidth`/`scrollHeight`, `LogicalSize`, primary monitor work area.

- [ ] **Step 1: Add pure logical clamp tests**

```rust
#[test]
fn settings_size_fits_small_work_area() {
    assert_eq!(clamp_settings_size(432.0, 884.0, 360.0, 720.0), (344.0, 704.0));
}

#[test]
fn settings_size_keeps_template_size_when_space_allows() {
    assert_eq!(clamp_settings_size(432.0, 884.0, 1920.0, 1040.0), (432.0, 884.0));
}
```

The function reserves an 8px margin on every side, has minimum logical width 320px, preferred width 432px, and clamps height to the work area minus 16px.

Run: `cargo test settings_size_ --lib`

Expected: FAIL until `clamp_settings_size` exists.

- [ ] **Step 2: Query the primary monitor work area and DPI**

Use `MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY)`, `GetMonitorInfoW`, and `GetDpiForMonitor` when available; fall back to 96 DPI. Convert physical work-area dimensions to logical pixels before calling `clamp_settings_size`.

Add only the required `windows-sys` features: `Win32_Graphics_Gdi`, `Win32_UI_HiDpi`, and `Win32_UI_WindowsAndMessaging`.

- [ ] **Step 3: Update the resize command and position function**

Accept measured `width` and `height`, clamp both, call `window.set_size(LogicalSize::new(width, height))`, then place the physical window at `work_area.right - outer_width - 8` and `work_area.bottom - outer_height - 8`.

Use the same `primary_work_area` result for size and position so a primary-screen switch cannot mix two monitors' coordinate systems.

- [ ] **Step 4: Update frontend measurement**

Measure the settings content after every visibility/layout change:

```ts
await invoke("resize_settings_window", {
  width: settingsElement.value.scrollWidth,
  height: settingsElement.value.scrollHeight,
});
```

Remove the layout-key early return when DPI changes. Listen to Tauri window scale changes and schedule one new measurement; retain the existing `ResizeObserver` guard to avoid resize loops.

- [ ] **Step 5: Update conservative window defaults**

In `tauri.conf.json`, set the settings window defaults to logical width `432`, height `884`, minimum width `320`, remove the fixed `maxWidth`, and keep decorations/visibility/taskbar flags unchanged.

- [ ] **Step 6: Run automated checks**

Run: `cargo test settings_size_ --lib`

Expected: both tests PASS.

Run: `cargo check --lib`

Expected: PASS.

Run: `npm run build`

Expected: PASS.

- [ ] **Step 7: Test common display configurations**

Open settings at 100%, 150%, and 200% scaling and at a work area shorter than 900 logical pixels.

Expected: preferred 384px card width when space allows; minimum 320px window constraint; no horizontal overflow; vertical scrolling appears only when content exceeds work area; window remains 8px from the primary work area's right and bottom edges.

---

### Task 5: Final UI and Proxy Verification

**Files:**
- Inspect only: all files listed above.

**Interfaces:**
- Consumes: completed local-font, Vue settings, proxy, refresh, and responsive-size paths.
- Produces: verification evidence for handoff.

- [ ] **Step 1: Run all relevant automated checks**

Run: `npm run build`

Expected: PASS.

Run: `cargo test --lib`

Expected: PASS.

Run: `cargo check --lib`

Expected: PASS.

- [ ] **Step 2: Verify proxy priority**

Run once with `--proxy http://127.0.0.1:7890` and a different URL in `config.json`.

Expected: log identifies command-line proxy as the selected source without logging credentials.

Run without `--proxy` and with valid `config.json`.

Expected: file proxy is selected.

Run with neither.

Expected: direct connection is attempted and the rest of the widget still starts.

- [ ] **Step 3: Verify fixed GPT behavior**

Load an existing SQLite config whose stored interval is not 60 seconds.

Expected: returned frontend config reports 60; automatic requests are scheduled every 60 seconds; manual refresh works; no refresh-period, proxy, or reset controls are visible.

- [ ] **Step 4: Review scope without Git**

Run: `rg -n "gpt_refresh_seconds|gpt_proxy_url|reset|cdn.tailwindcss.com|fonts.googleapis.com|T[O]DO|T[B]D" src/App.vue src-tauri/src/lib.rs src-tauri/src/proxy_config.rs package.json src-tauri/tauri.conf.json`

Expected: compatibility fields may remain in Rust serialization, but settings markup contains no refresh-period, editable proxy, reset action, CDN URL, or placeholder.

- [ ] **Step 5: Compare against the supplied template visually**

Confirm the local Tauri settings window preserves the supplied template's glass card, 48px top bar, 24px outer padding, 16px content gap, 72px metric cards, blue/purple/orange/green/pink accents, local typography, and compact filter row. Record only intentional differences: Chinese accessible text, live data, responsive clamping, and native interactions.
