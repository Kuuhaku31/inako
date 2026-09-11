
// src/utils.rs
// 工具函数

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
#[cfg(any(feature = "media", feature = "gui"))]
use std::os::unix::net::UnixStream;
#[cfg(any(feature = "gui", test))]
use crate::models::playlist_manager::PlaylistChangeKind;
#[cfg(test)]
use crate::models::playlist_manager::Playlist;
#[cfg(feature = "gui")]
use crate::models::WindowGeometry;

#[cfg(feature = "gui")]
use gtk::glib::object::CastNone;
#[cfg(feature = "gui")]
use gtk::glib::types::StaticType;
#[cfg(feature = "gui")]
use gtk::prelude::AdjustmentExt;
#[cfg(feature = "gui")]
use gtk::prelude::WidgetExt;
#[cfg(feature = "media")]
use lofty::file::TaggedFileExt;

#[cfg(feature = "media")]
use lofty::tag::{Accessor, ItemKey};

#[cfg(feature = "media")]
use crate::models::{LyricLine, Lyrics, MediaContent, Timestamp};

/// 将秒数格式化为 `MM:SS`.
#[cfg(feature = "media")]
pub(crate) fn format_time(seconds: f64) -> String {
    let total = seconds.max(0.0) as u64;
    format!("{:02}:{:02}", total / 60, total % 60)
}

/// 将本地媒体路径规范化并验证其存在.
#[cfg(feature = "daemon")]
pub(crate) fn canonical_media_path(path: &Path) -> Result<PathBuf, String> {
    path.canonicalize()
        .map_err(|error| format!("canonical_media_path 无法读取媒体文件 {}: {error}", path.display()))
}

/// 计算一次播放列表变更中每个原索引对应的新索引.
///
/// 返回数组的下标是原索引,值为新索引;被删除的项目对应 None.
#[cfg(any(feature = "gui", test))]
pub(crate) fn playlist_index_map(
    change: &PlaylistChangeKind,
    playlist_len: usize,
) -> Result<Vec<Option<usize>>, String> {
    let mut index_map: Vec<Option<usize>> = (0..playlist_len).map(Some).collect();
    match change {
        PlaylistChangeKind::Add(items) => {
            let mut additions = vec![0usize; playlist_len + 1];
            for (index, _) in items {
                if *index > playlist_len {
                    return Err(format!(
                        "播放列表插入位置越界: {index}, 当前长度为 {playlist_len}"
                    ));
                }
                additions[*index] += 1;
            }
            let mut inserted_before = 0;
            for index in 0..playlist_len {
                inserted_before += additions[index];
                index_map[index] = Some(index + inserted_before);
            }
        }
        PlaylistChangeKind::Delete(indices) => {
            let mut removed = vec![false; playlist_len];
            for index in indices {
                if *index >= playlist_len {
                    return Err(format!(
                        "播放列表删除位置越界: {index}, 当前长度为 {playlist_len}"
                    ));
                }
                if std::mem::replace(&mut removed[*index], true) {
                    return Err(format!("播放列表删除位置重复: {index}"));
                }
            }
            let mut next_index = 0;
            for index in 0..playlist_len {
                if removed[index] {
                    index_map[index] = None;
                } else {
                    index_map[index] = Some(next_index);
                    next_index += 1;
                }
            }
        }
        PlaylistChangeKind::Move(items) => {
            let mut moved_sources = vec![false; playlist_len];
            let mut destinations = vec![None::<usize>; playlist_len];
            for (source, destination) in items {
                if *source >= playlist_len {
                    return Err(format!(
                        "播放列表移动源位置越界: {source}, 当前长度为 {playlist_len}"
                    ));
                }
                if *destination >= playlist_len {
                    return Err(format!(
                        "播放列表移动目标位置越界: {destination}, 当前长度为 {playlist_len}"
                    ));
                }
                if std::mem::replace(&mut moved_sources[*source], true) {
                    return Err(format!("播放列表移动源位置重复: {source}"));
                }
                if destinations[*destination].replace(*source).is_some() {
                    return Err(format!("播放列表移动目标位置重复: {destination}"));
                }
            }
            for (new_index, source) in
                destinations_for_map(items, playlist_len)?.into_iter().enumerate()
            {
                index_map[source] = Some(new_index);
            }
        }
        PlaylistChangeKind::ResetCurrent { .. } => {}
        // 重建后旧项目不再具有稳定的索引对应关系.
        PlaylistChangeKind::Rebuild(_, _) => index_map.fill(None),
    }
    Ok(index_map)
}

