//! 单实例防护：第二次启动不再开第二个窗口，而是把已有窗口提到前台。
//!
//! 目标（issue #14）：双击图标第二次启动时，不创建新的 GUI 窗口，并让已经
//! 在跑的实例浮到最前面。
//!
//! * **Unix**：在用户运行时目录下 `bind` 一个 Unix socket（`sodam.sock`）。
//!   首实例监听；后来者 `connect` 一下并退出，首实例收到连接就激活主窗口。
//!   进程崩溃残留的 socket 文件会被识别出来（`connect` 失败）并清理后重建。
//! * **Windows**：命名互斥体判断是否已有实例；已有时用 `FindWindowW` 找到
//!   `SodaM` 窗口，`ShowWindow(SW_RESTORE)` + `SetForegroundWindow` 提到前台。
//!
//! 任何一步失败都退回「照常启动」——锁坏掉不能让人打不开应用。

use std::path::PathBuf;

use sodam_core::log_outlet::{log_event, LogLevel, LogTopic};

/// 单实例锁不可用时的统一出口：照常启动，但把原因记进日志。
///
/// 不记日志的话，「双击图标开第二个窗口」这类反馈查不到任何线索——
/// 用户看到的只是功能“偶尔失效”。
fn degraded(reason: &str) -> Instance {
    log_event(
        LogLevel::Warn,
        LogTopic::App,
        "single_instance_degraded",
        &[("reason", reason)],
    );
    Instance::Primary {
        _guard: Guard {
            _backend: Backend::None,
        },
        activations: std::sync::mpsc::channel().1,
    }
}

/// 启动结果。
pub enum Instance {
    /// 本进程是首个实例；`activations` 会在「又有人启动了一次」时收到通知。
    Primary {
        _guard: Guard,
        activations: std::sync::mpsc::Receiver<()>,
    },
    /// 已经有实例在跑：已尽力把它提到前台，本进程应当直接退出。
    Secondary,
}

/// 进程存活期间持有它，Drop 时释放单实例标记。
pub struct Guard {
    /// 只为 Drop 时释放资源而持有。
    _backend: Backend,
}

enum Backend {
    #[cfg(unix)]
    Socket {
        path: PathBuf,
        /// 只用来保持 socket 处于监听状态。
        _listener: std::os::unix::net::UnixListener,
    },
    #[cfg(target_os = "windows")]
    Mutex {
        /// 只用来保持互斥体不被释放。
        _handle: WindowsMutex,
    },
    /// 平台不支持时的空实现（照常启动）。
    None,
}

impl Drop for Backend {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Backend::Socket { path, .. } = self {
            // 只删自己创建的那个 socket 文件。
            let _ = std::fs::remove_file(path);
        }
    }
}

/// 取单实例。第二个实例会顺手请求已有窗口到前台。
pub fn acquire() -> Instance {
    acquire_at(default_socket_path())
}

/// 与 [`acquire`] 相同，但显式指定 Unix socket 路径（测试用）。
pub fn acquire_at(path: PathBuf) -> Instance {
    #[cfg(unix)]
    {
        acquire_unix(path)
    }
    #[cfg(target_os = "windows")]
    {
        let _ = path;
        acquire_windows()
    }
    #[cfg(not(any(unix, target_os = "windows")))]
    {
        let _ = path;
        degraded("platform unsupported")
    }
}

/// Unix socket 的默认路径：优先 `$XDG_RUNTIME_DIR`（Linux），其余退回配置目录。
fn default_socket_path() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR").filter(|dir| !dir.is_empty()) {
        return PathBuf::from(dir).join("sodam.sock");
    }
    sodam_core::config::Settings::config_path()
        .parent()
        .map(|dir| dir.join("instance.sock"))
        .unwrap_or_else(|| std::env::temp_dir().join("sodam-instance.sock"))
}

