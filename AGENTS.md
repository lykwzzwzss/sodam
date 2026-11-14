# AGENTS.md — sodam 开发约定

面向在本仓库工作的 AI agent / 协作者。动代码前先读本文件。

## 项目定位

`sodam`（SodaM）是基于 **GPUI** 的**纯本地音乐播放器**：「文件夹即歌单」，
不登录、不联网。2026-11 起把此前基于
[libresoda](https://github.com/sodahub-org/libresoda) /
[libmssdk](https://github.com/sodahub-org/libmssdk) 的在线能力全部移除
（搜索、推荐、歌单、收藏、缓存下载、预取、播放上报、签名），历史实现见 git 历史。

交互与视觉风格参考汽水音乐官方 PC 客户端和 [sonora](https://github.com/sonorahq/sonora)。
只借鉴工程思路与交互；不复制 GPL 代码。

## 目录

```text
crates/sodam-core/src/   领域层：配置 / 本地曲库 / 队列 / 播放引擎（不依赖 GPUI、不联网）
  config.rs              配置读写（语言/快捷键/音量/本地文件夹）
  local.rs               「文件夹即歌单」扫描：LocalLibrary / LocalPlaylist / LocalTrack
  models.rs              TrackItem（id = "local:" + 绝对路径）
  queue.rs               播放队列与三种播放模式（列循 / 单循 / 随机轮次）
  audio.rs               rodio 播放线程（解码器：Symphonia）
  artwork.rs             内嵌封面解码、OKLab 聚类取色、9 阶主题色板
  lyrics.rs              内嵌 LRC/USLT + 同名 .lrc 解析（时间轴/翻译/offset）
  preferences.rs         音量等偏好的后台单写者
  log_outlet.rs          本机诊断日志（轮转 + 内存降级）
crates/sodam/src/        GPUI 应用
  main.rs                入口、窗口、托盘与系统媒体控制装配
  app.rs                 Root 状态、渲染入口
  app_actions.rs         播放、队列、音量动作
  app_local.rs           选文件夹、扫描、本地歌单切换与播放
  app_artwork.rs         封面异步加载、整页取色应用
  app_lyrics.rs          歌词异步加载、当前行同步、播放页开关
  app_scroll.rs          列表滚轮平滑滚动
  tray.rs                托盘（Windows/macOS: tray-icon；Linux: StatusNotifier）
  media/                 系统媒体控制（Windows SMTC / macOS Now Playing / Linux MPRIS）
  taskbar_player/        任务栏快捷播控（UIA 空间探测 + 窗口宿主 + 播放条视图 + 显隐淡入淡出）
  ui/                    theme、motion（统一动效）、sidebar、topbar、backdrop、player_bar、icons、i18n
  views/                 Local、Player（大封面+歌词）
docs/ARCHITECTURE.md     架构与线程模型
docs/ROADMAP.md          当前状态与剩余工作
docs/UI-SPEC.md          UI 约束与主题规范
docs/UI-INTERACTIONS.md  全部可感知交互与过渡动画清单
docs/MOTION.md           动效系统：时长/缓动/交叉淡化/列表动画规范
docs/LOCAL_LIBRARY.md    本地音乐库契约
packaging/arch/          Arch Linux PKGBUILD 与 desktop 文件
packaging/windows/       Windows 安装器脚本与本机验收注意事项
```

## 硬性约定

1. **领域层不依赖 GPUI、不联网**：`sodam-core` 保持纯 Rust，可单独测试；
   任何 `ureq`/`reqwest`/HTTP 客户端与账号字段都禁止重新引入（见 docs/ROADMAP.md 非目标）。
2. **不阻塞渲染线程**：扫描、文件存在性检查等 IO 必须放在 `cx.background_spawn`
   或专用线程，结果回 UI 后 `cx.notify()`。
3. **状态只有一处**：播放、队列、导航、设置都由 `Root` 或 `sodam-core` 模型持有；
   UI 只读状态、生成元素、派发动作。
4. **错误不 panic**：文件/配置错误写入状态行（`Root::status_line`，见 `set_status_text`）
   或页面提示；只有启动期不可恢复错误才允许 panic。
5. **凭据不入库、不入日志**：任何账号、Cookie、签名 Token 禁止提交或打印。
6. **UI 改动必须真机验证**：Windows 用 `_dev/run.ps1` 启动确认；Arch 用
   `scripts/run.sh`；不能只过 `cargo check`。
7. **提交前必须跑** check + fmt + clippy + test（Arch 用 `scripts/check.sh` 资源护栏；
   Windows 直接跑四件套）。
8. **界面文案走 i18n**：新增用户可见文案使用 `root.tr` / `root.localized`，
   中文原文作为稳定 key，并同步英文词典（`ui/i18n.rs` 有静态守卫测试）。
9. **不要引入个人数据**：不要提交真实配置、本机路径或账号数据。

## 开发环境（Windows 工作区）

全部工具链都在仓库目录内（可直接随仓库移动）：

```text
rustup/  cargo/  mingw/  三套工具链；target/ 是构建产物缓存
env.ps1  设置 RUSTUP_HOME/CARGO_HOME/PATH；从工作区根跑 cargo 即可（默认 target/）
_dev/    真机验证脚本（run*.ps1 启动 sodam + 窗口操作 + 截图）
```

`_dev/run-window.ps1`（自然启动尺寸截图）、`_dev/measure-window.ps1`（量窗口矩形）等
是验收 UI 用的；改动界面后至少跑一个。

## GPUI 依赖

使用 `Cargo.lock` 固定的 GPUI 源码直接构建。Windows 主窗口从系统读取主屏 DPI，
任务栏窗口读取任务栏 DPI。Windows 队列侧板用 `occlude()` 隔开下层的原生标题栏命中区，
侧板内的列表继续处理滚动。

## 资源护栏

2026-09-15 一台 Arch Linux ARM64 / 15GB 设备出现过编译与测试引发的 OOM。必须遵守：

1. 使用 `scripts/check.sh` / `scripts/build.sh` / `scripts/run.sh`；
2. 编译时不运行 GUI，运行 GUI 时不编译；
3. 大改后优先 `scripts/check.sh`，不要无护栏反复全量 release 编译；
4. 卡顿时先看 `journalctl -k | grep -iE "drm|flip_done|oom"`；
5. 明确禁止本地编译时，至少跑 `scripts/check.sh --fmt-only`，编译与测试交给远端 CI。