/// 返回批量移动后的新列表位置对应的原索引顺序.
///
/// 参数: 
/// - `items`: 包含源索引和目标索引的元组列表, 表示批量移动操作.
/// - `playlist_len`: 当前播放列表的长度.
/// 
/// 返回值: 按新列表顺序排列的原索引列表.
#[cfg(any(feature = "gui", test))]
pub(crate) fn destinations_for_map(
    items: &[(usize, usize)],
    playlist_len: usize,
) -> Result<Vec<usize>, String> {
    let mut moved_sources = vec![false; playlist_len];
    let mut destinations = vec![None::<usize>; playlist_len];
    for (source, destination) in items {
        if *source >= playlist_len || *destination >= playlist_len {
            return Err("播放列表移动索引越界".to_string());
        }
        moved_sources[*source] = true;
        destinations[*destination] = Some(*source);
    }
    let mut remaining_sources = (0..playlist_len).filter(|index| !moved_sources[*index]);
    Ok(destinations
        .into_iter()
        .map(|source| source.unwrap_or_else(|| remaining_sources.next().unwrap()))
        .collect())
}

/// 应用播放列表变更并返回原索引到新索引的映射.
#[cfg(test)]
pub(crate) fn apply_playlist_change(
    change: &PlaylistChangeKind,
    playlist: &mut Playlist,
    current: &mut Option<usize>,
) -> Result<Vec<Option<usize>>, String> {
    let old_len = playlist.len();
    let index_map = playlist_index_map(change, old_len)?;
    match change {
        PlaylistChangeKind::Add(items) => {
            let mut additions = vec![Vec::<PathBuf>::new(); old_len + 1];
            for (index, path) in items {
                additions[*index].push(path.clone());
            }
            let mut updated = Vec::with_capacity(old_len + items.len());
            for (index, path) in playlist.iter().enumerate() {
                updated.append(&mut additions[index]);
                updated.push(path.clone());
            }
            updated.append(&mut additions[old_len]);
            *playlist = updated;
        }
        PlaylistChangeKind::Delete(indices) => {
            let removed: std::collections::HashSet<usize> = indices.iter().copied().collect();
            *playlist = playlist
                .iter()
                .enumerate()
                .filter(|(index, _)| !removed.contains(index))
                .map(|(_, path)| path.clone())
                .collect();
        }
        PlaylistChangeKind::Move(items) => {
            let order = destinations_for_map(items, old_len)?;
            *playlist = order
                .into_iter()
                .map(|old_index| playlist[old_index].clone())
                .collect();
        }
        PlaylistChangeKind::ResetCurrent(current_index) => {
            *current = current_index.filter(|index| *index < old_len);
        }
        PlaylistChangeKind::Rebuild(entries, current_index) => {
            // 完整快照同时替换列表和当前项.
            *playlist = entries.clone();
            *current = current_index.filter(|index| *index < playlist.len());
        }
    }
    // 显式设置当前项的操作不需要按旧索引再次映射.
    if !matches!(
        change,
        PlaylistChangeKind::ResetCurrent(_) | PlaylistChangeKind::Rebuild(_, _)
    ) {
        *current = current.and_then(|index| index_map.get(index).copied().flatten());
    }
    Ok(index_map)
}

/// 返回持久化播放列表状态路径
#[cfg(feature = "gui")]
pub(crate) fn state_path() -> PathBuf {

    // 默认使用 XDG_STATE_HOME 或 HOME/.local/state/inako.
    env::var_os("XDG_STATE_HOME").map(PathBuf::from)
        .or_else(|| {
            env::var_os("HOME").map(|home| PathBuf::from(home).join(".local").join("state"))
        })
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("inako")
}

