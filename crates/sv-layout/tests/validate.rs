use serde_json::{Value, json};
use sv_layout::{ErrorCode, analyse, descriptors, slot, validate};

fn example() -> Value {
    json!({
      "schema": "sv.layout.v1",
      "key": "service.request.summary",
      "title": {"en": "Request summary", "zh": "x"},
      "params": {"record_id": {"type": "string", "required": true}},
      "data": {
        "req": {"source": "record", "form": "$bind.requests", "record": "$params.record_id",
                "fields": ["title", "status"]},
        "notes": {"source": "query", "form": "$bind.notes",
                  "filter": [{"field": "request_id", "op": "eq", "value": "$params.record_id"}],
                  "sort": [{"field": "created_at", "dir": "desc"}], "fields": ["body"], "page_size": 20},
        "form": {"source": "form", "form": "$bind.requests", "fields": ["title"]}
      },
      "actions": {
        "close": {"kind": "invoke", "package": "sv/service-desk", "export": "close_request",
                  "input": {"record_id": "$params.record_id"}, "confirm": {"en": "Close?"}},
        "send": {"kind": "submit", "data": "form", "mode": "create"}
      },
      "root": {"type": "stack", "props": {"gap": "md"}, "children": [
        {"type": "stat", "props": {"label": {"en": "Status"}, "value": "$data.req.values.status"}},
        {"type": "table", "props": {"rows": "$data.notes.items", "columns": [{"field": "body"}]}},
        {"type": "form", "props": {"data": "form", "submit": "send"}},
        {"type": "button", "when": "data.req.values.status != 'closed'",
         "props": {"label": {"en": "Close"}, "variant": "danger"}, "on_click": "close"}
      ]}
    })
}

fn codes(v: &Value) -> Vec<(String, ErrorCode)> {
    match validate(v, None) {
        Ok(_) => vec![],
        Err(e) => e.into_iter().map(|e| (e.path, e.code)).collect(),
    }
}

#[test]
fn contract_example_is_valid_and_analysed() {
    let ok = validate(&example(), None).expect("valid");
    let a = analyse(&ok.layout);
    assert_eq!(a.packages, vec!["sv/service-desk"]);
    assert!(
        a.capabilities
            .iter()
            .any(|c| c.id == "forms.records.read" && c.target == "$bind.requests")
    );
    assert!(a.capabilities.iter().any(|c| c.id == "forms.records.create"));
    assert_eq!(descriptors().len(), 12);
}

#[test]
fn bad_props_carry_the_node_path() {
    let mut v = example();
    v["root"]["children"][1]["props"]["columns"] = json!("nope");
    let e = codes(&v);
    assert!(
        e.iter()
            .any(|(p, c)| p.starts_with("/root/children/1/props") && *c == ErrorCode::PropsInvalid),
        "{e:?}"
    );
}

#[test]
fn unknown_component_and_children_rules() {
    let mut v = example();
    v["root"]["children"][0]["type"] = json!("carousel");
    v["root"]["children"][3]["children"] = json!([{"type": "text", "props": {"value": "x"}}]);
    let e = codes(&v);
    assert!(
        e.contains(&("/root/children/0/type".into(), ErrorCode::NodeTypeUnknown)),
        "{e:?}"
    );
    assert!(
        e.contains(&("/root/children/3/children".into(), ErrorCode::ChildrenInvalid)),
        "{e:?}"
    );
}

#[test]
fn references_and_actions_are_resolved() {
    let mut v = example();
    v["root"]["children"][0]["props"]["value"] = json!("$data.nope.x");
    v["root"]["children"][3]["on_click"] = json!("missing");
    v["root"]["children"][2]["props"]["submit"] = json!("close");
    let e = codes(&v);
    assert!(
        e.contains(&("/root/children/0/props/value".into(), ErrorCode::RefUnknownSource)),
        "{e:?}"
    );
    assert!(
        e.contains(&("/root/children/3/on_click".into(), ErrorCode::ActionUnknown)),
        "{e:?}"
    );
    assert!(
        e.contains(&("/root/children/2/props/submit".into(), ErrorCode::ActionUnknown)),
        "{e:?}"
    );
}

#[test]
fn limits_and_schema() {
    let mut v = example();
    let mut node = json!({"type": "text", "props": {"value": "x"}});
    for _ in 0..30 {
        node = json!({"type": "stack", "children": [node]});
    }
    v["root"] = node;
    assert!(codes(&v).iter().any(|(_, c)| *c == ErrorCode::LimitExceeded));
    let mut w = example();
    w["schema"] = json!("sv.layout.v2");
    assert_eq!(codes(&w), vec![("/schema".to_owned(), ErrorCode::SchemaUnsupported)]);
    let mut x = example();
    x["data"]["req"] = json!({"source": "plugin", "package": "a/b", "export": "c"});
    assert!(codes(&x).iter().any(|(_, c)| *c == ErrorCode::SourceNotAvailable));
    let mut y = example();
    y["data"]["req"]["form"] = json!("0b2f0c3e-1111-2222-3333-444455556666");
    assert!(
        codes(&y)
            .iter()
            .any(|(p, c)| p == "/data/req/form" && *c == ErrorCode::RefInvalid)
    );
}

#[test]
fn slot_restrictions() {
    let s = slot("form.record.actions").expect("slot");
    let e = validate(&example(), Some(&s)).expect_err("stack with non-button children");
    assert!(e.iter().any(|e| e.code == ErrorCode::SlotNodeNotAllowed));
    let w = slot("project.home.widgets").expect("slot");
    let e = validate(&example(), Some(&w)).expect_err("stack root");
    assert!(
        e.iter()
            .any(|e| e.path == "/root/type" && e.code == ErrorCode::SlotNodeNotAllowed)
    );
}
