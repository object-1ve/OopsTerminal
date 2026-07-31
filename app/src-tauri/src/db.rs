use rusqlite::{params, Connection, Result};
use std::path::PathBuf;

pub struct WindowState {
    pub width: f64,
    pub height: f64,
    pub x: i32,
    pub y: i32,
}

pub fn init_db(app_dir: PathBuf) -> Result<Connection> {
    let db_path = app_dir.join("oops_template.db");
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
