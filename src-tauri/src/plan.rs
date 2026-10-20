use serde::Serialize;
use serde_json::Value as JsonValue;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanNode {
    pub label: String,
    pub detail: String,
    pub cost: Option<f64>,
    pub rows: Option<f64>,
    pub children: Vec<PlanNode>,
}

pub fn parse_plan(engine: &str, columns: &[String], rows: &[Vec<JsonValue>]) -> PlanNode {
    let blob = rows
        .first()
        .and_then(|row| row.first())
        .and_then(|value| value.as_str())
        .unwrap_or("");
    if let Ok(json) = serde_json::from_str::<JsonValue>(blob) {
        if engine == "postgres" || json.get("Plan").is_some() || json.as_array().and_then(|items| items.first()).and_then(|item| item.get("Plan")).is_some() {
            return parse_postgres(&json);
        }
        if json.get("query_block").is_some() || engine == "mysql" || engine == "mariadb" {
            return parse_mysql(&json);
        }
    }
    if blob.contains("<ShowPlanXML") || blob.contains("<RelOp") {
        return parse_showplan_xml(blob);
    }
    if let Some(parent_idx) = columns.iter().position(|name| name.eq_ignore_ascii_case("parent")) {
        if columns.iter().any(|name| name.eq_ignore_ascii_case("detail") || name.eq_ignore_ascii_case("id")) {
            return parse_sqlite(columns, rows, parent_idx);
        }
    }
    if columns.iter().any(|name| name.eq_ignore_ascii_case("StmtText")) {
        return parse_showplan_rows(columns, rows);
    }
    PlanNode {
        label: "计划".into(),
        detail: if blob.is_empty() {
            rows.iter()
                .map(|row| row.iter().map(cell).collect::<Vec<_>>().join(" | "))
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            blob.to_string()
        },
        cost: None,
        rows: None,
        children: Vec::new(),
    }
}

fn parse_mysql(value: &JsonValue) -> PlanNode {
    walk_mysql(value, "查询")
}

fn walk_mysql(value: &JsonValue, fallback: &str) -> PlanNode {
    let mut node = PlanNode {
        label: fallback.into(),
        detail: String::new(),
        cost: value
            .get("cost_info")
            .and_then(|item| item.get("query_cost"))
            .and_then(as_f64),
        rows: value
            .get("rows_examined_per_scan")
            .or_else(|| value.get("rows_produced_per_join"))
            .and_then(as_f64),
        children: Vec::new(),
    };
    if let Some(name) = value.get("table_name").and_then(|item| item.as_str()) {
        node.label = format!("表 {name}");
        let access = value.get("access_type").and_then(|item| item.as_str()).unwrap_or("");
        node.detail = access.to_string();
    } else if let Some(name) = value.get("select_id") {
        node.label = format!("查询块 {name}");
    }
    if let Some(object) = value.as_object() {
        for (key, child) in object {
            if matches!(key.as_str(), "query_block" | "table" | "ordering_operation" | "grouping_operation" | "duplicates_removal" | "attached_subqueries" | "union_result" | "query_specifications") {
                if let Some(array) = child.as_array() {
                    for item in array {
                        node.children.push(walk_mysql(item, key));
                    }
                } else {
                    node.children.push(walk_mysql(child, key));
                }
            }
            if key == "nested_loop" {
                if let Some(array) = child.as_array() {
                    for item in array {
                        if let Some(table) = item.get("table") {
                            node.children.push(walk_mysql(table, "表"));
                        } else {
                            node.children.push(walk_mysql(item, "嵌套循环"));
                        }
                    }
                }
            }
        }
    }
    node
}

fn parse_postgres(value: &JsonValue) -> PlanNode {
    let plan = value
        .get("Plan")
        .or_else(|| value.as_array().and_then(|items| items.first()).and_then(|item| item.get("Plan")))
        .unwrap_or(value);
    walk_postgres(plan)
}

fn walk_postgres(value: &JsonValue) -> PlanNode {
    let kind = value.get("Node Type").and_then(|item| item.as_str()).unwrap_or("节点");
    let relation = value.get("Relation Name").and_then(|item| item.as_str()).unwrap_or("");
    let label = if relation.is_empty() {
        kind.to_string()
    } else {
        format!("{kind} {relation}")
    };
    let children = value
        .get("Plans")
        .and_then(|item| item.as_array())
        .map(|items| items.iter().map(walk_postgres).collect())
        .unwrap_or_default();
    PlanNode {
        label,
        detail: value
            .get("Filter")
            .or_else(|| value.get("Index Name"))
            .and_then(|item| item.as_str())
            .unwrap_or("")
            .to_string(),
        cost: value.get("Total Cost").and_then(as_f64),
        rows: value.get("Plan Rows").and_then(as_f64),
        children,
    }
}

