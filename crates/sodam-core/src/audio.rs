//! 播放引擎：用 rodio 播放本地音频文件。
//!
//! 设计：`OutputStream` 在 rodio 0.20 里是 `!Send`，所以**音频对象全部留在专属线程**，
//! UI 侧只通过命令通道操作、通过 `Arc<Mutex<PlaybackSnapshot>>` 读状态。
//!
//! 没有可用音频设备时**不 panic**：把错误写进快照，界面照常能跑（用户可能没接耳机）。
//!
//! **跟随系统默认输出设备**：Windows(WASAPI)、macOS(CoreAudio) 下流在创建时就跟设备
//! 绑死，切换默认设备（音响 → USB 耳机）对已开的流无效，必须重建；否则要重启应用才生效。
//! 播放线程每 2 秒探测一次默认设备 ID，变化时换新输出并从原位置续播。
//! Linux 的 cpal ALSA 后端恒返回 `default` 单一设备，且 PipeWire/WirePlumber 会把现有流
//! 自动迁到新的默认节点，因此探测天然不触发 —— 行为与修复前一致。

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};

use crate::log_outlet::{log_event, LogLevel, LogTopic};
use crate::models::TrackItem;

/// 引擎能给出的用户说法只有这两种：它们描述「用户现在能做什么」，不复述驱动/解码器原文。
///
/// 技术原文一律进 `log_engine_failure`。文案不许承诺引擎没在做的事——「正在重新获取」
/// 就不实：本地文件失效之后没有任何自动重取，要用户按下播放才会重新打开。
const TRACK_FILE_UNREADABLE: &str = "这首歌的文件读不出来，请重新播放。";
const AUDIO_DEVICE_UNAVAILABLE: &str = "音频设备不可用，请重新播放。";

/// 播放器状态快照（UI 每帧读这个，加锁时间极短）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PlaybackSnapshot {
    pub track_id: String,
    pub title: String,
    pub playing: bool,
    pub position_seconds: f64,
    pub duration_seconds: f64,
    pub volume: f32,
    /// 最近一次错误（设备不可用、解码失败…），UI 直接展示
    pub error: String,
    /// 当前曲目是否已播完（UI 据此自动切下一首）
    pub finished: bool,
    /// 播完序号：每播完一次 +1。UI 用它做「处理过了吗」的判断 ——
    /// 之前用 track_id 比较，遇到循环同一首或 id 重复就会漏切。
    pub finished_seq: u64,
    /// 跳转序号：每次 seek 请求 +1。UI 用它丢弃 seek 之前积压的完成事件。
    pub seek_seq: u64,
    /// seek 请求已被登记、但音频线程还没处理完。此窗口内不能上报播完，
    /// 否则会消费一个完成后又自动切到下一首。
    pub(crate) seek_pending: bool,
    /// 最近处理完的装载请求（成功/失败都会更新），UI 不再只用 track_id 判断完成。
    pub load_seq: u64,
}

impl PlaybackSnapshot {
    /// 总时长 `mm:ss`。
    pub fn duration_label(&self) -> String {
        format_seconds(self.duration_seconds)
    }
}

type OutputErrors = Arc<Mutex<Option<String>>>;
type OutputResult = Result<rodio::MixerDeviceSink, rodio::DeviceSinkError>;

enum Command {
    Load {
        track: Box<TrackItem>,
        path: PathBuf,
        sequence: u64,
        /// 从这一秒开始（0 = 从头）。
        position_seconds: f64,
        /// 装载完成后保持暂停（恢复上次进度用）。
        paused: bool,
    },
    Toggle,
    SetVolume(f32),
    /// 跳转到指定秒数
    Seek {
        sequence: u64,
        seconds: f64,
        track_id: String,
    },
}

/// 响度均衡增益包装器：在实时音频回调中零锁、零分配地应用固定增益，
/// 并在目标增益变化时执行约 100ms 的线性平滑渐变，避免切歌或开关切换时突变。
struct LoudnessSource<S> {
    inner: S,
    target_bits: Arc<AtomicU32>,
    current_gain: f32,
    ramp_samples: f32,
}

impl<S: rodio::Source> LoudnessSource<S> {
    fn new(inner: S, target_bits: Arc<AtomicU32>, initial_gain: f32) -> Self {
        let rate = inner.sample_rate().get() as f32;
        let channels = inner.channels().get() as f32;
        let ramp_samples = (rate * channels * 0.1).max(1.0);
        Self {
            inner,
            target_bits,
            current_gain: initial_gain,
            ramp_samples,
        }
    }
}

impl<S: rodio::Source> Iterator for LoudnessSource<S> {
    type Item = S::Item;

    fn next(&mut self) -> Option<Self::Item> {
        let sample = self.inner.next()?;
        let target = f32::from_bits(self.target_bits.load(Ordering::Relaxed));
        let diff = target - self.current_gain;
        if diff.abs() > 1e-5 {
            let step = (diff.abs() / self.ramp_samples).max(1e-5);
            if diff > 0.0 {
                self.current_gain = (self.current_gain + step).min(target);
            } else {
                self.current_gain = (self.current_gain - step).max(target);
            }
        } else {
            self.current_gain = target;
        }
        Some(sample * self.current_gain)
    }
}

impl<S: rodio::Source> rodio::Source for LoudnessSource<S> {
    fn current_span_len(&self) -> Option<usize> {
        self.inner.current_span_len()
    }

    fn channels(&self) -> rodio::ChannelCount {
        self.inner.channels()
    }

