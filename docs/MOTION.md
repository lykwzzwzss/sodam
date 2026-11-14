# 动效系统（Motion Design System）

SodaM 的动效定位：**Fluent Design 的克制 + Apple 式的空间连续性 + 音乐播放器的柔和流动感**。
这一页是动画参数的唯一规范；代码里的时长/缓动都从 `crates/sodam/src/ui/motion.rs` 取。

## 1. 三个原则

1. **轻盈**：位移 4–12px，缩放 0.96–1.02，不做夸张弹跳。
2. **连续**：旧内容不突然消失；换内容用交叉淡化/双层封面，而不是「新内容直接顶掉」。
3. **灵敏**：点击、拖拽、播放状态立即响应；动画只做视觉衔接，绝不阻塞业务状态。

## 2. 三层节奏

| 层级 | 时长 | 内容 |
| --- | --- | --- |
| 操作反馈 | 80–140ms | 悬停、按下、焦点、图标状态 |
| 内容与布局 | 160–320ms | 标题/列表更新、页面、抽屉、封面、歌词 |
| 环境氛围 | 400–520ms | 整页主题色、背景色场 |

三层可以同时发生。切歌时文字先完成、封面稍后、背景最后稳定下来，形成有层次的节奏。

## 3. 统一参数

| 类别 | 时长 | 缓动 | 代码 |
| --- | --- | --- | --- |
| 悬停 | 160ms | ease-out-quint | `duration::HOVER` |
| 焦点 | 180ms | ease-out-quint | `duration::FOCUS` |
| 图标状态 | 200ms | ease-out-quint | `duration::ICON` |
| 列表「正在播放」 | 200ms | 线性插值 | `duration::PLAYING` |
| 右键菜单 | 220 / 150ms | ease-out-quint | `duration::MENU_IN/OUT` |
| 内容文字 | 220ms | ease-out-quint | `duration::CONTENT` |
| 搜索模式 | 350ms | ease-out-quint | `duration::SEARCH` |
| 歌单标题/歌数 | 400ms | ease-out-quint | `duration::PLAYLIST_CONTENT` |
| 同图 96→320 升级 | 120ms | ease-out-quint | `duration::COVER_UPGRADE` |
| 侧栏悬停 | 160 / 180ms | ease-out-quint | `duration::HOVER/SIDEBAR_HOVER_OUT` |
| 侧栏旧行背景 | 240ms | ease-out-quint | `duration::SIDEBAR_BG_OUT` |
| 侧栏新行背景 | 320ms | ease-out-quint | `duration::SIDEBAR_BG_IN` |
| 侧栏旧行文字 | 220ms | ease-out-quint | `duration::SIDEBAR_TEXT_OUT` |
| 侧栏新行文字 | 280ms | ease-out-quint | `duration::SIDEBAR_TEXT_IN` |
| 侧栏细条 | 380 / 280ms | ease-out-quint | `duration::SIDEBAR_MARKER_NEAR/FAR` |
| 页面切换 | 600ms | ease-out-quint | `duration::PAGE` |
| 队列抽屉 | 420 / 320ms | ease-out-quint | `duration::DRAWER_OPEN/CLOSE` |
| 封面更换 | 550 / 420ms | ease-out-quint | `duration::COVER/COVER_SMALL` |
| 缩略图出现 | 160ms | ease-out-quint | `duration::THUMB` |
| 歌词换行 | 420 / 380ms | ease-out-quint | `duration::LYRIC/LYRIC_OUT` |
| 状态行 | 160 / 140ms | ease-out-quint | `duration::STATUS_IN/OUT` |
| 轻提示 | 200 / 150ms | ease-out-quint | `duration::TOAST_IN/OUT` |
| 空态/错误条 | 220ms | ease-out-quint | `duration::STATE` |
| 队列删除 | 140 / 180ms | ease-out-quint | `duration::QUEUE_ROW_OUT/REFLOW` |
| 主题色 | 800ms | ease-in-out-cubic | `duration::THEME` |

`motion::standard(d)` = ease-out-quint；`motion::soft(d)` = ease-in-out-cubic。
`motion::ease_in_out_cubic` 比 GPUI 自带的二次版本更圆滑，主题色和选中高亮用它。

## 4. 底层机制

### 4.1 动画时钟与中断

- 所有时间动画走 GPUI 的 `with_animation`：按元素路径保存起始时刻，逐帧推进，
  通过 `window.request_animation_frame()` 请求下一帧，不依赖 16ms 定时器。
- 元素 id / 代次只在**状态真正变化**时更新；同一状态的重绘不会重播动画。
- 主题色使用 `Root::palette_anim`：每次从**当前中间色**继续插值，快速连续切歌不会跳回旧色。
- 需要「从当前位置打断」的动画（队列抽屉）用动画窗口控制：只在开合后的一小段时间挂
  `with_animation`，窗口外直接落最终位置，后续重绘不会把面板送回屏外。
- GPUI 的 `with_animation` 自带 `reduce_motion` 支持：开启减少动态时立即落终态。

### 4.2 业务状态与视觉状态分离

