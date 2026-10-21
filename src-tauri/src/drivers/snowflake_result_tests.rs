use super::*;

fn response() -> JsonValue {
    json!({
        "statementHandle":"fixture-handle",
        "resultSetMetaData": {
            "numRows": "3",
            "rowType": [{"name":"amount", "type":"fixed", "nullable":false}, {"name":"label", "type":"text", "nullable":true}],
            "partitionInfo":[{"rowCount":1},{"rowCount":2}]
        },
        "data":[["9007199254740993", "null"]]
    })
}

#[test]
fn decode_preserves_null_text_exact_numbers_and_column_metadata_across_partitions() {
    let collected = collect_partitions(response(), None, |_, _| Ok(json!({"data":[["2",null],["3","NULL"]]}))).unwrap();
    let outcome = decode_result(collected).unwrap();
    assert_eq!(outcome.rows, vec![vec![json!("9007199254740993"), json!("null")], vec![json!("2"), JsonValue::Null], vec![json!("3"), json!("NULL")]]);
    assert_eq!(outcome.columns[0].name, "amount");
    assert_eq!(outcome.columns[0].data_type, "fixed");
    assert!(!outcome.columns[0].nullable);
    assert_eq!(outcome.columns[1].data_type, "text");
    assert!(outcome.columns[1].nullable);
    assert!(!outcome.truncated);
}

#[test]
fn pending_and_throttled_results_are_polled_without_replaying_the_statement() {
    let mut attempts = 0;
    let doc = finish_statement(202, json!({"statementHandle":"fixture-handle"}), |handle| {
        assert_eq!(handle, "fixture-handle");
        attempts += 1;
        Ok(if attempts == 1 { (429, json!({"statementHandle":"fixture-handle"})) } else { (200, response()) })
    }).unwrap();
    assert_eq!(attempts, 2);
    assert_eq!(doc, response());
    assert!(finish_statement(500, json!({}), |_| unreachable!()).is_err());
}

#[test]
fn partition_fetch_failures_and_inconsistent_totals_do_not_return_partial_success() {
    assert!(collect_partitions(response(), None, |_, _| Err(AppError::msg("HTTP 202 pending partition"))).is_err());
    assert!(collect_partitions(response(), None, |_, _| Ok(json!({"data":[["2",null]]}))).is_err());
    let mut invalid = response();
    invalid["resultSetMetaData"]["numRows"] = json!(1);
    assert!(collect_partitions(invalid, None, |_, _| Ok(json!({"data":[["2",null],["3",null]]}))).is_err());
    let mut missing = response();
    missing["resultSetMetaData"].as_object_mut().unwrap().remove("numRows");
    assert!(collect_partitions(missing, None, |_, _| unreachable!()).is_err());
}

#[test]
fn empty_result_retains_column_metadata_without_fetching_partitions() {
    let document = json!({"resultSetMetaData":{"numRows":0, "rowType":[{"name":"n","type":"fixed"}],"partitionInfo":[]},"data":[]});
    let outcome = decode_result(collect_partitions(document, None, |_, _| unreachable!()).unwrap()).unwrap();
    assert!(outcome.rows.is_empty());
    assert_eq!(outcome.columns[0].data_type, "fixed");
    assert!(!outcome.truncated);
}
