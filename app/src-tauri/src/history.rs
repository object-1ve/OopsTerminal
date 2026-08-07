use serde::{Deserialize, Serialize};
use std::env;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

const MAX_ENTRIES: usize = 2000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub time: Option<String>,
    pub command: String,
}

#[derive(Serialize)]
pub struct OopsHistory {
    pub source: String,
    pub total: usize,
    pub entries: Vec<HistoryEntry>,
}

fn appdata_dir() -> Option<PathBuf> {
    env::var_os("APPDATA").map(PathBuf::from).or_else(|| {
        env::var_os("USERPROFILE")
            .or_else(|| env::var_os("HOME"))
            .map(|home| PathBuf::from(home).join("AppData").join("Roaming"))
    })
}

fn oops_history_path() -> Option<PathBuf> {
    appdata_dir().map(|dir| dir.join("OopsTerminal").join("history-with-time.jsonl"))
}

fn psreadline_history_path() -> Option<PathBuf> {
    appdata_dir().map(|dir| {
        dir.join("Microsoft")
            .join("Windows")
            .join("PowerShell")
            .join("PSReadLine")
            .join("ConsoleHost_history.txt")
    })
}

fn latest_entries(mut entries: Vec<HistoryEntry>, max: usize) -> (usize, Vec<HistoryEntry>) {
    let total = entries.len();
    entries.reverse();
    entries.truncate(max);
    (total, entries)
}

fn parse_jsonl_entries(text: &str, max: usize) -> (usize, Vec<HistoryEntry>) {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let entries: Vec<HistoryEntry> = text
        .lines()
        .map(|line| line.trim_end_matches('\r'))
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str::<HistoryEntry>(line).ok())
        .filter(|entry| !entry.command.trim().is_empty())
        .collect();
    latest_entries(entries, max)
}

fn parse_psreadline_entries(text: &str, max: usize) -> (usize, Vec<HistoryEntry>) {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let entries: Vec<HistoryEntry> = text
        .lines()
        .map(|line| line.trim_end_matches('\r'))
        .filter(|line| !line.trim().is_empty())
        .map(|command| HistoryEntry {
            time: None,
            command: command.to_string(),
        })
        .collect();
    latest_entries(entries, max)
}

fn read_history_text(path: &std::path::Path) -> Result<Option<String>, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("读取历史文件失败: {e}")),
    }
}

fn append_jsonl(path: &std::path::Path, time: &str, command: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("创建历史目录失败: {e}"))?;
    }

    let line = serde_json::json!({
        "time": time,
        "command": command,
    });
    let mut content = line.to_string();
    content.push('\n');

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| format!("打开历史文件失败: {e}"))?;
    file.write_all(content.as_bytes())
        .map_err(|e| format!("写入历史文件失败: {e}"))
}

#[tauri::command]
pub fn read_oops_history() -> Result<OopsHistory, String> {
    let oops_path =
        oops_history_path().ok_or_else(|| "无法确定 OopsTerminal 历史文件位置".to_string())?;

    if let Some(text) = read_history_text(&oops_path)? {
        let (total, entries) = parse_jsonl_entries(&text, MAX_ENTRIES);
        if total > 0 {
            return Ok(OopsHistory {
                source: "oops".to_string(),
                total,
                entries,
            });
        }
    }

    let psreadline_path =
        psreadline_history_path().ok_or_else(|| "无法确定 PSReadLine 历史文件位置".to_string())?;
    if let Some(text) = read_history_text(&psreadline_path)? {
        let (total, entries) = parse_psreadline_entries(&text, MAX_ENTRIES);
        return Ok(OopsHistory {
            source: "psreadline".to_string(),
            total,
            entries,
        });
    }

    Ok(OopsHistory {
        source: "none".to_string(),
        total: 0,
        entries: Vec::new(),
    })
}

#[tauri::command]
pub fn record_oops_history(time: String, command: String) -> Result<(), String> {
    if command.trim().is_empty() {
        return Ok(());
    }

    let path = oops_history_path()
        .ok_or_else(|| "无法确定 OopsTerminal 历史文件位置".to_string())?;
    append_jsonl(&path, &time, &command)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_jsonl_entries_newest_first() {
        let text = "\u{feff}{\"time\":\"2026-08-07T10:00:00+08:00\",\"command\":\"Set-Alias j jcode\"}\r\n{\"time\":\"2026-08-07T09:00:00+08:00\",\"command\":\"Set-Alias l lazygit\"}\n\n{\"time\":\"2026-08-07T08:00:00+08:00\",\"command\":\"irm https://example.test\"}";
        let (total, entries) = parse_jsonl_entries(text, 10);

        assert_eq!(total, 3);
        assert_eq!(entries[0].command, "irm https://example.test");
        assert_eq!(
            entries[0].time.as_deref(),
            Some("2026-08-07T08:00:00+08:00")
        );
        assert_eq!(entries[2].command, "Set-Alias j jcode");
    }

    #[test]
    fn ignores_malformed_and_empty_jsonl_entries() {
        let text = "not-json\n{\"command\":\"ls\"}\n{\"time\":\"2026-08-07T10:00:00+08:00\",\"command\":\"\"}\n";
        let (total, entries) = parse_jsonl_entries(text, 10);

        assert_eq!(total, 1);
        assert_eq!(entries[0].command, "ls");
        assert_eq!(entries[0].time, None);
    }

    #[test]
    fn limits_jsonl_entries_to_latest() {
        let text = "{\"time\":\"2026-08-07T10:00:00+08:00\",\"command\":\"a\"}\n{\"time\":\"2026-08-07T11:00:00+08:00\",\"command\":\"b\"}\n{\"time\":\"2026-08-07T12:00:00+08:00\",\"command\":\"c\"}\n";
        let (total, entries) = parse_jsonl_entries(text, 2);

        assert_eq!(total, 3);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].command, "c");
        assert_eq!(entries[1].command, "b");
    }

    #[test]
    fn parses_psreadline_lines_newest_first_without_time() {
        let (total, entries) = parse_psreadline_entries(
            "irm https://example.test\r\nSet-Alias l lazygit\n\nSet-Alias j jcode",
            10,
        );

        assert_eq!(total, 3);
        assert_eq!(entries[0].command, "Set-Alias j jcode");
        assert_eq!(entries[0].time, None);
        assert_eq!(entries[2].command, "irm https://example.test");
    }

    #[test]
    fn appends_jsonl_entries_without_overwriting() {
        let dir = std::env::temp_dir().join(format!(
            "oopsterminal-history-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = dir.join("history-with-time.jsonl");

        append_jsonl(
            &path,
            "2026-08-07T10:00:00+08:00",
            "Set-Alias j \"jcode\"",
        )
        .expect("append first");
        append_jsonl(&path, "2026-08-07T11:00:00+08:00", "irm https://example.test")
            .expect("append second");

        let text = std::fs::read_to_string(&path).expect("read history");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2, "追加记录不应覆盖已有内容");

        let first: serde_json::Value = serde_json::from_str(lines[0]).expect("parse first");
        assert_eq!(first["time"], "2026-08-07T10:00:00+08:00");
        assert_eq!(first["command"], "Set-Alias j \"jcode\"");

        let second: serde_json::Value = serde_json::from_str(lines[1]).expect("parse second");
        assert_eq!(second["time"], "2026-08-07T11:00:00+08:00");
        assert_eq!(second["command"], "irm https://example.test");
    }
}
