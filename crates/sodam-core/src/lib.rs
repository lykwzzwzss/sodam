//! sodam 的领域层：配置、本地音乐库、播放与界面数据模型。
//!
//! 这里**不依赖 GPUI**，方便单独测试与复用（未来做 CLI/TUI 或守护进程都能直接用）。

pub mod artwork;
pub mod audio;
pub mod config;
pub mod local;
pub mod log_outlet;
pub mod loudness;
pub mod lyrics;
pub mod models;
pub mod preferences;
pub mod queue;
pub mod session;

pub use audio::{PlaybackEngine, PlaybackSnapshot};
pub use config::Settings;
pub use log_outlet::{flush_global, LogLevel, LogTopic};
pub use models::TrackItem;
pub use queue::{PlayMode, Queue};

/// 软件本地数据根目录：固定在可执行文件所在目录下的 `data/` 文件夹。
pub fn app_data_root() -> std::path::PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|dir| dir.to_path_buf()))
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("data")
}
