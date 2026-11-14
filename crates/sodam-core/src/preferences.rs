//! 播放偏好单写者：滑块拖动防抖，退出时刷尾；只合并偏好字段，不覆盖其它设置。

use crate::config::{Settings, SETTINGS_WRITE_LOCK};
use crate::log_outlet::{log_event, LogLevel, LogTopic};
use crate::queue::PlayMode;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

#[derive(Clone)]
pub struct PlaybackPreferences {
    volume: f32,
    volume_before_mute: f32,
    play_mode: PlayMode,
    lyrics_translation: bool,
}

impl From<&Settings> for PlaybackPreferences {
    fn from(settings: &Settings) -> Self {
        Self {
            volume: settings.volume,
            volume_before_mute: settings.volume_before_mute,
            play_mode: settings.play_mode,
            lyrics_translation: settings.lyrics_translation,
        }
    }
}

impl PlaybackPreferences {
    fn save_to(self, path: &Path) -> anyhow::Result<()> {
        let _guard = SETTINGS_WRITE_LOCK
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        let mut settings = match std::fs::read(path) {
            Ok(bytes) => match serde_json::from_slice::<Settings>(&bytes) {
                Ok(settings) => settings,
                Err(err) => {
                    // 配置损坏不能变成「以后再也存不上设置」：启动时本来就已经按默认值
                    // 在跑，这里记下原因后继续写一份可用的配置。
                    let detail = err.to_string();
                    log_event(
                        LogLevel::Warn,
                        LogTopic::App,
                        "config_parse_failed_on_save",
                        &[("detail", detail.as_str())],
                    );
                    Settings::default()
                }
            },
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Settings::default(),
            Err(err) => return Err(err.into()),
        };
        settings.volume = self.volume;
        settings.volume_before_mute = self.volume_before_mute;
        settings.play_mode = self.play_mode;
        settings.lyrics_translation = self.lyrics_translation;
        settings.normalize_playback();
        settings.save_to_locked(path)
    }
}

enum Message {
    Update(PlaybackPreferences),
    /// 刷尾回执：worker 落盘完成后向这个通道发一条消息。
    Flush(mpsc::Sender<()>),
    Shutdown,
}

pub struct PreferencesWriter {
    tx: mpsc::Sender<Message>,
    thread: Option<std::thread::JoinHandle<()>>,
    error: Arc<Mutex<Option<String>>>,
}

impl PreferencesWriter {
    pub fn new(path: PathBuf) -> Self {
        let (tx, rx) = mpsc::channel();
        let error = Arc::new(Mutex::new(None));
        let worker_error = error.clone();
        let spawned = std::thread::Builder::new()
            .name("sodam-preferences".into())
            .spawn(move || {
                let mut pending = None;
                loop {
                    let message = if pending.is_some() {
                        rx.recv_timeout(Duration::from_millis(300))
                    } else {
                        rx.recv().map_err(|_| mpsc::RecvTimeoutError::Disconnected)
                    };
                    let (quitting, acknowledge) = match message {
                        Ok(Message::Update(value)) => {
                            pending = Some(value);
                            continue;
                        }
                        Ok(Message::Flush(acknowledge)) => (false, Some(acknowledge)),
                        Ok(Message::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                            (true, None)
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => (false, None),
                    };
                    if let Some(value) = pending.take() {
                        if let Err(err) = value.save_to(&path) {
                            if let Ok(mut error) = worker_error.lock() {
                                *error = Some(err.to_string());
                            }
                        }
                    }
                    // 回执收件人已经没了说明调用方放弃等待；刷尾本身已经完成。
                    if let Some(ack) = acknowledge {
                        let _ = ack.send(());
                    }
                    if quitting {
                        break;
                    }
                }
            });
        let thread = match spawned {
            Ok(thread) => Some(thread),
            Err(err) => {
                if let Ok(mut error) = error.lock() {
                    *error = Some(err.to_string());
                }
                None
            }
        };
        Self { tx, thread, error }
    }

    pub fn schedule(&self, settings: &Settings) {
        if self.tx.send(Message::Update(settings.into())).is_err() {
            if let Ok(mut error) = self.error.lock() {
                *error = Some("播放设置写入线程已退出".to_string());
            }
        }
    }

    /// 应用退出前立即刷尾：同步等待 worker 的回执，最多 2 秒。
    ///
    /// 只在退出路径调用；正常调音量从不等磁盘（`schedule`）。
    /// worker 已经退场（线程启用失败/已断开）时直接返回，不阻塞退出。
    pub fn flush(&self) {
        let (ack_tx, ack_rx) = mpsc::channel();
        if self.tx.send(Message::Flush(ack_tx)).is_err() {
            return;
        }
        let _ = ack_rx.recv_timeout(Duration::from_secs(2));
    }

    pub fn take_error(&self) -> Option<String> {
        self.error.lock().ok()?.take()
    }
}

impl Drop for PreferencesWriter {
    fn drop(&mut self) {
        // 仅在 Root/应用销毁时等待最后一次本地写盘，正常调音量从不等磁盘。
        let _ = self.tx.send(Message::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writer_flushes_last_value_without_overwriting_other_settings() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let mut settings = Settings::default();
        settings.save_to(&path).unwrap();
        settings.play_mode = PlayMode::RepeatOne;
        settings.lyrics_translation = false;
        let writer = PreferencesWriter::new(path.clone());
        for volume in [0.9, 0.3, 0.37] {
            settings.set_volume(volume);
            writer.schedule(&settings);
        }
        // 模拟调音量后另一个设置操作保存了语言。
        let mut other = Settings::load_from(&path);
        other.language = "en".into();
        other.save_to(&path).unwrap();
        drop(writer);
        let loaded = Settings::load_from(&path);
        assert_eq!(loaded.volume, 0.37);
        assert_eq!(loaded.volume_before_mute, 0.37);
        assert_eq!(loaded.play_mode, PlayMode::RepeatOne);
        assert!(!loaded.lyrics_translation);
        assert_eq!(loaded.language, "en");
    }

    #[test]
    fn explicit_flush_writes_before_writer_is_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let writer = PreferencesWriter::new(path.clone());
        let mut settings = Settings::default();
        settings.set_volume(0.42);
        writer.schedule(&settings);
        writer.flush();
        assert_eq!(Settings::load_from(&path).volume, 0.42);
    }

    #[test]
    fn corrupted_config_is_replaced_instead_of_blocking_saves() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, "{not json").unwrap();
        let writer = PreferencesWriter::new(path.clone());
        let mut settings = Settings::default();
        settings.set_volume(0.55);
        writer.schedule(&settings);
        writer.flush();
        assert_eq!(Settings::load_from(&path).volume, 0.55);
    }

    #[test]
    fn muted_restart_restores_previous_level() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let writer = PreferencesWriter::new(path.clone());
        let mut settings = Settings::default();
        settings.set_volume(0.24);
        settings.toggle_mute();
        writer.schedule(&settings);
        drop(writer);
        let mut loaded = Settings::load_from(&path);
        assert_eq!(loaded.volume, 0.0);
        loaded.toggle_mute();
        assert_eq!(loaded.volume, 0.24);
    }
}
