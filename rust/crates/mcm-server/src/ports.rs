use crate::error::ApiError;
use mcm_core::config::PortRange;
use rusqlite::{Connection, params};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

pub fn allocate_port(
    db: &Arc<Mutex<Connection>>,
    range: PortRange,
    requested: Option<u16>,
) -> Result<u16, ApiError> {
    let conn = db.lock().unwrap();
    let mut used = HashSet::new();

    {
        let mut stmt = conn.prepare("SELECT host_port FROM servers WHERE host_port > 0")?;
        let rows = stmt.query_map([], |r| r.get::<_, i64>(0))?;
        for r in rows {
            if let Ok(p) = r
                && let Ok(p16) = u16::try_from(p)
            {
                used.insert(p16);
            }
        }
    }

    if let Ok(mut stmt) =
        conn.prepare("SELECT host_port FROM server_port_reservations WHERE host_port > 0")
        && let Ok(rows) = stmt.query_map([], |r| r.get::<_, i64>(0))
    {
        for r in rows {
            if let Ok(p) = r
                && let Ok(p16) = u16::try_from(p)
            {
                used.insert(p16);
            }
        }
    }

    if let Some(req) = requested
        && req > 0
    {
        if used.contains(&req) {
            return Err(ApiError::conflict(
                "port_in_use",
                "The selected port is already in use by another server.",
            ));
        }
        return Ok(req);
    }

    for port in range.start..=range.end {
        if !used.contains(&port) {
            return Ok(port);
        }
    }

    Err(ApiError::conflict("port_pool_full", "port pool full"))
}

pub fn list_reservations(db: &Arc<Mutex<Connection>>) -> Result<Vec<serde_json::Value>, ApiError> {
    let conn = db.lock().unwrap();
    let mut stmt = conn.prepare(
        "SELECT host_port, server_id, COALESCE(protocol,'tcp') FROM server_port_reservations ORDER BY host_port",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(serde_json::json!({
            "host_port": r.get::<_, i64>(0)?,
            "server_id": r.get::<_, String>(1)?,
            "protocol": r.get::<_, String>(2)?,
        }))
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

pub fn create_reservation(
    _db: &Arc<Mutex<Connection>>,
    port: u16,
    _label: &str,
    protocol: &str,
) -> Result<serde_json::Value, ApiError> {
    Ok(serde_json::json!({
        "host_port": port,
        "protocol": protocol,
    }))
}

pub fn delete_reservation(db: &Arc<Mutex<Connection>>, id: &str) -> Result<(), ApiError> {
    if let Ok(port) = id.parse::<i64>() {
        let conn = db.lock().unwrap();
        conn.execute(
            "DELETE FROM server_port_reservations WHERE host_port = ?1",
            params![port],
        )?;
    }
    Ok(())
}
