# Windows 11 Taskbar Transparency Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the primary taskbar fully transparent on this machine's Windows 11 IoT Enterprise LTSC 24H2 build, while retaining the existing classic-taskbar path and restoring the stock appearance safely.

**Architecture:** Move transparency selection out of the large application module into a focused Rust manager. Build and embed a small C++ XAML Diagnostics TAP DLL; Windows 11 24H2 injects that TAP into Explorer and controls it through registered window messages, while classic taskbars keep `SetWindowCompositionAttribute`. A message-only Rust window reapplies the requested state after Explorer, display, work-area, or DPI changes.

**Tech Stack:** Rust 2021, Tauri 2, `windows-sys 0.61`, MSVC C++20/C++/WinRT, Windows XAML Diagnostics (`InitializeXamlDiagnosticsEx`), Vue 3 window events.

## Global Constraints

- Do not call, bundle, or copy TranslucentTB source; independently implement only the required clear/restore behavior.
- The requested clear color is exactly `ARGB = 0x00000000` (`alpha = 0`), with no tint or blur.
- Guarantee the XAML path only for the current Windows 11 24H2 machine; retain a conservative classic fallback boundary for Windows 10/older taskbars.
- Do not change the taskbar widget's horizontal visual layout.
- Do not use Git or Python, delete files, read `node_modules`, or read `src-tauri/target`.
- Keep Explorer-injected code limited to XAML element discovery, clear/restore, and message handling; no network, database, or business logic.
- Every failure must leave the stock taskbar usable and append a contextual entry to the existing `widget.log`.

## File Structure

- Create `src-tauri/src/taskbar_transparency.rs`: backend selection, classic accent path, TAP materialization/injection, message-driven manager, pure unit tests.
- Create `src-tauri/taskbar-helper/tap.cpp`: COM TAP class, XAML visual tree tracking, original brush retention, clear/restore commands.
- Create `src-tauri/taskbar-helper/tap.def`: exports `DllGetClassObject` and `DllCanUnloadNow`.
- Modify `src-tauri/build.rs`: compile/link the x64 helper DLL and expose its path to Rust.
- Modify `src-tauri/Cargo.toml`: add the build helper and exact Win32 feature flags.
- Modify `src-tauri/src/lib.rs`: own `TaskbarTransparencyManager`, call it on setup/config save/exit, remove the duplicated old accent implementation.
- Modify `src-tauri/src/taskbar.rs`: add a safe reflow entry point that recalculates physical width/position from current taskbar DPI.
- Modify `src/App.vue`: listen for the backend display-environment event and force one width resynchronization; do not alter taskbar markup or styles.

---

### Task 1: Backend Selection and Classic Transparency

**Files:**
- Create: `src-tauri/src/taskbar_transparency.rs`
- Modify: `src-tauri/Cargo.toml`
- Modify: `src-tauri/src/lib.rs:15-84, 546-624`

**Interfaces:**
- Produces: `pub(crate) enum TransparencyBackend { ClassicAccent, XamlTap }`
- Produces: `pub(crate) fn backend_for_build(build: u32, xaml_taskbar: bool) -> TransparencyBackend`
- Produces: `pub(crate) fn read_windows_build() -> Result<u32, String>`
- Produces: `fn apply_classic(mode: TaskbarTransparencyMode) -> Result<(), String>`
- Consumes: existing `TaskbarTransparencyMode::{Off, Clear}` and existing log writer in `lib.rs`.

- [ ] **Step 1: Add failing backend-selection tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_26100_xaml_uses_tap() {
        assert_eq!(backend_for_build(26100, true), TransparencyBackend::XamlTap);
    }

    #[test]
    fn build_19045_uses_classic_accent() {
        assert_eq!(backend_for_build(19045, false), TransparencyBackend::ClassicAccent);
    }

    #[test]
    fn classic_shell_on_new_build_stays_classic() {
        assert_eq!(backend_for_build(26100, false), TransparencyBackend::ClassicAccent);
    }
}
```

- [ ] **Step 2: Run the focused tests and confirm the expected compile failure**

Run: `cargo test taskbar_transparency::tests --lib`

Expected: FAIL because `taskbar_transparency` and `backend_for_build` do not exist.

- [ ] **Step 3: Add version detection and backend selection**

Use `RtlGetVersion` from `ntdll.dll` so the result is not affected by the application manifest. Detect an XAML taskbar by locating `Shell_TrayWnd` and its `Windows.UI.Composition.DesktopWindowContentBridge` descendant; do not select TAP from the build number alone.

```rust
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub(crate) enum TransparencyBackend {
    ClassicAccent,
    XamlTap,
}