/// 返回持久化播放列表状态路径
#[cfg(feature = "daemon")]
pub(crate) fn state_dir() -> PathBuf {

    // 默认使用 XDG_STATE_HOME 或 HOME/.local/state/inako.
    env::var_os("XDG_STATE_HOME").map(PathBuf::from)
        .or_else(|| {
            env::var_os("HOME").map(|home| PathBuf::from(home).join(".local").join("state"))
        })
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("inako")
}

/// 与播放列表状态相同目录下的 gui.json.
#[cfg(feature = "gui")]
pub(crate) fn window_state_path() -> PathBuf {
    state_path()
        .parent()
        .map(|parent| parent.join("gui.json"))
        .unwrap_or_else(|| PathBuf::from("/tmp/inako-gui.json"))
}

/// 查询当前进程窗口的几何信息并保存, 供下次启动恢复.
#[cfg(feature = "gui")]
pub(crate) fn save_window_geometry(geometry: WindowGeometry) {
    let path = window_state_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(data) = serde_json::to_string_pretty(&geometry) {
        let _ = fs::write(path, data);
    }
}

/// 读取保存的窗口几何信息, 文件缺失或内容无效时返回默认值.
#[cfg(feature = "gui")]
pub(crate) fn load_window_geometry() -> WindowGeometry {
    fs::read_to_string(window_state_path())
        .ok()
        .and_then(|data| serde_json::from_str(&data).ok())
        .unwrap_or_else(|| WindowGeometry {
            x: 20,
            y: 600,
            // 默认窗口采用约 3:1 的宽高比, 为上下两层内容保留足够横向空间.
            width: 1200,
            height: 420,
        })
}

/// 检查 socket 是否可以连接.
#[cfg(any(feature = "media", feature = "gui"))]
pub(crate) fn socket_available(path: &Path) -> bool {
    UnixStream::connect(path).is_ok()
}

/// 返回可用的 daemon socket 路径.
///
/// 按显式路径, 环境变量和默认路径的顺序检查, 最后即使默认 socket
/// 尚未创建也返回默认路径, 供 daemon 首次启动时绑定.
#[cfg(any(feature = "media", feature = "gui"))]
pub(crate) fn get_socket_path(socket_path: Option<PathBuf>) -> PathBuf {

    if let Some(ref path) = socket_path {
        if socket_available(path) {
            return path.clone();
        }
    }

    default_socket_path()
}

/// 返回 daemon 的默认 socket 路径.
pub(crate) fn default_socket_path() -> PathBuf {
    env::var_os("XDG_RUNTIME_DIR")
    .map(PathBuf::from)
    .map(|path| path    .join("inako"))
    .unwrap_or_else(|| {
        PathBuf::from("/tmp").join(format!("inako-{}", effective_uid()))
    })
    .join("daemon.sock")
}

/// 读取 Linux 进程状态中的 effective UID, 用于隔离临时 socket 目录.
pub(crate) fn effective_uid() -> String {
    fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find(|line| line.starts_with("Uid:"))
                .and_then(|line| line.split_whitespace().nth(2))
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "unknown".to_string())
}


/// 从媒体文件标签读取 LRC 歌词和内嵌封面.
#[cfg(feature = "media")]
pub(crate) fn read_media(path: &Path) -> Result<MediaContent, String> {
    let Ok(file) = lofty::read_from_path(path) else {
        return Err(format!("无法读取媒体文件: {}", path.display()));
    };
    let Some(tag) = file.primary_tag().or_else(|| file.first_tag()) else {
        return Err(format!("无法读取媒体标签: {}", path.display()));
    };

    // 读取 LRC 歌词和内嵌封面, 如果失败则返回 None
    let lyrics = tag
        .get_string(ItemKey::Lyrics)
        .or_else(|| tag.get_string(ItemKey::UnsyncLyrics))
        .map(parse_lrc);
    let cover = tag.pictures().first().map(|picture| picture.data().to_vec());

    Ok(MediaContent {
        title: tag
            .title()
            .map(|value| value.into_owned())
            .unwrap_or_else(|| {
                path.file_stem()
                    .and_then(|value| value.to_str())
                    .map(str::to_owned)
                    .unwrap_or_else(|| "未知曲目".to_string())
            }),
        artist: tag
            .artist()
            .map(|value| value.into_owned())
            .unwrap_or_default(),
        album: tag
            .album()
            .map(|value| value.into_owned())
            .unwrap_or_default(),
        lyrics,
        cover,
    })
}

