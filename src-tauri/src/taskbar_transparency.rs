#[cfg(not(target_os = "windows"))]
use std::path::PathBuf;

use super::TaskbarTransparencyMode;

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

fn visual_diag_endpoint_names() -> Vec<String> {
    (1..=16)
        .map(|index| format!("VisualDiagConnection{index}"))
        .collect()
}

fn should_initialize_xaml_tap(mode: TaskbarTransparencyMode, helper_exists: bool) -> bool {
    mode == TaskbarTransparencyMode::Clear && !helper_exists
}

fn helper_window_belongs_to_explorer(candidate_pid: u32, explorer_pid: u32) -> bool {
    candidate_pid != 0 && candidate_pid == explorer_pid
}

fn tap_command_succeeded(command_result: usize) -> bool {
    command_result == 1
}

#[derive(Copy, Clone)]
enum RecoveryAttemptState {
    Initial,
    InFlight,
    RetryAfterStale,
}

fn should_start_recovery_attempt(state: RecoveryAttemptState) -> bool {
    matches!(
        state,
        RecoveryAttemptState::Initial | RecoveryAttemptState::RetryAfterStale
    )
}

#[cfg(target_os = "windows")]
mod platform {
    use std::{
        cell::{Cell, RefCell},
        ffi::{c_void, OsStr},
        fs,
        os::windows::ffi::OsStrExt,
        path::PathBuf,
        ptr::{null, null_mut},
        sync::{
            atomic::{AtomicBool, AtomicU32, Ordering},
            mpsc, Arc, Mutex,
        },
        thread::{self, JoinHandle},
        time::{Duration, Instant},
    };

    use serde::Serialize;
    use tauri::{AppHandle, Emitter, Manager, WebviewWindow};
    use windows_sys::{
        core::{GUID, HRESULT},
        Win32::{
            Foundation::{
                FreeLibrary, GetLastError, ERROR_CLASS_ALREADY_EXISTS, HWND, LPARAM, LRESULT,
                WPARAM,
            },
            System::{
                LibraryLoader::{
                    GetModuleHandleW, GetProcAddress, LoadLibraryExW, LoadLibraryW,
                    LOAD_LIBRARY_SEARCH_SYSTEM32,
                },
                SystemInformation::GetSystemDirectoryW,
            },
            UI::WindowsAndMessaging::{
                CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, EnumChildWindows,
                FindWindowExW, FindWindowW, GetClassNameW, GetMessageW, GetWindowThreadProcessId,
                PostMessageW, PostQuitMessage, RegisterClassW, RegisterWindowMessageW,
                SendMessageTimeoutW, TranslateMessage, HWND_MESSAGE, MSG, SMTO_ABORTIFHUNG,
                SPI_SETWORKAREA, WM_CLOSE, WM_DESTROY, WM_DISPLAYCHANGE, WM_DPICHANGED,
                WM_SETTINGCHANGE, WNDCLASSW,
            },
        },
    };

    use super::{
        super::{append_error_log, TaskbarTransparencyMode},
        backend_for_build, helper_window_belongs_to_explorer, should_initialize_xaml_tap,
        tap_command_succeeded, visual_diag_endpoint_names, TransparencyBackend,
    };

    const WCA_ACCENT_POLICY: u32 = 19;
    const ACCENT_DISABLED: i32 = 0;
    const ACCENT_ENABLE_TRANSPARENTGRADIENT: i32 = 2;
    const TAP_WINDOW_CLASS: &str = "TBWidget.TaskbarTap.26100";
    const TAP_CLEAR_MESSAGE: &str = "TBWidget.TaskbarTap.Clear";
    const TAP_RESTORE_MESSAGE: &str = "TBWidget.TaskbarTap.Restore";
    const WORKER_WINDOW_CLASS: &str = "TBWidget.TaskbarTransparency.Manager.0.1.0";
    const TASKBAR_CREATED_MESSAGE: &str = "TaskbarCreated";
    const XAML_BRIDGE_CLASS: &str = "Windows.UI.Composition.DesktopWindowContentBridge";

    static TAP_DLL_BYTES: &[u8] = include_bytes!(env!("TBWIDGET_TAP_DLL"));

    const TASKBAR_TAP_CLSID: GUID = GUID {
        data1: 0x8d5e8c44,
        data2: 0x1474,
        data3: 0x4e0d,
        data4: [0xb4, 0xa8, 0x6e, 0x2b, 0x86, 0x58, 0x0d, 0x2a],
    };

    #[repr(C)]
    struct RtlOsVersionInfoW {
        size: u32,
        major: u32,
        minor: u32,
        build: u32,
        platform_id: u32,
        service_pack: [u16; 128],
    }

    #[link(name = "ntdll")]
    extern "system" {
        fn RtlGetVersion(version: *mut RtlOsVersionInfoW) -> i32;
    }

    #[repr(C)]
    struct AccentPolicy {
        accent_state: i32,
        accent_flags: i32,
        gradient_color: u32,
        animation_id: i32,
    }

    #[repr(C)]
    struct WindowCompositionAttributeData {
        attribute: u32,
        data: *mut c_void,
        size_of_data: usize,
    }

    type SetWindowCompositionAttributeFn =
        unsafe extern "system" fn(HWND, *const WindowCompositionAttributeData) -> i32;
    type InitializeXamlDiagnosticsExFn = unsafe extern "system" fn(
        *const u16,
        u32,
        *const u16,
        *const u16,
        GUID,
        *const u16,
    ) -> HRESULT;

    struct ManagerInner {
        requested_clear: Arc<AtomicBool>,
        log_path: PathBuf,
        apply_lock: Mutex<()>,
        app_handle: Mutex<Option<AppHandle>>,
        recovery_in_flight: AtomicBool,
        xaml_tap_initializing_pid: Arc<AtomicU32>,
    }

