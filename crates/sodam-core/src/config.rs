//! 应用配置。Windows 默认保存在可执行文件旁，其它平台使用系统配置目录。

use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::log_outlet::{log_event, LogLevel, LogTopic};

pub(crate) static SETTINGS_WRITE_LOCK: Mutex<()> = Mutex::new(());

/// 界面与播放设置。
///
/// `#[serde(default)]` + 逐字段缺省：旧版本配置里被删掉的字段（cookie、音质、缓存上限…）
/// 会在读取时被直接忽略，不会破坏解析。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// 当前音量与静音前音量（0..=1）。旧配置缺字段时沿用 100%。
    pub volume: f32,
    pub volume_before_mute: f32,
    /// 播放模式：列表循环 / 单曲循环 / 随机播放。旧配置缺字段时用列表循环。
    #[serde(default)]
    pub play_mode: crate::queue::PlayMode,
    /// 界面语言：`zh` / `en`；空或 `auto` = 跟随系统语言。
    pub language: String,
    /// Linux 下是否自绘窗口控制按钮：`auto`（默认，按桌面装饰能力）/ `always` / `never`。
    /// Windows 与 macOS 由系统负责，此项不生效。
    pub titlebar: String,
    /// 播放/暂停快捷键（gpui 的 keystroke 写法，如 `space`、`ctrl-left`）。
    /// 空串 = 用内置默认值 `DEFAULT_SHORTCUT_*`。
    pub shortcut_toggle: String,
    /// 上一首。
    pub shortcut_previous: String,
    /// 下一首。
    pub shortcut_next: String,
    /// 详细日志：打开后把所有话题都提到 DEBUG，即时生效、重启保持。
    ///
    /// 与环境变量是并集关系——两边都只能「记更多」，不存在谁覆盖谁。
    #[serde(default)]
    pub detailed_logs: bool,
    /// 本地音乐根目录；空串 = 还没选过。
    ///
    /// 存绝对路径而不是相对路径：应用的工作目录随启动方式变化（命令行、托盘、安装器），
    /// 相对路径下次启动就指向别处了。
    #[serde(default)]
    pub local_music_folder: String,
    /// 双语歌词是否显示第二行翻译。默认开启；旧配置缺字段时也按 true 处理。
    #[serde(default = "default_true")]
    pub lyrics_translation: bool,
    /// 任务栏快捷播控是否开启。默认关闭；开启后由宿主层在任务栏右侧显示播放条。
    #[serde(default)]
    pub taskbar_player: bool,
    /// 自动响度均衡是否开启。默认关闭。
    #[serde(default)]
    pub loudness_equalization: bool,
    /// 旧版本的上次播放记录（`local:绝对路径`）与进度（毫秒）；空串 = 没有。
    ///
    /// 完整会话现在由 `session` 模块的 `session.json` / `checkpoint.json` 保存；
    /// 这两个字段只在升级后的第一次启动读一次（旧版本里播过歌的用户不至于丢恢复），
    /// 不再由本应用写入。
    #[serde(default)]
    pub last_track_id: String,
    #[serde(default)]
    pub last_position_ms: u64,
}

fn default_true() -> bool {
    true
}

/// 播放控制快捷键的内置默认值。
///
/// 播放/暂停三平台都用 `Space`；上一首/下一首按平台分开：
/// * macOS 用 `⌘←` / `⌘→`（Apple Music 就是这两个键）。**不能用 `⌃←`/`⌃→`**：
///   那是系统「切换桌面（Move left/right a space）」的默认快捷键，真键盘按下去
///   会被系统先吃掉，应用根本收不到（`com.apple.symbolichotkeys` 里的 key 79/81）。
/// * Windows / Linux 用 `Ctrl+←` / `Ctrl+→`（浏览器与播放器的通行做法，系统没占用）。
///
/// 播放操作绑在 `!input` 上下文上，所以在文本框里 `⌘←` 仍是「行首」，不受影响。
pub const DEFAULT_SHORTCUT_TOGGLE: &str = "space";
#[cfg(target_os = "macos")]
pub const DEFAULT_SHORTCUT_PREVIOUS: &str = "cmd-left";
#[cfg(not(target_os = "macos"))]
pub const DEFAULT_SHORTCUT_PREVIOUS: &str = "ctrl-left";
#[cfg(target_os = "macos")]
pub const DEFAULT_SHORTCUT_NEXT: &str = "cmd-right";
#[cfg(not(target_os = "macos"))]
pub const DEFAULT_SHORTCUT_NEXT: &str = "ctrl-right";