    fn sample_rate(&self) -> rodio::SampleRate {
        self.inner.sample_rate()
    }

    fn total_duration(&self) -> Option<std::time::Duration> {
        self.inner.total_duration()
    }

    fn try_seek(&mut self, pos: std::time::Duration) -> Result<(), rodio::source::SeekError> {
        self.inner.try_seek(pos)
    }
}

/// 播放引擎句柄（可以在任意线程调用，内部只有发命令与读快照）。
pub struct PlaybackEngine {
    tx: Sender<Command>,
    snapshot: Arc<Mutex<PlaybackSnapshot>>,
    next_load: AtomicU64,
    loudness: crate::loudness::LoudnessManager,
    loudness_gain_bits: Arc<AtomicU32>,
}

impl PlaybackEngine {
    pub fn with_volume(saved_volume: f32) -> Self {
        Self::with_output(saved_volume, open_output, probe_default_output_id)
    }

    // 单测注入故障输出与设备探测，不触碰硬件/COM；正式运行始终使用上面的真实实现。
    fn with_output(
        saved_volume: f32,
        open_output: impl FnMut(OutputErrors) -> OutputResult + Send + 'static,
        probe: impl FnMut() -> Option<String> + Send + 'static,
    ) -> Self {
        let (tx, rx) = channel();
        // 显式环境覆盖仍优先（静音开发测试），无覆盖时恢复用户保存的值。
        let initial_volume =
            initial_volume(saved_volume, std::env::var("SODAM_VOLUME").ok().as_deref());
        let snapshot = Arc::new(Mutex::new(PlaybackSnapshot {
            volume: initial_volume,
            ..Default::default()
        }));
        let loudness =
            crate::loudness::LoudnessManager::new(crate::loudness::default_cache_path(), false);
        let loudness_gain_bits = Arc::new(AtomicU32::new(1.0_f32.to_bits()));
        let worker_loudness = loudness.clone();
        let worker_gain_bits = Arc::clone(&loudness_gain_bits);
        let state = snapshot.clone();
        let spawned = std::thread::Builder::new()
            .name("sodam-audio".to_string())
            .spawn(move || {
                audio_thread(
                    rx,
                    state,
                    worker_loudness,
                    worker_gain_bits,
                    open_output,
                    probe,
                )
            });
        if let Err(err) = spawned {
            let detail = err.to_string();
            log_event(
                LogLevel::Error,
                LogTopic::Audio,
                "thread_spawn_failed",
                &[("detail", detail.as_str())],
            );
            snapshot
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .error = "音频播放没能启动，请重启应用。".to_string();
        }
        Self {
            tx,
            snapshot,
            next_load: AtomicU64::new(1),
            loudness,
            loudness_gain_bits,
        }
    }

    pub fn loudness(&self) -> &crate::loudness::LoudnessManager {
        &self.loudness
    }

    pub fn set_loudness_equalization(&self, enabled: bool) {
        self.loudness.set_enabled(enabled);
        if !enabled {
            self.loudness_gain_bits
                .store(1.0_f32.to_bits(), Ordering::Relaxed);
            return;
        }
        let track_id = self
            .snapshot
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .track_id
            .clone();
        if let Some(raw_path) = track_id.strip_prefix("local:") {
            let path = PathBuf::from(raw_path);
            let loudness = self.loudness.clone();
            let gain_bits = Arc::clone(&self.loudness_gain_bits);
            std::thread::Builder::new()
                .name("sodam-loudness-current".into())
                .spawn(move || {
                    let gain = loudness.get_or_analyze_gain(&track_id, &path);
                    if loudness.is_enabled() {
                        gain_bits.store(gain.to_bits(), Ordering::Relaxed);
                    }
                })
                .ok();
        }
    }