歌曲、队列、播放位置、搜索结果立即更新；只有封面、文字、底色、主题色做插值。
进度条、音量条、逐字歌词永远跟手/跟音频，不额外加缓动。

### 4.3 交叉淡化

- **文字**：`motion::crossfade_text` 把旧文案绝对定位在上层淡出，新文案占布局并淡入 +
  4px 位移；`TrackTextSwap` / `PlaylistTextSwap` 记录旧值与发生时间，只在动画窗口内渲染旧值。
- **图标**：`motion::crossfade_icon` 两层绝对居中，旧图标淡出、新图标淡入并 0.94→1 轻缩放。
- **封面**：`motion::cover_layers` 生成双层结构：旧封面保持在下，新封面在上层
  280ms 淡入 + 0.985→1 放大；新封面为空时旧封面淡出、露出占位。
  歌单头图、播放栏小封面、播放页大封面都用同一套。

### 4.4 虚拟列表

`uniform_list` 的行会被复用/卸载，不能给每行单独存动画状态：

- **悬停**：每个列表一个 `motion::RowHover`，只记录「当前悬停行」和「刚离开行」，
  行渲染时比较自己的下标，得到 `(hovered, seq)`；
- **正在播放**：`Root::now_playing_fade` 记录新旧曲目 id 和进度，
  行内按 id 计算 0..1 的高亮强度，序号/♪、标题颜色、队列底色一起过渡；
- **内容整体切换**（切歌单、搜索）：只对列表容器做一次淡入 + 6px 归位，不逐行入场；
- **队列删除**：数据立即删除；被删行用一层幽灵停在原槽位 140ms 淡出并左移 6px，
  下面的可见行用 180ms 从 +64px 滑回原位（FLIP 思路，按稳定槽位计算）。

### 4.5 悬停

- 控件（按钮、标题栏、侧栏行）用 `HoverStates` + `motion::hover_layer`：
  进入 120ms 淡入、离开 120ms 淡出；按下用 `active().layer_scale(0.96)` 做轻缩放。
- 列表行用 `RowHover` + `motion::row_hover_layer`，避免为成百上千行保存状态。
- 点击歌单行时另有一层 200ms 的临时悬停淡出，盖住「行变成选中态后悬停底色瞬间消失」。

## 5. 组件对照

| 区域 | 动效 |
| --- | --- |
| 标题栏 | 悬停底色/图标颜色 160ms；最大化图标随系统瞬时切换 |
| 顶栏 | 圆形按钮悬停 160ms；搜索框描边/底色 180ms；清除按钮 160ms 淡入 + 0.92→1 |
| 侧栏 | 每行独立背景：旧行 240ms 淡出、新行 320ms 淡入；旧文字 220ms 变暗、新文字 280ms 变亮；悬停 160/180ms；左侧细条相邻 380ms 滑动、远距离 280ms 交叉淡化 |
| 本地页 | 切歌单：标题/歌数 400ms 交叉淡化、头图 550ms 交叉淡化（同图升级 120ms，不缩放）；曲目列表直接替换，无过渡；搜索模式 350ms；加载指示延迟 220ms |
| 播放栏 | 播放/暂停、静音、播放模式图标 200ms 交叉淡化；标题/歌手 220ms；小封面 420ms；主题色 800ms |
| 队列抽屉 | 开 420ms / 关 320ms 滑入滑出；行悬停 160ms；正在播放行 200ms；删除 140ms 幽灵 + 180ms 归位；右键菜单 220ms/150ms |
| 播放页 | 页面 600ms；大封面 550ms 双层；标题/歌手/专辑 220ms；歌词面板 450ms；换行缩放 420/380ms；居中软弹簧约 0.6s |
| 全局 | 状态行 160ms 进入、140ms 退出、替换时 140ms 交叉淡化、停留 3s；轻提示 200ms 进入、150ms 退出、同 key 原位交叉淡化、最多 3 条 |

## 6. 明确不做的动画

| 场景 | 原因 |
| --- | --- |
| 音量/进度拖拽 | 必须跟手，任何缓动都会让指针和数值脱节 |
| 搜索逐字符过滤 | 保持即时；只对进入/退出搜索模式做结构过渡 |
| 滚轮连续滚动 | 保留现有 `k=180/c=30` 弹簧，不叠加第二轮缓动 |
| 窗口尺寸拖动 | 系统实时重排 |
| 进度位置、逐字歌词 | 每帧跟音频，保证时间连续 |
| 播放状态逻辑切换 | 立即执行，视觉动画不能影响播放 |

## 7. 已知取舍与后续

- **共享元素**：从小封面展开成播放页大封面的跨页面连续动画需要页面级共享元素，
  当前架构下成本过高；保留 260ms 页面淡入 + 8px 位移。
- **最大化图标**：跟随系统窗口状态瞬时切换（与原生标题栏一致）。
- **轻提示堆叠位移**：删除一条时其余提示仍是瞬时上移；需要 FLIP 才能平滑。
- **抽屉打断**：关闭途中再次打开会从屏外重新滑入，而不是从当前中间位置反向。
- **减少动态**：`with_animation` 已尊重系统设置；手动插值（主题色、行高亮）是颜色变化，
  不产生位移，未单独降级。
