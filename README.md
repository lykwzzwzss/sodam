<div align="center">
  <img src="crates/sodam/assets/brand/sodam-logo.svg" width="96" alt="SodaM logo">
  <h1>SodaM</h1>
  <p>基于 GPUI + Rust 的原生桌面音乐播放器：本地文件夹即歌单，免登录、不联网</p>
  <p>
    <a href="https://github.com/sodahub-org/sodam/blob/main/LICENSE"><img src="https://img.shields.io/badge/License-AGPL%203.0%20or%20later-blue.svg" alt="License: AGPL-3.0-or-later"></a>
    <a href="https://www.rust-lang.org/"><img src="https://img.shields.io/badge/Rust-1.85%2B-orange.svg" alt="Rust 1.85+"></a>
    <a href="https://github.com/sodahub-org/gpui"><img src="https://img.shields.io/badge/UI-GPUI-8B5CF6.svg" alt="GPUI"></a>
    <img src="https://img.shields.io/badge/platform-Windows%20%7C%20macOS%20%7C%20Linux-success.svg" alt="Windows / macOS / Linux">
  </p>
</div>

SodaM 是一个**纯本地**的桌面音乐播放器：选一个音乐文件夹，它就把文件夹里的
音频变成歌单；不登录、不联网、不上报任何数据。界面按汽水音乐桌面端的比例和
手感重做，播放页会读取音频里的内嵌封面与内嵌歌词：

- 封面 → OKLab 取色 → 9 阶主题色，整页颜色随封面变化（侧栏/播放栏磨砂透色）；
- 点击播放栏封面/标题展开「正在播放」：大封面 + 右侧同步歌词；
- 歌词支持内嵌 LRC / USLT，也支持同名 `.lrc` 文件；当前行高亮并居中滚动，点击跳转。

> 早期版本曾是在线客户端。联网/登录/搜索/推荐/缓存下载/播放上报等能力已于
> 2026-11 全部移除，历史实现见 git 历史。当前主分支不包含任何 HTTP 客户端。

## 功能

### 本地音乐库

- 文件夹即歌单：根目录下的音乐统一进「全部歌单」，每个子文件夹各自成为一个歌单；
- 后台扫描，按曲名排序；扫描失败/空歌单/未选文件夹都有可行动的空态；
- 曲目列表虚拟化，滚轮平滑滚动；正在播放行用主题色标记。

### 播放

- rodio + Symphonia：MP3 / FLAC / M4A(MP4/AAC) / WAV / OGG / Vorbis；
- 播放 / 暂停、上一首 / 下一首、拖动进度、音量、静音；
- 列表循环 / 单曲循环 / 随机播放，跨启动保留；
- 退出时记住整个播放现场：队列顺序、当前曲目与进度、随机播放的历史、
  浏览中的歌单；下次启动恢复到原位置并保持暂停；
- 队列分为「已播放 / 正在播放 / 接下来」，右键可移除；
- 系统媒体控制：Windows SMTC、macOS Now Playing、Linux MPRIS。

### 正在播放页

- 360 大封面 + 曲名/歌手/专辑，右侧 560 宽歌词；
- 内嵌歌词/侧车 `.lrc`，支持时间轴、一行多时间戳、翻译行与 offset；
- 当前行随播放位置高亮并居中滚动，点击歌词行 seek；
- 没有封面/歌词时给出明确的空态，不空白页。

### 界面

- 侧栏 256、顶栏 60、播放栏 112、队列侧板 420，按汽水音乐实机比例；
- 播放按钮 100×56 胶囊，圆角 8/12/999，大字重 500/600；
- 顶栏文件夹胶囊、白色「更改文件夹」主按钮；
- 队列侧板与菜单使用磨砂玻璃（GPUI backdrop blur）；
- 固定深色主题；
- 中英文界面，系统托盘，关闭到托盘。

## 构建

需要 Rust 1.85+。仓库自带 Windows GNU 工具链时，确保 `RUSTUP_HOME`、
`CARGO_HOME` 与 `PATH` 指向工作区内的 `rustup/`、`cargo/`、`mingw/`。

```bash
cargo build -p sodam          # debug
cargo test --workspace        # 单元测试
cargo clippy --workspace      # 静态检查
cargo fmt --all
```

运行：

```bash
cargo run -p sodam
```

首次启动点击顶栏的文件夹图标选择音乐文件夹。开发期可用 `SODAM_LOCAL_FOLDER` 临时覆盖
音乐目录（不写回配置），`SODAM_FRAME_LOG=1` 查看慢帧，`SODAM_SCROLL_TEST=1`
做滚动压测。

## 目录

```text
crates/sodam-core/   领域层：配置、扫描、队列、播放、封面取色、歌词解析（无 GPUI）
crates/sodam/        GPUI 应用：Root 状态、侧栏/顶栏/播放栏、播放页、托盘
docs/                架构、UI 规范、路线图、本地库契约
packaging/           Arch Linux / Windows 打包
```

## 文档

- [架构](docs/ARCHITECTURE.md)
- [UI 规范](docs/UI-SPEC.md)
- [路线图](docs/ROADMAP.md)
- [本地音乐库契约](docs/LOCAL_LIBRARY.md)
- [隐私说明](PRIVACY.md)

## 许可证

[AGPL-3.0-or-later](LICENSE)。非官方项目，与汽水音乐官方无关。