pub(crate) fn backend_for_build(build: u32, xaml_taskbar: bool) -> TransparencyBackend {
    if build >= 22_621 && xaml_taskbar {
        TransparencyBackend::XamlTap
    } else {
        TransparencyBackend::ClassicAccent
    }
}
```

- [ ] **Step 4: Move the existing accent structs and calls into the new module**

Keep the existing values exactly:

```rust
const WCA_ACCENT_POLICY: u32 = 19;
const ACCENT_DISABLED: i32 = 0;
const ACCENT_ENABLE_TRANSPARENTGRADIENT: i32 = 2;

let accent = AccentPolicy {
    accent_state: match mode {
        TaskbarTransparencyMode::Off => ACCENT_DISABLED,
        TaskbarTransparencyMode::Clear => ACCENT_ENABLE_TRANSPARENTGRADIENT,
    },
    accent_flags: if mode == TaskbarTransparencyMode::Clear { 2 } else { 0 },
    gradient_color: 0x0000_0000,
    animation_id: 0,
};
```

Remove the unconditional `println!` diagnostics from the old function. Return errors with the backend name, taskbar handle discovery stage, API name, and Win32 error code.

- [ ] **Step 5: Add required Windows features and run tests**

Add only the features used by this module: `Win32_System_LibraryLoader`, `Win32_System_SystemInformation`, `Win32_UI_WindowsAndMessaging`, and `Win32_Foundation`.

Run: `cargo test taskbar_transparency::tests --lib`

Expected: 3 PASS.

- [ ] **Step 6: Inspect the affected files without Git**

Run: `rg -n "TransparencyBackend|RtlGetVersion|ACCENT_ENABLE_TRANSPARENTGRADIENT|println!" src-tauri/src/taskbar_transparency.rs src-tauri/src/lib.rs src-tauri/Cargo.toml`

Expected: selection and accent code exist only in `taskbar_transparency.rs`; the removed transparency debug prints no longer exist in `lib.rs`.

---

### Task 2: Build and Embed the XAML TAP DLL

**Files:**
- Create: `src-tauri/taskbar-helper/tap.cpp`
- Create: `src-tauri/taskbar-helper/tap.def`
- Modify: `src-tauri/build.rs`
- Modify: `src-tauri/Cargo.toml`

**Interfaces:**
- Produces: `tbwidget_taskbar_tap.dll` in Cargo `OUT_DIR`.
- Produces: compile-time environment value `TBWIDGET_TAP_DLL` containing the DLL path.
- Produces: COM CLSID `{8D5E8C44-1474-4E0D-B4A8-6E2B86580D2A}` used by both Rust and C++.
- Consumes: x64 MSVC toolchain and Windows SDK headers/libraries already required by the Tauri Windows build.

- [ ] **Step 1: Replace the empty helper source with a deliberately minimal exported COM server**

Start with exports that compile but return `CLASS_E_CLASSNOTAVAILABLE`; this establishes the build pipeline before XAML code is added:

```cpp
#include <windows.h>
#include <unknwn.h>

extern "C" HRESULT __stdcall DllGetClassObject(REFCLSID, REFIID, void**) {
    return CLASS_E_CLASSNOTAVAILABLE;
}

extern "C" HRESULT __stdcall DllCanUnloadNow() {
    return S_FALSE;
}

BOOL WINAPI DllMain(HINSTANCE, DWORD, LPVOID) {
    return TRUE;
}
```

`tap.def` must contain:

```def
LIBRARY "tbwidget_taskbar_tap"
EXPORTS
    DllGetClassObject
    DllCanUnloadNow