    pub fn snapshot(&self) -> PlaybackSnapshot {
        self.snapshot
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    /// 播放本地音频文件。
    pub fn load(&self, track: TrackItem, path: PathBuf) -> u64 {
        self.load_at(track, path, 0.0, false)
    }

    /// 从 `position_seconds` 开始装载；`paused` 为 true 时不自动播放。
    pub fn load_at(
        &self,
        track: TrackItem,
        path: PathBuf,
        position_seconds: f64,
        paused: bool,
    ) -> u64 {
        let sequence = self.next_load.fetch_add(1, Ordering::Relaxed);
        self.send(Command::Load {
            track: Box::new(track),
            path,
            sequence,
            position_seconds: if position_seconds.is_finite() {
                position_seconds.max(0.0)
            } else {
                0.0
            },
            paused,
        });
        sequence
    }

    fn send(&self, command: Command) {
        if let Err(err) = self.tx.send(command) {
            let message = "音频线程已退出，请重启应用".to_string();
            if let Command::Load {
                track, sequence, ..
            } = err.0
            {
                load_failed(&self.snapshot, &track, sequence, message);
            } else {
                set_error(&self.snapshot, message);
            }
        }
    }

    pub fn toggle(&self) {
        self.send(Command::Toggle);
    }

    pub fn set_volume(&self, volume: f32) {
        if volume.is_finite() {
            self.send(Command::SetVolume(volume.clamp(0.0, 1.0)));
        }
    }

    /// 跳转到指定秒数（越界会被引擎裁剪到 [0, 时长]）。
    pub fn seek(&self, seconds: f64) {
        if !seconds.is_finite() {
            return;
        }
        // 在发命令前同步失效完成事件。音频线程最多要等 100ms 才取到命令，
        // UI 心跳可能先看到旧曲目的 finished=true，因此不能只在音频线程里清。
        let (sequence, target_seconds, track_id) = {
            let mut snap = self
                .snapshot
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if snap.track_id.is_empty() {
                return;
            }
            let clamped = if snap.duration_seconds > 0.0 {
                seconds.clamp(0.0, snap.duration_seconds)
            } else {
                seconds.max(0.0)
            };
            snap.seek_seq = snap.seek_seq.wrapping_add(1);
            snap.seek_pending = true;
            snap.finished = false;
            snap.position_seconds = clamped;
            (snap.seek_seq, clamped, snap.track_id.clone())
        };
        self.send(Command::Seek {
            sequence,
            seconds: target_seconds,
            track_id,
        });
    }
}

fn set_error(state: &Arc<Mutex<PlaybackSnapshot>>, message: String) {
    {
        let mut snap = state.lock().unwrap_or_else(|error| error.into_inner());
        snap.error = message;
        snap.playing = false;
        snap.finished = false;
    }
}

/// 引擎侧的技术原文只进日志（`audio` 话题），
/// 界面拿到的 `message` 必须是不带驱动/解码器术语的一句人话。
fn log_engine_failure(context: &str, detail: impl std::fmt::Display) {
    let detail = detail.to_string();
    log_event(
        LogLevel::Warn,
        LogTopic::Audio,
        "engine_failure",
        &[("context", context), ("detail", detail.as_str())],
    );
}

fn initial_volume(saved: f32, override_value: Option<&str>) -> f32 {
    override_value
        .and_then(|value| value.trim().parse::<f32>().ok())
        .filter(|value| value.is_finite())
        .unwrap_or(if saved.is_finite() { saved } else { 1.0 })
        .clamp(0.0, 1.0)
}

fn load_failed(
    state: &Arc<Mutex<PlaybackSnapshot>>,
    track: &TrackItem,
    sequence: u64,
    message: String,
) {
    {
        let mut snap = state.lock().unwrap_or_else(|error| error.into_inner());
        snap.track_id = track.id.clone();
        snap.title = track.title.clone();
        snap.load_seq = sequence;
        snap.position_seconds = 0.0;
        snap.duration_seconds = track.duration_seconds.max(0) as f64;
        snap.error = message;
        snap.playing = false;
        snap.finished = false;
    }
}

fn sync_progress(state: &Arc<Mutex<PlaybackSnapshot>>, player: &rodio::Player) {
    // 不拿快照锁调用 rodio，避免设备/解码器锁等待传染到 UI 的 snapshot()。
    let position = player.get_pos().as_secs_f64();
    let playing = !player.is_paused() && !player.empty();
    {
        let mut snap = state.lock().unwrap_or_else(|error| error.into_inner());
        if !snap.seek_pending {
            snap.position_seconds = position;
        }
        snap.playing = playing;
    }
}

fn should_report_completion(empty: bool, finished_reported: bool, seek_pending: bool) -> bool {
    empty && !finished_reported && !seek_pending
}

fn mark_seek_succeeded(
    state: &Arc<Mutex<PlaybackSnapshot>>,
    sequence: u64,
    seconds: f64,
    playing: bool,
) {
    {
        let mut snap = state.lock().unwrap_or_else(|error| error.into_inner());
        if snap.seek_seq != sequence {
            return;
        }
        snap.seek_pending = false;
        snap.finished = false;
        snap.error.clear();
        snap.position_seconds = seconds;
        snap.playing = playing;
    }
}

fn mark_seek_failed(state: &Arc<Mutex<PlaybackSnapshot>>, sequence: u64, message: String) {
    {
        let mut snap = state.lock().unwrap_or_else(|error| error.into_inner());
        if snap.seek_seq != sequence {
            return;
        }
        snap.seek_pending = false;
        snap.finished = false;
        snap.error = message;
        snap.playing = false;
    }
}

/// 从文件重新建立播放器，并恢复到指定位置。
///
/// 已播完的 rodio source 是不可逆的：`try_seek()` 会返回 `Ok` 但不会移动。
/// 所以 seek 失败/源已结束时必须重新打开解码器，不能继续复用旧 `Player`。
fn rebuild_player_at(
    mixer: &rodio::mixer::Mixer,
    path: &std::path::Path,
    position: std::time::Duration,
    volume: f32,
    paused: bool,
    loudness_gain_bits: Arc<AtomicU32>,
    initial_gain: f32,
) -> Result<rodio::Player, String> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(err) => {
            log_engine_failure("重新打开音频文件失败", &err);
            return Err(TRACK_FILE_UNREADABLE.to_string());
        }
    };
    // 使用 TryFrom<File>，让 rodio 知道 byte length，后退 seek 才可靠。
    let decoder = match rodio::Decoder::try_from(file) {
        Ok(decoder) => decoder,
        Err(err) => {
            log_engine_failure("重新解码音频失败", &err);
            return Err(TRACK_FILE_UNREADABLE.to_string());
        }
    };
    let (player, source) = rodio::Player::new();
    player.set_volume(volume);
    player.append(LoudnessSource::new(
        decoder,
        loudness_gain_bits,
        initial_gain,
    ));
    // 先暂停并挂到 mixer，再执行 try_seek：rodio 的 try_seek 需要音频回调驱动才会返回，
    // 挂入前已 pause()，不会从旧起点漏出声音。
    player.pause();
    mixer.add(source);
    if position > std::time::Duration::ZERO && player.try_seek(position).is_err() {
        return Err("音频源不支持后退/跳转".to_string());
    }
    if !paused {
        player.play();
    }
    Ok(player)
}

