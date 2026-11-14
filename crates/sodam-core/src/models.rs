//! UI 友好的数据模型（本地曲目就是界面直接用得上的形状）。

use serde::{Deserialize, Serialize};

/// 曲目（列表行 / 播放器 / 队列都用它）。
///
/// `id` 对本地曲目是 `"local:" + 绝对路径`（见 [`crate::local`]），本身就是磁盘上的稳定主键；
/// `cover` 已随在线功能移除——本地文件不以 URL 表示封面。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TrackItem {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_seconds: i64,
}

impl TrackItem {
    /// `mm:ss`（时长未知时返回 `--:--`）。
    pub fn duration_label(&self) -> String {
        if self.duration_seconds <= 0 {
            return "--:--".to_string();
        }
        format!(
            "{:02}:{:02}",
            self.duration_seconds / 60,
            self.duration_seconds % 60
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_label_formats() {
        let mut track = TrackItem {
            duration_seconds: 236,
            ..Default::default()
        };
        assert_eq!(track.duration_label(), "03:56");
        track.duration_seconds = 0;
        assert_eq!(track.duration_label(), "--:--");
    }

    /// 老版本的队列缓存里 TrackItem 带 cover / vip / *_id 字段：反序列化要能读出来。
    #[test]
    fn old_serialized_tracks_load_with_extra_fields_ignored() {
        let text = r#"{
            "id": "local:C:/music/a.flac",
            "title": "a",
            "artist": "x",
            "album": "",
            "artist_id": "1",
            "album_id": "2",
            "cover": "http://example/cover.jpg",
            "duration_seconds": 200,
            "vip": false
        }"#;
        let item: TrackItem = serde_json::from_str(text).expect("老字段应被忽略");
        assert_eq!(item.title, "a");
        assert_eq!(item.duration_seconds, 200);
    }
}
