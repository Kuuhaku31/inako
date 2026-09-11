
// src/daemon/playlist_manager.rs
// 播放列表管理器, 负责加载, 持久化和增量变更操作.

#[cfg(test)]
use crate::utils;
#[cfg(any(feature = "daemon", feature = "gui", test))]
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub type Playlist = Vec<PathBuf>; // 播放列表, 只考虑地址序列


#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct PlaylistChangeMessage {
    pub(crate) state_number: usize,
    pub(crate) change: PlaylistChangeKind,
}

/// 一次增量播放列表操作.
///
/// 操作所依据的版本号由 IPC 消息的 state_number 字段统一携带, 避免每个
/// 操作变体重复保存相同的版本信息.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) enum PlaylistChangeKind {

    /// 插入位置基于插入操作执行前的原列表索引确定,
    /// 索引 i 表示在原列表第 i 个元素之前插入,
    /// i == 列表长度 表示追加到列表末尾.
    ///
    /// 多个插入操作的目标位置均以原列表索引为准,
    /// 不因其他插入操作导致索引偏移.
    /// 在指定索引插入新路径
    ///
    ///         0  1  2  3  4  5  6  7
    /// 原列表: A, B, C, D, E, F, G
    ///
    /// 插入 (0, x), (3, y), (7, z) 后:
    ///
    ///         0  1  2  3  4  5  6  7  8  9
    /// 新列表: x, A, B, C, y, D, E, F, G, z
    Add(Vec<(usize, PathBuf)>),

    /// 删除多个元素时, 以原列表索引确定待删除元素,
    /// 并一次性删除, 避免索引偏移.
    ///
    /// 删除指定索引的路径
    ///
    ///         0  1  2  3  4  5  6  7
    /// 原列表: A, B, C, D, E, F, G
    ///
    /// 删除 1, 3, 5 后:
    ///
    ///         0  1  2  3
    /// 新列表: A, C, E, G
    Delete(Vec<usize>),

    /// 一次性移动多个元素, (m, n) 表示将原列表中索引 m 的元素移动到新列表索引 n.
    /// 所有索引均基于操作开始前的原列表计算, 移动操作同时生效.
    /// 未移动元素保持原有相对顺序填充剩余位置.
    ///
    ///         0  1  2  3  4  5  6  7
    /// 原列表: A, B, C, D, E, F, G, H
    ///
    /// 移动: (1, 5), (4, 0), (6, 3)
    ///
    ///         0  1  2  3  4  5  6  7
    /// 新列表: E, A, C, G, D, B, F, H
    Move(Vec<(usize, usize)>),

    /// 重置当前播放索引, 允许设置为 None 表示未选择歌曲.
    ResetCurrent(Option<usize>),

    /// 完全重构
    Rebuild(Playlist, Option<usize>),
}

/// daemon 唯一持有的播放列表状态和状态文件 serde 表示.
#[cfg(any(feature = "daemon", feature = "gui", test))]
#[derive(Default, Deserialize, Serialize)]
pub(crate) struct PlaylistManager {

    state_number: usize, // 状态号, 仅在当前 daemon 进程内递增, 新进程始终从 0 开始

    entries: Playlist, // 持久化的播放列表路径.
    current: Option<usize>, // 当前播放项索引

}

#[cfg(any(feature = "daemon", feature = "gui", test))]
impl PlaylistManager {

    /// 实例化一个空的播放列表管理器
    pub(crate) fn new() -> Self {
        Self {
            state_number: 0,
            entries: Vec::new(),
            current: None,
        }
    }

    /// 重载
    #[cfg(any(feature = "daemon", test))]
    pub(crate) fn reload(&mut self, new_playlist: Playlist, current: Option<usize>)
        -> Result<PlaylistChangeMessage, String>
    {
        // 更新内部状态
        self.entries = new_playlist;
        self.current = current;
        self.state_number_increment();

        // Rebuild 快照携带替换完成后的当前版本.
        let change_message = PlaylistChangeMessage {
            state_number: self.state_number,
            change: PlaylistChangeKind::Rebuild(self.entries.clone(), self.current),
        };

        // 返回变化
        Ok(change_message)
    }

    /// 获取重载消息
    #[cfg(any(feature = "daemon", test))]
    pub(crate) fn get_reload_message(&self) -> PlaylistChangeMessage {
        PlaylistChangeMessage {
            state_number: self.state_number,
            change: PlaylistChangeKind::Rebuild(self.entries.clone(), self.current),
        }
    }

    pub(crate) fn get_playlist_ref(&self) -> &Playlist { &self.entries }
    pub(crate) fn get_current(&self) -> Option<usize> { self.current }
    pub(crate) fn get_state_number(&self) -> usize { self.state_number }