/// 处理 cpal 的流错误。
///
/// 缓冲区欠载（ALSA XRUN / EPIPE）是偶发抖动，cpal 自己会 `try_recover` 接着播，
/// 所以只记日志；其余错误才算输出失效，写进 `errors` 让播放线程拆掉输出。
/// 返回写出的日志事件名，测试据此区分两条路径。
fn record_stream_error(err: &rodio::cpal::StreamError, errors: &OutputErrors) -> &'static str {
    let detail = err.to_string();
    let recoverable = matches!(err, rodio::cpal::StreamError::BufferUnderrun);
    let event = if recoverable {
        "underrun"
    } else {
        "device_lost"
    };
    log_event(
        LogLevel::Warn,
        LogTopic::Audio,
        event,
        &[("detail", detail.as_str())],
    );
    if !recoverable {
        *errors.lock().unwrap_or_else(|error| error.into_inner()) =
            Some("音频输出中断了。".to_string());
    }
    event
}

fn open_output(errors: OutputErrors) -> OutputResult {
    rodio::DeviceSinkBuilder::from_default_device()?
        .with_error_callback(move |err| {
            record_stream_error(&err, &errors);
        })
        .open_sink_or_fallback()
}

struct OutputWatchdog {
    position: f64,
    advanced_at: std::time::Instant,
}

impl OutputWatchdog {
    fn new(now: std::time::Instant) -> Self {
        Self {
            position: 0.0,
            advanced_at: now,
        }
    }

    fn stalled(&mut self, position: f64, running: bool, now: std::time::Instant) -> bool {
        if !running || (position - self.position).abs() > 0.001 {
            self.position = position;
            self.advanced_at = now;
        }
        running
            && now.saturating_duration_since(self.advanced_at) >= std::time::Duration::from_secs(15)
    }
}

/// 探测系统默认输出设备的稳定 ID（`None` = 现在拿不到默认设备）。
fn probe_default_output_id() -> Option<String> {
    use rodio::cpal::traits::{DeviceTrait, HostTrait};
    let host = rodio::cpal::default_host();
    let device = host.default_output_device()?;
    device.id().ok().map(|id| id.to_string())
}

/// 默认输出设备轮询节流：播放线程 100ms 一个 tick，20 个 tick ≈ 2 秒。
const DEVICE_POLL_TICKS: u8 = 20;

/// 默认输出设备变化检测。
///
/// 初次探测为空（启动时没有设备）不算切换，避免启动瞬间误重建。
/// 探测失败（一瞬间拿不到默认设备）也不算切换，避免把正在播的歌打断。
struct DeviceWatcher {
    last: Option<String>,
    ticks: u8,
}

impl DeviceWatcher {
    fn new(initial: Option<String>) -> Self {
        Self {
            last: initial,
            ticks: 0,
        }
    }

    /// 每个 tick 调用一次；返回 `true` 表示默认设备确实换了。
    fn poll(&mut self, probe: &mut impl FnMut() -> Option<String>) -> bool {
        self.ticks = self.ticks.saturating_add(1);
        if self.ticks < DEVICE_POLL_TICKS {
            return false;
        }
        self.ticks = 0;
        let Some(current) = probe() else {
            return false;
        };
        let changed = match &self.last {
            Some(previous) => *previous != current,
            None => false,
        };
        self.last = Some(current);
        changed
    }
}

