# Inako

基于 Rust + GTK4 + libmpv 实现 Hyprland 环境音乐播放器桌面控制.

Inako 守护进程内嵌 libmpv 播放核心, CLI 和 GUI 通过 IPC 与守护进程通信.

开发架构和扩展说明见 [docs/development.md](docs/development.md).

## Inako 守护进程

通过 `inako-dae` 命令启动 Inako 守护进程, 哪怕窗口关闭, 播放仍会继续.

守护进程维护播放器和播放列表的权威状态.

## CLI 操作

通过 `inako-cli` 命令操作守护进程.

## GUI 窗口操作

通过 `inako-gui` 命令启动窗口. 窗口分成以下部分:

### 封面&歌词

显示从媒体文件标签中提取的封面图片&歌词.

### 播放列表

- 显示歌曲信息
- 当前播放歌曲高亮
- 双击播放指定歌曲
- 鼠标拖拽调整播放顺序
- 几个功能按钮

### 控制面板

- 进度条
- 音量条
- 歌曲播放结束后自动切下一首
- 几个功能按钮

## 数据保存

数据保存文件夹结构:

```
$XDG_STATE_HOME/inako/
├── daemon_snapshot.json  # 守护进程快照
├── gui.json              # GUI 配置
└── playlists/
    ├── playlist_A.json   # 播放列表
    ├── playlist_B.json   # 播放列表
    └── ...
```

`daemon_snapshot.json`: 守护进程快照, 包含当前使用的播放列表名称, 播放进度, 音量等信息.
用于下次启动时守护恢复上次的播放状态.

`gui.json`: GUI 配置, 包含 GUI 窗口启动大小, 位置等信息.
窗口关闭时会默认保存当前位置信息. 用于下次启动时恢复.

`playlists/` 文件夹下保存用户自定义的播放列表 JSON 文件.
JSON 文件名称就是播放列表名称.
以媒体文件地址数组的形式保存.
由 Inako 守护进程会读写该文件夹下的文件.

## 构建&运行&配置

系统需要 Rust, GTK 4, Blueprint Compiler, gtk4-layer-shell 和 libmpv.
Arch Linux 的 `mpv` 包同时提供构建所需的 libmpv:

```sh
sudo pacman -S rust gtk4 blueprint-compiler gtk4-layer-shell mpv
cargo build --release
```

最终产物是三个可执行文件:

- `inako-dae`: Inako 守护进程
- `inako-cli`: 命令行工具
- `inako-gui`: Inako GUI 窗口

### 命令一览

#### `inako-dae`

| 命令                  | 操作                |
| --------------------- | ------------------- |
| `inako-dae [options]` | 启动 Inako 守护进程 |

#### `inako-gui`

| 命令                                 | 操作                            |
| ------------------------------------ | ------------------------------- |
| `inako-gui toggle-display [options]` | 启动信息窗口, 或切换其显示/隐藏 |
| `inako-gui quit [options]`           | 关闭窗口并终止信息窗口进程      |

#### `inako-cli`

守护进程操作

| 命令                         | 操作                                |
| ---------------------------- | ----------------------------------- |
| `inako-cli status [options]` | 输出一次状态 JSON                   |
| `inako-cli watch [options]`  | 接收守护进程推送并持续输出状态 JSON |
| `inako-cli stop [options]`   | 终止守护进程                        |

播放列表操作

| 命令                                                  | 操作                               |
| ----------------------------------------------------- | ---------------------------------- |
| `inako-cli add <file_path> [options]`                 | 通过文件地址添加歌曲到当前播放列表 |
| `inako-cli clear [options]`                           | 清空播放列表                       |
| `inako-cli move <from_index> <to_index> [options]`    | 调整歌曲顺序                       |
| `inako-cli remove <index> [options]`                  | 删除歌曲                           |
| `inako-cli shuffle [options]`                         | 随机打乱播放顺序                   |
| `inako-cli load-playlist <file_path> [options]`       | 用文本文件替换并重载播放列表       |
| `inako-cli switch-playlist <playlist_name> [options]` | 加载并切换到指定播放列表           |
| `inako-cli save-playlist <playlist_name> [options]`   | 保存当前播放列表到指定名称         |

播放控制操作

| 命令                                 | 操作               |
| ------------------------------------ | ------------------ |
| `inako-cli play <index> [options]`   | 播放指定歌曲       |
| `inako-cli toggle [options]`         | 播放或暂停         |
| `inako-cli next [options]`           | 下一首             |
| `inako-cli previous [options]`       | 上一首             |
| `inako-cli seek <seconds> [options]` | 跳转到绝对播放位置 |

- `index`, `from_index` 和 `to_index` 为歌曲在播放列表中的索引 (从 0 开始).
- `load-playlist-by-txt-file` 接受每行一个媒体文件路径的 UTF-8 文本文件.

#### 通用选项

| 选项                   | 操作                            |
| ---------------------- | ------------------------------- |
| `--socket socket_path` | 指定守护进程的 Unix Socket 路径 |
| `--help`               | 显示帮助信息                    |
| `--version`            | 显示版本信息                    |

### Waybar 播放状态栏常态显示配置

支持配置 Waybar 实现实时显示当前播放歌曲信息, 歌词, 播放进度等.

在 Waybar 配置的 `modules-left`, `modules-center` 或 `modules-right`
中加入 `custom/music`, 并添加:

```json
{
  "custom/music": {
    "exec": "inako-cli watch",
    "return-type": "json",
    "restart-interval": 1,
    "on-click": "inako-gui toggle-display",
    "on-click-right": "inako-cli toggle",
    "on-scroll-up": "inako-cli next",
    "on-scroll-down": "inako-cli previous",
    "escape": true
  }
}
```
