//! Stable protocol capabilities shared by Rust guards and generated UI metadata.
//! Per-table constraints (keys, collations and storage engine) remain runtime checks.
use crate::models::Engine;
use serde::Serialize;

#[derive(Debug, Clone, Copy, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct EngineCapabilities {
    pub engine: Engine,
    pub label: &'static str,
    pub default_port: u16,
    pub sql: bool,
    pub schema_tree: bool,
    pub filters: bool,
    pub atomic_writes: bool,
    pub persistent_console: bool,
    pub read_snapshot: bool,
    pub query_cancel: bool,
}

#[cfg(test)]
pub const ENGINES: [Engine; 10] = [
    Engine::Mysql,
    Engine::Mariadb,
    Engine::Postgres,
    Engine::Sqlite,
    Engine::Mongodb,
    Engine::Redis,
    Engine::Mssql,
    Engine::Oracle,
    Engine::Snowflake,
    Engine::Dameng,
];

impl Engine {
    pub fn capabilities(self) -> EngineCapabilities {
        EngineCapabilities {
            engine: self,
            label: self.label(),
            default_port: self.default_port(),
            sql: !matches!(self, Engine::Mongodb | Engine::Redis),
            schema_tree: matches!(self, Engine::Postgres | Engine::Mssql | Engine::Oracle),
            filters: self != Engine::Redis,
            atomic_writes: !matches!(self, Engine::Mongodb | Engine::Redis | Engine::Snowflake),
            persistent_console: !matches!(self, Engine::Oracle | Engine::Snowflake),
            read_snapshot: matches!(
                self,
                Engine::Sqlite | Engine::Mysql | Engine::Mariadb | Engine::Postgres
            ),
            query_cancel: matches!(
                self,
                Engine::Sqlite | Engine::Mysql | Engine::Mariadb | Engine::Postgres
            ),
        }
    }
}