fn audio_thread(
    rx: Receiver<Command>,
    state: Arc<Mutex<PlaybackSnapshot>>,
    loudness: crate::loudness::LoudnessManager,
    loudness_gain_bits: Arc<AtomicU32>,
    mut open_output: impl FnMut(OutputErrors) -> OutputResult,
    mut probe: impl FnMut() -> Option<String>,
) {
    let stream_error = Arc::new(Mutex::new(None));
    // 失效的输出会在下一次装载时重新打开，不把启动时/休眠后的失败永久锁死。
    let mut output = match open_output(stream_error.clone()) {
        Ok(device) => Some(device),
        Err(err) => {
            log_engine_failure("音频设备不可用", &err);
            set_error(&state, "找不到可用的音频设备。".to_string());
            None
        }
    };

    let mut player: Option<rodio::Player> = None;
    // 重建输出时要从原位置续播，必须记住当前曲目的文件路径。
    let mut current_path: Option<PathBuf> = None;
    let mut device_watcher = DeviceWatcher::new(probe());
    let mut watchdog = OutputWatchdog::new(std::time::Instant::now());
    // 当前曲目是否已经上报过「播完」；换曲时重置
    let mut finished_reported = false;
    loop {
        let stream_failure = stream_error
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        if let Some(error) = stream_failure {
            if let Some(previous) = player.take() {
                previous.stop();
            }
            current_path = None;
            output = None;
            set_error(&state, error);
            finished_reported = true;
        }
        // 为什么要超时 recv：进度必须**持续**更新（UI 的进度条依赖它）。
        // 之前只在 play/pause 时同步一次，所以进度条永远停在 0。
        let command = match rx.recv_timeout(std::time::Duration::from_millis(100)) {
            Ok(command) => Some(command),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => None,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        };
        let Some(command) = command else {
            if let Some(current) = &player {
                if watchdog.stalled(
                    current.get_pos().as_secs_f64(),
                    !current.is_paused() && !current.empty(),
                    std::time::Instant::now(),
                ) {
                    if let Some(previous) = player.take() {
                        previous.stop();
                    }
                    current_path = None;
                    output = None;
                    set_error(&state, "音频输出长时间无响应，请重试播放".to_string());
                    finished_reported = true;
                    continue;
                }
                sync_progress(&state, current);
                // 播完：置 finished，UI 会据此自动切下一首
                // 注意：sync_progress 会把 playing 置 false，所以这里不能用 playing 判断，
                // 必须用本地的 finished_reported 标志（否则自动下一首永远不触发）。
                let empty = current.empty();
                let mut snap = state.lock().unwrap_or_else(|error| error.into_inner());
                if should_report_completion(empty, finished_reported, snap.seek_pending) {
                    finished_reported = true;
                    snap.playing = false;
                    snap.finished = true;
                    snap.finished_seq = snap.finished_seq.wrapping_add(1);
                }
            }
            // 设备切换放在「播完」判定**之后**：如果恰好在这个 tick 播完，
            // 必须先上报 finished（UI 才会切下一首），否则重建输出会把空播放器
            // 丢掉，finished 永远不触发，播放会卡在上一首的结尾。
            // 系统默认输出设备被切换（Windows 音响 → USB 耳机、macOS 换 AirPods…）：
            // 已打开的流仍绑在旧设备上，不重建就得重启应用才生效。
            if device_watcher.poll(&mut probe) {
                follow_default_output(
                    &mut output,
                    &mut player,
                    &mut current_path,
                    &mut watchdog,
                    &state,
                    &stream_error,
                    &mut open_output,
                    &loudness_gain_bits,
                );
            }
            continue;
        };
        match command {
            Command::Load {
                track,
                path,
                sequence,
                position_seconds,
                paused,
            } => {
                if let Some(previous) = player.take() {
                    previous.stop();
                }
                if output.is_none() {
                    match open_output(stream_error.clone()) {
                        Ok(device) => output = Some(device),
                        Err(err) => {
                            log_engine_failure("装载时重开音频设备失败", &err);
                            load_failed(
                                &state,
                                &track,
                                sequence,
                                "找不到可用的音频设备。".to_string(),
                            );
                            continue;
                        }
                    }
                }
                let Some(device) = &output else {
                    continue;
                };
                let file = match std::fs::File::open(&path) {
                    Ok(file) => file,
                    Err(err) => {
                        log_engine_failure("打开音频文件失败", &err);
                        // 旧路径必须清掉：否则之后拖进度条会用上一首的文件重建播放，
                        // 界面显示新曲目、实际播的是旧音频。
                        current_path = None;
                        let message = TRACK_FILE_UNREADABLE.to_string();
                        load_failed(&state, &track, sequence, message);
                        continue;
                    }
                };
                let decoder = match rodio::Decoder::try_from(file) {
                    Ok(decoder) => decoder,
                    Err(err) => {
                        log_engine_failure("解码失败（文件可能是坏的音频文件）", &err);
                        current_path = None;
                        let message = TRACK_FILE_UNREADABLE.to_string();
                        load_failed(&state, &track, sequence, message);
                        continue;
                    }
                };
                let (new_player, source) = rodio::Player::new();
                let volume = state
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .volume;
                let target_gain = if loudness.is_enabled() {
                    loudness.get_or_analyze_gain(&track.id, &path)
                } else {
                    1.0
                };
                loudness_gain_bits.store(target_gain.to_bits(), Ordering::Relaxed);
                new_player.set_volume(volume);
                new_player.append(LoudnessSource::new(
                    decoder,
                    Arc::clone(&loudness_gain_bits),
                    target_gain,
                ));
                // 先暂停再接到 mixer：恢复上次进度时不会先漏出一小段开头。
                new_player.pause();
                device.mixer().add(source);
                let mut start_position = 0.0;
                if position_seconds > 0.0 {
                    let target = std::time::Duration::from_secs_f64(position_seconds);
                    match new_player.try_seek(target) {
                        Ok(()) => start_position = position_seconds,
                        Err(err) => log_engine_failure("恢复播放位置失败", &err),
                    }
                }
                if !paused {
                    new_player.play();
                }
                {
                    let mut snap = state.lock().unwrap_or_else(|error| error.into_inner());
                    snap.load_seq = sequence;
                    snap.track_id = track.id.clone();
                    snap.title = track.title.clone();
                    snap.playing = !paused;
                    snap.position_seconds = start_position;
                    snap.duration_seconds = track.duration_seconds.max(0) as f64;
                    snap.error.clear();
                    snap.finished = false;
                    snap.seek_pending = false;
                    finished_reported = false;
                }
                player = Some(new_player);
                current_path = Some(path);
                watchdog = OutputWatchdog::new(std::time::Instant::now());
            }
            Command::Toggle => {
                if let Some(current) = &player {
                    if current.is_paused() {
                        current.play();
                    } else {
                        current.pause();
                    }
                    sync_progress(&state, current);
                } else {
                    set_error(&state, "还没有可播放的曲目".to_string());
                }
            }
            Command::Seek {
                sequence,
                seconds,
                track_id,
            } => {
                let current_track = state
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .track_id
                    .clone();
                // 排队期间已切到别的曲目：这次 seek 已经过期。
                if current_track != track_id {
                    continue;
                }

                let target = std::time::Duration::from_secs_f64(seconds);
                let volume = state
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .volume;
                let paused = player.as_ref().is_some_and(rodio::Player::is_paused);
                let in_place = player
                    .as_ref()
                    .filter(|current| !current.empty())
                    .is_some_and(|current| current.try_seek(target).is_ok() && !current.empty());
                if in_place {
                    mark_seek_succeeded(&state, sequence, seconds, !paused);
                    finished_reported = false;
                    watchdog = OutputWatchdog::new(std::time::Instant::now());
                    continue;
                }

                if let Some(previous) = player.take() {
                    previous.stop();
                }
                if output.is_none() {
                    match open_output(stream_error.clone()) {
                        Ok(device) => output = Some(device),
                        Err(err) => {
                            finished_reported = true;
                            log_engine_failure("重开音频输出失败", &err);
                            let message = AUDIO_DEVICE_UNAVAILABLE.to_string();
                            mark_seek_failed(&state, sequence, message);
                            continue;
                        }
                    }
                }
                let Some(path) = current_path.clone() else {
                    finished_reported = true;
                    mark_seek_failed(&state, sequence, "当前曲目没有可重新打开的文件".to_string());
                    continue;
                };
                let Some(device) = output.as_ref() else {
                    finished_reported = true;
                    let message = AUDIO_DEVICE_UNAVAILABLE.to_string();
                    mark_seek_failed(&state, sequence, message);
                    continue;
                };
                let initial_gain = f32::from_bits(loudness_gain_bits.load(Ordering::Relaxed));
                match rebuild_player_at(
                    device.mixer(),
                    &path,
                    target,
                    volume,
                    paused,
                    Arc::clone(&loudness_gain_bits),
                    initial_gain,
                ) {
                    Ok(rebuilt) => {
                        player = Some(rebuilt);
                        watchdog = OutputWatchdog::new(std::time::Instant::now());
                        mark_seek_succeeded(&state, sequence, seconds, !paused);
                        finished_reported = false;
                    }
                    Err(err) => {
                        // 绝不能把空 player 留在“尚未上报完成”的状态：
                        // 下一个 tick 会重新上报 finished，UI 仍会自动下一首。
                        finished_reported = true;
                        mark_seek_failed(&state, sequence, err);
                    }
                }
            }
            Command::SetVolume(volume) => {
                if let Some(current) = &player {
                    current.set_volume(volume);
                }
                {
                    let mut snap = state.lock().unwrap_or_else(|error| error.into_inner());
                    snap.volume = volume;
                }
            }
        }
        if let Some(current) = &player {
            sync_progress(&state, current);
        }
    }
}

