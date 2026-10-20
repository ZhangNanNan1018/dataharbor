//! Explicit opt-in tests against disposable local databases, never saved profiles.
use std::{fs, path::PathBuf};

use serde_json::json;
use uuid::Uuid;

use crate::data::{commit_changes, export_table_csv, fetch_rows, import_table_csv};
use crate::models::*;
use crate::sqlutil::quote_ident;
use crate::state::AppState;

struct Fixture {
    state: AppState,
    id: String,
    database: String,
    schema: String,
    table: String,
    table_sql: String,
    dir: PathBuf,
}

impl Fixture {
    fn open(engine: Engine, port_var: &str) -> Self {
        let port = std::env::var(port_var)
            .expect("请先启动专用测试容器并设置测试端口")
            .parse()
            .expect("测试端口需要是整数");
        let dir = std::env::temp_dir().join(format!("dataharbor-server-test-{}", Uuid::new_v4()));
        let state = AppState::open_in(dir.clone(), true).unwrap();
        let database = "demo".to_string();
        let schema = if engine == Engine::Postgres {
            "public"
        } else {
            ""
        }
        .to_string();
        let saved = state
            .save_connection(SaveConnectionRequest {
                name: "临时集成测试".into(),
                engine,
                host: "127.0.0.1".into(),
                port,
                username: if engine == Engine::Postgres {
                    "postgres"
                } else {
                    "root"
                }
                .into(),
                password: Some("dataharbor".into()),
                database: database.clone(),
                ..Default::default()
            })
            .unwrap();
        let id = saved.connection.id;
        state.connect(&id).unwrap();
        let table = format!("dh_test_{}", Uuid::new_v4().simple());
        let table_sql = quote_ident(engine, &table).unwrap();
        let fixture = Self {
            state,
            id,
            database,
            schema,
            table,
            table_sql,
            dir,
        };
        fixture.sql(&format!("CREATE TABLE {} (label VARCHAR(80), tenant INTEGER NOT NULL, id INTEGER NOT NULL, PRIMARY KEY(tenant, id))", fixture.table_sql));
        fixture
    }

    fn sql(&self, sql: &str) {
        self.state
            .with_session(&self.id, |session| {
                session.execute(&self.database, sql, Some(100))
            })
            .unwrap();
    }

    fn page(&self) -> DataPage {
        fetch_rows(
            &self.state,
            FetchRowsRequest {
                connection_id: self.id.clone(),
                database: self.database.clone(),
                schema: self.schema.clone(),
                table: self.table.clone(),
                filters: vec![],
                sorts: vec![],
                page: 1,
                page_size: 10,
            },
        )
        .unwrap()
    }

    fn check_round_trip(&self) {
        self.sql(&format!(
            "INSERT INTO {} (label,tenant,id) VALUES ('first',1,1),('second',1,2)",
            self.table_sql
        ));
        let page = self.page();
        assert_eq!(page.total, 2);
        assert_eq!(page.rows[0][0], json!("first"));
        assert_eq!(
            page.columns
                .iter()
                .filter(|column| column.primary_key)
                .count(),
            2
        );
        let key = vec![
            FieldValue {
                column: "tenant".into(),
                value: page.rows[0][1].clone(),
            },
            FieldValue {
                column: "id".into(),
                value: page.rows[0][2].clone(),
            },
        ];
        let request = |changes| CommitRequest {
            connection_id: self.id.clone(),
            database: self.database.clone(),
            schema: self.schema.clone(),
            table: self.table.clone(),
            changes,
        };
        let update = RowChange {
            kind: "update".into(),
            pk: key,
            values: vec![FieldValue {
                column: "label".into(),
                value: json!("changed"),
            }],
        };
        commit_changes(&self.state, request(vec![update.clone()])).unwrap();
        assert_eq!(self.page().rows[0][0], json!("changed"));
        let mut pending = update.clone();
        pending.values[0].value = json!("must roll back");
        let mut invalid = update;
        invalid.pk.pop();
        assert!(commit_changes(&self.state, request(vec![pending, invalid])).is_err());
        assert_eq!(self.page().rows[0][0], json!("changed"));

        let path = self.dir.join("round-trip.csv").display().to_string();
        let exported = export_table_csv(
            &self.state,
            ExportTableRequest {
                connection_id: self.id.clone(),
                database: self.database.clone(),
                schema: self.schema.clone(),
                table: self.table.clone(),
                filters: vec![],
                sorts: vec![],
                path: path.clone(),
            },
        )
        .unwrap();
        assert_eq!(exported.row_count, 2);
        assert!(!exported.truncated);
        self.sql(&format!("DELETE FROM {}", self.table_sql));
        let imported = import_table_csv(
            &self.state,
            ImportCsvRequest {
                connection_id: self.id.clone(),
                database: self.database.clone(),
                schema: self.schema.clone(),
                table: self.table.clone(),
                path,
                truncate: false,
            },
        )
        .unwrap();
        assert_eq!(imported.inserted, 2);
        assert_eq!(self.page().rows[0][0], json!("changed"));
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.state.with_session(&self.id, |session| {
            session.execute(
                &self.database,
                &format!("DROP TABLE IF EXISTS {}", self.table_sql),
                None,
            )
        });
        if self.schema == "public" {
            let _ = self.state.with_session(&self.id, |session| {
                session.execute(
                    &self.database,
                    &format!("DROP SEQUENCE IF EXISTS {}_seq", self.table),
                    None,
                )
            });
        }
        self.state.disconnect(&self.id);
        let _ = fs::remove_dir_all(&self.dir);
    }
}

#[test]
#[ignore = "需要专用本地 MySQL 测试容器"]
fn mysql_real_round_trip() {
    Fixture::open(Engine::Mysql, "DATAHARBOR_TEST_MYSQL_PORT").check_round_trip();
}

#[test]
#[ignore = "需要专用本地 PostgreSQL 测试容器"]
fn postgres_real_round_trip() {
    let fixture = Fixture::open(Engine::Postgres, "DATAHARBOR_TEST_POSTGRES_PORT");
    fixture.check_round_trip();
    fixture.sql(&format!("CREATE SEQUENCE {}_seq", fixture.table));
    let query = format!(
        "WITH counted AS MATERIALIZED (SELECT nextval('{}_seq') AS n) SELECT n FROM counted",
        fixture.table
    );
    let response = crate::query::execute_sql(
        &fixture.state,
        ExecuteSqlRequest {
            connection_id: fixture.id.clone(),
            database: fixture.database.clone(),
            sql: query,
            page: 1,
            page_size: 20,
            only_statement: None,
        },
    )
    .unwrap();
    assert!(
        response.statements[0].ok,
        "{:?}",
        response.statements[0].error
    );
    let value = fixture
        .state
        .with_session(&fixture.id, |session| {
            session.execute(
                &fixture.database,
                &format!("SELECT last_value FROM {}_seq", fixture.table),
                Some(1),
            )
        })
        .unwrap();
    assert_eq!(
        crate::sqlutil::json_count(&value.rows[0][0]),
        Some(1),
        "自动分页不应额外重放函数进行COUNT"
    );
}
