#[path = "../src/flow/page_transfer_contract.rs"]
#[allow(dead_code)]
mod contract;
#[path = "../src/flow/page_transfer_prepare.rs"]
#[allow(dead_code)]
mod prepare;
#[test]
fn requests_never_accept_raw_snapshots_or_fake_server_unknown() {
    let value = serde_json::json!({"kind":"move","target_object_id":"11111111-1111-4111-8111-111111111111","root_block_cid":"cid:x:Map","selection":"block","target_parent_block_cid":null,"target_index":0,"expected_source_frontier":"AA==","expected_target_frontier":"AA==","source_snapshot":"forbidden"});
    assert!(serde_json::from_value::<contract::PrepareInput>(value).is_err());
    assert!(serde_json::from_str::<contract::State>("\"unknown\"").is_err());
}
