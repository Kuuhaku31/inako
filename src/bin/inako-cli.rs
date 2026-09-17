
// src/bin/inako-cli.rs
// CLI 可执行文件入口

use std::env;
use std::path::PathBuf;
use std::time::Duration;

use inako::cli::{self, CliCommand, CliArguments};


fn main() {

    if let Err(error) = parse_command()
    .and_then(|a| cli::execute(a)) {
        eprintln!("{error}");
        std::process::exit(1);
    }
}


/// 解析 CLI 命令行参数并返回解析后的命令和可选的 Unix Socket 路径
fn parse_command() -> Result<CliArguments, String> {

    let mut arguments = env::args().skip(1);
    let command = arguments.next().unwrap_or_else(|| "status".to_string());
    let mut socket = None;
    let mut values = Vec::new();
    while let Some(argument) = arguments.next() {
        if argument == "--socket" || argument == "--socket-path" {
            socket = Some(PathBuf::from(arguments.next().ok_or("--socket 需要路径参数")?));
        } else {
            values.push(argument);
        }
    }
    let mut values = values.into_iter();
    let command = match command.as_str() {
        "status" => CliCommand::Status,
        "watch" => CliCommand::Watch,
        "stop" => CliCommand::Stop,
        "toggle" => CliCommand::Toggle,
        "seek" => CliCommand::Seek(parse_seconds(values.next())?),
        "next" => CliCommand::Next,
        "previous" | "prev" => CliCommand::Previous,
        "load-playlist" | "load-playlist-by-txt-file" => CliCommand::LoadPlaylist(PathBuf::from(required(&mut values, "播放列表文件路径")?)),
        "switch-playlist" => CliCommand::SwitchPlaylist(required(&mut values, "播放列表名称")?),
        "save-playlist" => CliCommand::SavePlaylist(required(&mut values, "播放列表名称")?),
        "add" => CliCommand::Add(PathBuf::from(required(&mut values, "媒体文件路径")?)),
        "clear" => CliCommand::Clear,
        "remove" => CliCommand::Remove(parse_index(values.next())?),
        "move" => CliCommand::Move { from: parse_index(values.next())?, to: parse_index(values.next())? },
        "shuffle" => CliCommand::Shuffle,
        "play" => CliCommand::Play {
            index: parse_index(values.next())?,
            position: Duration::from_secs_f64(values.next().map(|value| parse_seconds(Some(value))).transpose()?.unwrap_or(0.0)),
        },
        "help" | "--help" | "-h" => {
            print_help();
            std::process::exit(0);
        }
        "version" | "--version" | "-V" => {
            print_version();
            std::process::exit(0);
        }
        _ => return Err(format!("未知命令: {command}\n运行 inako-cli help 查看用法")),
    };
    if values.next().is_some() {
        return Err("命令参数过多".to_string());
    }
    Ok((command, socket))
}

/// Return the next required positional argument.
fn required(arguments: &mut impl Iterator<Item = String>, name: &str) -> Result<String, String> {
    arguments.next().ok_or_else(|| format!("缺少{name}"))
}

/// Parse a zero-based playlist index.
fn parse_index(value: Option<String>) -> Result<usize, String> {
    value.ok_or_else(|| "缺少播放列表索引".to_string())?.parse()
        .map_err(|_| "播放列表索引必须是从 0 开始的整数".to_string())
}

/// Parse a non-negative finite number of seconds.
fn parse_seconds(value: Option<String>) -> Result<f64, String> {
    let seconds: f64 = value.ok_or_else(|| "缺少秒数".to_string())?.parse()
        .map_err(|_| "秒数必须是数字".to_string())?;
    if !seconds.is_finite() || seconds < 0.0 {
        return Err("秒数必须是非负有限数字".to_string());
    }
    Ok(seconds)
}

/// Print the CLI-specific usage text.
fn print_help() {
    println!(
        "inako-cli {}\n\n用法:\n  inako-cli <命令> [选项]\n\n守护进程操作:\n  status                              输出一次状态 JSON\n  watch                               持续输出状态 JSON\n  stop                                终止守护进程\n\n播放列表操作:\n  add <文件路径>                      添加歌曲到当前播放列表\n  clear                               清空播放列表\n  move <源索引> <目标索引>            调整歌曲顺序\n  remove <索引>                       删除歌曲\n  shuffle                             随机打乱播放顺序\n  load-playlist <文本文件路径>        用文本文件替换并重载播放列表\n  switch-playlist <播放列表名称>      加载并切换指定播放列表\n  save-playlist <播放列表名称>        保存当前播放列表\n\n播放控制操作:\n  play <索引> [秒数]                  播放指定歌曲\n  toggle                              播放或暂停\n  next                                下一首\n  previous                            上一首\n  seek <秒数>                         跳转到绝对播放位置\n\n通用选项:\n  --socket <路径>                     指定守护进程 Unix Socket 路径\n  --help                              显示帮助信息\n  --version                           显示版本信息\n\n索引从 0 开始. 文本播放列表为 UTF-8 格式, 每行一个媒体文件路径.",
        env!("CARGO_PKG_VERSION")
    );
}

/// Print the CLI version.
fn print_version() {
    println!("inako-cli {}", env!("CARGO_PKG_VERSION"));
}