/// 配置为空（或只有空白）时回退到内置默认值。
fn effective_shortcut<'a>(value: &'a str, default: &'a str) -> &'a str {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        default
    } else {
        trimmed
    }
}

/// 把旧版本录制时写下的**显示形式**改回可解析的规范存储串。
///
/// 只认三类无歧义的本应用产物，其余一律返回 `None`（保持原样）：
/// * 符号修饰键：`^`→`ctrl`、`⌥`→`alt`、`⇧`→`shift`、`⌘`/`⊞`/`❖`→本平台主修饰键；
/// * 符号键名：`←↑→↓`→`left/right/up/down`；
/// * 被大写化的单字符键：`ctrl-A`→`ctrl-a`（旧 Display 把 `Ctrl+K` 写成大写，
///   而 parse 把大写读成「带 shift」，所以小写化是还原用户本意）。
///
/// 已经是合法 ASCII 写法的（`ctrl-left`、`cmd-left`、`secondary-a`）**一概不改**：
/// 改写用户自己写进配置文件的值，会让他找不到原来写的是什么。认不出的值保持原样，
/// 启动时不会被绑定，也不在这里猜。
pub(crate) fn normalize_legacy_keystroke(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let looks_like_our_output = !value.is_ascii()
        || value.split('-').any(|part| {
            let mut chars = part.chars();
            matches!(chars.next(), Some(ch) if ch.is_ascii_uppercase()) && chars.next().is_none()
        });
    if !looks_like_our_output {
        return None;
    }

    let mut function = false;
    let mut control = false;
    let mut alt = false;
    let mut platform = false;
    let mut shift = false;
    let mut key = String::new();
    for component in value.split('-') {
        match component {
            "" => {}
            "ctrl" | "control" => control = true,
            "alt" => alt = true,
            "shift" => shift = true,
            "fn" | "function" => function = true,
            "secondary" | "cmd" | "command" | "super" | "win" => platform = true,
            "left" | "right" | "up" | "down" => key = component.to_string(),
            other => {
                for ch in other.chars() {
                    match ch {
                        '^' => control = true,
                        '⌥' => alt = true,
                        '⇧' => shift = true,
                        '⌘' | '⊞' | '❖' => platform = true,
                        '←' => key = "left".into(),
                        '→' => key = "right".into(),
                        '↑' => key = "up".into(),
                        '↓' => key = "down".into(),
                        ch if ch.is_ascii_uppercase() => key.push(ch.to_ascii_lowercase()),
                        ch if ch.is_ascii() => key.push(ch),
                        _ => return None,
                    }
                }
            }
        }
    }
    if key.is_empty() {
        return None;
    }

    let mut parts: Vec<&str> = Vec::with_capacity(6);
    if function {
        parts.push("fn");
    }
    if control {
        parts.push("ctrl");
    }
    if alt {
        parts.push("alt");
    }
    if platform {
        // 与 gpui `Keystroke::unparse` 的写法一致，免得同一个修饰键在配置里有两种拼法
        // （`secondary` 在非 macOS 上会被 parse 成 ctrl，绝不能拿来表示徽标键）。
        parts.push(if cfg!(target_os = "macos") {
            "cmd"
        } else if cfg!(target_os = "windows") {
            "win"
        } else {
            "super"
        });
    }
    if shift {
        parts.push("shift");
    }
    parts.push(key.as_str());
    let normalized = parts.join("-");
    (normalized != value).then_some(normalized)
}

