//! 极简中英文案层。中文原文作为 key，便于在 UI 里渐进接入。

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Language {
    System,
    #[default]
    Chinese,
    English,
}

impl Language {
    pub fn from_key(key: &str) -> Option<Self> {
        match key.trim().to_ascii_lowercase().as_str() {
            "auto" | "system" => Some(Self::System),
            "zh" | "zh-cn" | "chinese" => Some(Self::Chinese),
            "en" | "en-us" | "english" => Some(Self::English),
            _ => None,
        }
    }

    pub fn system_locale() -> Self {
        sys_locale::get_locale()
            .as_deref()
            .map(|locale| {
                if locale.to_ascii_lowercase().starts_with("zh") {
                    Self::Chinese
                } else {
                    Self::English
                }
            })
            .unwrap_or(Self::Chinese)
    }

    pub fn resolved(self) -> Self {
        match self {
            Self::System => Self::system_locale(),
            language => language,
        }
    }

    pub fn is_zh(self) -> bool {
        self.resolved() == Self::Chinese
    }

    /// 静态文案翻译；中文原文作为稳定 key。
    pub fn text(self, key: &'static str) -> &'static str {
        let language = self.resolved();
        if language.is_zh() {
            return key;
        }
        Self::english_text(key).unwrap_or(key)
    }

    /// 英文词典查表；`None` 表示没有这一条，界面会回退中文原文。
    ///
    /// 词条按使用方分组：本地音乐页、播放条/队列、托盘与系统按钮、
    /// 快捷键卡片与杂项状态。
    fn english_text(key: &str) -> Option<&'static str> {
        Some(match key {
            // 通用短语。
            "{} 首" => "{} tracks",
            "播放" => "Play",
            "暂停" => "Pause",
            "上一首" => "Previous Track",
            "下一首" => "Next Track",
            "播放中" => "Playing",
            "已暂停" => "Paused",
            "未在播放" => "Not playing",
            "选择一首歌开始播放吧" => "Pick a song to start playing",
            "就绪" => "Ready",
            "正在播放" => "Now Playing",
            "退出" => "Quit",
            "SodaM" => "SodaM",

            // 本地音乐页。
            "本地音乐" => "Local Music",
            "本地音乐就绪：点击顶栏的文件夹图标选择音乐文件夹" => {
                "Local music is ready: click the folder icon in the top bar to pick a music folder"
            }
            "选择文件夹" => "Choose Folder",
            "正在扫描本地音乐…" => "Scanning local music…",
            "全部歌单" => "All Songs",
            "本地音乐文件夹" => "Local music folder",
            "这个平台暂不支持选择文件夹，请在 config.json 里填写 local_music_folder" => {
                "Choosing a folder is not available on this platform; set local_music_folder in config.json instead"
            }
            "选择一个文件夹，里面的音乐会进入「全部歌单」，每个子文件夹各自成为一个歌单。" => {
                "Choose a folder. Music inside it appears in All Songs, and each subfolder becomes its own playlist."
            }
            "这个歌单里还没有歌" => "This playlist is empty",
            "播放全部" => "Play All",
            "歌单" => "Playlists",
            "歌曲" => "Songs",
            "标题" => "Title",
            "歌手" => "Artist",
            "时长" => "Duration",
            "本地音乐库还是空的" => "Your local library is empty",
            "搜索本地音乐" => "Search local music",
            "搜索结果" => "Search Results",
            "没有找到本地音乐" => "No local music found",
            "词" => "Lyrics",
            "译文" => "Translation",
            "开" => "On",
            "关" => "Off",
            "还没有开始播放" => "Nothing is playing yet",
            "这首歌没有内嵌歌词" => "This track has no embedded lyrics",
            "支持内嵌 LRC 歌词或同名 .lrc 文件" => {
                "Supports embedded LRC lyrics or a matching .lrc file"
            }
            "播放队列 · {} 首" => "Play Queue · {} tracks",
            "已播放 · {} 首" => "Played · {}",
            "接下来 · {} 首" => "Up Next · {}",
            "正在播放的曲目不能从队列移除，可直接点「下一首」" => {
                "The playing track can't be removed from the queue. Use Next instead."
            }
            "从队列移除" => "Remove from Queue",
            "读取本地音乐失败：" => "Failed to scan local music: ",

            // 播放与队列动作。
            "正在准备播放：{}…" => "Preparing playback: {}…",
            "播放中：{}" => "Playing: {}",
            "已恢复上次播放：{}" => "Resumed: {}",
            "播放模式：{}" => "Play mode: {}",
            "列表循环" => "Loop queue",
            "单曲循环" => "Repeat one",
            "随机播放" => "Shuffle",
            "队列是空的：在本地音乐页点一首歌开始播放" => {
                "Queue is empty. Pick a song in Local Music to start."
            }
            "这条曲目不是本地文件，重新扫描本地音乐库后即可播放" => {
                "This track is not a local file. Rescan the local library to make it playable."
            }
            "这个文件已经不在原来的位置了，请重新扫描后播放" => {
                "This file has moved. Rescan and play again."
            }

            // 引擎/装载反馈（人话，原文见 core 的 audio.rs）。
            "没能保存，检查磁盘空间后重试。" => "Couldn't save. Check your disk space.",
            "保存播放设置失败" => "Failed to save playback settings",
            "任务栏播放控制已开启" => "Taskbar player enabled",
            "任务栏播放控制已关闭" => "Taskbar player disabled",
            "保存任务栏播放控制失败" => "Failed to save the taskbar player setting",
            "任务栏播放控制设置没有写入" => "The taskbar player setting was not saved",

            // 托盘 / 系统按钮。
            "显示主窗口" => "Show Main Window",
            "关闭窗口" => "Close Window",
            "最小化" => "Minimize",

            // macOS 菜单栏。
            "窗口" => "Window",

            // 设置页。
            "设置" => "Settings",
            "外观与语言" => "Appearance & Language",
            "界面语言" => "Language",
            "切换应用界面显示语言，立即生效" => "Switch display language immediately",
            "跟随系统" => "System",
            "本地曲库" => "Local Library",
            "音乐文件夹" => "Music Folder",
            "未选择文件夹" => "No folder selected",
            "更改文件夹" => "Change Folder",
            "重新扫描" => "Rescan",
            "播放与歌词" => "Playback & Lyrics",
            "双语歌词翻译" => "Bilingual Lyrics",
            "在播放页显示第二行歌词译文" => "Show translated lyrics on the player page",
            "响度均衡" => "Loudness Equalization",
            "自动平衡不同歌曲的播放音量。" => "Automatically balance playback volume across tracks.",
            "任务栏快捷播控" => "Taskbar Player",
            "在 Windows 任务栏右侧嵌入迷你播放条" => "Embed mini player in Windows taskbar",
            "开启" => "On",
            "关闭" => "Off",
            "快捷键" => "Shortcuts",
            "播放 / 暂停" => "Play / Pause",
            "全局播放控制快捷键" => "Global playback toggle shortcut",
            "切换到上一首曲目" => "Switch to previous track",
            "切换到下一首曲目" => "Switch to next track",
            "诊断与关于" => "Diagnostics & About",
            "详细日志" => "Detailed Logs",
            "记录更多调试信息到本地日志文件，重启保持" => "Write verbose debug logs to local file",
            "应用版本" => "App Version",
            _ => return None,
        })
    }