#[cfg(unix)]
fn acquire_unix(path: PathBuf) -> Instance {
    use std::os::unix::net::{UnixListener, UnixStream};

    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let listener = match UnixListener::bind(&path) {
        Ok(listener) => listener,
        Err(err) if err.kind() == std::io::ErrorKind::AddrInUse => {
            // 已有实例在听就通知它；连不上说明是崩溃残留的 socket 文件，清掉重来。
            if UnixStream::connect(&path).is_ok() {
                return Instance::Secondary;
            }
            let _ = std::fs::remove_file(&path);
            match UnixListener::bind(&path) {
                Ok(listener) => listener,
                Err(err) => {
                    return degraded(&format!("bind {}: {err}", path.display()));
                }
            }
        }
        Err(err) => return degraded(&format!("bind {}: {err}", path.display())),
    };

    let (tx, rx) = std::sync::mpsc::channel();
    // 独立线程只负责 accept，收到连接就发一个信号；不碰 UI 状态。
    let spawned = listener.try_clone().and_then(|accept_listener| {
        std::thread::Builder::new()
            .name("sodam-single-instance".to_string())
            .spawn(move || {
                for stream in accept_listener.incoming() {
                    if stream.is_ok() && tx.send(()).is_err() {
                        break;
                    }
                }
            })
            .map(|_| ())
    });
    if let Err(err) = spawned {
        let detail = err.to_string();
        log_event(
            LogLevel::Warn,
            LogTopic::App,
            "single_instance_accept_failed",
            &[("detail", detail.as_str())],
        );
    }

    Instance::Primary {
        _guard: Guard {
            _backend: Backend::Socket {
                path,
                _listener: listener,
            },
        },
        activations: rx,
    }
}

#[cfg(target_os = "windows")]
mod win {
    use std::ffi::c_void;

    pub type Handle = *mut c_void;
    pub const ERROR_ALREADY_EXISTS: u32 = 183;
    pub const SW_RESTORE: i32 = 9;
    pub const SW_SHOW: i32 = 5;
    pub const EVENT_MODIFY_STATE: u32 = 0x0002;
    pub const INFINITE: u32 = 0xFFFF_FFFF;
    pub const WAIT_OBJECT_0: u32 = 0;

    #[link(name = "kernel32")]
    extern "system" {
        pub fn CreateMutexW(attrs: *mut c_void, owner: i32, name: *const u16) -> Handle;
        pub fn GetLastError() -> u32;
        pub fn CloseHandle(handle: Handle) -> i32;
        pub fn CreateEventW(
            attrs: *mut c_void,
            manual_reset: i32,
            initial_state: i32,
            name: *const u16,
        ) -> Handle;
        pub fn OpenEventW(access: u32, inherit: i32, name: *const u16) -> Handle;
        pub fn SetEvent(handle: Handle) -> i32;
        pub fn WaitForSingleObject(handle: Handle, milliseconds: u32) -> u32;
    }

    #[link(name = "user32")]
    extern "system" {
        pub fn FindWindowW(class: *const u16, title: *const u16) -> Handle;
        pub fn IsIconic(hwnd: Handle) -> i32;
        pub fn ShowWindow(hwnd: Handle, cmd: i32) -> i32;
        pub fn SetForegroundWindow(hwnd: Handle) -> i32;
        pub fn BringWindowToTop(hwnd: Handle) -> i32;
    }

    pub fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }
}

#[cfg(target_os = "windows")]
struct WindowsMutex(win::Handle);

#[cfg(target_os = "windows")]
impl Drop for WindowsMutex {
    fn drop(&mut self) {
        unsafe { win::CloseHandle(self.0) };
    }
}

#[cfg(target_os = "windows")]
struct WindowsEvent(win::Handle);

// 内核事件句柄归这个包装独占；等待和关闭都可以在别的线程做。
#[cfg(target_os = "windows")]
unsafe impl Send for WindowsEvent {}

#[cfg(target_os = "windows")]
impl Drop for WindowsEvent {
    fn drop(&mut self) {
        unsafe { win::CloseHandle(self.0) };
    }
}

#[cfg(target_os = "windows")]
impl WindowsEvent {
    /// 等下一次激活信号；句柄失效时返回 `false`，线程退出。
    fn wait(&self) -> bool {
        unsafe { win::WaitForSingleObject(self.0, win::INFINITE) == win::WAIT_OBJECT_0 }
    }
}

