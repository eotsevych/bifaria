//! Sends Ctrl+V on Linux through the desktop's own input tools.
//!
//! enigo injects keys through X11's XTest, which on Wayland reaches only XWayland windows: in
//! native Wayland apps nothing is pasted and no error is reported. So Bifaria first tries the
//! command-line tools that work there, in the order that works best on each desktop, and only
//! then falls back to enigo.
//!
//! Tool order and the ydotool key syntax detection are adapted from Handy
//! (https://github.com/cjpais/Handy, MIT License, Copyright (c) 2025 CJ Pais).
//!
//! Compiled on every platform so its logic is unit-tested everywhere; only Linux calls it.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::OnceLock;

/// The display server and desktop Bifaria runs in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Session {
    pub wayland: bool,
    pub gnome: bool,
    pub kde: bool,
}

impl Session {
    pub fn detect() -> Self {
        let var = |name: &str| std::env::var(name).ok();
        Self::from_vars(var("WAYLAND_DISPLAY").as_deref(), var("XDG_SESSION_TYPE").as_deref(), var("XDG_CURRENT_DESKTOP").as_deref())
    }

    fn from_vars(wayland_display: Option<&str>, session_type: Option<&str>, desktop: Option<&str>) -> Self {
        let wayland = wayland_display.is_some_and(|v| !v.is_empty())
            || session_type.is_some_and(|v| v.eq_ignore_ascii_case("wayland"));
        let desktop = desktop.unwrap_or("").to_ascii_uppercase();
        Self {
            wayland,
            gnome: desktop.contains("GNOME"),
            kde: desktop.contains("KDE"),
        }
    }
}

/// A command-line tool that can send Ctrl+V.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Wtype,
    Dotool,
    Ydotool,
    Xdotool,
}

impl Tool {
    pub fn command(self) -> &'static str {
        match self {
            Tool::Wtype => "wtype",
            Tool::Dotool => "dotool",
            Tool::Ydotool => "ydotool",
            Tool::Xdotool => "xdotool",
        }
    }
}

/// The tools to try, best first. wtype needs the virtual-keyboard protocol, which GNOME and KDE
/// don't implement; dotool and ydotool type through the kernel's uinput, so they work everywhere
/// once set up.
pub fn paste_tools(session: Session) -> Vec<Tool> {
    if session.wayland {
        let mut tools = Vec::new();
        if !session.gnome && !session.kde {
            tools.push(Tool::Wtype);
        }
        tools.extend([Tool::Dotool, Tool::Ydotool]);
        tools
    } else {
        vec![Tool::Xdotool, Tool::Ydotool]
    }
}

/// Whether an executable called `name` is on PATH.
pub fn on_path(name: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| is_executable(&dir.join(name))))
        .unwrap_or(false)
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

/// Sends Ctrl+V with the first installed tool that works. `Err` lists what was tried, or is
/// `NoTool` when none of the tools for this session is installed.
pub fn send_ctrl_v(session: Session) -> Result<Tool, PasteToolError> {
    let mut failures = Vec::new();
    for tool in paste_tools(session).into_iter().filter(|t| on_path(t.command())) {
        match run(tool) {
            Ok(()) => return Ok(tool),
            Err(e) => {
                log::warn!("Paste with {} failed: {}", tool.command(), e);
                failures.push(format!("{}: {}", tool.command(), e));
            }
        }
    }
    if failures.is_empty() {
        Err(PasteToolError::NoTool)
    } else {
        Err(PasteToolError::Failed(failures.join("; ")))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PasteToolError {
    NoTool,
    Failed(String),
}

fn run(tool: Tool) -> Result<(), String> {
    let output = match tool {
        Tool::Wtype => Command::new("wtype").args(["-M", "ctrl", "-k", "v", "-m", "ctrl"]).output(),
        Tool::Xdotool => Command::new("xdotool").args(["key", "--clearmodifiers", "ctrl+v"]).output(),
        Tool::Ydotool => Command::new("ydotool").args(ydotool_ctrl_v_args(ydotool_syntax())).output(),
        Tool::Dotool => {
            // dotool reads commands from stdin.
            let child = Command::new("dotool")
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn();
            match child {
                Ok(mut child) => {
                    if let Some(mut stdin) = child.stdin.take() {
                        use std::io::Write;
                        stdin.write_all(b"key ctrl+v\n").map_err(|e| e.to_string())?;
                    }
                    child.wait_with_output()
                }
                Err(e) => Err(e),
            }
        }
    }
    .map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("exited with {} {}", output.status, stderr.trim()))
    }
}

/// ydotool 1.x takes raw evdev keycodes; 0.x took names like `ctrl+v`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum YdotoolSyntax {
    RawKeycodes,
    Symbolic,
}

fn ydotool_ctrl_v_args(syntax: YdotoolSyntax) -> &'static [&'static str] {
    match syntax {
        // KEY_LEFTCTRL = 29, KEY_V = 47
        YdotoolSyntax::RawKeycodes => &["key", "29:1", "47:1", "47:0", "29:0"],
        YdotoolSyntax::Symbolic => &["key", "ctrl+v"],
    }
}

