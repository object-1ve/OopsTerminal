use rusqlite::{params, Connection, Result};
use std::path::PathBuf;

pub struct WindowState {
    pub width: f64,
    pub height: f64,
    pub x: i32,
    pub y: i32,
}

#[derive(Clone, serde::Serialize)]
pub struct Settings {
    pub toggle_window_shortcut: Option<String>,
    pub quit_shortcut: Option<String>,
}

pub fn init_db(app_dir: PathBuf) -> Result<Connection> {
    let db_path = app_dir.join("oops_terminal.db");
    let conn = Connection::open(db_path)?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS window_state (
            id INTEGER PRIMARY KEY,
            width REAL,
            height REAL,
            x INTEGER,
            y INTEGER
        )",
        [],
    )?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS settings (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        )",
        [],
    )?;

    Ok(conn)
}

pub fn save_window_state(conn: &Connection, state: &WindowState) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO window_state (id, width, height, x, y) VALUES (1, ?1, ?2, ?3, ?4)",
        params![state.width, state.height, state.x, state.y],
    )?;
    Ok(())
}

pub fn load_window_state(conn: &Connection) -> Result<Option<WindowState>> {
    let mut stmt = conn.prepare("SELECT width, height, x, y FROM window_state WHERE id = 1")?;
    let mut rows = stmt.query([])?;

    if let Some(row) = rows.next()? {
        Ok(Some(WindowState {
            width: row.get(0)?,
            height: row.get(1)?,
            x: row.get(2)?,
            y: row.get(3)?,
        }))
    } else {
        Ok(None)
    }
}

pub fn load_settings(conn: &Connection) -> Result<Settings> {
    Ok(Settings {
        toggle_window_shortcut: load_setting(conn, "toggle_window_shortcut")?,
        quit_shortcut: load_setting(conn, "quit_shortcut")?,
    })
}

fn load_setting(conn: &Connection, key: &str) -> Result<Option<String>> {
    let mut stmt = conn.prepare("SELECT value FROM settings WHERE key = ?1")?;
    let mut rows = stmt.query(params![key])?;

    if let Some(row) = rows.next()? {
        Ok(Some(row.get(0)?))
    } else {
        Ok(None)
    }
}

pub fn save_setting(conn: &Connection, key: &str, value: Option<&str>) -> Result<()> {
    match value {
        Some(v) => {
            conn.execute(
                "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)",
                params![key, v],
            )?;
        }
        None => {
            conn.execute("DELETE FROM settings WHERE key = ?1", params![key])?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_roundtrip_and_clear() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
            [],
        )
        .unwrap();

        save_setting(&conn, "toggle_window_shortcut", Some("Ctrl+Shift+K")).unwrap();
        save_setting(&conn, "quit_shortcut", None).unwrap();

        let loaded = load_settings(&conn).unwrap();
        assert_eq!(
            loaded.toggle_window_shortcut.as_deref(),
            Some("Ctrl+Shift+K")
        );
        assert_eq!(loaded.quit_shortcut, None);

        // Clearing an existing key removes the row.
        save_setting(&conn, "toggle_window_shortcut", None).unwrap();
        let loaded = load_settings(&conn).unwrap();
        assert_eq!(loaded.toggle_window_shortcut, None);
    }
}

