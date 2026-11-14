# SodaM 隐私说明

生效日期：2026-11-10

SodaM 是一个**纯本地**音乐播放器：不登录、不联网、不上报任何数据。
早期在线版本的能力（账号、Cookie、签名服务、播放上报、缓存下载）已经移除。

## 1. 不收集的数据

SodaM 不收集、不上传任何个人信息，也没有账号体系、广告、用户画像或遥测。
应用运行期间不会发起任何网络请求。

## 2. 本地保存的数据

SodaM 只在本机读写以下内容：

- **配置**：音量、播放模式、主题、语言、窗口选项、快捷键、详细日志开关、
  本地音乐文件夹路径。
  - Windows：`sodam.exe` 同级的 `config.json`。升级时若同级文件不存在，会从旧的
    `%APPDATA%\sodam\config.json` 导入已有设置。
  - Linux：`~/.config/sodam/config.json`
  - macOS：`~/Library/Application Support/sodam/config.json`
- **播放会话**：上次的播放队列、当前曲目与播放位置、随机播放的历史与
  浏览中的歌单，写入本机数据目录下的 `sodam/session.json` 与
  `sodam/checkpoint.json`（Windows 为 `%LOCALAPPDATA%`，Linux 为
  `~/.local/share`，macOS 为 `~/Library/Application Support`）。
- **封面缓存**：从音频内嵌图片解码出的原图与预模糊背景，写入系统缓存目录下的
  `sodam/artwork/`。删除缓存目录即可清除；下一次播放会重新生成。
- **诊断日志**：默认关闭详细日志；开启后写入用户数据目录下的 `sodam/logs/`。
  日志只记录播放/设备/文件错误等本机事件，不包含音频内容或账号凭据。

播放列表来自用户选择的本地文件夹；SodaM 不会把文件路径、曲目列表或封面
发送到任何服务器。

## 3. 系统集成

- Windows 系统媒体控件（SMTC）会向系统暴露当前曲名、歌手、专辑与播放状态，
  这是系统媒体面板的常规行为，数据留在本机操作系统内。
- macOS Now Playing、Linux MPRIS 同理。
- 托盘图标与「关闭到托盘」只使用本机进程状态。

## 4. 第三方组件

SodaM 使用 rodio/Symphonia（解码）、lofty（标签）、GPUI（界面）等开源库。
这些库在本地运行，不包含 SodaM 主动调用的网络功能。

## 5. 变更

如果未来版本重新引入任何联网能力，会先更新本说明并在界面中明确告知。
当前主分支不包含 HTTP 客户端、Cookie、Token 或设备指纹字段。

## 6. 联系

问题反馈请走仓库 issue。请勿在 issue 中附带个人音乐库路径、账号凭据或
其他隐私数据。
