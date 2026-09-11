# Inako 开发文档

## 技术栈

Inako 使用以下组件:

- Rust 负责状态管理, 播放列表, 播放业务和 IPC
- GTK4 负责原生 Linux 和 Wayland GUI
- Blueprint 负责静态界面布局
- CSS 负责界面样式
- gtk4-layer-shell 负责悬浮窗口定位
- libmpv2 负责音频解码, 播放, 暂停, seek 和播放结束事件
- Lofty 负责读取歌曲元数据, 封面和歌词

libmpv 播放核心内嵌在 Inako 守护进程中.

## 进程模型

```text
| inako-cli                       inako-gui
|   |                                  |
|   +---------------+------------------+
|                   |
|      (Unix socket communication)
|                   |
|                   v
|           inako-dae
|           +-------------------+
|           | Playlist state    |
|           | libmpv2::Mpv      |
|           +-------------------+
```

GUI 和 CLI 通过 Inako Unix socket 请求 Inako 守护进程 (inako-dae).
Inako 守护进程 (inako-dae) 是唯一直接持有播放列表状态和 `libmpv2::Mpv` 的进程.
GUI 可以独立退出或重建, 不会中断 Inako 守护进程 (inako-dae) 中的播放.

## 源码结构

本项目使用 Rust 作为主要编程语言, `inako-dae`, `inako-cli` 和 `inako-gui`
分别对应 `crate::daemon`, `crate::cli` 和 `crate::gui` 三个模块.

主要模型和 IPC 通信协议放在 `crate::models` 中.

`crate::bin` 存放可执行文件入口.

## GUI 相关

### GUI 按钮说明

| 按钮         | 控件名称                          | 功能                               |
| ------------ | --------------------------------- | ---------------------------------- |
| 刷新         | `button_refresh_playlist`         | 从守护进程重新获取完整播放列表     |
| 上一首       | `button_previous`                 | 播放上一首歌曲                     |
| 播放/暂停    | `button_toggle_play`              | 切换播放和暂停状态                 |
| 下一首       | `button_next`                     | 播放下一首歌曲                     |
| 定位         | `button_locate_current`           | 选中当前播放项并滚动到其位置       |
| 上移         | `button_select_to_up`             | 将所有选中项向上移动一位           |
| 下移         | `button_select_to_down`           | 将所有选中项向下移动一位           |
| 移到顶部     | `button_select_to_top`            | 把所有选中项移动到播放列表顶部     |
| 移到底部     | `button_select_to_bottom`         | 把所有选中项移动到播放列表底部     |
| 播放到选中前 | `button_current_to_selected_up`   | 把当前播放项移动到所有选中项的上方 |
| 选中到播放后 | `button_selected_to_current_down` | 把所有选中项移动到当前播放项的下方 |

### 列表交互

- `Ctrl + 左键`:切换单个项目的选中状态
- `Shift + 左键`:选择当前项目与上次选中项目之间的连续范围
- `Ctrl + A`:全选播放列表

拖动任意一个已选项目: 整组已选项目一起移动, 保持原有相对顺序,
当存在多个选中项目时, 对操作的定义统一作用于全部选中项目.