/// 解析 LRC 文本并按时间升序返回歌词行.
#[cfg(feature = "media")]
pub(crate) fn parse_lrc(input: &str) -> Vec<LyricLine> {
    let mut lines = Vec::new();
    for source_line in input.lines() {
        let mut rest = source_line.trim();
        let mut timestamps = Vec::new();
        while let Some(value) = rest.strip_prefix('[') {
            let Some(end) = value.find(']') else {
                break;
            };
            let timestamp = &value[..end];
            if let Some(at) = parse_timestamp(timestamp) {
                timestamps.push(at);
            }
            rest = &value[end + 1..];
        }
        let text = rest.trim();
        for at in timestamps {
            lines.push(LyricLine {
                at,
                text: text.to_string(),
            });
        }
    }
    lines.sort_by(|left, right| left.at.total_cmp(&right.at));
    lines
}

/// 将单个 `MM:SS.xx` 时间标签转换为秒数.
#[cfg(feature = "media")]
fn parse_timestamp(value: &str) -> Option<f64> {
    let (minutes, seconds) = value.split_once(':')?;
    let minutes = minutes.parse::<u64>().ok()?;
    let seconds = seconds.parse::<f64>().ok()?;
    (seconds < 60.0).then_some(minutes as f64 * 60.0 + seconds)
}