/// Reads which syntax this ydotool takes from its help text, once.
fn ydotool_syntax() -> YdotoolSyntax {
    static SYNTAX: OnceLock<YdotoolSyntax> = OnceLock::new();
    *SYNTAX.get_or_init(|| {
        // 0.x prints its help to stderr, with varying exit codes.
        let help = Command::new("ydotool")
            .args(["key", "--help"])
            .output()
            .map(|o| format!("{}\n{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr)))
            .unwrap_or_default();
        classify_ydotool_help(&help).unwrap_or(YdotoolSyntax::RawKeycodes)
    })
}

fn classify_ydotool_help(help: &str) -> Option<YdotoolSyntax> {
    let help = help.to_ascii_lowercase();
    if help.contains("<keycode>:<pressed>") || help.contains("[keycodes]") || help.contains("using raw keycodes") {
        Some(YdotoolSyntax::RawKeycodes)
    } else if help.contains("separated by plus (+)") || (help.contains("<key sequence>") && help.contains("alt+r")) {
        Some(YdotoolSyntax::Symbolic)
    } else {
        None
    }
}

/// The richest format on the Wayland clipboard and its data: files, then an image, then HTML,
/// then text, else the first one offered. `None` when the clipboard is empty.
pub fn wl_snapshot() -> Result<Option<(String, Vec<u8>)>, String> {
    let listed = Command::new("wl-paste").arg("--list-types").stdin(Stdio::null()).stderr(Stdio::null()).output().map_err(|e| e.to_string())?;
    if !listed.status.success() {
        return Ok(None);
    }
    let listed = String::from_utf8_lossy(&listed.stdout);
    let kinds: Vec<&str> = listed.lines().map(str::trim).filter(|k| !k.is_empty()).collect();
    let rank = |kind: &&str| match *kind {
        "text/uri-list" => 0,
        k if k.starts_with("image/png") => 1,
        k if k.starts_with("image/") => 2,
        "text/html" => 3,
        k if k.starts_with("text/plain") => 4,
        _ => 5,
    };
    let Some(kind) = kinds.into_iter().min_by_key(rank) else {
        return Ok(None);
    };
    let data = Command::new("wl-paste").args(["--no-newline", "--type", kind]).stdin(Stdio::null()).stderr(Stdio::null()).output().map_err(|e| e.to_string())?;
    Ok(data.status.success().then(|| (kind.to_string(), data.stdout)))
}

/// Puts a `wl_snapshot` back on the Wayland clipboard; `None` empties it.
pub fn wl_restore(saved: Option<&(String, Vec<u8>)>) -> Result<(), String> {
    use std::io::Write;
    let Some((kind, data)) = saved else {
        return Command::new("wl-copy").arg("--clear").status().map(|_| ()).map_err(|e| e.to_string());
    };
    // wl-copy forks a process that keeps serving the data once stdin closes.
    let mut child = Command::new("wl-copy")
        .args(["--type", kind])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    child.stdin.take().ok_or("wl-copy has no stdin")?.write_all(data).map_err(|e| e.to_string())?;
    let status = child.wait().map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("wl-copy exited with {}", status))
    }
}

/// The text on the Wayland clipboard; `None` when it holds no text.
pub fn wl_paste() -> Result<Option<String>, String> {
    let output = Command::new("wl-paste")
        .args(["--no-newline", "--type", "text"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    // wl-paste fails when the clipboard is empty or holds no text.
    Ok(output.status.success().then(|| String::from_utf8_lossy(&output.stdout).into_owned()))
}

/// Copies `text` with wl-copy, which serves it to native Wayland apps.
pub fn wl_copy(text: &str) -> Result<(), String> {
    // wl-copy forks a process that keeps serving the text; don't wait on its output.
    let status = Command::new("wl-copy")
        .arg("--")
        .arg(text)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("wl-copy exited with {}", status))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_wayland_from_either_variable() {
        assert!(Session::from_vars(Some("wayland-0"), None, None).wayland);
        assert!(Session::from_vars(None, Some("wayland"), None).wayland);
        assert!(!Session::from_vars(Some(""), Some("x11"), None).wayland);
        assert!(!Session::from_vars(None, None, None).wayland);
    }

    #[test]
    fn detects_desktop_case_insensitively() {
        let s = Session::from_vars(Some("wayland-0"), None, Some("ubuntu:GNOME"));
        assert!(s.gnome && !s.kde);
        let s = Session::from_vars(Some("wayland-0"), None, Some("kde"));
        assert!(s.kde && !s.gnome);
    }

    #[test]
    fn skips_wtype_on_gnome_and_kde() {
        let gnome = Session { wayland: true, gnome: true, kde: false };
        assert_eq!(paste_tools(gnome), vec![Tool::Dotool, Tool::Ydotool]);
        let sway = Session { wayland: true, gnome: false, kde: false };
        assert_eq!(paste_tools(sway), vec![Tool::Wtype, Tool::Dotool, Tool::Ydotool]);
    }

    #[test]
    fn prefers_xdotool_on_x11() {
        let x11 = Session { wayland: false, gnome: true, kde: false };
        assert_eq!(paste_tools(x11), vec![Tool::Xdotool, Tool::Ydotool]);
    }

    #[test]
    fn reads_ydotool_syntax_from_help() {
        assert_eq!(
            classify_ydotool_help("Usage: key [OPTION]... [KEYCODES]...\nSyntax: <keycode>:<pressed>"),
            Some(YdotoolSyntax::RawKeycodes)
        );
        assert_eq!(
            classify_ydotool_help("Usage: key <key sequence>\nKey sequences are separated by plus (+), e.g. alt+r"),
            Some(YdotoolSyntax::Symbolic)
        );
        assert_eq!(classify_ydotool_help("something else"), None);
        assert_eq!(ydotool_ctrl_v_args(YdotoolSyntax::Symbolic), &["key", "ctrl+v"]);
    }

    #[test]
    fn finds_executables_on_path() {
        assert!(on_path("sh"));
        assert!(!on_path("definitely-not-a-real-tool-bifaria"));
    }
}
