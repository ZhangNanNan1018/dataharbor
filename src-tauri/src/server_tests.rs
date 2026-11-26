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
            original: Some(page.columns.iter().zip(&page.rows[0]).map(|(column, value)| FieldValue { column: column.name.clone(), value: value.clone() }).collect()),
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
        ExecuteSqlRequest { session_id: None,
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


fn check_precision_and_conflicts(engine: Engine, port: &str) {
    let fixture = Fixture::open(engine, port);
    fixture.sql(&format!("ALTER TABLE {} ADD amount DECIMAL(38,18)", fixture.table_sql));
    fixture.sql(&format!("ALTER TABLE {} ADD large_number DECIMAL(20,0)", fixture.table_sql));
    if engine == Engine::Mysql {
        fixture.sql(&format!("ALTER TABLE {} ADD stamped DATETIME(6), ADD duration TIME(6), ADD payload BLOB", fixture.table_sql));
    }
    let path = fixture.dir.join("precise.csv").display().to_string();
    fs::write(&path, "label,tenant,id,amount,large_number\n00123,1,1,12345678901234567890.123456789012345678,18446744073709551615\n\"a\"\"b\n第二行\",1,2,0.000000000000000001,9007199254740993\n").unwrap();
    import_table_csv(&fixture.state, ImportCsvRequest {
        connection_id: fixture.id.clone(), database: fixture.database.clone(), schema: fixture.schema.clone(),
        table: fixture.table.clone(), path: path.clone(), truncate: false,
    }).unwrap();
    if engine == Engine::Mysql {
        fixture.sql(&format!("UPDATE {} SET stamped='2026-10-21 12:34:56.123456', duration='49:02:03.654321', payload=X'4142'", fixture.table_sql));
    }
    let before = fixture.page();
    assert_eq!(before.rows[0][0], json!("00123"));
    assert_eq!(before.rows[0][3], json!("12345678901234567890.123456789012345678"));
    assert_eq!(before.rows[0][4], json!("18446744073709551615"));
    assert!(before.columns[3].data_type.to_ascii_lowercase().contains("38,18"));
    export_table_csv(&fixture.state, ExportTableRequest {
        connection_id: fixture.id.clone(), database: fixture.database.clone(), schema: fixture.schema.clone(), table: fixture.table.clone(), path: path.clone(), filters: vec![], sorts: vec![],
    }).unwrap();
    fixture.sql(&format!("DELETE FROM {}", fixture.table_sql));
    crate::studio::import_file(&fixture.state, crate::studio::FileImportRequest {
        connection_id: fixture.id.clone(), database: fixture.database.clone(), schema: fixture.schema.clone(), table: fixture.table.clone(), path, format: "csv".into(),
    }).unwrap();
    assert_eq!(fixture.page().rows, before.rows);
    let original = before.columns.iter().zip(&before.rows[0]).map(|(col,val)| FieldValue { column: col.name.clone(), value: val.clone() }).collect::<Vec<_>>();
    let keys = before.columns.iter().zip(&before.rows[0]).filter(|(col,_)| col.primary_key).map(|(col,val)| FieldValue { column: col.name.clone(), value: val.clone() }).collect::<Vec<_>>();
    let request = |value| CommitRequest {
        connection_id: fixture.id.clone(), database: fixture.database.clone(), schema: fixture.schema.clone(), table: fixture.table.clone(),
        changes: vec![RowChange { kind: "update".into(), original: Some(original.clone()), pk: keys.clone(), values: vec![FieldValue { column: "label".into(), value }] }],
    };
    // Matched-but-unchanged updates must not be mistaken for stale rows on MySQL.
    assert_eq!(commit_changes(&fixture.state, request(json!("00123"))).unwrap().applied, 1);
    fixture.sql(&format!("UPDATE {} SET label='other writer' WHERE tenant=1 AND id=1", fixture.table_sql));
    assert!(commit_changes(&fixture.state, request(json!("stale overwrite"))).is_err());
    assert_eq!(fixture.page().rows[0][0], json!("other writer"));
    fixture.sql(&format!("DELETE FROM {} WHERE tenant=1 AND id=1", fixture.table_sql));
    assert!(commit_changes(&fixture.state, request(json!("missing row"))).is_err());
}

#[test]
#[ignore = "需要专用本地 MySQL 测试容器"]
fn mysql_precise_csv_and_concurrent_edits() { check_precision_and_conflicts(Engine::Mysql, "DATAHARBOR_TEST_MYSQL_PORT"); }

#[test]
#[ignore = "需要专用本地 PostgreSQL 测试容器"]
fn postgres_precise_csv_and_concurrent_edits() { check_precision_and_conflicts(Engine::Postgres, "DATAHARBOR_TEST_POSTGRES_PORT"); }


#[test]
#[ignore = "需要专用 PostgreSQL 容器及 DATAHARBOR_TEST_PG_CONTAINER"]
fn postgres_native_copy_backup_restores_through_restricted_psql() {
    let container = std::env::var("DATAHARBOR_TEST_PG_CONTAINER").expect("需要专用测试容器 ID");
    let fixture = Fixture::open(Engine::Postgres, "DATAHARBOR_TEST_POSTGRES_PORT");
    fixture.sql(&format!("INSERT INTO {} VALUES ('quotes '' and ;\n第二行',1,1)", fixture.table_sql));
    let expected = fixture.page().rows;
    let remote_dump = format!("/tmp/{}.sql", fixture.table);
    let remote_ready = format!("/tmp/{}-ready.sql", fixture.table);
    let host_dump = fixture.dir.join("native.sql");
    let host_ready = fixture.dir.join("ready.sql");
    // Run the production tool plans inside the disposable test container,
    // where pg_dump/psql are installed, without depending on host PATH setup.
    let run = |mut plan: crate::native::NativePlan| {
        let program = plan.program.clone();
        let mut args = vec!["exec".into()];
        for (name, value) in &plan.env { args.extend(["-e".into(), format!("{name}={value}")]); }
        args.extend([container.clone(), program]);
        args.extend(plan.args);
        plan.program = "docker".into(); plan.args = args; plan.env.clear();
        crate::native::run_plan(&plan).unwrap();
    };
    let mut dump = crate::native::pg_plan("127.0.0.1", 5432, "postgres", "demo", &remote_dump, "dataharbor");
    dump.args.extend(["--table".into(), fixture.table_sql.clone()]);
    run(dump);
    let copy = |source: &str, target: &str| {
        assert!(std::process::Command::new("docker").args(["cp", source, target]).status().unwrap().success());
    };
    copy(&format!("{container}:{remote_dump}"), host_dump.to_str().unwrap());
    assert!(fs::read_to_string(&host_dump).unwrap().contains("COPY "));
    crate::native::prepare_pg_script(&host_dump, &host_ready).unwrap();
    copy(host_ready.to_str().unwrap(), &format!("{container}:{remote_ready}"));
    fixture.sql(&format!("DROP TABLE {}", fixture.table_sql));
    run(crate::native::pg_restore_plan("127.0.0.1", 5432, "postgres", "demo", &remote_ready, "dataharbor", false));
    assert_eq!(fixture.page().rows, expected);
    let _ = std::process::Command::new("docker").args(["exec", &container, "rm", "-f", &remote_dump, &remote_ready]).status();
}

#[test]
#[ignore = "requires disposable MySQL 8 at DATAHARBOR_TEST_MYSQL_PORT"]
fn mysql_bit_binary_geometry_round_trip() {
    check_bit_binary_geometry(Engine::Mysql, "DATAHARBOR_TEST_MYSQL_PORT");
    if std::env::var_os("DATAHARBOR_TEST_MARIADB_PORT").is_some() {
        check_bit_binary_geometry(Engine::Mariadb, "DATAHARBOR_TEST_MARIADB_PORT");
    }
}

fn check_bit_binary_geometry(engine: Engine, port: &str) {
    let f = Fixture::open(engine, port);
    let axis = if engine == Engine::Mysql {
        ",'axis-order=long-lat'"
    } else {
        ""
    };
    let t = &f.table_sql;
    f.sql(&format!("DROP TABLE {t}"));
    f.sql(&format!("CREATE TABLE {t} (k BIT(64) PRIMARY KEY, flag BIT(1), mask8 BIT(8), g POINT, bin VARBINARY(8), txt TEXT)"));
    f.sql(&format!("INSERT INTO {t} VALUES (18446744073709551615,1,65,ST_GeomFromText('POINT(120.5 31.25)',4326{axis}),X'00FF','0x4142'),(18446744073709551614,0,255,NULL,X'','base64:QQ=='),(256,NULL,0,NULL,NULL,NULL)"));
    let page = f.page();
    let select = |query: &str| {
        f.state
            .with_session(&f.id, |s| s.execute(&f.database, query, None))
            .unwrap()
    };
    let result = select(&format!("SELECT * FROM {t} ORDER BY k"));
    assert_eq!(page.rows, result.rows);
    assert_eq!(result.columns[4].data_type, "binary");
    assert_eq!(result.columns[5].data_type, "text");
    assert_eq!(result.rows[0][0], json!(256));
    assert!(result.rows[0][1].is_null());
    assert_eq!(result.rows[1][1..3], [json!(0), json!(255)]);
    assert_eq!(
        result.rows[1][4..],
        [json!("base64:"), json!("base64:QQ==")]
    );
    for (column, op, value) in [("k", "eq", "18446744073709551615"), ("mask8", "contains", "65")] {
        let filtered = fetch_rows(&f.state, FetchRowsRequest {
            connection_id: f.id.clone(), database: f.database.clone(), schema: f.schema.clone(), table: f.table.clone(),
            filters: vec![FilterSpec { column: column.into(), op: op.into(), value: Some(value.into()) }],
            sorts: vec![], page: 1, page_size: 10,
        }).unwrap();
        assert_eq!(filtered.rows, vec![page.rows[2].clone()]);
    }
    let last = &page.rows[2];
    assert_eq!(
        last[..3],
        [json!("18446744073709551615"), json!(1), json!(65)]
    );
    assert_eq!(last[3]["wkt"], "POINT(120.5 31.25)");
    assert_eq!(last[3]["srid"], 4326);
    assert_eq!(last[4..], [json!("base64:AP8="), json!("0x4142")]);
    let field = |name: &str, value| FieldValue {
        column: name.into(),
        value,
    };
    let commit = |page: &DataPage, op: &str, values| {
        let row = &page.rows[2];
        commit_changes(
            &f.state,
            CommitRequest {
                connection_id: f.id.clone(),
                database: f.database.clone(),
                schema: f.schema.clone(),
                table: f.table.clone(),
                changes: vec![RowChange {
                    kind: op.into(),
                    pk: vec![field("k", row[0].clone())],
                    values,
                    original: Some(
                        page.columns
                            .iter()
                            .zip(row)
                            .map(|(c, v)| field(&c.name, v.clone()))
                            .collect(),
                    ),
                }],
            },
        )
    };
    assert!(commit(&page, "update", vec![field("flag", json!("2"))]).is_err());
    commit(&page, "update", vec![field("txt", json!("updated"))]).unwrap();
    let updated = f.page();
    assert_eq!(updated.rows[2][3], last[3]);
    commit(
        &updated,
        "update",
        vec![
            field("g", json!({"srid":4326,"wkt":"POINT(121 32)"})),
            field("bin", json!("base64:/wA=")),
        ],
    )
    .unwrap();
    let expected = f.page();
    assert_eq!(expected.rows[2][3]["wkt"], "POINT(121 32)");
    assert_eq!(expected.rows[2][4], "base64:/wA=");
    let path = f.dir.join("typed.csv").display().to_string();
    export_table_csv(
        &f.state,
        ExportTableRequest {
            connection_id: f.id.clone(),
            database: f.database.clone(),
            schema: f.schema.clone(),
            table: f.table.clone(),
            filters: vec![],
            sorts: vec![],
            path: path.clone(),
        },
    )
    .unwrap();
    assert!(fs::read_to_string(&path)
        .unwrap()
        .contains("SRID=4326;POINT(121 32)"));
    commit(&expected, "delete", vec![]).unwrap();
    assert_eq!(f.page().rows.len(), 2);
    f.sql(&format!("DELETE FROM {t}"));
    import_table_csv(
        &f.state,
        ImportCsvRequest {
            connection_id: f.id.clone(),
            database: f.database.clone(),
            schema: f.schema.clone(),
            table: f.table.clone(),
            path,
            truncate: false,
        },
    )
    .unwrap();
    assert_eq!(f.page().rows, expected.rows);
}

#[test]
#[ignore = "requires disposable MySQL and PostgreSQL; also tests MariaDB when configured"]
fn idle_shared_connection_and_single_console_recovery() {
    for (engine, port) in [
        (Engine::Mysql, "DATAHARBOR_TEST_MYSQL_PORT"),
        (Engine::Mariadb, "DATAHARBOR_TEST_MARIADB_PORT"),
        (Engine::Postgres, "DATAHARBOR_TEST_POSTGRES_PORT"),
    ] {
        if engine == Engine::Mariadb && std::env::var_os(port).is_none() {
            continue;
        }
        let f = Fixture::open(engine, port);
        let identity = if engine == Engine::Postgres {
            "SELECT pg_backend_pid()"
        } else {
            "SELECT CONNECTION_ID()"
        };
        let console = |previous: Option<&str>| {
            f.state
                .prepare_query_session(&f.id, &f.database, previous, true)
                .unwrap()
        };
        let run = |key: &str, sql: &str| {
            crate::query::execute_sql(
                &f.state,
                ExecuteSqlRequest {
                    connection_id: f.id.clone(),
                    database: f.database.clone(),
                    session_id: Some(key.into()),
                    sql: sql.into(),
                    page: 1,
                    page_size: 20,
                    only_statement: None,
                },
            )
        };
        let scalar = |key: &str, sql: &str| run(key, sql).unwrap().statements[0].rows[0][0].clone();
        let a = console(None);
        let b = console(None);
        let b_id = scalar(&b, identity);
        let (set_variable, read_variable) = if engine == Engine::Postgres {
            (
                "SET dataharbor.sentinel = 'preserved'",
                "SELECT current_setting('dataharbor.sentinel')",
            )
        } else {
            ("SET @sentinel = 'preserved'", "SELECT @sentinel")
        };
        assert!(!run(&b, set_variable).unwrap().stopped);
        assert!(!run(&b, "CREATE TEMPORARY TABLE console_marker(n INTEGER); BEGIN; INSERT INTO console_marker VALUES (17)").unwrap().stopped);

        let shared_id = || {
            f.state
                .with_read_session(&f.id, &f.database, |s| {
                    Ok(s.execute(&f.database, identity, Some(1))?.rows[0][0].clone())
                })
                .unwrap()
        };
        if engine == Engine::Postgres {
            let target = f
                .state
                .with_read_session(&f.id, "postgres", |s| {
                    Ok(s.execute("postgres", identity, Some(1))?.rows[0][0].clone())
                })
                .unwrap();
            assert!(
                !run(
                    &a,
                    &format!(
                        "SELECT pg_terminate_backend({}, 5000)",
                        target.to_string().trim_matches('"')
                    )
                )
                .unwrap()
                .stopped
            );
            assert!(crate::schema::list_schemas(&f.state, &f.id, "postgres")
                .unwrap()
                .contains(&"public".into()));
        }
        let old = shared_id();
        if engine == Engine::Postgres {
            let kill = format!("SELECT pg_terminate_backend({old}, 5000)").replace('"', "");
            assert!(!run(&a, &kill).unwrap().stopped);
        } else {
            f.sql("SET SESSION wait_timeout = 1");
            std::thread::sleep(std::time::Duration::from_secs(2));
        }
        std::thread::scope(|scope| {
            let reads: Vec<_> = (0..4).map(|_| scope.spawn(&shared_id)).collect();
            let ids: Vec<_> = reads.into_iter().map(|t| t.join().unwrap()).collect();
            assert_ne!(ids[0], old);
            assert!(ids.iter().all(|id| id == &ids[0]));
        });
        assert!(
            crate::schema::list_tables(&f.state, &f.id, &f.database, &f.schema)
                .unwrap()
                .contains(&f.table)
        );
        assert_eq!(f.page().total, 0);
        let a_id = scalar(&a, identity).to_string().replace('"', "");
        let terminate = if engine == Engine::Postgres {
            format!("SELECT pg_terminate_backend({a_id})")
        } else {
            format!("KILL CONNECTION {a_id}")
        };
        let script = if engine == Engine::Postgres {
            format!(
                "SELECT 1; {terminate}; INSERT INTO {} VALUES ('unexpected', 1, 1)",
                f.table_sql
            )
        } else {
            f.sql(&terminate);
            format!(
                "SELECT 1; INSERT INTO {} VALUES ('unexpected', 1, 1)",
                f.table_sql
            )
        };
        let response = run(&a, &script).unwrap();
        assert!(response.stopped, "{engine:?}");
        assert_eq!(response.statements[0].ok, engine == Engine::Postgres);
        let failed = response.statements.last().unwrap();
        assert!(
            failed.disconnect_phase.is_some(),
            "{engine:?}: {}",
            failed.message
        );
        assert_eq!(f.page().total, 0);
        let next = console(Some(&a));
        assert!(run(&a, identity).is_err());
        assert_eq!(console(Some(&b)), b);
        assert_ne!(scalar(&next, identity).to_string().replace('"', ""), a_id);
        assert_eq!(scalar(&b, identity), b_id);
        assert_eq!(scalar(&b, read_variable), json!("preserved"));
        assert_eq!(
            scalar(&b, "SELECT n FROM console_marker")
                .to_string()
                .trim_matches('"'),
            "17"
        );
        assert!(!run(&b, "ROLLBACK").unwrap().stopped);
        assert!(
            run(&b, "SELECT n FROM console_marker").unwrap().statements[0]
                .rows
                .is_empty()
        );
    }
}