/// 返回播放位置对应的最后一行已开始歌词.
///
/// Lyrics 由 parse_lrc 按时间升序排列. partition_point 返回第一条尚未开始
/// 歌词的索引, 前一个元素就是当前位置应显示的歌词.
#[cfg(feature = "media")]
pub(crate) fn get_lyric_current_line<'a>(
    lyrics: &'a Lyrics,
    position: &Timestamp,
) -> Option<&'a str> {
    let next = lyrics.partition_point(|line| line.at <= *position);
    next.checked_sub(1)
        .and_then(|index| lyrics.get(index))
        .map(|line| line.text.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 验证负数被限制为 0 且分钟不在 60 处回绕.
    #[test]
    fn formats_time() {
        assert_eq!(format_time(-1.0), "00:00");
        assert_eq!(format_time(65.9), "01:05");
        assert_eq!(format_time(3600.0), "60:00");
    }

    /// 验证歌词在起始点和相邻时间戳之间正确切换.
    #[test]
    fn selects_current_lyric_line() {
        let lyrics = vec![
            LyricLine {
                at: 5.0,
                text: "first".to_string(),
            },
            LyricLine {
                at: 10.0,
                text: "second".to_string(),
            },
        ];

        assert_eq!(get_lyric_current_line(&lyrics, &4.9), None);
        assert_eq!(get_lyric_current_line(&lyrics, &5.0), Some("first"));
        assert_eq!(get_lyric_current_line(&lyrics, &9.9), Some("first"));
        assert_eq!(get_lyric_current_line(&lyrics, &10.0), Some("second"));
        assert_eq!(get_lyric_current_line(&lyrics, &20.0), Some("second"));
    }

    /// 验证多个插入位置都以操作前列表为基准.
    #[test]
    fn applies_batch_add_with_original_indices() {
        let mut playlist: Playlist = ["a", "b", "c"]
            .into_iter()
            .map(PathBuf::from)
            .collect();
        let mut current = Some(1);
        let index_map = apply_playlist_change(
            &PlaylistChangeKind::Add(vec![
                (0, PathBuf::from("x")),
                (2, PathBuf::from("y")),
                (3, PathBuf::from("z")),
            ]),
            &mut playlist,
            &mut current,
        )
        .unwrap();

        assert_eq!(
            playlist,
            ["x", "a", "b", "y", "c", "z"]
                .into_iter()
                .map(PathBuf::from)
                .collect::<Playlist>()
        );
        assert_eq!(current, Some(2));
        assert_eq!(index_map[2], Some(4));
    }

    /// 验证批量删除不会因前一项删除而偏移后续源索引.
    #[test]
    fn applies_batch_remove_with_original_indices() {
        let mut playlist: Playlist = ["a", "b", "c", "d", "e"]
            .into_iter()
            .map(PathBuf::from)
            .collect();
        let mut current = Some(3);
        let index_map = apply_playlist_change(
            &PlaylistChangeKind::Delete(vec![1, 3]),
            &mut playlist,
            &mut current,
        )
        .unwrap();

        assert_eq!(
            playlist,
            ["a", "c", "e"]
                .into_iter()
                .map(PathBuf::from)
                .collect::<Playlist>()
        );
        assert_eq!(current, None);
        assert_eq!(index_map[4], Some(2));
    }

    /// 验证批量移动固定目标位置并让未移动项按原顺序填空.
    #[test]
    fn applies_batch_move_simultaneously() {
        let mut playlist: Playlist = ["a", "b", "c", "d", "e", "f", "g", "h"]
            .into_iter()
            .map(PathBuf::from)
            .collect();
        let mut current = Some(5);
        let index_map = apply_playlist_change(
            &PlaylistChangeKind::Move(vec![(1, 5), (4, 0), (6, 3)]),
            &mut playlist,
            &mut current,
        )
        .unwrap();

        assert_eq!(
            playlist,
            ["e", "a", "c", "g", "d", "b", "f", "h"]
                .into_iter()
                .map(PathBuf::from)
                .collect::<Playlist>()
        );
        assert_eq!(current, Some(6));
        assert_eq!(index_map[1], Some(5));
    }

    /// 验证移动其他项目后仍能映射未移动的选中项.
    #[test]
    fn maps_unmoved_selection_after_move() {
        // 将旧索引 6 移到 2 时, 原索引 2 和 4 分别后移到 3 和 5.
        let index_map = playlist_index_map(
            &PlaylistChangeKind::Move(vec![(6, 2)]),
            7,
        ).unwrap();

        let selected = [2, 4]
            .into_iter()
            .filter_map(|index| index_map[index])
            .collect::<Vec<_>>();
        assert_eq!(selected, vec![3, 5]);
    }

    /// 验证会造成歧义的重复源位置或目标位置被拒绝.
    #[test]
    fn rejects_ambiguous_batch_indices() {
        let mut playlist: Playlist = ["a", "b", "c"]
            .into_iter()
            .map(PathBuf::from)
            .collect();
        let mut current = None;

        assert!(
            apply_playlist_change(
                &PlaylistChangeKind::Delete(vec![1, 1]),
                &mut playlist,
                &mut current,
            )
            .is_err()
        );
        assert!(
            apply_playlist_change(
                &PlaylistChangeKind::Move(vec![(0, 2), (0, 1)]),
                &mut playlist,
                &mut current,
            )
            .is_err()
        );
        assert!(
            apply_playlist_change(
                &PlaylistChangeKind::Move(vec![(0, 2), (1, 2)]),
                &mut playlist,
                &mut current,
            )
            .is_err()
        );
    }
}


/// 判断指定播放列表行是否至少有一部分位于当前可视区域内.
#[cfg(feature = "gui")]
pub(crate) fn playlist_row_is_partly_visible(playlist_box: &gtk::ListBox, index: usize) -> bool {
    let Some(row) = playlist_box.row_at_index(index as i32) else {
        return false;
    };
    let Some(adjustment) = playlist_box
        .ancestor(gtk::ScrolledWindow::static_type())
        .and_downcast::<gtk::ScrolledWindow>()
        .map(|scroll| scroll.vadjustment())
    else {
        return false;
    };
    let visible_top = adjustment.value();
    let visible_bottom = visible_top + adjustment.page_size();
    let row_top = row.allocation().y() as f64;
    let row_bottom = row_top + row.allocation().height() as f64;
    row_bottom > visible_top && row_top < visible_bottom
}