    /// 引擎错误文案的查表入口。
    ///
    /// 单独一张表是因为形状不同：`PlaybackSnapshot::error` 是运行时 `String`，而
    /// [`Self::text`] 的键是编译期 `&'static str`；放宽 `text` 的签名会连带
    /// 一批函数的返回类型，不在本改动里。表里的键必须与 `sodam-core/src/audio.rs`
    /// 写进快照的说法逐字一致。
    pub fn engine_error(self, message: &str) -> String {
        if self.resolved().is_zh() {
            return message.to_string();
        }
        let english = match message {
            "音频播放没能启动，请重启应用。" => {
                "Audio playback couldn't start. Restart the app."
            }
            "音频线程已退出，请重启应用" => {
                "The audio worker stopped. Please restart the app."
            }
            "音频输出中断了。" => "Audio output stopped.",
            "找不到可用的音频设备。" => "No audio device is available.",
            "音频设备不可用，请重新播放。" => {
                "The audio device is unavailable. Press play to try again."
            }
            "音频输出长时间无响应，请重试播放" => {
                "Audio output stopped responding. Try playing again."
            }
            "切换音频设备后没能继续播放。" => {
                "Playback didn't resume after switching the audio device."
            }
            "这首歌的文件读不出来，请重新播放。" => {
                "This song's file can't be read. Press play to try again."
            }
            "当前曲目没有可重新打开的文件" => {
                "The current track has no file to reopen"
            }
            "音频源不支持后退/跳转" => {
                "This track can't seek backward or jump to a position"
            }
            "还没有可播放的曲目" => "Nothing to play yet",
            _ => message,
        };
        english.to_string()
    }
}

