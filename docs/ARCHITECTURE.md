# 架构

## 总览

```text
┌────────────────────────── crates/sodam（GPUI 层） ───────────────────────────┐
│ Root                                                                          │
│  ├── 导航（本地音乐 / 设置）/ 设置 / 播放队列快照                              │
│  ├── app_actions.rs    播放、队列、主题/语言、诊断日志动作                      │
│  ├── app_local.rs      选文件夹、扫描、本地歌单切换与播放                       │
│  ├── app_artwork.rs    内嵌封面异步加载、切歌色板过渡                           │
│  ├── app_lyrics.rs     内嵌歌词异步加载、当前行同步、播放页展开/收起            │
│  ├── app_scroll.rs     列表滚轮平滑滚动                                        │
│  ├── ui::sidebar       侧边栏：本地音乐 + 文件夹歌单 + 设置                     │
│  ├── ui::topbar        文件夹胶囊、扫描/换文件夹、窗口控制                       │
│  ├── ui::backdrop      9 阶主题色组成的整页色场                                  │
│  ├── views::local      本地音乐页：歌单头图 + 曲目表                            │
│  ├── views::player     「正在播放」页：大封面 + 右侧同步歌词                    │
│  ├── ui::player_bar    播放控制、全宽进度线、音量、播放模式、队列侧板           │
│  ├── ui::theme         封面取色主题、深色结构色、尺寸、共享颜色                 │
│  ├── media/            MPRIS（Linux）/ SMTC（Windows）/ Now Playing（macOS）   │
│  └── tray.rs           托盘与托盘菜单                                          │
└───────────────────────────────────────────────────────────────────────────────┘
                                 │
                                 ▼
┌──────────────────── crates/sodam-core（领域层，无 GPUI，不联网） ─────────────┐
│ Settings        配置文件读写                                                   │
│ local           「文件夹即歌单」扫描：LocalLibrary / LocalPlaylist             │
│ artwork         内嵌封面解码、OKLab 聚类取色、9 阶主题色板                      │
│ lyrics          内嵌 LRC/USLT + 同名 .lrc 解析、时间轴/翻译/offset             │
│ models          TrackItem（id = "local:" + 绝对路径）                          │
│ Queue           当前曲目、随机轮次、列表循环 / 单曲循环 / 随机                  │
│ PlaybackEngine  rodio 线程；接收 Load / Play / Pause / Seek / Volume 命令       │
│ preferences     音量等偏好单写者（防抖 + 退出刷尾）                             │
│ log_outlet      本机诊断日志（按大小轮转 + 内存降级）                           │
└───────────────────────────────────────────────────────────────────────────────┘
```

应用**不联网**：没有 HTTP 客户端、没有账号、没有 Cookie。之前走
libresoda / libmssdk 的在线能力（搜索、推荐、收藏、缓存下载、播放上报、签名）
已于 2026-11 移除，历史实现见 git 历史。

## 状态模型

`Root` 是 GPUI 侧唯一根实体。子模块不保存第二份全局状态，只读 `&Root` 或通过
`cx.listener` / entity update 派发动作。滚动句柄、菜单位置等 UI 状态放在 `Root`。

当前曲目的取值顺序是：

```text
pending_track（正在装载的新歌）
  → queue.current()（已生效的当前歌）
```

这样可以避免切歌时出现“界面显示 A、耳朵听到 B”的竞态。

反馈分两条通道，各管一件事：

- **状态行**（`Root::status_line` → `ui::status_line`）：普通操作反馈（扫描、
  播放、模式切换…）的固定出口，渲染在播放栏上方。写入方只管「写什么」；
  去重、超时与切页清空都在这一处。
- **全局 message**（`ui::message`）：需要用户注意的事件（快捷键冲突、
  文件失效、设置保存失败）。同一来源用稳定 key 替换，最多同时 4 条。
  这类事件同时写状态行：轻提示 3 秒就走，状态行留到 5 秒。

## 线程与异步

1. 本地库扫描、文件存在性检查放到 `cx.background_spawn`；
2. 完成后用 `cx.spawn` + entity update 回到 GPUI 实体上下文，然后 `cx.notify()`；
3. 带顺序语义的装载用 `play_seq` 丢弃过期结果（装载中途换歌时旧结果作废）；
4. 音频线程独立于 GPUI，通过 channel 接收命令，通过 snapshot 共享状态。

## 播放链路

```text
LocalTrack（扫描产出：路径 + 标签）
  → TrackItem（id = "local:" + path）
  → PlaybackEngine::load(track, path, "本地") → 装载序号
  → rodio Decoder + Player（专属线程）
  → snapshot: track_id / title / playing / position / duration / error
```