```

- [ ] **Step 2: Add an MSVC DLL build to `build.rs`**

Add build dependency `cc = "1"`. Use `cc::Build::new().cpp(true).get_compiler()` to locate `cl.exe`, compile with `/nologo /std:c++20 /EHsc /MD /c`, then invoke the sibling `link.exe` with `/DLL`, `/DEF:taskbar-helper/tap.def`, `/OUT:<OUT_DIR>/tbwidget_taskbar_tap.dll`, `windowsapp.lib`, `runtimeobject.lib`, `ole32.lib`, and `user32.lib`.

Emit exactly:

```rust
println!("cargo:rerun-if-changed=taskbar-helper/tap.cpp");
println!("cargo:rerun-if-changed=taskbar-helper/tap.def");
println!("cargo:rustc-env=TBWIDGET_TAP_DLL={}", dll.display());
tauri_build::build();
```

On non-Windows hosts, skip helper compilation and still call `tauri_build::build()`.

- [ ] **Step 3: Build the Rust crate and helper**

Run: `cargo check --lib`

Expected: PASS and build output states that `tbwidget_taskbar_tap.dll` was linked. Do not open or enumerate `src-tauri/target`.

- [ ] **Step 4: Embed the helper bytes from `OUT_DIR`**

In `taskbar_transparency.rs` add:

```rust
#[cfg(target_os = "windows")]
static TAP_DLL_BYTES: &[u8] = include_bytes!(env!("TBWIDGET_TAP_DLL"));
```

Materialize it as `tbwidget-taskbar-tap-0.1.0.dll` in `std::env::temp_dir().join("tbwidget")`. Create the directory if absent; write only when size/content differs. Return the exact path to the initializer.

- [ ] **Step 5: Verify the embedded-helper compile path**

Run: `cargo check --lib`

Expected: PASS with the `include_bytes!` path resolved at compile time.

---

### Task 3: Implement the XAML Visual Tree TAP

**Files:**
- Modify: `src-tauri/taskbar-helper/tap.cpp`

**Interfaces:**
- Produces: COM class for CLSID `{8D5E8C44-1474-4E0D-B4A8-6E2B86580D2A}` implementing `IObjectWithSite` and `IVisualTreeServiceCallback2`.
- Produces: message-only window class `TBWidget.TaskbarTap.26100`.
- Consumes: registered messages `TBWidget.TaskbarTap.Clear` and `TBWidget.TaskbarTap.Restore`.
- Tracks: `Taskbar.TaskbarFrame > Grid#RootGrid > Taskbar.TaskbarBackground > Grid > Rectangle#BackgroundFill` and `Rectangle#BackgroundStroke`.

- [ ] **Step 1: Implement a standard COM class factory**

Implement atomic reference counting, `QueryInterface`, `CreateInstance`, and the two DLL exports. `DllMain` may only retain the module handle and call `DisableThreadLibraryCalls`; it must not initialize COM or XAML.

```cpp
class TapClassFactory final : public IClassFactory {
public:
    HRESULT STDMETHODCALLTYPE QueryInterface(REFIID, void**) noexcept override;
    ULONG STDMETHODCALLTYPE AddRef() noexcept override;
    ULONG STDMETHODCALLTYPE Release() noexcept override;
    HRESULT STDMETHODCALLTYPE CreateInstance(IUnknown*, REFIID, void**) noexcept override;
    HRESULT STDMETHODCALLTYPE LockServer(BOOL) noexcept override;
};
```

- [ ] **Step 2: Implement `IObjectWithSite::SetSite` initialization**

On non-null site:

1. Query `IXamlDiagnostics` and `IVisualTreeService3` from the supplied site.
2. Register the callback with `AdviseVisualTreeChange`.
3. Register both command messages with `RegisterWindowMessageW`.
4. Create an `HWND_MESSAGE` window using class `TBWidget.TaskbarTap.26100` on the same XAML thread.
5. Enumerate existing roots through the visual-tree service so taskbar elements present before injection are discovered.

On null site, restore stored brushes, unadvise, and destroy the message window.

- [ ] **Step 3: Track only the taskbar background shapes**

For each added `InstanceHandle`, retrieve its `IInspectable`, runtime class name, `FrameworkElement::Name`, and parent chain. Accept only these two exact paths under `Taskbar.TaskbarFrame`:

```text
Taskbar.TaskbarBackground > Grid > Rectangle#BackgroundFill
Taskbar.TaskbarBackground > Grid > Rectangle#BackgroundStroke
```

Store each accepted `Windows::UI::Xaml::Shapes::Shape` together with its original `Fill` brush. Remove entries when their instance handle is reported as removed. Ignore every other visual.

- [ ] **Step 4: Implement clear and restore on the XAML thread**

For clear, assign a `SolidColorBrush` whose color is `{ A: 0, R: 0, G: 0, B: 0 }` to each tracked shape. For restore, assign the exact retained original brush. Store the current requested state so newly discovered shapes immediately receive clear when appropriate.

```cpp
enum class RequestedAppearance { Stock, Clear };

void ApplyClear() {
    wuxm::SolidColorBrush transparent;
    transparent.Color({ 0, 0, 0, 0 });
    for (auto& item : trackedShapes) item.shape.Fill(transparent);
}
```