impl Language {
    /// 用顺序占位符 `{}` 格式化。
    pub fn textf(self, key: &'static str, args: &[String]) -> String {
        let mut value = self.text(key).to_string();
        for arg in args {
            value = value.replacen("{}", arg, 1);
        }
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                rs_files(&path, out);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                out.push(path);
            }
        }
    }

    /// 取出 `needle`（如 `.tr(`）后紧跟的字符串字面量；允许换行缩进。
    fn literal_keys<'a>(source: &'a str, needle: &str) -> Vec<&'a str> {
        let mut keys = Vec::new();
        let mut index = 0;
        while let Some(found) = source[index..].find(needle) {
            let after_needle = index + found + needle.len();
            let rest = &source[after_needle..];
            let trimmed = rest.trim_start();
            let skipped = rest.len() - trimmed.len();
            let Some(body) = trimmed.strip_prefix('"') else {
                index = after_needle;
                continue;
            };
            let Some(end) = body.find('"') else {
                break;
            };
            let key = &body[..end];
            if !key.chars().any(|c| matches!(c, '\\' | '\n' | '\r')) {
                keys.push(key);
            }
            index = after_needle + skipped + end + 2;
        }
        keys
    }

    /// 从 `start`（开括号之后）按顶层逗号切分参数；字符串字面量整体跳过。
    fn call_arguments(source: &str, start: usize) -> Option<Vec<&str>> {
        let bytes = source.as_bytes();
        let mut args = Vec::new();
        let mut depth = 0usize;
        let mut arg_start = start;
        let mut i = start;
        while i < bytes.len() {
            match bytes[i] {
                b'"' => {
                    i += 1;
                    while i < bytes.len() {
                        match bytes[i] {
                            b'\\' => i += 2,
                            b'"' => {
                                i += 1;
                                break;
                            }
                            _ => i += 1,
                        }
                    }
                    continue;
                }
                b'(' | b'[' | b'{' => depth += 1,
                b')' => {
                    if depth == 0 {
                        args.push(&source[arg_start..i]);
                        return Some(args);
                    }
                    depth -= 1;
                }
                b']' | b'}' => depth = depth.saturating_sub(1),
                b',' if depth == 0 => {
                    args.push(&source[arg_start..i]);
                    arg_start = i + 1;
                }
                _ => {}
            }
            i += 1;
        }
        None
    }

    /// 取出 `needle`（如 `.failure_text(`）调用的第 `index` 个参数；只接受纯字符串字面量。
    fn nth_argument_literal<'a>(source: &'a str, needle: &str, index: usize) -> Vec<&'a str> {
        let mut keys = Vec::new();
        let mut search = 0;
        while let Some(found) = source[search..].find(needle) {
            let after_needle = search + found + needle.len();
            if let Some(arg) =
                call_arguments(source, after_needle).and_then(|args| args.get(index).copied())
            {
                let arg = arg.trim();
                let literal = arg
                    .strip_prefix('"')
                    .and_then(|s| s.strip_suffix('"'))
                    .filter(|body| !body.chars().any(|c| matches!(c, '\\' | '"' | '\n' | '\r')));
                if let Some(body) = literal {
                    keys.push(body);
                }
            }
            search = after_needle;
        }
        keys
    }

    /// `failure_text` 的第三个参数才是用户可见文案；常量参数不参与字典检查。
    #[test]
    fn failure_text_visible_literal_is_scanned() {
        let source = r#"
            let a = root.failure_text("上下文", &err, "可见文案");
            let c = root.failure_text(
                "多行上下文",
                &err,
                "多行可见文案",
            );
        "#;
        assert_eq!(
            nth_argument_literal(source, ".failure_text(", 2),
            vec!["可见文案", "多行可见文案"]
        );
    }

    /// 英文界面的静态文案必须都能在词典里查到，否则会静默回退中文。
    #[test]
    fn every_static_ui_key_has_an_english_entry() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        rs_files(&src, &mut files);
        assert!(files.len() > 15, "没扫到源码文件，扫描逻辑可能已失效");

        let mut scanned = 0;
        let mut missing = Vec::new();
        for file in files {
            if file.ends_with("ui/i18n.rs") {
                continue;
            }
            let Ok(source) = std::fs::read_to_string(&file) else {
                continue;
            };
            let mut keys = literal_keys(&source, ".tr(");
            keys.extend(literal_keys(&source, ".localized("));
            keys.extend(literal_keys(&source, ".set_status("));
            keys.extend(nth_argument_literal(&source, ".failure_text(", 2));
            for key in keys {
                scanned += 1;
                if Language::english_text(key).is_none() {
                    missing.push(format!("{}: {key}", file.display()));
                }
            }
        }

        assert!(scanned > 30, "没扫到 UI 文案，扫描逻辑可能已失效");
        missing.sort();
        missing.dedup();
        assert!(
            missing.is_empty(),
            "英文词典缺少这些 key：\n{}",
            missing.join("\n")
        );
    }
}