/// Linux 下自绘窗口控制按钮（标题栏）的策略。
///
/// 不同桌面对 xdg-decoration 的回答差别很大：
/// * GNOME 等：协商成客户端装饰，告诉应用「你自己画」——必须自绘，否则没有按钮；
/// * Hyprland / sway 等 wlroots 平铺桌面：协商成服务端装饰，但自己同样不画标题栏，
///   这类桌面上窗口本来就由键盘/手势管理，通常不需要应用再画一套。
///
/// 所以默认 `Auto` 直接跟随协商结果，需要时可手改配置强制开或关。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TitlebarMode {
    /// 按桌面协商结果决定（默认）。
    #[default]
    Auto,
    /// 始终自绘。
    Always,
    /// 始终不自绘。
    Never,
}

impl TitlebarMode {
    pub fn from_key(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "always" | "on" | "show" => Self::Always,
            "never" | "off" | "hide" => Self::Never,
            _ => Self::Auto,
        }
    }

    /// `client_decorated` 来自 gpui 的 `Decorations::Client`：桌面是否要求应用自绘。
    pub fn draws_titlebar(self, client_decorated: bool) -> bool {
        match self {
            Self::Auto => client_decorated,
            Self::Always => true,
            Self::Never => false,
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            volume: 1.0,
            volume_before_mute: 1.0,
            play_mode: crate::queue::PlayMode::default(),
            language: String::new(),
            titlebar: String::new(),
            shortcut_toggle: String::new(),
            shortcut_previous: String::new(),
            shortcut_next: String::new(),
            detailed_logs: false,
            // 空串 = 「还没选过文件夹」，界面据此显示空态；不能用当前目录之类猜一个默认值。
            local_music_folder: String::new(),
            lyrics_translation: true,
            taskbar_player: false,
            loudness_equalization: false,
            last_track_id: String::new(),
            last_position_ms: 0,
        }
    }
}

impl Settings {
    pub fn config_path() -> PathBuf {
        if let Some(path) = std::env::var_os("SODAM_CONFIG_PATH").filter(|path| !path.is_empty()) {
            return path.into();
        }
        #[cfg(target_os = "windows")]
        {
            std::env::current_exe()
                .ok()
                .and_then(|exe| exe.parent().map(|dir| dir.join("config.json")))
                .unwrap_or_else(|| PathBuf::from("config.json"))
        }
        #[cfg(not(target_os = "windows"))]
        Self::user_config_path()
    }