#[cfg(target_os = "windows")]
fn acquire_windows() -> Instance {
    // `Local\` 前缀 = 每个登录会话一把锁，多用户同时登录互不影响。
    let name = win::wide(r"Local\SodaM-single-instance");
    let handle = unsafe { win::CreateMutexW(std::ptr::null_mut(), 0, name.as_ptr()) };
    if handle.is_null() {
        // 建不出互斥体就照常启动，不能因为锁失败让人打不开应用。
        let code = unsafe { win::GetLastError() };
        return degraded(&format!("CreateMutexW: error {code}"));
    }
    if unsafe { win::GetLastError() } == win::ERROR_ALREADY_EXISTS {
        unsafe { win::CloseHandle(handle) };
        // 窗口还在就顺手提到前台；窗口已经关了（留在托盘）也没关系，
        // 下面的事件会让首实例重建窗口。
        signal_existing_instance();
        focus_existing_window();
        return Instance::Secondary;
    }

    let (tx, rx) = std::sync::mpsc::channel();
    // 首实例没有窗口时（关到托盘），FindWindow 找不到窗口。命名事件是
    // 唯一能跨进程叫醒 UI 线程的通道：二次启动 set 一下，这边收到就
    // `show_window`，没有窗口就重新开一个。
    let event_name = win::wide(r"Local\SodaM-activate");
    let event = unsafe { win::CreateEventW(std::ptr::null_mut(), 0, 0, event_name.as_ptr()) };
    if event.is_null() {
        // 事件建不出来时锁仍然有效，只是「关到托盘后再启动」叫不醒这个实例。
        let code = unsafe { win::GetLastError() };
        let detail = format!("CreateEventW: error {code}");
        log_event(
            LogLevel::Warn,
            LogTopic::App,
            "single_instance_activate_unavailable",
            &[("detail", detail.as_str())],
        );
    } else {
        let event = WindowsEvent(event);
        let spawned = std::thread::Builder::new()
            .name("sodam-activate".to_string())
            .spawn(move || {
                while event.wait() {
                    if tx.send(()).is_err() {
                        break;
                    }
                }
            });
        if let Err(err) = spawned {
            let detail = err.to_string();
            log_event(
                LogLevel::Warn,
                LogTopic::App,
                "single_instance_activate_unavailable",
                &[("detail", detail.as_str())],
            );
        }
    }

    Instance::Primary {
        _guard: Guard {
            _backend: Backend::Mutex {
                _handle: WindowsMutex(handle),
            },
        },
        activations: rx,
    }
}

/// 通知已经在跑的首实例「有人又启动了一次」。
#[cfg(target_os = "windows")]
fn signal_existing_instance() {
    let name = win::wide(r"Local\SodaM-activate");
    let event = unsafe { win::OpenEventW(win::EVENT_MODIFY_STATE, 0, name.as_ptr()) };
    if event.is_null() {
        return;
    }
    unsafe {
        win::SetEvent(event);
        win::CloseHandle(event);
    }
}

/// 把已经在跑的那个窗口提到前台（Windows）。
#[cfg(target_os = "windows")]
fn focus_existing_window() {
    let title = win::wide("SodaM");
    let hwnd = unsafe { win::FindWindowW(std::ptr::null(), title.as_ptr()) };
    if hwnd.is_null() {
        return;
    }
    // 最小化时先还原；只是隐藏/最大化的窗口用 SW_SHOW，不能把它退回普通尺寸。
    unsafe {
        if win::IsIconic(hwnd) != 0 {
            win::ShowWindow(hwnd, win::SW_RESTORE);
        } else {
            win::ShowWindow(hwnd, win::SW_SHOW);
        }
        win::BringWindowToTop(hwnd);
        win::SetForegroundWindow(hwnd);
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// 每个用例一个独立目录，避免并行测试互相抢同一个 socket。
    fn temp_socket(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir =
            std::env::temp_dir().join(format!("sodam-si-{}-{tag}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        dir.join("sodam.sock")
    }

    /// 同一个 socket 路径：第一次拿到 Primary，第二次判定为 Secondary。
    #[test]
    fn second_instance_is_secondary() {
        let path = temp_socket("second");

        let first = acquire_at(path.clone());
        assert!(matches!(first, Instance::Primary { .. }));

        let second = acquire_at(path.clone());
        assert!(matches!(second, Instance::Secondary));

        drop(first);
        // 守卫释放后 socket 文件被清掉，可以重新获得
        let third = acquire_at(path);
        assert!(matches!(third, Instance::Primary { .. }));
    }

    /// 崩溃残留的 socket 文件（没人 listen）不应把新进程挡在外面。
    #[test]
    fn stale_socket_file_is_reclaimed() {
        let path = temp_socket("stale");
        {
            // bind 后立即 drop，文件留下但没人 accept
            let listener = std::os::unix::net::UnixListener::bind(&path).expect("bind");
            drop(listener);
        }
        assert!(path.exists());

        let instance = acquire_at(path.clone());
        assert!(matches!(instance, Instance::Primary { .. }));
        drop(instance);
        let _ = std::fs::remove_file(&path);
    }

    /// 第二个实例能通知到首实例（activations 收到一次）。
    #[test]
    fn second_instance_signals_the_first() {
        let path = temp_socket("signal");
        // 注意：`..` 会立刻丢掉守卫（等于释放 socket），必须显式绑定住。
        let Instance::Primary {
            activations,
            _guard,
        } = acquire_at(path.clone())
        else {
            panic!("first instance should be primary");
        };
        assert!(matches!(acquire_at(path), Instance::Secondary));

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while std::time::Instant::now() < deadline {
            if activations.try_recv().is_ok() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        panic!("第一个实例没有收到第二次启动的信号");
    }
}