- [ ] **Step 5: Wire registered-message commands**

The message-window procedure must compare the incoming message ID with the two registered IDs and call `ApplyClear()` or `RestoreAll()`. Return `1` on success and `0` after catching a C++/WinRT exception. Do not process arbitrary `WM_COPYDATA` payloads.

- [ ] **Step 6: Compile the TAP**

Run: `cargo check --lib`

Expected: PASS; C++ errors include the exact source line when a Windows SDK interface or namespace is incorrect.

- [ ] **Step 7: Static safety inspection**

Run: `rg -n "DllMain|SetSite|AdviseVisualTreeChange|BackgroundFill|BackgroundStroke|SolidColorBrush|RestoreAll|CreateWindowExW" src-tauri/taskbar-helper/tap.cpp`

Expected: `DllMain` contains no XAML/COM initialization; all brush mutations occur after `SetSite` on the XAML thread.

---

### Task 4: Inject, Control, and Reapply the TAP

**Files:**
- Modify: `src-tauri/src/taskbar_transparency.rs`
- Modify: `src-tauri/src/lib.rs:123-140, 640-667, 1440-1505`

**Interfaces:**
- Produces: `pub(crate) struct TaskbarTransparencyManager`.
- Produces: `pub(crate) fn new(log_path: PathBuf, initial: TaskbarTransparencyMode) -> Result<Self, String>`.
- Produces: `pub(crate) fn set_mode(&self, mode: TaskbarTransparencyMode) -> Result<(), String>`.
- Produces: `pub(crate) fn restore(&self) -> Result<(), String>`.
- Consumes: the TAP CLSID, embedded DLL path, `Shell_TrayWnd` Explorer PID, and the registered TAP command messages.

- [ ] **Step 1: Add pure endpoint-name tests**

```rust
#[test]
fn visual_diag_names_are_bounded_and_unique() {
    let names = visual_diag_endpoint_names();
    assert_eq!(names.len(), 16);
    assert_eq!(names.first().unwrap(), "VisualDiagConnection1");
    assert_eq!(names.last().unwrap(), "VisualDiagConnection16");
}
```

Run: `cargo test taskbar_transparency::tests --lib`

Expected: FAIL until `visual_diag_endpoint_names` is implemented.

- [ ] **Step 2: Implement XAML Diagnostics initialization**

1. Read Explorer PID with `GetWindowThreadProcessId(Shell_TrayWnd)`.
2. Load `Windows.UI.Xaml.dll` from System32 and resolve `InitializeXamlDiagnosticsEx`.
3. Materialize the embedded TAP DLL.
4. Attempt endpoint names `VisualDiagConnection1` through `VisualDiagConnection16`, stopping on the first `SUCCEEDED(hr)`.
5. Poll `FindWindowExW(HWND_MESSAGE, ..., "TBWidget.TaskbarTap.26100", ...)` for at most 2 seconds with 50ms intervals.
6. Send the requested command using `SendMessageTimeoutW(..., SMTO_ABORTIFHUNG, 1000, ...)`.

Return HRESULTs in eight-digit hexadecimal form and include the failed stage in every error.

- [ ] **Step 3: Implement the manager's message-only worker**

Create a dedicated Rust thread with a message-only window. Register `TaskbarCreated` and handle:

```text
TaskbarCreated
WM_DISPLAYCHANGE
WM_SETTINGCHANGE where wParam == SPI_SETWORKAREA
WM_DPICHANGED
```

On each relevant message, re-detect the backend and reapply the current atomic mode after a single 200ms Explorer-settle delay. Do not add a periodic polling loop.

- [ ] **Step 4: Integrate manager ownership into application state**

Add `taskbar_transparency: TaskbarTransparencyManager` to `WidgetState`. Construct it in `setup_app` after log/database/config initialization. Replace direct calls to `apply_taskbar_transparency` in startup and `save_widget_settings` with `state.taskbar_transparency.set_mode(...)`.

For `quit_application`, call `restore()` before `app.exit(0)`. Implement `Drop` as a best-effort second restore without panicking.

- [ ] **Step 5: Run focused and crate tests**

Run: `cargo test taskbar_transparency::tests --lib`

Expected: all selection and endpoint tests PASS.

Run: `cargo check --lib`

Expected: PASS.

- [ ] **Step 6: Manual clear/restore test on the current 24H2 machine**

Run the normal Tauri development command, open settings, and toggle TASKBAR transparent on and off.

Expected:

