//! The client versions setup recorded (`client_versions`): what hooks read instead of
//! running a client's CLI.

use super::Vault;
use crate::clients::{Caps, ClientKind, Version};
use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};

/// A client version setup (or `doctor --fix`) stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredVersion {
    pub version: Version,
    /// When it was detected, UTC (`%Y-%m-%dT%H:%M:%SZ`).
    pub detected_at: String,
}

/// The stored version of `kind`, if any, from an open connection (`doctor` reads a
/// database it must not migrate). An unparsable stored value counts as none.
pub fn stored_client_version(conn: &Connection, kind: ClientKind) -> Result<Option<StoredVersion>> {
    let row: Option<(String, String)> = conn
        .query_row(
            "SELECT version, detected_at FROM client_versions WHERE client = ?1",
            [kind.token()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    Ok(row.and_then(|(version, detected_at)| {
        Some(StoredVersion {
            version: Version::parse(&version)?,
            detected_at,
        })
    }))
}

impl Vault {
    /// The version stored for `kind` at its last setup.
    pub fn client_version(&self, kind: ClientKind) -> Result<Option<StoredVersion>> {
        stored_client_version(&self.conn, kind)
    }

    /// Stores the version `kind` reported now, replacing the previous one.
    pub fn record_client_version(&self, kind: ClientKind, version: Version) -> Result<()> {
        self.conn.execute(
            "INSERT INTO client_versions (client, version) VALUES (?1, ?2)
             ON CONFLICT(client) DO UPDATE SET
                version = excluded.version,
                detected_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')",
            params![kind.token(), version.to_string()],
        )?;
        Ok(())
    }

    /// The capabilities of `kind` at its stored version, or its latest known ones when
    /// none is stored. For hooks and the plugin: never runs the client's CLI.
    pub fn client_caps(&self, kind: ClientKind) -> Result<&'static Caps> {
        let stored = self.client_version(kind)?.map(|stored| stored.version);
        Ok(kind.caps(stored))
    }
}