    /// 根据索引返回地址
    #[cfg(any(feature = "daemon", test))]
    pub(crate) fn get_path_by_index(&self, index: usize) -> Result<PathBuf, String> {
        self.entries.get(index).cloned().ok_or("索引越界".to_string())
    }

    /// 返回当前选中播放项的路径.
    #[cfg(any(feature = "daemon", test))]
    pub(crate) fn get_current_path(&self) -> Option<PathBuf> {
        // 当前项必须存在且索引仍在播放列表范围内.
        let current = self.current?;
        self.entries.get(current).cloned()
    }

    fn state_number_increment(&mut self) {
        self.state_number = self.state_number.wrapping_add(1);
    }

    /// 尝试应用增量播放列表修改
    pub(crate) fn apply_playlist_change(&mut self, change_message: PlaylistChangeMessage)
        -> Result<PlaylistChangeMessage, String>
    {
        // 校验 state_number 是否匹配
        let base_state_number = change_message.state_number;
        let is_rebuild = matches!(&change_message.change, PlaylistChangeKind::Rebuild(_, _));
        if self.state_number != base_state_number && !is_rebuild {
            return Err(format!("状态编号不匹配, 自身的: {}, 传入的: {}", self.state_number, base_state_number));
        }

        // 每个处理函数先校验全部索引, 再一次性提交列表变更.
        let change = match change_message.change {
            PlaylistChangeKind::Add(items) => self.handle_add(items),
            PlaylistChangeKind::Delete(items) => self.handle_delete(items),
            PlaylistChangeKind::Move(items) => self.handle_move(items),
            PlaylistChangeKind::ResetCurrent(current_index) => self.handle_reset_current(current_index),

            PlaylistChangeKind::Rebuild(path_bufs, current_index) => {
                // 完整快照不依赖本地版本, 并直接采用 daemon 的当前版本.
                self.handle_rebuild(path_bufs.clone(), current_index)?;
                self.state_number = base_state_number;
                return Ok(PlaylistChangeMessage {
                    state_number: base_state_number,
                    change: PlaylistChangeKind::Rebuild(path_bufs, current_index),
                });
            },
        }?;

        // 变更完成, 返回变化
        let res = Ok(PlaylistChangeMessage {
            state_number: self.state_number,
            change,
        });

        // 增加状态编号
        self.state_number_increment();
        res
    }

    /// 上一首
    #[cfg(any(feature = "daemon", test))]
    pub(crate) fn select_previous(&mut self) -> Result<PlaylistChangeMessage, String> {

        // 合法性检查
        let current = self.current.ok_or("当前没有选中的播放项".to_string())?;
        if current == 0 { return Err("已经是第一首".to_string()); }

        // 构造返回结果
        let new_current = Some(current - 1);
        let base_state_number = self.state_number;

        // 应用更改
        self.state_number_increment();
        self.current = new_current;

        // 返回
        Ok(PlaylistChangeMessage {
            state_number: base_state_number,
            change: PlaylistChangeKind::ResetCurrent(new_current),
        })
    }

    /// 下一首
    #[cfg(any(feature = "daemon", test))]
    pub(crate) fn select_next(&mut self) -> Result<PlaylistChangeMessage, String> {

        // 合法性检查
        let current = self.current.ok_or("当前没有选中的播放项".to_string())?;
        if current + 1 >= self.entries.len() {
            return Err("已经是最后一首".to_string());
        }

        // 构造返回结果
        let new_current = Some(current + 1);
        let base_state_number = self.state_number;

        // 应用更改
        self.state_number_increment();
        self.current = new_current;

        // 返回
        Ok(PlaylistChangeMessage {
            state_number: base_state_number,
            change: PlaylistChangeKind::ResetCurrent(new_current),
        })
    }

    /// 选择新的
    #[cfg(any(feature = "daemon", test))]
    pub(crate) fn select(&mut self, new_current: usize)
        -> Result<PlaylistChangeMessage, String> {

        // 合法性检查
        if new_current >= self.entries.len() {
            return Err("选择的播放项索引越界".to_string());
        }

        // 构造返回结果
        let new_current = Some(new_current);
        let base_state_number = self.state_number;

        // 应用更改
        self.state_number_increment();
        self.current = new_current;

        // 返回
        Ok(PlaylistChangeMessage {
            state_number: base_state_number,
            change: PlaylistChangeKind::ResetCurrent(new_current),
        })
    }


    // ====================================================================== //