- Clear: background and top stroke disappear; taskbar icons remain visible and interactive.
- Off: the original background and stroke return without restarting Explorer.
- `widget.log`: contains build 26100-or-current, backend `XamlTap`, injection result, and clear/restore result.

If the TAP cannot locate the exact 24H2 path, stop at this task and inspect logged runtime class/name/parent information only for elements below `Taskbar.TaskbarFrame`; do not broaden mutations to unrelated XAML controls.

---

### Task 5: DPI, Display, and Taskbar Widget Reflow

**Files:**
- Modify: `src-tauri/src/taskbar.rs`
- Modify: `src-tauri/src/taskbar_transparency.rs`
- Modify: `src-tauri/src/lib.rs`
- Modify: `src/App.vue`

**Interfaces:**
- Produces: `pub fn reflow(window: &WebviewWindow, position: WidgetPosition) -> Result<(), String>`.
- Produces: Tauri event `display-environment-changed` with payload `{ reason: String }`.
- Consumes: existing `set_width`, `GetDpiForWindow`, and Vue `syncWindowWidth`.

- [ ] **Step 1: Add pure DPI conversion boundary tests**

Move `logical_to_physical_width` into a testable section and add:

```rust
#[test]
fn width_scales_for_common_dpi_values() {
    assert_eq!(logical_to_physical_width(260, 96), 260);
    assert_eq!(logical_to_physical_width(260, 144), 390);
    assert_eq!(logical_to_physical_width(260, 192), 520);
}
```

Run: `cargo test taskbar::tests --lib`

Expected: PASS for the existing formula.

- [ ] **Step 2: Add `taskbar::reflow`**

Read the current child window rectangle to recover its logical width from the current taskbar DPI, then call the same positioning calculation used by `set_width`. Do not duplicate `SetWindowPos` logic; extract the shared physical placement into one internal function.

- [ ] **Step 3: Emit a display event from the manager worker**

Give `TaskbarTransparencyManager` an `AppHandle` only after Tauri setup is complete through `attach_app_handle(app.handle().clone())`. On `WM_DISPLAYCHANGE`, work-area change, and `WM_DPICHANGED`, emit `display-environment-changed` after reapplying transparency.

- [ ] **Step 4: Resynchronize Vue width without changing widget markup**

Register one additional Tauri listener during `initializeWidget`:

```ts
unlistenDisplay = await listen("display-environment-changed", async () => {
  lastReportedWidth = 0;
  await nextTick();
  await syncWindowWidth();
});
```

Dispose it in `cleanupWidget`. Keep the existing horizontal taskbar template and CSS byte-for-byte unchanged.

- [ ] **Step 5: Verify DPI behavior**

Run: `cargo test taskbar::tests --lib`

Expected: all width tests PASS.

Run: `npm run build`

Expected: Vue type check and Vite build PASS.

Manually switch 100%, 150%, and 200% scaling or equivalent resolutions.

Expected: the widget remains aligned to the selected side, uses the taskbar's full height, and reports a newly scaled physical width after every change.

---

### Task 6: Final Transparency Verification

**Files:**
- Inspect only: all files listed above.

**Interfaces:**
- Consumes: completed clear/restore, Explorer lifecycle, and DPI reflow paths.
- Produces: verification evidence for the implementation handoff.

- [ ] **Step 1: Run the minimal automated suite**

Run: `cargo test --lib`

Expected: PASS.

Run: `cargo check --lib`

Expected: PASS and helper DLL link succeeds.

Run: `npm run build`

Expected: PASS.

- [ ] **Step 2: Exercise Explorer restart recovery**

With clear mode enabled, restart Explorer through Task Manager, not through a scripted kill command.

Expected: after `TaskbarCreated`, the manager reinjects the TAP and restores the completely clear background without restarting tbwidget.

- [ ] **Step 3: Exercise safe failure behavior**

Temporarily run with transparency off, verify the stock taskbar remains unchanged, then enable it once.

Expected: a helper error never hides icons, blocks Explorer input, or terminates Explorer; the error is present in `widget.log` and the application remains usable.

- [ ] **Step 4: Review scope without Git**

Run: `rg -n "TranslucentTB|T[O]DO|T[B]D|unwrap\(|expect\(" src-tauri/src/taskbar_transparency.rs src-tauri/taskbar-helper src-tauri/build.rs src-tauri/src/lib.rs src-tauri/src/taskbar.rs src/App.vue`

Expected: no copied TranslucentTB identifiers, no placeholders, and no new panic path in runtime transparency code.