播放结束时音频线程标记 `finished`；GPUI 心跳按序号（`finished_seq`/`seek_seq`）
去重后自动续播：普通与随机模式推进队列，单曲循环原地重播。
装载失败（文件被移走、解码失败）只给一句可行动的原因（状态行 + 轻提示），
不自动跳歌；下一步由用户决定。

装载成功/失败都回执 `load_seq`，心跳据此退出 loading，不再只比较 track id。
输出设备错误或非暂停状态下 15 秒无进度会释放失效输出并报告错误；用户再次点播放时
重新打开文件与设备，不会因为启动/休眠后的一次设备失败而永远不可播放。

系统默认输出设备被切换时（音响 → USB 耳机），播放线程每 2 秒探测一次默认设备 ID，
变化后换新输出并从原位置续播 —— Windows(WASAPI) / macOS(CoreAudio) 的流在创建时就
绑死设备，不重建就得重启应用才生效。Linux 的 cpal ALSA 后端恒为单一的 `default` 设备，
现有流由 PipeWire/WirePlumber 自动迁到新的默认节点，因此探测不会触发。

## 封面、主题色与歌词

```text
音频文件
  ├─ lofty: 内嵌图片 → image 解码
  │    → OKLab k-means 取主色/活力色 → 9 阶主题色（base-1..9）
  │    → 整页色场 / 侧栏 / 播放栏一起随色板过渡
  └─ lofty: LYRICS / USLT → LRC 解析（无内嵌时读同名 .lrc）
       → 时间轴 / 翻译 / offset → 播放页当前行高亮
```

- 封面与歌词都在 `cx.background_spawn` 里准备，回 UI 线程前用请求序号丢弃过期结果；
- 切歌时 9 阶色板从旧封面插值到新封面（520ms），整页颜色连续变化；
- 主题色铺满整页背景，侧栏/播放栏用半透明底透出这层颜色；文字/描边仍是固定的白色透明度阶梯；
- 播放页由「点击播放栏封面/标题/词按钮」展开，Esc 或返回按钮关闭；歌词行点击可跳转。

## 播放偏好持久化

- `PreferencesWriter` 是后台单写者：音量/静音防抖保存，只合并这些字段，
  避免覆盖刚修改的主题/语言。所有配置写入用同一进程内锁和同目录原子替换，退出刷尾。

## 导航

左侧只有「本地音乐」一个导航入口。
「正在播放」不是导航项，而是 `Root::player_view_open` 覆盖在内容区上的
沉浸页；点播放栏封面/标题或 `词` 按钮进入，返回按钮/`Esc` 退出。

## 队列与历史

`Queue` 保存曲目列表和当前索引；播放后的曲目进入 `played_history`。
队列展示分为已播放、正在播放、接下来三段。

播放模式有三种，点击按「列表循环 → 单曲循环 → 随机播放」轮换，存在
`Settings.play_mode`：

- **列表循环**：播到末尾回到第一首。
- **单曲循环**：停在同一首，由播放器决定是否重播。
- **随机播放**：不洗牌队列本身（抽屉里仍是真实曲目顺序），而是预生成一整轮
  待播下标 `shuffle_round`，`advance` 依次取用；一轮播完立即重建并清空
  `history`。`history` 记录走过的下标，「上一首」沿它回退到真正播过的那首，
  `redo` 让「上一首 → 下一首」原路返回。

`shuffle_round` 与 `history` 存的都是下标，所以队列变更必须同步：
`remove` 左移、`jump` 把目标从轮次剔除（否则会重播）。「接下来」由
`Queue::upcoming()` 给出，与真实播放顺序一致。

## 主题与 i18n

- 固定深色主题，不提供明暗切换。
- 整页颜色来自当前封面的 9 阶色板；没有封面时回到深色中性板。
- 界面文案使用 `ui/i18n.rs`；中文原文是稳定 key，英文由词典映射，
  词典有静态守卫测试兜底。

## 系统媒体控制

- Windows：SMTC（souvlaki），挂在主窗口 HWND 上。
- macOS：Now Playing 中心（souvlaki）。
- Linux：MPRIS（`media/mpris.rs`）。
- 播放状态每 1 秒同步一次；内容没变时 publish 内部跳过。

## 打包与运行时

- Linux ARM64 使用 `packaging/arch/PKGBUILD` 生成 pacman 包。
- 安装内容：`/usr/bin/sodam`、desktop entry、hicolor icon。
- GUI 通过 `scripts/run.sh` 启动时带 `MemoryMax=2G` 护栏。
- 托盘和窗口关闭逻辑使用显式退出模式：关闭主窗口保留进程，退出走托盘菜单。