fn parse_sqlite(columns: &[String], rows: &[Vec<JsonValue>], parent_idx: usize) -> PlanNode {
    let id_idx = columns.iter().position(|name| name.eq_ignore_ascii_case("id")).unwrap_or(0);
    let detail_idx = columns
        .iter()
        .position(|name| name.eq_ignore_ascii_case("detail"))
        .unwrap_or(columns.len().saturating_sub(1));
    let nodes: Vec<(i64, i64, PlanNode)> = rows
        .iter()
        .map(|row| {
            let id = row.get(id_idx).and_then(as_f64).unwrap_or(0.0) as i64;
            let parent = row.get(parent_idx).and_then(as_f64).unwrap_or(0.0) as i64;
            let detail = cell(row.get(detail_idx).unwrap_or(&JsonValue::Null));
            (
                id,
                parent,
                PlanNode {
                    label: detail.clone(),
                    detail,
                    cost: None,
                    rows: None,
                    children: Vec::new(),
                },
            )
        })
        .collect();
    struct Item {
        id: i64,
        parent: i64,
        node: PlanNode,
    }
    let mut items: Vec<Item> = nodes
        .into_iter()
        .map(|(id, parent, node)| Item { id, parent, node })
        .collect();
    fn attach(items: &mut Vec<Item>, parent_id: i64) -> Vec<PlanNode> {
        let ids: Vec<i64> = items
            .iter()
            .filter(|item| item.parent == parent_id)
            .map(|item| item.id)
            .collect();
        let mut children = Vec::new();
        for id in ids {
            if let Some(pos) = items.iter().position(|item| item.id == id && item.parent == parent_id) {
                let mut item = items.remove(pos);
                item.node.children = attach(items, item.id);
                children.push(item.node);
            }
        }
        children
    }
    let children = attach(&mut items, 0);
    PlanNode {
        label: "SQLite 计划".into(),
        detail: String::new(),
        cost: None,
        rows: None,
        children: if children.is_empty() {
            items.into_iter().map(|item| item.node).collect()
        } else {
            children
        },
    }
}

fn parse_showplan_rows(columns: &[String], rows: &[Vec<JsonValue>]) -> PlanNode {
    let text_idx = columns.iter().position(|name| name.eq_ignore_ascii_case("StmtText")).unwrap_or(0);
    let cost_idx = columns.iter().position(|name| name.eq_ignore_ascii_case("TotalSubtreeCost"));
    let rows_idx = columns.iter().position(|name| name.eq_ignore_ascii_case("EstimateRows"));
    PlanNode {
        label: "SQL Server 计划".into(),
        detail: String::new(),
        cost: None,
        rows: None,
        children: rows
            .iter()
            .map(|row| PlanNode {
                label: cell(row.get(text_idx).unwrap_or(&JsonValue::Null)),
                detail: String::new(),
                cost: cost_idx.and_then(|index| row.get(index)).and_then(as_f64),
                rows: rows_idx.and_then(|index| row.get(index)).and_then(as_f64),
                children: Vec::new(),
            })
            .collect(),
    }
}

fn parse_showplan_xml(xml: &str) -> PlanNode {
    let mut root = PlanNode {
        label: "SQL Server XML 计划".into(),
        detail: String::new(),
        cost: None,
        rows: None,
        children: Vec::new(),
    };
    for (index, part) in xml.split("<RelOp ").enumerate() {
        if index == 0 {
            continue;
        }
        let head = part.split('>').next().unwrap_or("");
        let op = attr(head, "PhysicalOp").unwrap_or_else(|| "RelOp".into());
        let cost = attr(head, "EstimatedTotalSubtreeCost").and_then(|text| text.parse().ok());
        let rows = attr(head, "EstimateRows").and_then(|text| text.parse().ok());
        root.children.push(PlanNode {
            label: op,
            detail: String::new(),
            cost,
            rows,
            children: Vec::new(),
        });
    }
    root
}

fn attr(head: &str, name: &str) -> Option<String> {
    let needle = format!("{name}=\"");
    let rest = head.split(&needle).nth(1)?;
    Some(rest.split('"').next()?.to_string())
}

fn as_f64(value: &JsonValue) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
        .or_else(|| value.as_i64().map(|item| item as f64))
}

fn cell(value: &JsonValue) -> String {
    match value {
        JsonValue::String(text) => text.clone(),
        JsonValue::Null => String::new(),
        other => other.to_string(),
    }
}

#[cfg(test)]
fn max_cost(node: &PlanNode) -> f64 {
    let own = node.cost.unwrap_or(0.0);
    node.children.iter().map(max_cost).fold(own, f64::max)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_mysql_nested_loop_cost() {
        let raw = json!({
            "query_block": {
                "select_id": 1,
                "cost_info": {"query_cost": "2.50"},
                "nested_loop": [
                    {"table": {"table_name": "books", "access_type": "ALL", "rows_examined_per_scan": 5, "cost_info": {"query_cost": "1.25"}}}
                ]
            }
        });
        let node = parse_plan("mysql", &["EXPLAIN".into()], &[vec![JsonValue::String(raw.to_string())]]);
        assert!(node.label.contains("查询"));
        assert!(node.children.iter().any(|child| child.children.iter().any(|table| table.label.contains("books"))));
        assert!(max_cost(&node) >= 1.25);
    }

    #[test]
    fn parses_postgres_plan_tree() {
        let raw = json!([{
            "Plan": {
                "Node Type": "Nested Loop",
                "Total Cost": 10.5,
                "Plan Rows": 4,
                "Plans": [
                    {"Node Type": "Seq Scan", "Relation Name": "books", "Total Cost": 3.0, "Plan Rows": 4}
                ]
            }
        }]);
        let node = parse_plan("postgres", &["QUERY PLAN".into()], &[vec![JsonValue::String(raw.to_string())]]);
        assert_eq!(node.label, "Nested Loop");
        assert_eq!(node.children[0].label, "Seq Scan books");
        assert_eq!(node.cost, Some(10.5));
    }

    #[test]
    fn parses_showplan_xml_ops() {
        let xml = r#"<ShowPlanXML><RelOp PhysicalOp="Clustered Index Scan" EstimatedTotalSubtreeCost="0.8" EstimateRows="12"></RelOp></ShowPlanXML>"#;
        let node = parse_plan("mssql", &["plan".into()], &[vec![JsonValue::String(xml.into())]]);
        assert_eq!(node.children[0].label, "Clustered Index Scan");
        assert_eq!(node.children[0].cost, Some(0.8));
    }
}