    /// 按原列表索引批量插入路径, 并保持当前播放实体不变.
    fn handle_add(&mut self,
        items: Vec<(usize, PathBuf)>,) -> Result<PlaylistChangeKind, String> {
        let old_len = self.entries.len();
        let mut additions = vec![Vec::<PathBuf>::new(); old_len + 1];
        for (index, path) in &items {
            if *index > old_len {
                return Err(format!(
                    "播放列表插入位置越界: {index}, 当前长度为 {old_len}"
                ));
            }
            additions[*index].push(path.clone());
        }

        // 插入位置全部有效后构造新列表, 避免失败时留下部分修改.
        let mut updated = Vec::with_capacity(old_len + items.len());
        for (index, path) in self.entries.iter().enumerate() {
            updated.append(&mut additions[index]);
            updated.push(path.clone());
        }
        updated.append(&mut additions[old_len]);
        if let Some(index) = self.current.as_mut() {
            *index += items.iter().filter(|(position, _)| *position <= *index).count();
        }
        self.entries = updated;
        Ok(PlaylistChangeKind::Add(items))
    }

    /// 按原列表索引批量删除路径, 被删除的当前播放项重置为未选择.
    fn handle_delete(&mut self,
        items: Vec<usize>,) -> Result<PlaylistChangeKind, String> {
        let old_len = self.entries.len();
        let mut removed = vec![false; old_len];
        for index in &items {
            if *index >= old_len {
                return Err(format!(
                    "播放列表删除位置越界: {index}, 当前长度为 {old_len}"
                ));
            }
            if std::mem::replace(&mut removed[*index], true) {
                return Err(format!("播放列表删除位置重复: {index}"));
            }
        }

        // 校验完成后同时计算当前项的新索引和新列表.
        self.current = self.current.and_then(|index| {
            (!removed[index]).then(|| index - removed[..index].iter().filter(|value| **value).count())
        });
        self.entries = self.entries
            .iter()
            .enumerate()
            .filter(|(index, _)| !removed[*index])
            .map(|(_, path)| path.clone())
            .collect();
        Ok(PlaylistChangeKind::Delete(items))
    }

    /// 同时应用全部移动, 未移动项按原相对顺序填充剩余位置.
    fn handle_move(&mut self,
        items: Vec<(usize, usize)>,) -> Result<PlaylistChangeKind, String> {
        let old_len = self.entries.len();
        let mut moved_sources = vec![false; old_len];
        let mut destinations = vec![None::<usize>; old_len];
        for (source, destination) in &items {
            if *source >= old_len {
                return Err(format!(
                    "播放列表移动源位置越界: {source}, 当前长度为 {old_len}"
                ));
            }
            if *destination >= old_len {
                return Err(format!(
                    "播放列表移动目标位置越界: {destination}, 当前长度为 {old_len}"
                ));
            }
            if std::mem::replace(&mut moved_sources[*source], true) {
                return Err(format!("播放列表移动源位置重复: {source}"));
            }
            if destinations[*destination].replace(*source).is_some() {
                return Err(format!("播放列表移动目标位置重复: {destination}"));
            }
        }

        // 目标位置有效且无歧义后生成新位置到旧位置的完整映射.
        let mut remaining_sources = (0..old_len).filter(|index| !moved_sources[*index]);
        let order = destinations
            .into_iter()
            .map(|source| source.unwrap_or_else(|| remaining_sources.next().unwrap()))
            .collect::<Vec<_>>();
        self.current = self.current.and_then(|index| order.iter().position(|source| *source == index));
        self.entries = order
            .into_iter()
            .map(|old_index| self.entries[old_index].clone())
            .collect();
        Ok(PlaylistChangeKind::Move(items))
    }

    /// 设置当前播放索引, 越界索引按未选择处理.
    fn handle_reset_current(&mut self,
        current_index: Option<usize>,) -> Result<PlaylistChangeKind, String> {
        // 如果越界, 返回错误, 不处理
        if let Some(index) = current_index {
            if index >= self.entries.len() {
                return Err(format!(
                    "播放列表当前播放索引越界: {index}, 当前长度为 {}",
                    self.entries.len()
                ));
            }
        }
        self.current = current_index;
        Ok(PlaylistChangeKind::ResetCurrent(self.current))
    }

    /// 用完整快照替换播放列表, 并校正快照中的当前播放索引.
    fn handle_rebuild(&mut self,
        path_bufs: Vec<PathBuf>,
        current_index: Option<usize>,) -> Result<PlaylistChangeKind, String> {
        self.current = current_index.filter(|index| *index < path_bufs.len());
        self.entries = path_bufs.clone();
        Ok(PlaylistChangeKind::Rebuild(path_bufs, current_index))
    }
}


