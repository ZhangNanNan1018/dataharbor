use crate::error::AppResult;
use crate::models::ColumnMeta;
use crate::state::AppState;

pub fn list_databases(state: &AppState, connection_id: &str) -> AppResult<Vec<String>> {
    state.with_session(connection_id, |session| session.list_databases())
}

pub fn list_schemas(state: &AppState, connection_id: &str, database: &str) -> AppResult<Vec<String>> {
    state.with_session(connection_id, |session| session.list_schemas(database))
}

pub fn list_tables(
    state: &AppState,
    connection_id: &str,
    database: &str,
    schema: &str,
) -> AppResult<Vec<String>> {
    state.with_session(connection_id, |session| session.list_tables(database, schema))
}

pub fn list_views(
    state: &AppState,
    connection_id: &str,
    database: &str,
    schema: &str,
) -> AppResult<Vec<String>> {
    state.with_session(connection_id, |session| session.list_views(database, schema))
}

pub fn list_columns(
    state: &AppState,
    connection_id: &str,
    database: &str,
    schema: &str,
    object_name: &str,
) -> AppResult<Vec<ColumnMeta>> {
    state.with_session(connection_id, |session| {
        session.list_columns(database, schema, object_name)
    })
}