    #[derive(Clone, Serialize)]
    struct DisplayEnvironmentChangedPayload {
        reason: String,
    }

    pub(crate) struct TaskbarTransparencyManager {
        inner: Arc<ManagerInner>,
        worker_hwnd: isize,
        worker_thread: Option<JoinHandle<()>>,
    }

    thread_local! {
        static WORKER_INNER: RefCell<Option<Arc<ManagerInner>>> = const { RefCell::new(None) };
        static WORKER_TASKBAR_CREATED: Cell<u32> = const { Cell::new(0) };
    }

    struct RecoveryInFlightGuard<'a>(&'a AtomicBool);

    const MAIN_WINDOW_RECOVERY_RESULT_TIMEOUT: Duration = Duration::from_secs(10);
    const TAP_WINDOW_READY_TIMEOUT: Duration = Duration::from_secs(20);

    impl Drop for RecoveryInFlightGuard<'_> {
        fn drop(&mut self) {
            self.0.store(false, Ordering::Release);
        }
    }

    impl TaskbarTransparencyManager {
        pub(crate) fn new(
            log_path: PathBuf,
            initial: TaskbarTransparencyMode,
        ) -> Result<Self, String> {
            let inner = Arc::new(ManagerInner {
                requested_clear: Arc::new(AtomicBool::new(initial == TaskbarTransparencyMode::Clear)),
                log_path,
                apply_lock: Mutex::new(()),
                app_handle: Mutex::new(None),
                recovery_in_flight: AtomicBool::new(false),
                xaml_tap_initializing_pid: Arc::new(AtomicU32::new(0)),
            });
            let (ready_tx, ready_rx) = mpsc::sync_channel(1);
            let worker_inner = Arc::clone(&inner);
            let worker_thread = match thread::Builder::new()
                .name("taskbar-transparency".to_string())
                .spawn(move || run_worker(worker_inner, ready_tx))
            {
                Ok(worker) => worker,
                Err(error) => {
                    let message = format!("stage=worker-spawn backend=unknown error={error}");
                    append_error_log(&inner.log_path, &message);
                    return Err(message);
                }
            };

            let worker_hwnd = match ready_rx.recv_timeout(Duration::from_secs(2)) {
                Ok(Ok(hwnd)) => hwnd,
                Ok(Err(error)) => {
                    let _ = worker_thread.join();
                    append_error_log(&inner.log_path, &error);
                    return Err(error);
                }
                Err(error) => {
                    let message =
                        format!("stage=worker-ready backend=unknown timeout=2s error={error}");
                    append_error_log(&inner.log_path, &message);
                    return Err(message);
                }
            };
            let manager = Self {
                inner,
                worker_hwnd,
                worker_thread: Some(worker_thread),
            };
            let _ = manager.set_mode(initial);
            Ok(manager)
        }

        pub(crate) fn set_mode(&self, mode: TaskbarTransparencyMode) -> Result<(), String> {
            self.inner
                .requested_clear
                .store(mode == TaskbarTransparencyMode::Clear, Ordering::Release);
            self.inner.apply_requested("set-mode")
        }

        pub(crate) fn restore(&self) -> Result<(), String> {
            self.set_mode(TaskbarTransparencyMode::Off)
        }

        pub(crate) fn attach_app_handle(&self, app_handle: AppHandle) -> Result<(), String> {
            let mut attached = self.inner.app_handle.lock().map_err(|error| {
                format!("stage=attach-app-handle backend=unknown error={error}")
            })?;
            *attached = Some(app_handle);
            Ok(())
        }
    }

    impl Drop for TaskbarTransparencyManager {
        fn drop(&mut self) {
            let _ = self.restore();
            if self.worker_hwnd != 0 {
                unsafe {
                    PostMessageW(self.worker_hwnd as HWND, WM_CLOSE, 0, 0);
                }
            }
            if let Some(worker_thread) = self.worker_thread.take() {
                let _ = worker_thread.join();
            }
        }
    }

    impl ManagerInner {
        fn requested_mode(&self) -> TaskbarTransparencyMode {
            if self.requested_clear.load(Ordering::Acquire) {
                TaskbarTransparencyMode::Clear
            } else {
                TaskbarTransparencyMode::Off
            }
        }

        fn apply_requested(&self, trigger: &str) -> Result<(), String> {
            let _guard = match self.apply_lock.lock() {
                Ok(guard) => guard,
                Err(error) => {
                    let message = format!(
                        "任务栏透明失败 stage=apply-lock backend=unknown trigger={trigger} error={error}"
                    );
                    append_error_log(&self.log_path, &message);
                    return Err(message);
                }
            };
            let mode = self.requested_mode();
            let build = match super::read_windows_build() {
                Ok(build) => build,
                Err(error) => {
                    let message = format!(
                        "任务栏透明失败 stage=RtlGetVersion backend=unknown trigger={trigger} {error}"
                    );
                    append_error_log(&self.log_path, &message);
                    return Err(message);
                }
            };
            let xaml_taskbar = match has_xaml_taskbar() {
                Ok(value) => value,
                Err(error) => {
                    let message = format!(
                        "任务栏透明失败 stage=detect-taskbar backend=unknown build={build} trigger={trigger} {error}"
                    );
                    append_error_log(&self.log_path, &message);
                    return Err(message);
                }
            };
            let backend = backend_for_build(build, xaml_taskbar);
            let result = match backend {
                TransparencyBackend::ClassicAccent => apply_classic(mode),
                TransparencyBackend::XamlTap => self.apply_xaml_tap(mode),
            };
            let mode_name = mode_name(mode);
            match result {
                Ok(()) => {
                    append_error_log(
                        &self.log_path,
                        &format!(
                            "任务栏透明成功 stage=apply backend={backend:?} build={build} trigger={trigger} mode={mode_name}"
                        ),
                    );
                    Ok(())
                }
                Err(error) => {
                    let message = format!(
                        "任务栏透明失败 stage=apply backend={backend:?} build={build} trigger={trigger} mode={mode_name} {error}"
                    );
                    append_error_log(&self.log_path, &message);
                    Err(message)
                }
            }
        }

        fn apply_xaml_tap(&self, mode: TaskbarTransparencyMode) -> Result<(), String> {
            let explorer_pid = current_explorer_pid()?;
            let existing_helper = find_tap_window(explorer_pid);
            if !should_initialize_xaml_tap(mode, existing_helper.is_some())
                && existing_helper.is_none()
            {
                return Ok(());
            }
            let helper_window = match existing_helper {
                Some(hwnd) => hwnd,
                None => {
                    start_xaml_tap_initialization(
                        explorer_pid,
                        self.log_path.clone(),
                        Arc::clone(&self.xaml_tap_initializing_pid),
                        Arc::clone(&self.requested_clear),
                    )?;
                    wait_for_tap_window(explorer_pid, Duration::from_millis(1_500), &self.log_path)?
                }
            };
            send_xaml_tap_command(helper_window, mode, explorer_pid)
        }

        fn emit_display_environment_changed(&self, reason: &str) {
            let app_handle = match self.app_handle.lock() {
                Ok(app_handle) => app_handle.clone(),
                Err(error) => {
                    append_error_log(
                        &self.log_path,
                        &format!(
                            "显示环境通知失败 stage=read-app-handle reason={reason} error={error}"
                        ),
                    );
                    return;
                }
            };
            let Some(app_handle) = app_handle else {
                return;
            };
            if let Err(error) = app_handle.emit(
                "display-environment-changed",
                DisplayEnvironmentChangedPayload {
                    reason: reason.to_string(),
                },
            ) {
                append_error_log(
                    &self.log_path,
                    &format!(
                        "显示环境通知失败 stage=emit-display-environment-changed reason={reason} error={error}"
                    ),
                );
            }
        }

        fn recover_taskbar_widget_after_recreation(&self) {
            if self.recovery_in_flight.swap(true, Ordering::AcqRel) {
                return;
            }
            let _in_flight_guard = RecoveryInFlightGuard(&self.recovery_in_flight);
            let app_handle = match self.app_handle.lock() {
                Ok(app_handle) => app_handle.clone(),
                Err(error) => {
                    append_error_log(
                        &self.log_path,
                        &format!(
                            "任务栏组件恢复失败 stage=read-app-handle trigger=TaskbarCreated error={error}"
                        ),
                    );
                    return;
                }
            };
            let Some(app_handle) = app_handle else {
                return;
            };

            let mut last_error = "main 窗口仍在等待 Tauri 移除失效句柄".to_string();
            let mut attempt_state = super::RecoveryAttemptState::Initial;
            for attempt in 1..=10 {
                if !super::should_start_recovery_attempt(attempt_state) {
                    break;
                }
                attempt_state = super::RecoveryAttemptState::InFlight;
                debug_assert!(!super::should_start_recovery_attempt(attempt_state));
                let (result_tx, result_rx) = mpsc::sync_channel(1);
                let cancelled = Arc::new(AtomicBool::new(false));
                let recovery_app = app_handle.clone();
                let recovery_cancelled = Arc::clone(&cancelled);
                if let Err(error) = app_handle.run_on_main_thread(move || {
                    if recovery_cancelled.load(Ordering::Acquire) {
                        return;
                    }
                    let result = crate::recover_main_taskbar_window(&recovery_app, || {
                        recovery_cancelled.load(Ordering::Acquire)
                    });
                    if recovery_cancelled.load(Ordering::Acquire) {
                        if let Ok(crate::MainWindowRecovery::Ready {
                            window,
                            rebuilt: true,
                        }) = result
                        {
                            let _ = window.destroy();
                        }
                        return;
                    }
                    if let Err(error) = result_tx.send(result) {
                        if let Ok(crate::MainWindowRecovery::Ready {
                            window,
                            rebuilt: true,
                        }) = error.0
                        {
                            let _ = window.destroy();
                        }
                    }
                }) {
                    last_error = format!("无法调度 Tauri 主线程恢复：{error}");
                    break;
                }

                match result_rx.recv_timeout(MAIN_WINDOW_RECOVERY_RESULT_TIMEOUT) {
                    Ok(Ok(crate::MainWindowRecovery::Ready { window, .. })) => {
                        if let Err(error) = self.restore_taskbar_widget(&app_handle, &window) {
                            last_error = error;
                            break;
                        }
                        return;
                    }
                    Ok(Ok(crate::MainWindowRecovery::Stale)) => {
                        last_error = format!("第 {attempt} 次检查仍在等待失效窗口释放");
                        attempt_state = super::RecoveryAttemptState::RetryAfterStale;
                    }
                    Ok(Err(error)) => {
                        last_error = error;
                        break;
                    }
                    Err(error) => {
                        cancelled.store(true, Ordering::Release);
                        last_error = format!("等待 Tauri 主线程恢复结果超时或失败：{error}");
                        break;
                    }
                }
                thread::sleep(Duration::from_millis(100));
            }

            append_error_log(
                &self.log_path,
                &format!(
                    "任务栏组件恢复失败 stage=recover-main-window trigger=TaskbarCreated error={last_error}"
                ),
            );
        }

        fn restore_taskbar_widget(
            &self,
            app_handle: &AppHandle,
            window: &WebviewWindow,
        ) -> Result<(), String> {
            window
                .set_ignore_cursor_events(true)
                .map_err(|error| format!("无法恢复任务栏组件鼠标穿透：{error}"))?;
            let position = app_handle
                .state::<crate::WidgetState>()
                .config
                .lock()
                .map(|config| config.position)
                .map_err(|error| format!("无法读取组件位置配置：{error}"))?;
            crate::taskbar::reflow(window, position)
                .map_err(|error| format!("无法在任务栏重建后重排组件：{error}"))
        }
    }

    pub(super) fn read_windows_build_impl() -> Result<u32, String> {
        let mut version = RtlOsVersionInfoW {
            size: std::mem::size_of::<RtlOsVersionInfoW>() as u32,
            major: 0,
            minor: 0,
            build: 0,
            platform_id: 0,
            service_pack: [0; 128],
        };
        let status = unsafe { RtlGetVersion(&mut version) };
        if status < 0 {
            return Err(format!("NTSTATUS=0x{:08X}", status as u32));
        }
        Ok(version.build)
    }

    fn has_xaml_taskbar() -> Result<bool, String> {
        let taskbar = find_shell_taskbar()?;
        let mut found = false;
        unsafe extern "system" fn find_bridge(hwnd: HWND, data: LPARAM) -> i32 {
            let found = unsafe { &mut *(data as *mut bool) };
            let mut class_name = [0u16; 128];
            let length =
                unsafe { GetClassNameW(hwnd, class_name.as_mut_ptr(), class_name.len() as i32) };
            if length > 0
                && String::from_utf16_lossy(&class_name[..length as usize]) == XAML_BRIDGE_CLASS
            {
                *found = true;
                return 0;
            }
            1
        }
        unsafe {
            EnumChildWindows(
                taskbar,
                Some(find_bridge),
                &mut found as *mut bool as LPARAM,
            );
        }
        Ok(found)
    }

    fn apply_classic(mode: TaskbarTransparencyMode) -> Result<(), String> {
        let taskbar = find_shell_taskbar()?;
        let user32 = to_wide("user32.dll");
        let library = unsafe { LoadLibraryW(user32.as_ptr()) };
        if library.is_null() {
            return Err(format!(
                "stage=LoadLibraryW(user32.dll) Win32Error={}",
                unsafe { GetLastError() }
            ));
        }
        let Some(address) =
            (unsafe { GetProcAddress(library, b"SetWindowCompositionAttribute\0".as_ptr()) })
        else {
            let error = unsafe { GetLastError() };
            unsafe {
                FreeLibrary(library);
            }
            return Err(format!(
                "stage=GetProcAddress(SetWindowCompositionAttribute) Win32Error={error}"
            ));
        };
        let set_window_composition_attribute =
            unsafe { std::mem::transmute::<_, SetWindowCompositionAttributeFn>(address) };
        let mut accent = AccentPolicy {
            accent_state: match mode {
                TaskbarTransparencyMode::Off => ACCENT_DISABLED,
                TaskbarTransparencyMode::Clear => ACCENT_ENABLE_TRANSPARENTGRADIENT,
            },
            accent_flags: if mode == TaskbarTransparencyMode::Clear {
                2
            } else {
                0
            },
            gradient_color: 0x0000_0000,
            animation_id: 0,
        };
        let data = WindowCompositionAttributeData {
            attribute: WCA_ACCENT_POLICY,
            data: &mut accent as *mut AccentPolicy as *mut c_void,
            size_of_data: std::mem::size_of::<AccentPolicy>(),
        };
        let applied = unsafe { set_window_composition_attribute(taskbar, &data) };
        let error = if applied == 0 {
            Some(unsafe { GetLastError() })
        } else {
            None
        };
        unsafe {
            FreeLibrary(library);
        }
        if let Some(error) = error {
            return Err(format!(
                "stage=SetWindowCompositionAttribute Win32Error={error}"
            ));
        }
        Ok(())
    }

    fn send_xaml_tap_command(
        helper_window: HWND,
        mode: TaskbarTransparencyMode,
        explorer_pid: u32,
    ) -> Result<(), String> {
        let message_name = match mode {
            TaskbarTransparencyMode::Off => TAP_RESTORE_MESSAGE,
            TaskbarTransparencyMode::Clear => TAP_CLEAR_MESSAGE,
        };
        let message_wide = to_wide(message_name);
        let message = unsafe { RegisterWindowMessageW(message_wide.as_ptr()) };
        if message == 0 {
            return Err(format!(
                "stage=RegisterWindowMessageW({message_name}) Win32Error={}",
                unsafe { GetLastError() }
            ));
        }
        let mut command_result = 0usize;
        let delivered = unsafe {
            SendMessageTimeoutW(
                helper_window,
                message,
                0,
                0,
                SMTO_ABORTIFHUNG,
                1_000,
                &mut command_result,
            )
        };
        if delivered == 0 {
            return Err(format!(
                "stage=SendMessageTimeoutW({message_name}) ExplorerPid={explorer_pid} commandResult={command_result} timeout=1000ms Win32Error={}",
                unsafe { GetLastError() }
            ));
        }
        if !tap_command_succeeded(command_result) {
            return Err(format!(
                "stage=validate-helper-command({message_name}) ExplorerPid={explorer_pid} commandResult={command_result} expected=1"
            ));
        }
        Ok(())
    }

    fn start_xaml_tap_initialization(
        explorer_pid: u32,
        log_path: PathBuf,
        initializing_pid: Arc<AtomicU32>,
        requested_clear: Arc<AtomicBool>,
    ) -> Result<(), String> {
        let active_pid = initializing_pid.load(Ordering::Acquire);
        if active_pid == explorer_pid {
            append_error_log(
                &log_path,
                &format!(
                    "任务栏透明诊断 stage=initialize-xaml-tap-already-running ExplorerPid={explorer_pid}"
                ),
            );
            return Ok(());
        }
        if active_pid == 0 {
            if initializing_pid
                .compare_exchange(0, explorer_pid, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                return Err(format!(
                    "stage=initialize-xaml-tap-in-flight ExplorerPid={explorer_pid}"
                ));
            }
        } else {
            append_error_log(
                &log_path,
                &format!(
                    "任务栏透明诊断 stage=initialize-xaml-tap-replace-pid oldExplorerPid={active_pid} newExplorerPid={explorer_pid}"
                ),
            );
            initializing_pid.store(explorer_pid, Ordering::Release);
        }

        append_error_log(
            &log_path,
            &format!(
                "任务栏透明诊断 stage=spawn-xaml-tap-init-thread ExplorerPid={explorer_pid}"
            ),
        );
        let initializing_pid_for_thread = Arc::clone(&initializing_pid);
        match thread::Builder::new()
            .name("tbwidget-xaml-tap-init".to_string())
            .spawn(move || {
                append_error_log(
                    &log_path,
                    &format!(
                        "任务栏透明诊断 stage=initialize-xaml-tap-thread-start ExplorerPid={explorer_pid}"
                    ),
                );
                let result = initialize_xaml_tap_worker(explorer_pid, &log_path);
                let still_current = initializing_pid_for_thread.compare_exchange(
                    explorer_pid,
                    0,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ).is_ok();
                match result {
                    Ok(hwnd) => {
                        if !still_current {
                            return;
                        }
                        append_error_log(
                            &log_path,
                            &format!(
                                "任务栏透明成功 stage=initialize-xaml-tap-thread ExplorerPid={explorer_pid}"
                            ),
                        );
                        if requested_clear.load(Ordering::Acquire) {
                            if let Err(error) =
                                send_xaml_tap_command(hwnd, TaskbarTransparencyMode::Clear, explorer_pid)
                            {
                                append_error_log(
                                    &log_path,
                                    &format!(
                                        "任务栏透明失败 stage=initialize-xaml-tap-thread-apply ExplorerPid={explorer_pid} {error}"
                                    ),
                                );
                            }
                        }
                    }
                    Err(error) => append_error_log(
                        &log_path,
                        &format!(
                            "任务栏透明失败 stage=initialize-xaml-tap-thread ExplorerPid={explorer_pid} {error}"
                        ),
                    ),
                }
            }) {
            Ok(_) => Ok(()),
            Err(error) => {
                initializing_pid.store(0, Ordering::Release);
                Err(format!("stage=spawn-xaml-tap-init-thread error={error}"))
            }
        }
    }

    fn wait_for_tap_window(
        explorer_pid: u32,
        timeout: Duration,
        log_path: &PathBuf,
    ) -> Result<HWND, String> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(hwnd) = find_tap_window(explorer_pid) {
                append_error_log(
                    log_path,
                    &format!(
                        "任务栏透明诊断 stage=wait-helper-window-observed ExplorerPid={explorer_pid} hwnd=0x{:X} timeout={}ms",
                        hwnd as usize,
                        timeout.as_millis()
                    ),
                );
                return Ok(hwnd);
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "stage=wait-helper-window ExplorerPid={explorer_pid} class=TBWidget.TaskbarTap.26100 timeout={}ms initialization=in-flight candidates={}",
                    timeout.as_millis(),
                    tap_window_search_summary(explorer_pid)
                ));
            }
            thread::sleep(Duration::from_millis(50));
        }
    }

    fn initialize_xaml_tap_worker(explorer_pid: u32, log_path: &PathBuf) -> Result<HWND, String> {
        let tap_path = materialize_tap_dll(log_path)?;
        append_error_log(
            log_path,
            &format!(
                "任务栏透明诊断 stage=materialize-helper-ok ExplorerPid={explorer_pid} path={}",
                tap_path.display()
            ),
        );
        let tap_path_wide = os_to_wide(tap_path.as_os_str());
        let xaml_path_wide = system32_xaml_path()?;
        let xaml_path =
            String::from_utf16_lossy(&xaml_path_wide[..xaml_path_wide.len().saturating_sub(1)]);
        append_error_log(
            log_path,
            &format!(
                "任务栏透明诊断 stage=load-xaml-library ExplorerPid={explorer_pid} path={xaml_path}"
            ),
        );
        let xaml_library = unsafe {
            LoadLibraryExW(
                xaml_path_wide.as_ptr(),
                null_mut(),
                LOAD_LIBRARY_SEARCH_SYSTEM32,
            )
        };
        if xaml_library.is_null() {
            return Err(format!(
                "stage=LoadLibraryExW(Windows.UI.Xaml.dll,System32) Win32Error={}",
                unsafe { GetLastError() }
            ));
        }
        let initialize_address =
            unsafe { GetProcAddress(xaml_library, b"InitializeXamlDiagnosticsEx\0".as_ptr()) };
        let Some(initialize_address) = initialize_address else {
            let error = unsafe { GetLastError() };
            unsafe {
                FreeLibrary(xaml_library);
            }
            return Err(format!(
                "stage=GetProcAddress(InitializeXamlDiagnosticsEx) Win32Error={error}"
            ));
        };
        let initialize =
            unsafe { std::mem::transmute::<_, InitializeXamlDiagnosticsExFn>(initialize_address) };
        let initialization_data = [0u16];
        let mut last_hresult = 0x8000_4005u32 as i32;
        let mut initialized = false;
        for endpoint in visual_diag_endpoint_names() {
            let endpoint_wide = to_wide(&endpoint);
            let hresult = unsafe {
                initialize(
                    endpoint_wide.as_ptr(),
                    explorer_pid,
                    xaml_path_wide.as_ptr(),
                    tap_path_wide.as_ptr(),
                    TASKBAR_TAP_CLSID,
                    initialization_data.as_ptr(),
                )
            };
            last_hresult = hresult;
            append_error_log(
                log_path,
                &format!(
                    "任务栏透明诊断 stage=InitializeXamlDiagnosticsEx ExplorerPid={explorer_pid} endpoint={endpoint} HRESULT=0x{:08X}",
                    hresult as u32
                ),
            );
            if hresult >= 0 {
                initialized = true;
                break;
            }
        }
        unsafe {
            FreeLibrary(xaml_library);
        }
        if !initialized {
            return Err(format!(
                "stage=InitializeXamlDiagnosticsEx ExplorerPid={explorer_pid} endpoints=VisualDiagConnection1..VisualDiagConnection16 HRESULT=0x{:08X}",
                last_hresult as u32
            ));
        }

        let deadline = Instant::now() + TAP_WINDOW_READY_TIMEOUT;
        append_error_log(
            log_path,
            &format!(
                "任务栏透明诊断 stage=wait-helper-window-start ExplorerPid={explorer_pid} class=TBWidget.TaskbarTap.26100 timeout={}ms",
                TAP_WINDOW_READY_TIMEOUT.as_millis()
            ),
        );
        loop {
            if let Some(hwnd) = find_tap_window(explorer_pid) {
                append_error_log(
                    log_path,
                    &format!(
                        "任务栏透明诊断 stage=wait-helper-window-ok ExplorerPid={explorer_pid} hwnd=0x{:X}",
                        hwnd as usize
                    ),
                );
                return Ok(hwnd);
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "stage=wait-helper-window ExplorerPid={explorer_pid} class=TBWidget.TaskbarTap.26100 timeout={}ms",
                    TAP_WINDOW_READY_TIMEOUT.as_millis()
                ));
            }
            thread::sleep(Duration::from_millis(50));
        }
    }

    fn tap_dll_fingerprint() -> u64 {
        TAP_DLL_BYTES.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
        })
    }

    fn materialize_tap_dll(log_path: &PathBuf) -> Result<PathBuf, String> {
        let directory = std::env::temp_dir().join("tbwidget");
        fs::create_dir_all(&directory).map_err(|error| {
            format!(
                "stage=create-helper-directory path={} error={error}",
                directory.display()
            )
        })?;
        let fingerprint = tap_dll_fingerprint();
        let path = directory.join(format!(
            "tbwidget-taskbar-tap-0.1.0-{}-{fingerprint:016x}.dll",
            TAP_DLL_BYTES.len()
        ));
        let must_write = match fs::read(&path) {
            Ok(existing) => {
                let same = existing.as_slice() == TAP_DLL_BYTES;
                append_error_log(
                    log_path,
                    &format!(
                        "任务栏透明诊断 stage=read-helper path={} exists=true bytes={} expectedBytes={} same={same}",
                        path.display(),
                        existing.len(),
                        TAP_DLL_BYTES.len()
                    ),
                );
                !same
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
            Err(error) => {
                if let Ok(metadata) = fs::metadata(&path) {
                    if metadata.len() == TAP_DLL_BYTES.len() as u64 {
                        append_error_log(
                            log_path,
                            &format!(
                                "任务栏透明诊断 stage=read-helper-locked-assume-ok path={} bytes={} fingerprint={fingerprint:016x} error={error}",
                                path.display(),
                                TAP_DLL_BYTES.len()
                            ),
                        );
                        return Ok(path);
                    }
                }
                return Err(format!(
                    "stage=read-helper path={} expectedBytes={} fingerprint={fingerprint:016x} error={error}",
                    path.display(),
                    TAP_DLL_BYTES.len()
                ));
            }
        };
        if must_write {
            fs::write(&path, TAP_DLL_BYTES).map_err(|error| {
                format!(
                    "stage=write-helper path={} bytes={} fingerprint={fingerprint:016x} error={error}",
                    path.display(),
                    TAP_DLL_BYTES.len()
                )
            })?;
            append_error_log(
                log_path,
                &format!(
                    "任务栏透明诊断 stage=write-helper-ok path={} bytes={} fingerprint={fingerprint:016x}",
                    path.display(),
                    TAP_DLL_BYTES.len()
                ),
            );
        }
        Ok(path)
    }

    fn system32_xaml_path() -> Result<Vec<u16>, String> {
        let mut buffer = vec![0u16; 32_768];
        let length = unsafe { GetSystemDirectoryW(buffer.as_mut_ptr(), buffer.len() as u32) };
        if length == 0 || length as usize >= buffer.len() {
            return Err(format!("stage=GetSystemDirectoryW Win32Error={}", unsafe {
                GetLastError()
            }));
        }
        buffer.truncate(length as usize);
        if !buffer.ends_with(&[b'\\' as u16]) {
            buffer.push(b'\\' as u16);
        }
        buffer.extend("Windows.UI.Xaml.dll".encode_utf16());
        buffer.push(0);
        Ok(buffer)
    }

    fn find_shell_taskbar() -> Result<HWND, String> {
        let class_name = to_wide("Shell_TrayWnd");
        let taskbar = unsafe { FindWindowW(class_name.as_ptr(), null()) };
        if taskbar.is_null() {
            return Err(format!(
                "stage=FindWindowW(Shell_TrayWnd) Win32Error={}",
                unsafe { GetLastError() }
            ));
        }
        Ok(taskbar)
    }

    fn current_explorer_pid() -> Result<u32, String> {
        let taskbar = find_shell_taskbar()?;
        let mut explorer_pid = 0u32;
        if unsafe { GetWindowThreadProcessId(taskbar, &mut explorer_pid) } == 0 || explorer_pid == 0
        {
            return Err(format!(
                "stage=GetWindowThreadProcessId(Shell_TrayWnd) Win32Error={}",
                unsafe { GetLastError() }
            ));
        }
        Ok(explorer_pid)
    }

    fn find_tap_window(explorer_pid: u32) -> Option<HWND> {
        find_tap_window_in_parent(HWND_MESSAGE, explorer_pid)
            .or_else(|| find_tap_window_in_parent(null_mut(), explorer_pid))
    }

    fn find_tap_window_in_parent(parent: HWND, explorer_pid: u32) -> Option<HWND> {
        let class_name = to_wide(TAP_WINDOW_CLASS);
        let mut previous = null_mut();
        loop {
            let hwnd = unsafe { FindWindowExW(parent, previous, class_name.as_ptr(), null()) };
            if hwnd.is_null() {
                return None;
            }
            let mut candidate_pid = 0u32;
            if unsafe { GetWindowThreadProcessId(hwnd, &mut candidate_pid) } != 0
                && helper_window_belongs_to_explorer(candidate_pid, explorer_pid)
            {
                return Some(hwnd);
            }
            previous = hwnd;
        }
    }

    fn tap_window_search_summary(explorer_pid: u32) -> String {
        format!(
            "messageOnly=[{}] topLevel=[{}]",
            tap_window_candidates(HWND_MESSAGE, explorer_pid),
            tap_window_candidates(null_mut(), explorer_pid)
        )
    }

    fn tap_window_candidates(parent: HWND, explorer_pid: u32) -> String {
        let class_name = to_wide(TAP_WINDOW_CLASS);
        let mut previous = null_mut();
        let mut candidates = Vec::new();
        loop {
            let hwnd = unsafe { FindWindowExW(parent, previous, class_name.as_ptr(), null()) };
            if hwnd.is_null() {
                break;
            }
            let mut candidate_pid = 0u32;
            let thread_id = unsafe { GetWindowThreadProcessId(hwnd, &mut candidate_pid) };
            candidates.push(format!(
                "hwnd=0x{:X},thread={},pid={},match={}",
                hwnd as usize,
                thread_id,
                candidate_pid,
                helper_window_belongs_to_explorer(candidate_pid, explorer_pid)
            ));
            previous = hwnd;
        }
        if candidates.is_empty() {
            "none".to_string()
        } else {
            candidates.join(";")
        }
    }

    fn run_worker(inner: Arc<ManagerInner>, ready: mpsc::SyncSender<Result<isize, String>>) {
        WORKER_INNER.with(|slot| {
            slot.replace(Some(Arc::clone(&inner)));
        });
        let taskbar_created_name = to_wide(TASKBAR_CREATED_MESSAGE);
        let taskbar_created = unsafe { RegisterWindowMessageW(taskbar_created_name.as_ptr()) };
        if taskbar_created == 0 {
            let _ = ready.send(Err(format!(
                "stage=RegisterWindowMessageW(TaskbarCreated) backend=unknown Win32Error={}",
                unsafe { GetLastError() }
            )));
            clear_worker_locals();
            return;
        }
        WORKER_TASKBAR_CREATED.with(|message| message.set(taskbar_created));

        let class_name = to_wide(WORKER_WINDOW_CLASS);
        let instance = unsafe { GetModuleHandleW(null()) };
        if instance.is_null() {
            let _ = ready.send(Err(format!(
                "stage=GetModuleHandleW backend=unknown Win32Error={}",
                unsafe { GetLastError() }
            )));
            clear_worker_locals();
            return;
        }
        let window_class = WNDCLASSW {
            lpfnWndProc: Some(worker_window_proc),
            hInstance: instance,
            lpszClassName: class_name.as_ptr(),
            ..WNDCLASSW::default()
        };
        if unsafe { RegisterClassW(&window_class) } == 0 {
            let error = unsafe { GetLastError() };
            if error != ERROR_CLASS_ALREADY_EXISTS {
                let _ = ready.send(Err(format!(
                    "stage=RegisterClassW backend=unknown Win32Error={error}"
                )));
                clear_worker_locals();
                return;
            }
        }
        let hwnd = unsafe {
            CreateWindowExW(
                0,
                class_name.as_ptr(),
                null(),
                0,
                0,
                0,
                0,
                0,
                null_mut(),
                null_mut(),
                instance,
                null(),
            )
        };
        if hwnd.is_null() {
            let _ = ready.send(Err(format!(
                "stage=CreateWindowExW(hidden-top-level) backend=unknown Win32Error={}",
                unsafe { GetLastError() }
            )));
            clear_worker_locals();
            return;
        }
        if ready.send(Ok(hwnd as isize)).is_err() {
            unsafe {
                DestroyWindow(hwnd);
            }
            clear_worker_locals();
            return;
        }

        let mut message = MSG::default();
        loop {
            let result = unsafe { GetMessageW(&mut message, null_mut(), 0, 0) };
            if result <= 0 {
                if result < 0 {
                    append_error_log(
                        &inner.log_path,
                        &format!(
                            "任务栏透明失败 stage=GetMessageW backend=unknown Win32Error={}",
                            unsafe { GetLastError() }
                        ),
                    );
                }
                break;
            }
            unsafe {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        clear_worker_locals();
    }

    unsafe extern "system" fn worker_window_proc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        let handled = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if message == WM_CLOSE {
                unsafe {
                    DestroyWindow(hwnd);
                }
                return Some(0);
            }
            if message == WM_DESTROY {
                unsafe {
                    PostQuitMessage(0);
                }
                return Some(0);
            }
            let taskbar_created = WORKER_TASKBAR_CREATED.with(Cell::get);
            let trigger = reapply_trigger(message, wparam, taskbar_created);
            if let Some(trigger) = trigger {
                thread::sleep(Duration::from_millis(200));
                WORKER_INNER.with(|slot| {
                    if let Ok(inner) = slot.try_borrow() {
                        if let Some(inner) = inner.as_ref() {
                            let _ = inner.apply_requested(trigger);
                            if trigger == "TaskbarCreated" {
                                inner.recover_taskbar_widget_after_recreation();
                            }
                            inner.emit_display_environment_changed(trigger);
                        }
                    }
                });
                Some(0)
            } else {
                None
            }
        }));
        match handled {
            Ok(Some(result)) => result,
            Ok(None) | Err(_) => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
        }
    }

    pub(super) fn reapply_trigger(
        message: u32,
        wparam: WPARAM,
        taskbar_created: u32,
    ) -> Option<&'static str> {
        if message == taskbar_created {
            Some("TaskbarCreated")
        } else if message == WM_DISPLAYCHANGE {
            Some("WM_DISPLAYCHANGE")
        } else if message == WM_SETTINGCHANGE && wparam as u32 == SPI_SETWORKAREA {
            Some("WM_SETTINGCHANGE/SPI_SETWORKAREA")
        } else if message == WM_DPICHANGED {
            Some("WM_DPICHANGED")
        } else {
            None
        }
    }

    fn clear_worker_locals() {
        WORKER_TASKBAR_CREATED.with(|message| message.set(0));
        WORKER_INNER.with(|slot| {
            slot.replace(None);
        });
    }

    fn to_wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn os_to_wide(value: &OsStr) -> Vec<u16> {
        value.encode_wide().chain(std::iter::once(0)).collect()
    }

    fn mode_name(mode: TaskbarTransparencyMode) -> &'static str {
        match mode {
            TaskbarTransparencyMode::Off => "off",
            TaskbarTransparencyMode::Clear => "clear",
        }
    }
}