#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    /// 构造包含五项的测试播放列表和当前索引.
    fn fixture(current: usize) -> (Playlist, Option<usize>) {
        (
            ["a", "b", "c", "d", "e"]
                .into_iter()
                .map(PathBuf::from)
                .collect(),
            Some(current),
        )
    }

    /// 验证批量移动遵循固定目标位置并保持当前播放实体.
    #[test]
    fn moving_items_uses_original_indices() {
        let (mut playlist, mut current) = fixture(2);
        let index_map = utils::apply_playlist_change(
            &PlaylistChangeKind::Move(vec![(1, 4), (4, 0)]),
            &mut playlist,
            &mut current,
        )
        .unwrap();

        assert_eq!(
            playlist,
            ["e", "a", "c", "d", "b"]
                .into_iter()
                .map(PathBuf::from)
                .collect::<Playlist>()
        );
        assert_eq!(current, Some(2));
        assert_eq!(index_map[1], Some(4));
        assert_eq!(playlist[current.unwrap()], PathBuf::from("c"));
    }

    /// 验证完整快照可以覆盖任意旧版本并采用 daemon 版本.
    #[test]
    fn rebuild_adopts_daemon_state_number() {
        let mut manager = PlaylistManager::new();
        let entries = vec![PathBuf::from("new")];

        // 快照用于恢复版本分歧, 因此不要求本地版本预先匹配.
        manager.apply_playlist_change(PlaylistChangeMessage {
            state_number: 7,
            change: PlaylistChangeKind::Rebuild(entries.clone(), Some(0)),
        }).unwrap();

        assert_eq!(manager.get_state_number(), 7);
        assert_eq!(manager.get_playlist_ref(), &entries);
        assert_eq!(manager.get_current(), Some(0));
    }

    /// 验证版本不匹配的增量修改被拒绝且不改变本地状态.
    #[test]
    fn incremental_change_requires_matching_state_number() {
        let mut manager = PlaylistManager::new();

        // 增量操作只能应用到它声明的基础版本.
        let error = manager.apply_playlist_change(PlaylistChangeMessage {
            state_number: 1,
            change: PlaylistChangeKind::Add(vec![(0, PathBuf::from("new"))]),
        }).unwrap_err();

        assert!(error.contains("状态编号不匹配"));
        assert_eq!(manager.get_state_number(), 0);
        assert!(manager.get_playlist_ref().is_empty());
    }

    /// 验证版本匹配的增量修改成功应用并递增版本.
    #[test]
    fn incremental_change_advances_state_number() {
        let mut manager = PlaylistManager::new();

        // 广播携带操作前版本, 应用后本地版本递增一次.
        manager.apply_playlist_change(PlaylistChangeMessage {
            state_number: 0,
            change: PlaylistChangeKind::Add(vec![(0, PathBuf::from("new"))]),
        }).unwrap();

        assert_eq!(manager.get_state_number(), 1);
        assert_eq!(manager.get_playlist_ref(), &vec![PathBuf::from("new")]);
    }

    /// 验证 reload 广播的快照版本与替换后的 daemon 版本一致.
    #[test]
    fn reload_message_contains_current_state_number() {
        let mut manager = PlaylistManager::new();

        // reload 是一次结构变更, 返回的快照应直接表示变更后的状态.
        let message = manager.reload(vec![PathBuf::from("new")], Some(0)).unwrap();

        assert_eq!(message.state_number, manager.get_state_number());
        assert_eq!(message.state_number, 1);
    }

    /// 验证当前播放项路径可供 daemon 自动切歌时读取.
    #[test]
    fn gets_current_playlist_path() {
        let mut manager = PlaylistManager::new();

        // reload 同时设置播放列表和当前索引.
        manager.reload(
            vec![PathBuf::from("first"), PathBuf::from("second")],
            Some(1),
        ).unwrap();

        assert_eq!(manager.get_current_path().unwrap(), PathBuf::from("second"));
    }

    /// 验证上一首和下一首同时更新当前项及广播版本.
    #[test]
    fn navigates_between_playlist_items() {
        let mut manager = PlaylistManager::new();
        manager.reload(
            vec![PathBuf::from("first"), PathBuf::from("second")],
            Some(0),
        ).unwrap();

        // 下一首基于当前版本生成变更, 并将本地版本推进一次.
        let next = manager.select_next().unwrap();
        assert_eq!(next.state_number, 1);
        assert_eq!(manager.get_current(), Some(1));
        assert_eq!(manager.get_state_number(), 2);

        // 上一首使用推进后的版本并返回原播放项.
        let previous = manager.select_previous().unwrap();
        assert_eq!(previous.state_number, 2);
        assert_eq!(manager.get_current(), Some(0));
        assert_eq!(manager.get_state_number(), 3);
    }
}