    fn user_config_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("sodam")
            .join("config.json")
    }

    /// 读配置；文件不存在时返回默认值（不报错）。
    pub fn load() -> Self {
        let path = Self::config_path();
        #[cfg(target_os = "windows")]
        if !std::env::var_os("SODAM_CONFIG_PATH").is_some_and(|path| !path.is_empty()) {
            let legacy = Self::user_config_path();
            if let Err(error) = Self::import_legacy_config(&legacy, &path) {
                let detail = error.to_string();
                log_event(
                    LogLevel::Warn,
                    LogTopic::App,
                    "config_import_failed",
                    &[("detail", detail.as_str())],
                );
            }
        }
        Self::load_from(&path)
    }

    #[cfg(any(target_os = "windows", test))]
    fn import_legacy_config(legacy: &Path, target: &Path) -> anyhow::Result<()> {
        if target.try_exists()? {
            return Ok(());
        }
        let text = match std::fs::read_to_string(legacy) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        // 只导入当前支持的字段，旧版账号等字段不会跟着复制。
        let (mut settings, _) = serde_json::from_str::<Self>(&text)?.migrated();
        settings.normalize_playback();
        let parent = target
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        file.write_all(&serde_json::to_vec_pretty(&settings)?)?;
        match file.persist_noclobber(target) {
            Ok(_) => Ok(()),
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
            Err(error) => Err(error.error.into()),
        }
    }

    pub fn load_from(path: &Path) -> Self {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            // 文件不存在是正常首次启动，不报错。
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(error) => {
                let detail = error.to_string();
                let path_text = path.display().to_string();
                log_event(
                    LogLevel::Warn,
                    LogTopic::App,
                    "config_read_failed",
                    &[("path", path_text.as_str()), ("detail", detail.as_str())],
                );
                return Self::default();
            }
        };
        let settings = match serde_json::from_str::<Self>(&text) {
            Ok(settings) => settings,
            Err(error) => {
                let detail = error.to_string();
                let path_text = path.display().to_string();
                log_event(
                    LogLevel::Warn,
                    LogTopic::App,
                    "config_parse_failed",
                    &[("path", path_text.as_str()), ("detail", detail.as_str())],
                );
                return Self::default();
            }
        };
        let (mut settings, migrated) = settings.migrated();
        settings.normalize_playback();
        // 迁移后立刻落盘：用户下次打开配置文件看到的，就是真正生效的那份。
        // 写失败不影响启动，内存里的值已经是迁移后的。
        if migrated {
            if let Err(error) = settings.save_to(path) {
                let detail = error.to_string();
                log_event(
                    LogLevel::Warn,
                    LogTopic::App,
                    "config_migration_save_failed",
                    &[("detail", detail.as_str())],
                );
            }
        }
        settings
    }

    /// 把旧版本配置里的**显示形式**快捷键改到规范形式。
    ///
    /// 返回是否发生了改动（改动后由 `load_from` 回存）。
    fn migrated(mut self) -> (Self, bool) {
        let mut changed = false;

        // 旧版本把按键的**显示形式**写进了配置（`⌘←`、`^←`、`ctrl-A`），这些串 parse 不回
        // 原本那个键，绑上就是永不触发的死键（#39）。这里只改写「认得出是本应用产物」的值；
        // 认不出的保持原样，启动时不绑定。
        for field in [
            &mut self.shortcut_toggle,
            &mut self.shortcut_previous,
            &mut self.shortcut_next,
        ] {
            if let Some(normalized) = normalize_legacy_keystroke(field) {
                *field = normalized;
                changed = true;
            }
        }

        (self, changed)
    }

    /// 写配置（自动建目录）。
    pub fn save(&self) -> anyhow::Result<()> {
        self.save_to(&Self::config_path())
    }

    pub fn save_to(&self, path: &Path) -> anyhow::Result<()> {
        let _guard = SETTINGS_WRITE_LOCK
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        self.save_to_locked(path)
    }

    pub(crate) fn save_to_locked(&self, path: &Path) -> anyhow::Result<()> {
        let parent = path
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        std::fs::create_dir_all(parent)?;
        // 同目录原子替换，避免退出/并发保存留下半份 JSON（Windows 也支持替换旧文件）。
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        file.write_all(&serde_json::to_vec_pretty(self)?)?;
        file.persist(path).map_err(|err| err.error)?;
        Ok(())
    }

    pub fn normalize_playback(&mut self) {
        self.volume = if self.volume.is_finite() {
            self.volume.clamp(0.0, 1.0)
        } else {
            1.0
        };
        self.volume_before_mute =
            if self.volume_before_mute.is_finite() && self.volume_before_mute > 0.0 {
                self.volume_before_mute.min(1.0)
            } else {
                1.0
            };
    }

    pub fn set_volume(&mut self, volume: f32) {
        if !volume.is_finite() {
            return;
        }
        self.volume = volume.clamp(0.0, 1.0);
        if self.volume > 0.0 {
            self.volume_before_mute = self.volume;
        }
    }

    pub fn toggle_mute(&mut self) {
        if self.volume > 0.0 {
            self.volume_before_mute = self.volume;
            self.volume = 0.0;
        } else {
            self.set_volume(self.volume_before_mute);
        }
    }

    /// 生效的播放/暂停快捷键（配置为空时用内置默认值）。
    pub fn shortcut_toggle(&self) -> &str {
        effective_shortcut(&self.shortcut_toggle, DEFAULT_SHORTCUT_TOGGLE)
    }

    /// 生效的上一首快捷键。
    pub fn shortcut_previous(&self) -> &str {
        effective_shortcut(&self.shortcut_previous, DEFAULT_SHORTCUT_PREVIOUS)
    }

    /// 生效的下一首快捷键。
    pub fn shortcut_next(&self) -> &str {
        effective_shortcut(&self.shortcut_next, DEFAULT_SHORTCUT_NEXT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_config_without_online_fields_still_loads() {
        // 纯本地版删掉了 cookie / 签名 / 音质 / 缓存上限 / 搜索历史等字段；
        // 旧 config.json 里带着它们，读取必须不受影响。
        let mut settings: Settings = serde_json::from_str(
            r#"{"theme":"dark","cookie":"sessionid_ss=x","quality":"lossless","search_history":["a"]}"#,
        )
        .expect("老配置应能解析");
        // 解析成功本身就是断言：被删掉的字段被 serde 当作未知键忽略。

        assert_eq!(settings.volume, 1.0);
        settings.set_volume(0.31);
        settings.toggle_mute();
        settings.toggle_mute();
        assert_eq!(settings.volume, 0.31);
        settings.set_volume(f32::NAN);
        assert_eq!(settings.volume, 0.31);
        settings.volume = -5.0;
        settings.volume_before_mute = 0.0;
        settings.normalize_playback();
        assert_eq!(settings.volume, 0.0);
        assert_eq!(settings.volume_before_mute, 1.0);
    }

    /// 窗口按钮策略：默认 auto，未知值也回退 auto，老配置缺字段不影响。
    #[test]
    fn titlebar_mode_parses_and_resolves() {
        assert_eq!(TitlebarMode::default(), TitlebarMode::Auto);
        assert_eq!(TitlebarMode::from_key(""), TitlebarMode::Auto);
        assert_eq!(TitlebarMode::from_key("auto"), TitlebarMode::Auto);
        assert_eq!(TitlebarMode::from_key(" ALWAYS "), TitlebarMode::Always);
        assert_eq!(TitlebarMode::from_key("never"), TitlebarMode::Never);
        assert_eq!(TitlebarMode::from_key("nonsense"), TitlebarMode::Auto);

        // auto 跟随桌面：GNOME 这类要求客户端自绘 → 画；Hyprland 这类 → 不画
        assert!(TitlebarMode::Auto.draws_titlebar(true));
        assert!(!TitlebarMode::Auto.draws_titlebar(false));
        // 强制开关覆盖协商结果
        assert!(TitlebarMode::Always.draws_titlebar(false));
        assert!(!TitlebarMode::Never.draws_titlebar(true));

        // 旧配置没有 titlebar 字段时按默认值读出来
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, r#"{"theme":"dark","language":"zh"}"#).unwrap();
        let loaded = Settings::load_from(&path);
        assert_eq!(loaded.titlebar, "");
        assert_eq!(TitlebarMode::from_key(&loaded.titlebar), TitlebarMode::Auto);
    }

    /// 快捷键：默认值、空串回退、老配置缺字段也能读。
    #[test]
    fn shortcut_defaults_and_fallback() {
        let settings = Settings::default();
        assert_eq!(settings.shortcut_toggle(), DEFAULT_SHORTCUT_TOGGLE);
        assert_eq!(settings.shortcut_previous(), DEFAULT_SHORTCUT_PREVIOUS);
        assert_eq!(settings.shortcut_next(), DEFAULT_SHORTCUT_NEXT);

        let custom = Settings {
            shortcut_toggle: "alt-k".into(),
            // 只有空白也算未设置
            shortcut_previous: "   ".into(),
            ..Default::default()
        };
        assert_eq!(custom.shortcut_toggle(), "alt-k");
        assert_eq!(custom.shortcut_previous(), DEFAULT_SHORTCUT_PREVIOUS);

        // 老配置文件里没有这三个字段
        let parsed: Settings = serde_json::from_str(r#"{"cookie":"x"}"#).expect("老配置应能解析");
        assert_eq!(parsed.shortcut_next(), DEFAULT_SHORTCUT_NEXT);
        assert!(parsed.shortcut_toggle.is_empty());
    }

    /// gpui `unparse` 在本机给主修饰键用的拼法（macOS = cmd，Windows = win，其余 = super）。
    fn platform_key() -> &'static str {
        if cfg!(target_os = "macos") {
            "cmd"
        } else if cfg!(target_os = "windows") {
            "win"
        } else {
            "super"
        }
    }

    /// 旧 Display 写下来的符号形式被认出来并改写成规范串；合法写法与认不出的值一概不动。
    #[test]
    fn legacy_display_forms_migrate_and_legal_values_stay() {
        let expected = format!("{}-left", platform_key());
        assert_eq!(
            normalize_legacy_keystroke("⌘←").as_deref(),
            Some(expected.as_str())
        );
        assert_eq!(
            normalize_legacy_keystroke("^←").as_deref(),
            Some("ctrl-left"),
            "macOS 的 control 曾被写成不带连字符的 ^"
        );
        assert_eq!(
            normalize_legacy_keystroke("ctrl-A").as_deref(),
            Some("ctrl-a"),
            "大写单字符是旧 Display 的产物，本意是 Ctrl+A"
        );
        let expected = format!("{}-shift-i", platform_key());
        assert_eq!(
            normalize_legacy_keystroke("⇧⌘I").as_deref(),
            Some(expected.as_str())
        );

        // 已经是合法写法：不改，用户在配置文件里看到的仍是他写的东西。
        assert_eq!(normalize_legacy_keystroke("ctrl-left"), None);
        assert_eq!(normalize_legacy_keystroke("cmd-left"), None);
        assert_eq!(normalize_legacy_keystroke(""), None);
        // 认不出的符号不猜。
        assert_eq!(normalize_legacy_keystroke("☃"), None);
    }

    /// 启动读到旧格式：改写并回存，用户下次打开配置文件看到的就是真正生效的值。
    #[test]
    fn load_from_migrates_legacy_shortcut_in_place() {
        let dir = tempfile::tempdir().expect("临时目录");
        let path = dir.path().join("config.json");
        std::fs::write(&path, r#"{"cookie":"x","shortcut_previous":"⌘←"}"#).expect("写入临时配置");
        let expected = format!("{}-left", platform_key());

        let loaded = Settings::load_from(&path);
        assert_eq!(loaded.shortcut_previous, expected);

        let reread = std::fs::read_to_string(&path).expect("迁移后应回存");
        assert!(
            reread.contains(expected.as_str()),
            "配置文件里应已是规范串：{reread}"
        );
    }

    #[test]
    fn portable_config_import_keeps_existing_settings_and_leaves_old_file_intact() {
        let dir = tempfile::tempdir().unwrap();
        let legacy = dir.path().join("old.json");
        let target = dir.path().join("config.json");
        let old_text = r#"{"volume":0.4,"shortcut_previous":"^←","cookie":"obsolete"}"#;
        std::fs::write(&legacy, old_text).unwrap();

        Settings::import_legacy_config(&legacy, &target).unwrap();
        let settings = Settings::load_from(&target);
        assert_eq!(settings.volume, 0.4);
        assert_eq!(settings.shortcut_previous, "ctrl-left");
        assert!(!std::fs::read_to_string(&target).unwrap().contains("cookie"));
        assert_eq!(std::fs::read_to_string(&legacy).unwrap(), old_text);

        std::fs::write(&target, r#"{"volume":0.7}"#).unwrap();
        Settings::import_legacy_config(&legacy, &target).unwrap();
        assert_eq!(Settings::load_from(&target).volume, 0.7);
    }

    /// 详细日志：新配置默认关闭，旧配置缺字段也能读。
    #[test]
    fn detailed_logs_defaults_off_and_reads_old_configs() {
        assert!(!Settings::default().detailed_logs);
        let parsed: Settings = serde_json::from_str(r#"{"cookie":"x"}"#).expect("老配置应能解析");
        assert!(!parsed.detailed_logs);
        let enabled: Settings = serde_json::from_str(r#"{"detailed_logs":true}"#).expect("解析");
        assert!(enabled.detailed_logs);
    }

    /// 全量 round-trip：写出去再读回来不丢字段。
    #[test]
    fn round_trip_keeps_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let settings = Settings {
            local_music_folder: r"D:\music".into(),
            last_track_id: r"local:D:\music\a.mp3".into(),
            last_position_ms: 42_500,
            ..Default::default()
        };
        settings.save_to(&path).expect("save");

        let loaded = Settings::load_from(&path);
        assert_eq!(loaded.local_music_folder, r"D:\music");
        assert_eq!(loaded.last_track_id, r"local:D:\music\a.mp3");
        assert_eq!(loaded.last_position_ms, 42_500);
        assert!(loaded.language.is_empty());
    }
}