/// 默认设备变化后把输出重建到新设备，并从原位置继续播放。
///
/// 打开新设备失败时**保留旧流**（例如目标设备刚好在枚举瞬间消失），这次切换就此放弃：
/// 设备探测器已经记住了新的默认设备，下一次轮询不会再报同一次变化。
#[allow(clippy::too_many_arguments)]
fn follow_default_output(
    output: &mut Option<rodio::MixerDeviceSink>,
    player: &mut Option<rodio::Player>,
    current_path: &mut Option<PathBuf>,
    watchdog: &mut OutputWatchdog,
    state: &Arc<Mutex<PlaybackSnapshot>>,
    stream_error: &Arc<Mutex<Option<String>>>,
    open_output: &mut impl FnMut(OutputErrors) -> OutputResult,
    loudness_gain_bits: &Arc<AtomicU32>,
) {
    // 先把续播信息取出来，再去动旧流。
    let resume = player
        .as_ref()
        .filter(|current| !current.empty())
        .map(|current| (current.get_pos(), current.is_paused()));
    let volume = state
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .volume;

    let new_output = match open_output(stream_error.clone()) {
        Ok(new_output) => new_output,
        Err(err) => {
            // 每次设备变化只走到这里一次；不记日志的话「换了默认设备却没生效」无从排查。
            log_engine_failure("跟随默认输出失败", &err);
            return;
        }
    };
    if let Some(previous) = player.take() {
        previous.stop();
    }
    *output = Some(new_output);

    let Some((position, paused)) = resume else {
        // 没在播（已停止 / 已播完）：换好输出即可，下一次装载直接用新设备。
        *current_path = None;
        return;
    };
    let Some(path) = current_path.clone() else {
        return;
    };

    let Some(device) = output.as_ref() else {
        return;
    };
    let initial_gain = f32::from_bits(loudness_gain_bits.load(Ordering::Relaxed));
    let new_player = match rebuild_player_at(
        device.mixer(),
        &path,
        position,
        volume,
        paused,
        Arc::clone(loudness_gain_bits),
        initial_gain,
    ) {
        Ok(player) => player,
        Err(err) => {
            log_engine_failure("切换音频设备后重新打开失败", &err);
            set_error(state, "切换音频设备后没能继续播放。".to_string());
            return;
        }
    };
    *player = Some(new_player);
    *watchdog = OutputWatchdog::new(std::time::Instant::now());
    {
        let mut snap = state.lock().unwrap_or_else(|error| error.into_inner());
        snap.position_seconds = position.as_secs_f64();
        snap.playing = !paused;
        snap.error.clear();
    }
}