#[cfg(target_os = "windows")]
pub(crate) use platform::TaskbarTransparencyManager;

#[cfg(target_os = "windows")]
pub(crate) fn read_windows_build() -> Result<u32, String> {
    platform::read_windows_build_impl()
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn read_windows_build() -> Result<u32, String> {
    Err("RtlGetVersion 仅支持 Windows".to_string())
}

#[cfg(not(target_os = "windows"))]
pub(crate) struct TaskbarTransparencyManager;

#[cfg(not(target_os = "windows"))]
impl TaskbarTransparencyManager {
    pub(crate) fn new(
        _log_path: PathBuf,
        _initial: TaskbarTransparencyMode,
    ) -> Result<Self, String> {
        Ok(Self)
    }

    pub(crate) fn set_mode(&self, _mode: TaskbarTransparencyMode) -> Result<(), String> {
        Ok(())
    }

    pub(crate) fn restore(&self) -> Result<(), String> {
        Ok(())
    }

    pub(crate) fn attach_app_handle(&self, _app_handle: tauri::AppHandle) -> Result<(), String> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_26100_xaml_uses_tap() {
        assert_eq!(
            backend_for_build(26_100, true),
            TransparencyBackend::XamlTap
        );
    }

    #[test]
    fn build_19045_uses_classic_accent() {
        assert_eq!(
            backend_for_build(19_045, false),
            TransparencyBackend::ClassicAccent
        );
    }

    #[test]
    fn classic_shell_on_new_build_stays_classic() {
        assert_eq!(
            backend_for_build(26_100, false),
            TransparencyBackend::ClassicAccent
        );
    }

    #[test]
    fn visual_diag_names_are_bounded_and_unique() {
        let names = visual_diag_endpoint_names();
        assert_eq!(names.len(), 16);
        assert_eq!(names.first().unwrap(), "VisualDiagConnection1");
        assert_eq!(names.last().unwrap(), "VisualDiagConnection16");
        let unique = names.iter().collect::<std::collections::HashSet<_>>();
        assert_eq!(unique.len(), names.len());
    }

    #[test]
    fn xaml_tap_is_only_initialized_for_clear_without_a_helper() {
        assert!(!should_initialize_xaml_tap(
            super::super::TaskbarTransparencyMode::Off,
            false
        ));
        assert!(should_initialize_xaml_tap(
            super::super::TaskbarTransparencyMode::Clear,
            false
        ));
        assert!(!should_initialize_xaml_tap(
            super::super::TaskbarTransparencyMode::Clear,
            true
        ));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn worker_routes_only_required_environment_messages() {
        let taskbar_created = 0xc001;
        assert_eq!(
            platform::reapply_trigger(taskbar_created, 0, taskbar_created),
            Some("TaskbarCreated")
        );
        assert_eq!(
            platform::reapply_trigger(0x007e, 0, taskbar_created),
            Some("WM_DISPLAYCHANGE")
        );
        assert_eq!(
            platform::reapply_trigger(0x001a, 47, taskbar_created),
            Some("WM_SETTINGCHANGE/SPI_SETWORKAREA")
        );
        assert_eq!(
            platform::reapply_trigger(0x02e0, 0, taskbar_created),
            Some("WM_DPICHANGED")
        );
        assert_eq!(platform::reapply_trigger(0x001a, 0, taskbar_created), None);
    }

    #[test]
    fn helper_window_must_belong_to_current_explorer() {
        assert!(helper_window_belongs_to_explorer(4_200, 4_200));
        assert!(!helper_window_belongs_to_explorer(4_199, 4_200));
        assert!(!helper_window_belongs_to_explorer(0, 4_200));
    }

    #[test]
    fn tap_command_only_accepts_protocol_success_value() {
        assert!(tap_command_succeeded(1));
        assert!(!tap_command_succeeded(0));
        assert!(!tap_command_succeeded(2));
    }

    #[test]
    fn recovery_retries_only_after_stale_and_never_while_inflight() {
        assert!(should_start_recovery_attempt(RecoveryAttemptState::Initial));
        assert!(!should_start_recovery_attempt(RecoveryAttemptState::InFlight));
        assert!(should_start_recovery_attempt(
            RecoveryAttemptState::RetryAfterStale
        ));
    }
}
