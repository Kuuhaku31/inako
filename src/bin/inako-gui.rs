
// src/bin/inako-gui.rs
// GTK 窗口可执行文件入口和参数解析器

use std::env;
use std::ffi::OsString;
use std::path::PathBuf;

use inako::gui::{self, WindowCommand};


fn main() {
    let initial_arguments = env::args_os().collect::<Vec<_>>();
    if matches!(
        initial_arguments.get(1).and_then(|value| value.to_str()),
        Some("help" | "--help" | "-h")
    ) {
        print_help();
        return;
    }
    if matches!(
        initial_arguments.get(1).and_then(|value| value.to_str()),
        Some("version" | "--version" | "-V")
    ) {
        print_version();
        return;
    }
    match gui::run_window(parse_command) {
        Ok(_) => (),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}


/// 解析 GUI 命令行参数, 适用于初始进程和转发的调用
fn parse_command(arguments: &[OsString]) -> Result<WindowCommand, String> {
    let mut values = arguments.iter().skip(1);
    let mut socket_path = None;
    let mut command = None;
    while let Some(value) = values.next() {
        match value.to_str() {
            Some("--socket") | Some("--socket-path") => {
                socket_path = Some(PathBuf::from(values.next().ok_or("--socket 需要路径参数")?));
            }
            Some("toggle-display") => command = Some("toggle-display"),
            Some("quit") => command = Some("quit"),
            Some(value) => return Err(format!("未知命令: {value}")),
            None => return Err("命令参数必须是有效 UTF-8 文本".to_string()),
        }
    }
    match command.unwrap_or("toggle-display") {
        "toggle-display" => Ok(WindowCommand::ToggleDisplay { socket_path }),
        "quit" => Ok(WindowCommand::Quit),
        _ => unreachable!(),
    }
}

/// Print the GUI-specific usage text.
fn print_help() {
    println!(
        "inako-gui {}\n\n用法:\n  inako-gui toggle-display [选项]\n  inako-gui quit [选项]\n\n命令:\n  toggle-display                      启动信息窗口, 或切换其显示或隐藏\n  quit                                关闭窗口并终止信息窗口进程\n\n通用选项:\n  --socket <路径>                     指定守护进程 Unix Socket 路径\n  --help                              显示帮助信息\n  --version                           显示版本信息",
        env!("CARGO_PKG_VERSION")
    );
}

/// Print the GUI version.
fn print_version() {
    println!("inako-gui {}", env!("CARGO_PKG_VERSION"));
}