/// 秒 → `mm:ss`（超过 1 小时输出 `hh:mm:ss`）。
pub fn format_seconds(seconds: f64) -> String {
    if !seconds.is_finite() || seconds < 0.0 {
        return "00:00".to_string();
    }
    let total = seconds as u64;
    let hours = total / 3600;
    let minutes = (total % 3600) / 60;
    let secs = total % 60;
    if hours > 0 {
        format!("{hours:02}:{minutes:02}:{secs:02}")
    } else {
        format!("{minutes:02}:{secs:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 跑满一个轮询周期，返回每个 tick 的 `poll` 结果。
    fn advance(
        watcher: &mut DeviceWatcher,
        probe: &mut impl FnMut() -> Option<String>,
    ) -> Vec<bool> {
        (0..DEVICE_POLL_TICKS)
            .map(|_| watcher.poll(probe))
            .collect()
    }

    #[test]
    fn pending_seek_suppresses_completion_until_it_finishes() {
        // 音频线程还没取到 Seek 时，旧的空 Player 不能再次上报完成；
        // 否则 UI 会消费新事件并自动下一首，正是 issue #50 的复现路径。
        assert!(!should_report_completion(true, false, true));
        assert!(should_report_completion(true, false, false));
        assert!(!should_report_completion(true, true, false));
        assert!(!should_report_completion(false, false, false));
    }

    #[test]
    fn seek_immediately_invalidates_finished_before_audio_thread_runs() {
        let (tx, rx) = channel();
        let engine = PlaybackEngine {
            tx,
            snapshot: Arc::new(Mutex::new(PlaybackSnapshot {
                track_id: "track".into(),
                finished: true,
                finished_seq: 4,
                position_seconds: 120.0,
                ..Default::default()
            })),
            next_load: AtomicU64::new(1),
            loudness: crate::loudness::LoudnessManager::new(
                std::env::temp_dir().join("sodam-test-loudness-1.json"),
                false,
            ),
            loudness_gain_bits: Arc::new(AtomicU32::new(1.0_f32.to_bits())),
        };

        engine.seek(10.0);

        let snap = engine.snapshot();
        assert!(!snap.finished);
        assert!(snap.seek_pending);
        assert_eq!(snap.seek_seq, 1);
        assert_eq!(snap.position_seconds, 10.0);
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::Seek {
                sequence: 1,
                seconds: 10.0,
                ref track_id,
            }) if track_id == "track"
        ));
    }

    #[test]
    fn seek_completion_updates_snapshot_without_replaying_finished_event() {
        let state = Arc::new(Mutex::new(PlaybackSnapshot {
            track_id: "track".into(),
            error: "old".into(),
            finished: true,
            seek_pending: true,
            ..Default::default()
        }));

        mark_seek_succeeded(&state, 0, 42.0, true);

        let snap = state.lock().unwrap();
        assert!(!snap.seek_pending);
        assert!(!snap.finished);
        assert!(snap.error.is_empty());
        assert!(snap.playing);
        assert_eq!(snap.position_seconds, 42.0);
    }

    #[test]
    fn failed_seek_does_not_leave_a_completion_to_be_reported_again() {
        let state = Arc::new(Mutex::new(PlaybackSnapshot {
            seek_pending: true,
            playing: true,
            ..Default::default()
        }));

        mark_seek_failed(&state, 0, "cannot rebuild".into());

        let snap = state.lock().unwrap();
        assert!(!snap.seek_pending);
        assert!(!snap.finished);
        assert!(!snap.playing);
        assert_eq!(snap.error, "cannot rebuild");
    }

    #[test]
    fn device_watcher_reports_each_switch_exactly_once() {
        let device = std::rc::Rc::new(std::cell::RefCell::new(Some("wasapi:speakers".to_string())));
        let mut watcher = DeviceWatcher::new(device.borrow().clone());
        let mut probe = || device.borrow().clone();

        // 没换设备：一个周期内一次都不报。
        assert!(advance(&mut watcher, &mut probe)
            .iter()
            .all(|changed| !changed));

        // 切到 USB 耳机：只在真正探测的那一个 tick 报一次。
        *device.borrow_mut() = Some("wasapi:usb-headset".to_string());
        let ticks = advance(&mut watcher, &mut probe);
        assert_eq!(ticks.iter().filter(|changed| **changed).count(), 1);
        assert!(ticks.last().copied().unwrap_or(false));

        // 之后不再重复报（否则会持续重建输出）。
        assert!(advance(&mut watcher, &mut probe)
            .iter()
            .all(|changed| !changed));
    }

    #[test]
    fn device_watcher_ignores_unknown_start_and_transient_failures() {
        let device: std::rc::Rc<std::cell::RefCell<Option<String>>> =
            std::rc::Rc::new(std::cell::RefCell::new(None));
        // 启动时没有默认设备：首次探测到设备只做采纳，不算切换。
        let mut watcher = DeviceWatcher::new(None);
        let mut probe = || device.borrow().clone();
        *device.borrow_mut() = Some("coreaudio:builtin".to_string());
        assert!(advance(&mut watcher, &mut probe)
            .iter()
            .all(|changed| !changed));

        // 探测失败（一瞬间拿不到默认设备）不能把正在播的歌打断。
        *device.borrow_mut() = None;
        assert!(advance(&mut watcher, &mut probe)
            .iter()
            .all(|changed| !changed));

        // 设备回来且没变：不报切换。
        *device.borrow_mut() = Some("coreaudio:builtin".to_string());
        assert!(advance(&mut watcher, &mut probe)
            .iter()
            .all(|changed| !changed));

        // 真正换成 AirPods 才报。
        *device.borrow_mut() = Some("coreaudio:airpods".to_string());
        assert_eq!(
            advance(&mut watcher, &mut probe)
                .iter()
                .filter(|changed| **changed)
                .count(),
            1
        );
    }

    #[test]
    fn watchdog_reports_stalls_but_not_pauses_or_normal_progress() {
        let now = std::time::Instant::now();
        let mut watchdog = OutputWatchdog::new(now);
        assert!(!watchdog.stalled(0.0, true, now + std::time::Duration::from_secs(14)));
        assert!(watchdog.stalled(0.0, true, now + std::time::Duration::from_secs(15)));
        assert!(!watchdog.stalled(0.0, false, now + std::time::Duration::from_secs(100)));
        assert!(!watchdog.stalled(0.0, true, now + std::time::Duration::from_secs(101)));
        assert!(!watchdog.stalled(2.0, true, now + std::time::Duration::from_secs(115)));
        assert!(!watchdog.stalled(3.0, true, now + std::time::Duration::from_secs(129)));
    }

    #[test]
    fn saved_volume_and_explicit_override_are_sanitized() {
        assert_eq!(initial_volume(0.37, None), 0.37);
        assert_eq!(initial_volume(0.0, None), 0.0);
        assert_eq!(initial_volume(0.37, Some("0")), 0.0);
        assert_eq!(initial_volume(0.37, Some("NaN")), 0.37);
        assert_eq!(initial_volume(0.37, Some("bad")), 0.37);
        assert_eq!(initial_volume(3.0, None), 1.0);
    }

    #[test]
    fn audio_thread_acknowledges_failed_loads_and_accepts_retries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("invalid.m4a");
        std::fs::write(&path, b"not an audio file").unwrap();
        let opens = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let worker_opens = opens.clone();
        let engine = PlaybackEngine::with_output(
            0.0,
            move |_| {
                worker_opens.fetch_add(1, Ordering::SeqCst);
                Err(rodio::DeviceSinkError::NoDevice)
            },
            || None,
        );
        for _ in 0..2 {
            let seq = engine.load(
                TrackItem {
                    id: "invalid".into(),
                    ..Default::default()
                },
                path.clone(),
            );
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while engine.snapshot().load_seq != seq && std::time::Instant::now() < deadline {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            let snap = engine.snapshot();
            assert_eq!(snap.load_seq, seq);
            assert!(!snap.error.is_empty());
            assert!(!snap.playing && !snap.finished);
        }
        // 启动一次、两次装载各重试一次，失败没有把设备恢复永久锁死。
        assert_eq!(opens.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn disconnected_worker_acknowledges_loads() {
        let (tx, rx) = channel();
        drop(rx);
        let engine = PlaybackEngine {
            tx,
            snapshot: Arc::new(Mutex::new(PlaybackSnapshot::default())),
            next_load: AtomicU64::new(1),
            loudness: crate::loudness::LoudnessManager::new(
                std::env::temp_dir().join("sodam-test-loudness-1.json"),
                false,
            ),
            loudness_gain_bits: Arc::new(AtomicU32::new(1.0_f32.to_bits())),
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cached.m4a");
        std::fs::write(&path, b"cached").unwrap();
        let seq = engine.load(
            TrackItem {
                id: "failed".into(),
                ..Default::default()
            },
            path.clone(),
        );
        let snap = engine.snapshot();
        assert_eq!(snap.load_seq, seq);
        assert_eq!(snap.track_id, "failed");
        assert!(!snap.playing && !snap.finished && !snap.error.is_empty());
    }

    #[test]
    fn load_failure_acknowledges_the_request_and_clears_finished() {
        let state = Arc::new(Mutex::new(PlaybackSnapshot {
            finished: true,
            playing: true,
            ..Default::default()
        }));
        let track = TrackItem {
            id: "failed-track".into(),
            ..Default::default()
        };
        load_failed(&state, &track, 42, "device unavailable".into());
        let snap = state.lock().unwrap();
        assert_eq!(snap.load_seq, 42);
        assert_eq!(snap.track_id, track.id);
        assert!(!snap.playing && !snap.finished);
        assert!(!snap.error.is_empty());
    }

    #[test]
    fn buffer_underrun_does_not_stop_playback_but_device_loss_does() {
        // 回归：ALSA 欠载只是一次抖动，cpal 会 try_recover 接着播；
        // 写成致命错误会让界面停在「音频输出中断了。」（蓝牙 A2DP 下实测踩过）。
        let errors: OutputErrors = Arc::new(Mutex::new(None));
        assert_eq!(
            record_stream_error(&rodio::cpal::StreamError::BufferUnderrun, &errors),
            "underrun"
        );
        assert!(
            errors.lock().unwrap().is_none(),
            "欠载不能写致命错误，否则整首会停住"
        );

        // 设备真的没了，仍然要停机报错。
        assert_eq!(
            record_stream_error(&rodio::cpal::StreamError::DeviceNotAvailable, &errors),
            "device_lost"
        );
        assert_eq!(
            errors.lock().unwrap().as_deref(),
            Some("音频输出中断了。"),
            "设备丢失必须写致命错误"
        );
    }

    #[test]
    fn engine_starts_without_audio_device_and_reports_state() {
        // 确定性模拟没有音频设备：不依赖 runner 恰好未安装声卡。
        let engine =
            PlaybackEngine::with_output(1.0, |_| Err(rodio::DeviceSinkError::NoDevice), || None);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while engine.snapshot().error.is_empty() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let snap = engine.snapshot();
        assert_eq!(snap.volume, 1.0);
        assert!(!snap.playing);
        assert!(!snap.error.is_empty());
    }
}
