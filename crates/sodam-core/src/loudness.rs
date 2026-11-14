//! 自动响度均衡：ITU-R BS.1770 / EBU R128 测量、缓存与后台分析。

use crate::log_outlet::{log_event, LogLevel, LogTopic};
use ebur128::{EbuR128, Mode};
use rodio::Source;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::UNIX_EPOCH;

pub const TARGET_LUFS: f64 = -16.0;
pub const TRUE_PEAK_CEILING_DBTP: f64 = -1.0;
pub const MAX_POSITIVE_GAIN_DB: f64 = 12.0;
pub const ALGO_VERSION: u32 = 1;
pub const WORKER_CONCURRENCY: usize = 4;
const SILENCE_LUFS_FLOOR: f64 = -70.0;
const CHUNK_FRAMES: usize = 4096;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LoudnessEntry {
    pub track_id: String,
    pub integrated_lufs: f64,
    pub true_peak_dbtp: f64,
    pub gain_db: f64,
    pub gain_linear: f32,
    pub file_size: u64,
    pub modified_unix_secs: u64,
    pub algo_version: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct LoudnessCacheFile {
    #[serde(default)]
    entries: HashMap<String, LoudnessEntry>,
}

/// 根据 Integrated LUFS 与 True Peak (dBTP) 计算单曲归一化增益 `(gain_db, gain_linear)`。
///
/// 若测量值无效或接近静音，返回 `None`，调用方应安全回退到原始音量 `1.0`。
pub fn compute_gain(integrated_lufs: f64, true_peak_dbtp: f64) -> Option<(f64, f32)> {
    if !integrated_lufs.is_finite()
        || !true_peak_dbtp.is_finite()
        || integrated_lufs <= SILENCE_LUFS_FLOOR
    {
        return None;
    }
    let loudness_gain = TARGET_LUFS - integrated_lufs;
    let peak_gain = TRUE_PEAK_CEILING_DBTP - true_peak_dbtp;
    let final_gain_db = loudness_gain.min(peak_gain).min(MAX_POSITIVE_GAIN_DB);
    if !final_gain_db.is_finite() {
        return None;
    }
    let linear = 10_f64.powf(final_gain_db / 20.0) as f32;
    if !linear.is_finite() || linear <= 0.0 {
        return None;
    }
    Some((final_gain_db, linear))
}

fn file_signature(path: &Path) -> Option<(u64, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    let size = meta.len();
    let modified = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    Some((size, modified))
}

/// 使用 ITU-R BS.1770 / EBU R128 分块测量音频文件的 Integrated LUFS 与 True Peak (dBTP)。
pub fn analyze_file(path: &Path, track_id: &str) -> Result<LoudnessEntry, String> {
    let (file_size, modified_unix_secs) =
        file_signature(path).ok_or_else(|| "无法读取音频文件元信息".to_string())?;
    let file = File::open(path).map_err(|e| format!("打开音频文件失败: {e}"))?;
    let decoder = rodio::Decoder::try_from(file).map_err(|e| format!("解码音频文件失败: {e}"))?;

    let channels = u32::from(decoder.channels().get());
    let sample_rate = decoder.sample_rate().get();
    if channels == 0 || sample_rate == 0 {
        return Err("无效的音频声道或采样率".to_string());
    }

    let mut r128 = EbuR128::new(channels, sample_rate, Mode::I | Mode::TRUE_PEAK)
        .map_err(|e| format!("初始化 EBU R128 分析器失败: {e}"))?;

    let chunk_samples = CHUNK_FRAMES * channels as usize;
    let mut buf = Vec::with_capacity(chunk_samples);
    for sample in decoder {
        buf.push(sample);
        if buf.len() >= chunk_samples {
            r128.add_frames_f32(&buf)
                .map_err(|e| format!("EBU R128 帧处理失败: {e}"))?;
            buf.clear();
        }
    }
    if !buf.is_empty() {
        r128.add_frames_f32(&buf)
            .map_err(|e| format!("EBU R128 尾帧处理失败: {e}"))?;
    }

    let integrated_lufs = r128
        .loudness_global()
        .map_err(|e| format!("读取 Integrated LUFS 失败: {e}"))?;

    let mut max_true_peak_linear = 0.0_f64;
    for ch in 0..channels {
        let tp = r128
            .true_peak(ch)
            .map_err(|e| format!("读取 True Peak 失败: {e}"))?;
        if tp.is_finite() && tp > max_true_peak_linear {
            max_true_peak_linear = tp;
        }
    }
    let true_peak_dbtp = if max_true_peak_linear > 0.0 {
        20.0 * max_true_peak_linear.log10()
    } else {
        -120.0
    };

    let (gain_db, gain_linear) =
        compute_gain(integrated_lufs, true_peak_dbtp).unwrap_or((0.0, 1.0));

    Ok(LoudnessEntry {
        track_id: track_id.to_string(),
        integrated_lufs,
        true_peak_dbtp,
        gain_db,
        gain_linear,
        file_size,
        modified_unix_secs,
        algo_version: ALGO_VERSION,
    })
}

pub fn default_cache_path() -> PathBuf {
    crate::app_data_root().join("loudness_cache.json")
}

type LoudnessQueue = Arc<(Mutex<VecDeque<(String, PathBuf)>>, Condvar)>;

/// 线程安全的响度分析缓存与后台低并发分析队列。
#[derive(Clone)]
pub struct LoudnessManager {
    cache_path: PathBuf,
    entries: Arc<Mutex<HashMap<String, LoudnessEntry>>>,
    queue: LoudnessQueue,
    enabled: Arc<AtomicBool>,
    generation: Arc<AtomicU64>,
}

impl LoudnessManager {
    pub fn new(cache_path: PathBuf, initial_enabled: bool) -> Self {
        let loaded = Self::load_from(&cache_path);
        let entries = Arc::new(Mutex::new(loaded));
        let queue = Arc::new((Mutex::new(VecDeque::new()), Condvar::new()));
        let enabled = Arc::new(AtomicBool::new(initial_enabled));
        let generation = Arc::new(AtomicU64::new(0));
        let pending_saves = Arc::new(AtomicUsize::new(0));

        for idx in 0..WORKER_CONCURRENCY {
            let worker_entries = Arc::clone(&entries);
            let worker_queue = Arc::clone(&queue);
            let worker_enabled = Arc::clone(&enabled);
            let worker_gen = Arc::clone(&generation);
            let worker_saves = Arc::clone(&pending_saves);
            let worker_path = cache_path.clone();

            std::thread::Builder::new()
                .name(format!("sodam-loudness-{idx}"))
                .spawn(move || {
                    Self::worker_loop(
                        worker_path,
                        worker_entries,
                        worker_queue,
                        worker_enabled,
                        worker_gen,
                        worker_saves,
                    );
                })
                .ok();
        }

        Self {
            cache_path,
            entries,
            queue,
            enabled,
            generation,
        }
    }

    fn load_from(path: &Path) -> HashMap<String, LoudnessEntry> {
        let Ok(text) = std::fs::read_to_string(path) else {
            return HashMap::new();
        };
        serde_json::from_str::<LoudnessCacheFile>(&text)
            .map(|file| file.entries)
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let snapshot = {
            let guard = self.entries.lock().unwrap_or_else(|e| e.into_inner());
            LoudnessCacheFile {
                entries: guard.clone(),
            }
        };
        Self::save_to(&self.cache_path, &snapshot);
    }

    fn save_to(path: &Path, data: &LoudnessCacheFile) {
        let Some(parent) = path.parent() else {
            return;
        };
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
        let Ok(json) = serde_json::to_string_pretty(data) else {
            return;
        };
        if let Ok(mut tmp) = tempfile::NamedTempFile::new_in(parent) {
            if tmp.write_all(json.as_bytes()).is_ok() && tmp.flush().is_ok() {
                let _ = tmp.persist(path);
            }
        }
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
        if !enabled {
            self.generation.fetch_add(1, Ordering::Relaxed);
            let (lock, _) = &*self.queue;
            lock.lock().unwrap_or_else(|e| e.into_inner()).clear();
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    /// 若缓存有效（算法版本、文件大小、修改时间均匹配），直接返回线性增益。
    pub fn get_valid_gain(&self, track_id: &str, path: &Path) -> Option<f32> {
        let (size, mtime) = file_signature(path)?;
        let guard = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        let entry = guard.get(track_id)?;
        if entry.algo_version == ALGO_VERSION
            && entry.file_size == size
            && entry.modified_unix_secs == mtime
        {
            Some(entry.gain_linear)
        } else {
            None
        }
    }

    /// 优先分析单首曲目（用于播放前），若分析失败安全回退 `1.0`。
    pub fn get_or_analyze_gain(&self, track_id: &str, path: &Path) -> f32 {
        if !self.is_enabled() {
            return 1.0;
        }
        if let Some(gain) = self.get_valid_gain(track_id, path) {
            return gain;
        }
        match analyze_file(path, track_id) {
            Ok(entry) => {
                let gain = entry.gain_linear;
                {
                    let mut guard = self.entries.lock().unwrap_or_else(|e| e.into_inner());
                    guard.insert(track_id.to_string(), entry);
                }
                self.save();
                gain
            }
            Err(err) => {
                let path_text = path.display().to_string();
                log_event(
                    LogLevel::Warn,
                    LogTopic::Audio,
                    "loudness_analyze_failed",
                    &[("path", path_text.as_str()), ("detail", err.as_str())],
                );
                1.0
            }
        }
    }

    /// 将曲库中缺少有效缓存的曲目加入后台分析队列。
    pub fn enqueue_missing(&self, items: Vec<(String, PathBuf)>) {
        if !self.is_enabled() || items.is_empty() {
            return;
        }
        let (lock, cvar) = &*self.queue;
        let mut queue = lock.lock().unwrap_or_else(|e| e.into_inner());
        for (track_id, path) in items {
            if self.get_valid_gain(&track_id, &path).is_none() {
                queue.push_back((track_id, path));
            }
        }
        cvar.notify_all();
    }

    fn worker_loop(
        cache_path: PathBuf,
        entries: Arc<Mutex<HashMap<String, LoudnessEntry>>>,
        queue: LoudnessQueue,
        enabled: Arc<AtomicBool>,
        generation: Arc<AtomicU64>,
        pending_saves: Arc<AtomicUsize>,
    ) {
        loop {
            let item = {
                let (lock, cvar) = &*queue;
                let mut guard = lock.lock().unwrap_or_else(|e| e.into_inner());
                while guard.is_empty() || !enabled.load(Ordering::Relaxed) {
                    if pending_saves.swap(0, Ordering::Relaxed) > 0 {
                        Self::save_snapshot(&cache_path, &entries);
                    }
                    guard = cvar.wait(guard).unwrap_or_else(|e| e.into_inner());
                }
                guard.pop_front()
            };

            let Some((track_id, path)) = item else {
                continue;
            };
            if !enabled.load(Ordering::Relaxed) {
                continue;
            }
            let gen_before = generation.load(Ordering::Relaxed);
            if let Some((size, mtime)) = file_signature(&path) {
                let already_valid = {
                    let guard = entries.lock().unwrap_or_else(|e| e.into_inner());
                    guard.get(&track_id).is_some_and(|e| {
                        e.algo_version == ALGO_VERSION
                            && e.file_size == size
                            && e.modified_unix_secs == mtime
                    })
                };
                if already_valid {
                    continue;
                }
            }

            if let Ok(entry) = analyze_file(&path, &track_id) {
                if generation.load(Ordering::Relaxed) == gen_before
                    && enabled.load(Ordering::Relaxed)
                {
                    entries
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .insert(track_id, entry);
                    if pending_saves.fetch_add(1, Ordering::Relaxed) + 1 >= 5 {
                        pending_saves.store(0, Ordering::Relaxed);
                        Self::save_snapshot(&cache_path, &entries);
                    }
                }
            }
        }
    }

    fn save_snapshot(cache_path: &Path, entries: &Arc<Mutex<HashMap<String, LoudnessEntry>>>) {
        let snap = LoudnessCacheFile {
            entries: entries.lock().unwrap_or_else(|e| e.into_inner()).clone(),
        };
        Self::save_to(cache_path, &snap);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn low_loudness_boosts_up_to_max_12db() {
        let (gain_db, linear) = compute_gain(-35.0, -20.0).unwrap();
        assert!((gain_db - 12.0).abs() < 1e-6);
        assert!((linear - 10_f32.powf(12.0 / 20.0)).abs() < 1e-4);
    }

    #[test]
    fn high_loudness_attenuates_to_target() {
        let (gain_db, linear) = compute_gain(-10.0, -2.0).unwrap();
        assert!((gain_db - (-6.0)).abs() < 1e-6);
        assert!((linear - 10_f32.powf(-6.0 / 20.0)).abs() < 1e-4);
    }

    #[test]
    fn true_peak_ceiling_limits_positive_gain() {
        // loudness_gain = -16 - (-20) = +4 dB，但 peak_gain = -1 - (-2.5) = +1.5 dB
        let (gain_db, _) = compute_gain(-20.0, -2.5).unwrap();
        assert!((gain_db - 1.5).abs() < 1e-6);
    }

    #[test]
    fn silence_or_invalid_returns_none() {
        assert!(compute_gain(-80.0, -90.0).is_none());
        assert!(compute_gain(f64::NAN, -1.0).is_none());
        assert!(compute_gain(-16.0, f64::INFINITY).is_none());
    }
}
