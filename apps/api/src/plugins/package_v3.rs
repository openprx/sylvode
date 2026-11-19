//! Plugin manifest protocol 3: pure parsing and validation, with no database access.
//!
//! Protocol 1/2 manifests keep their structure and validation in [`super::package`]; this module
//! only adds the `openpr.plugin.v3` manifest, the v3 binding grant rules and the v3 layout helpers.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::package::{APPLY, Capability, MAX_AMOUNT_CENTS, Manifest, PREVIEW, Placement, Protocol, valid_label};
use crate::{
    error::{ApiError, ApiErrorKind},
    forms::schema::normalize_key,
};

pub const SCHEMA_V3: &str = "openpr.plugin.v3";
pub const UI_DISPLAY: &str = "ui.display";
pub const READ_GRANT_PREFIX: &str = "read:";

const SCHEMA_V2: &str = "openpr.plugin.v2";
const WASM_ABI: &str = "openpr.plugin.v1";
const ENTRY_PATH: &str = "panel.html";
const WASM_PATH: &str = "plugin.wasm";
const ENTRY_MAX_BYTES: usize = 64 * 1024;
const WASM_MAX_BYTES: usize = 256 * 1024;
const RESERVED_KEY: &str = "library";
const MAX_CONTRIBUTIONS: usize = 8;
const MAX_VIEWS: usize = 2;
const MAX_READS: usize = 4;
const MAX_FIELDS: usize = 32;
const MAX_POSITION: u8 = 99;
const DEFAULT_POSITION: u8 = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Profile {
    Ui,
    Wasm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
pub enum Slot {
    #[serde(rename = "form.record.detail")]
    FormRecordDetail,
    #[serde(rename = "form.record.actions")]
    FormRecordActions,
    #[serde(rename = "form.list.toolbar")]
    FormListToolbar,
    #[serde(rename = "project.view")]
    ProjectView,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Panel,
    Action,
    View,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EntryV3 {
    pub path: String,
    pub size: usize,
    pub sha256: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WasmV3 {
    pub abi: String,
    pub path: String,
    pub size: usize,
    pub sha256: String,
    pub form_key: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ContributionV3 {
    pub id: String,
    pub kind: Kind,
    pub slot: Slot,
    pub label: String,
    pub view: String,
    pub route: Option<String>,
    pub form_key: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReadV3 {
    pub id: String,
    pub form_key: String,
    pub fields: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestV3 {
    pub schema_version: String,
    pub key: String,
    pub name: String,
    pub version: String,
    pub publisher: String,
    pub host_protocol: Protocol,
    pub profile: Profile,
    pub bridge_version: u32,
    pub entry: EntryV3,
    pub wasm: Option<WasmV3>,
    pub contributions: Vec<ContributionV3>,
    pub reads: Vec<ReadV3>,
    pub capabilities: Vec<Capability>,
}

#[derive(Debug)]
pub enum AnyManifest {
    V2(Manifest),
    V3(ManifestV3),
}

/// Builds the v3 error envelope: `details.reason` carries the stable reason string.
pub fn plugin_error(kind: ApiErrorKind, reason: &'static str, message: &str) -> ApiError {
    ApiError::typed_with_details(kind, message, json!({ "reason": reason }))
}

fn error_with(reason: &'static str, message: &str, name: &str, value: &Value) -> ApiError {
    ApiError::typed_with_details(
        ApiErrorKind::InvalidUpdate,
        message,
        json!({ "reason": reason, name: value }),
    )
}

fn invalid(reason: &'static str, message: &str) -> ApiError {
    plugin_error(ApiErrorKind::InvalidUpdate, reason, message)
}

fn field_invalid(field: &'static str, message: &str) -> ApiError {
    error_with("manifest_field_invalid", message, "field", &json!(field))
}

fn contribution_invalid(index: usize, message: &str) -> ApiError {
    error_with("contributions_invalid", message, "index", &json!(index))
}

fn malformed(message: &str) -> ApiError {
    invalid("manifest_malformed", message)
}

fn unsupported(message: &str) -> ApiError {
    plugin_error(
        ApiErrorKind::UnsupportedProtocol,
        "manifest_protocol_unsupported",
        message,
    )
}

/// Parses a manifest and dispatches on `schema_version`. Performs no semantic validation.
pub fn parse_manifest(value: Value) -> Result<AnyManifest, ApiError> {
    let schema = value
        .get("schema_version")
        .and_then(Value::as_str)
        .ok_or_else(|| malformed("manifest requires a string schema_version"))?;
    match schema {
        SCHEMA_V2 => serde_json::from_value(value)
            .map(AnyManifest::V2)
            .map_err(|_| malformed("manifest does not match the plugin package schema")),
        SCHEMA_V3 => serde_json::from_value(value)
            .map(AnyManifest::V3)
            .map_err(|_| malformed("manifest does not match the plugin package schema")),
        _ => Err(unsupported("unsupported plugin manifest schema version")),
    }
}

fn is_key(value: &str, max_len: usize) -> bool {
    value.len() <= max_len && normalize_key(value).as_deref() == Ok(value)
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

impl ManifestV3 {
    /// Applies rules M1 through P3, C1 through C8 and R1 through R4.
    pub fn validate(&self) -> Result<(), ApiError> {
        self.validate_header()?;
        self.validate_entry()?;
        self.validate_profile()?;
        self.validate_contributions()?;
        self.validate_reads()?;
        self.validate_capability_target()
    }

    pub fn contribution(&self, id: &str) -> Option<&ContributionV3> {
        self.contributions.iter().find(|contribution| contribution.id == id)
    }

    pub fn read(&self, id: &str) -> Option<&ReadV3> {
        self.reads.iter().find(|read| read.id == id)
    }

    fn validate_header(&self) -> Result<(), ApiError> {
        if self.schema_version != SCHEMA_V3
            || self.host_protocol.min != 3
            || self.host_protocol.max != 3
            || self.bridge_version != 2
        {
            return Err(unsupported("unsupported plugin manifest or bridge protocol version"));
        }
        if !is_key(&self.key, 64) || self.key == RESERVED_KEY {
            return Err(field_invalid("key", "plugin key is invalid or reserved"));
        }
        if !valid_label(&self.name) {
            return Err(field_invalid("name", "plugin name must be a nonempty display label"));
        }
        if !valid_label(&self.publisher) {
            return Err(field_invalid(
                "publisher",
                "plugin publisher must be a nonempty display label",
            ));
        }
        if self.version.len() > 64 || semver::Version::parse(&self.version).is_err() {
            return Err(field_invalid(
                "version",
                "plugin version must be a SemVer release identifier",
            ));
        }
        Ok(())
    }

    fn validate_entry(&self) -> Result<(), ApiError> {
        if self.entry.path != ENTRY_PATH
            || self.entry.size == 0
            || self.entry.size > ENTRY_MAX_BYTES
            || !is_sha256_hex(&self.entry.sha256)
        {
            return Err(invalid("asset_invalid", "invalid plugin entry path, size, or SHA-256"));
        }
        Ok(())
    }

    fn validate_profile(&self) -> Result<(), ApiError> {
        match (self.profile, &self.wasm) {
            (Profile::Ui, None) => {
                if self.capabilities.is_empty() {
                    Ok(())
                } else {
                    Err(invalid("profile_mismatch", "ui profile must not declare capabilities"))
                }
            }
            (Profile::Ui, Some(_)) => Err(invalid("profile_mismatch", "ui profile must not declare a wasm asset")),
            (Profile::Wasm, None) => Err(invalid("profile_mismatch", "wasm profile requires a wasm asset")),
            (Profile::Wasm, Some(wasm)) => {
                if wasm.abi != WASM_ABI
                    || wasm.path != WASM_PATH
                    || wasm.size == 0
                    || wasm.size > WASM_MAX_BYTES
                    || !is_key(&wasm.form_key, 64)
                {
                    return Err(invalid(
                        "profile_mismatch",
                        "wasm asset ABI, path, size, or form key is invalid",
                    ));
                }
                if !is_sha256_hex(&wasm.sha256) {
                    return Err(invalid("asset_invalid", "invalid wasm SHA-256"));
                }
                self.validate_amount_capabilities()
            }
        }
    }

    fn validate_amount_capabilities(&self) -> Result<(), ApiError> {
        let mismatch = || {
            invalid(
                "profile_mismatch",
                "wasm profile requires the order amount preview and apply capabilities",
            )
        };
        if self.capabilities.len() != 2 {
            return Err(mismatch());
        }
        for (name, permission) in [(PREVIEW, "record.read"), (APPLY, "record.update")] {
            let capability = self
                .capabilities
                .iter()
                .find(|item| item.name == name)
                .ok_or_else(mismatch)?;
            if capability.permission != permission
                || capability.input_schema != json!({"type":"object","additionalProperties":false})
                || capability.output_schema
                    != json!({
                        "type":"object","additionalProperties":false,"required":["amount_cents"],
                        "properties":{"amount_cents":{"type":"integer","minimum":0,"maximum":MAX_AMOUNT_CENTS}}
                    })
            {
                return Err(mismatch());
            }
        }
        Ok(())
    }

    fn validate_contributions(&self) -> Result<(), ApiError> {
        if self.contributions.is_empty() || self.contributions.len() > MAX_CONTRIBUTIONS {
            return Err(error_with(
                "contributions_invalid",
                "a package must declare between 1 and 8 contributions",
                "index",
                &Value::Null,
            ));
        }
        let mut ids = HashSet::new();
        let mut routes = HashSet::new();
        let mut targets = HashSet::new();
        let mut views = 0;
        for (index, item) in self.contributions.iter().enumerate() {
            if !is_key(&item.id, 64) || !ids.insert(item.id.as_str()) {
                return Err(contribution_invalid(index, "contribution ID is invalid or duplicated"));
            }
            if !matches!(
                (item.slot, item.kind),
                (Slot::FormRecordDetail, Kind::Panel)
                    | (Slot::FormRecordActions | Slot::FormListToolbar, Kind::Action)
                    | (Slot::ProjectView, Kind::View)
            ) {
                return Err(contribution_invalid(index, "contribution slot and kind do not match"));
            }
            if !valid_label(&item.label) {
                return Err(contribution_invalid(index, "contribution label is invalid"));
            }
            if !is_key(&item.view, 64) {
                return Err(contribution_invalid(index, "contribution view name is invalid"));
            }
            let target = match (item.slot, item.route.as_deref(), item.form_key.as_deref()) {
                (Slot::ProjectView, Some(route), None) if is_key(route, 32) => {
                    views += 1;
                    if !routes.insert(route) {
                        return Err(contribution_invalid(index, "project view route is duplicated"));
                    }
                    route
                }
                (Slot::ProjectView, ..) => {
                    return Err(contribution_invalid(
                        index,
                        "project views require a route and no form key",
                    ));
                }
                (_, None, Some(form_key)) if is_key(form_key, 64) => form_key,
                _ => {
                    return Err(contribution_invalid(
                        index,
                        "record and list contributions require a form key and no route",
                    ));
                }
            };
            if views > MAX_VIEWS {
                return Err(contribution_invalid(
                    index,
                    "a package may declare at most two project views",
                ));
            }
            if !targets.insert((item.slot, target)) {
                return Err(contribution_invalid(
                    index,
                    "a slot and target may only be contributed once per package",
                ));
            }
        }
        Ok(())
    }

    fn validate_reads(&self) -> Result<(), ApiError> {
        let reads_invalid = |message: &str| invalid("reads_invalid", message);
        if self.reads.len() > MAX_READS {
            return Err(reads_invalid("a package may declare at most four reads"));
        }
        let mut ids = HashSet::new();
        for read in &self.reads {
            if !is_key(&read.id, 32) || !ids.insert(read.id.as_str()) {
                return Err(reads_invalid("read ID is invalid or duplicated"));
            }
            if !is_key(&read.form_key, 64) {
                return Err(reads_invalid("read form key is invalid"));
            }
            let mut fields = HashSet::new();
            if read.fields.is_empty()
                || read.fields.len() > MAX_FIELDS
                || !read
                    .fields
                    .iter()
                    .all(|field| is_key(field, 64) && fields.insert(field.as_str()))
            {
                return Err(reads_invalid("read fields must be 1 to 32 unique field keys"));
            }
        }
        Ok(())
    }

    fn validate_capability_target(&self) -> Result<(), ApiError> {
        let Some(wasm) = &self.wasm else {
            return Ok(());
        };
        let targeted = self.contributions.iter().any(|item| {
            matches!(item.slot, Slot::FormRecordDetail | Slot::FormRecordActions)
                && item.form_key.as_deref() == Some(wasm.form_key.as_str())
        });
        if targeted {
            Ok(())
        } else {
            Err(invalid(
                "capability_target_missing",
                "a wasm package needs a record contribution for the wasm form key",
            ))
        }
    }
}

/// Sorts `grants` and applies the §2.5.3 grant rules. `manifest == None` means unbinding.
pub fn validate_v3_grants(manifest: Option<&ManifestV3>, grants: &mut [String]) -> Result<(), ApiError> {
    let reject = |message: &str| invalid("binding_grants_invalid", message);
    grants.sort_unstable();
    if grants
        .windows(2)
        .any(|pair| matches!(pair, [left, right] if left == right))
    {
        return Err(reject("grants must not contain duplicates"));
    }
    let Some(manifest) = manifest else {
        return if grants.is_empty() {
            Ok(())
        } else {
            Err(reject("grants must be empty without a package"))
        };
    };
    let allowed = |grant: &str| {
        grant == UI_DISPLAY
            || grant
                .strip_prefix(READ_GRANT_PREFIX)
                .is_some_and(|id| manifest.read(id).is_some())
            || manifest.capabilities.iter().any(|capability| capability.name == grant)
    };
    if !grants.iter().all(|grant| allowed(grant)) {
        return Err(reject("grant is not declared by the package"));
    }
    let has = |name: &str| grants.iter().any(|grant| grant == name);
    if !grants.is_empty() && !has(UI_DISPLAY) {
        return Err(reject("non-empty grants must include ui.display"));
    }
    if has(APPLY) && !has(PREVIEW) {
        return Err(reject("amount.apply requires amount.preview"));
    }
    Ok(())
}

/// Sorts `placements` by ID and requires exactly one in-range entry per declared contribution.
pub fn normalize_v3_placements(manifest: &ManifestV3, placements: &mut [Placement]) -> Result<(), ApiError> {
    placements.sort_unstable_by(|left, right| left.id.cmp(&right.id));
    let valid = placements.len() == manifest.contributions.len()
        && placements
            .windows(2)
            .all(|pair| matches!(pair, [left, right] if left.id != right.id))
        && placements
            .iter()
            .all(|placement| placement.position <= MAX_POSITION && manifest.contribution(&placement.id).is_some());
    if valid {
        Ok(())
    } else {
        Err(invalid(
            "placements_invalid",
            "layout must configure each declared contribution exactly once with position between 0 and 99",
        ))
    }
}

/// Merges stored rows into the declared contributions: undeclared rows are dropped and missing
/// ones default to enabled at position 50. The result is sorted by ID.
pub fn merge_v3_layout(manifest: &ManifestV3, stored: &[Placement]) -> Vec<Placement> {
    let mut merged: Vec<_> = manifest
        .contributions
        .iter()
        .map(|contribution| {
            stored
                .iter()
                .find(|placement| placement.id == contribution.id)
                .cloned()
                .unwrap_or_else(|| Placement {
                    id: contribution.id.clone(),
                    enabled: true,
                    position: DEFAULT_POSITION,
                })
        })
        .collect();
    merged.sort_unstable_by(|left, right| left.id.cmp(&right.id));
    merged
}

#[cfg(test)]
#[allow(clippy::semicolon_if_nothing_returned, clippy::type_complexity)]
mod tests {
    use super::*;

    const HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn amount_capability(name: &str, permission: &str) -> Value {
        json!({
            "name": name, "permission": permission,
            "input_schema": {"type":"object","additionalProperties":false},
            "output_schema": {
                "type":"object","additionalProperties":false,"required":["amount_cents"],
                "properties":{"amount_cents":{"type":"integer","minimum":0,"maximum":MAX_AMOUNT_CENTS}}
            }
        })
    }

    fn ui_json() -> Value {
        json!({
            "schema_version": SCHEMA_V3, "key": "order_board", "name": "Order board", "version": "1.0.0",
            "publisher": "Project author", "host_protocol": {"min":3,"max":3}, "profile": "ui",
            "bridge_version": 2,
            "entry": {"path":"panel.html","size":5120,"sha256":HASH},
            "wasm": null,
            "contributions": [
                {"id":"board","kind":"view","slot":"project.view","label":"Order board","view":"board","route":"board","form_key":null},
                {"id":"record_summary","kind":"panel","slot":"form.record.detail","label":"Order summary","view":"summary","route":null,"form_key":"order_line"},
                {"id":"list_tool","kind":"action","slot":"form.list.toolbar","label":"Order totals","view":"totals","route":null,"form_key":"order_line"}
            ],
            "reads": [{"id":"orders","form_key":"order_line","fields":["quantity","unit_price","amount"]}],
            "capabilities": []
        })
    }

    fn wasm_json() -> Value {
        let mut value = ui_json();
        *slot(&mut value, "/profile") = json!("wasm");
        *slot(&mut value, "/wasm") =
            json!({"abi":"openpr.plugin.v1","path":"plugin.wasm","size":4492,"sha256":HASH,"form_key":"order_line"});
        *slot(&mut value, "/capabilities") = json!([
            amount_capability(PREVIEW, "record.read"),
            amount_capability(APPLY, "record.update")
        ]);
        value
    }

    fn slot<'a>(value: &'a mut Value, pointer: &str) -> &'a mut Value {
        value.pointer_mut(pointer).expect("test manifest path exists")
    }

    fn insert(value: &mut Value, pointer: &str, key: &str) {
        if let Some(object) = slot(value, pointer).as_object_mut() {
            object.insert(key.to_owned(), json!(true));
        }
    }

    fn parse_v3(value: Value) -> ManifestV3 {
        match parse_manifest(value) {
            Ok(AnyManifest::V3(manifest)) => manifest,
            other => panic!("expected a v3 manifest, got {other:?}"),
        }
    }

    fn reason(error: ApiError) -> String {
        let ApiError::Typed { details, .. } = error else {
            panic!("expected a typed error");
        };
        details
            .and_then(|details| details.get("reason").and_then(Value::as_str).map(str::to_owned))
            .unwrap_or_default()
    }

    fn detail(error: &ApiError, name: &str) -> Value {
        match error {
            ApiError::Typed {
                details: Some(details), ..
            } => details.get(name).cloned().unwrap_or(Value::Null),
            _ => Value::Null,
        }
    }

    fn check(value: Value) -> Result<(), ApiError> {
        parse_v3(value).validate()
    }

    fn assert_reason(value: Value, expected: &str) {
        match check(value) {
            Err(error) => assert_eq!(reason(error), expected),
            Ok(()) => panic!("expected rejection with {expected}"),
        }
    }

    fn mutated(mut base: Value, edit: impl FnOnce(&mut Value)) -> Value {
        edit(&mut base);
        base
    }

    #[test]
    fn valid_samples_pass() {
        assert!(check(ui_json()).is_ok());
        assert!(check(wasm_json()).is_ok());
    }

    #[test]
    fn m1_protocol() {
        for edit in [
            |v: &mut Value| *slot(v, "/host_protocol/max") = json!(4),
            |v: &mut Value| *slot(v, "/host_protocol/min") = json!(2),
            |v: &mut Value| *slot(v, "/bridge_version") = json!(1),
        ] {
            assert_reason(mutated(ui_json(), edit), "manifest_protocol_unsupported");
        }
        let error = parse_manifest(mutated(ui_json(), |v| {
            *slot(v, "/schema_version") = json!("openpr.plugin.v9")
        }))
        .expect_err("unknown schema is unsupported");
        assert_eq!(reason(error), "manifest_protocol_unsupported");
    }

    #[test]
    fn m2_fields() {
        let cases: [(&str, fn(&mut Value)); 7] = [
            ("key", |v| *slot(v, "/key") = json!("Bad-Key")),
            ("key", |v| *slot(v, "/key") = json!("library")),
            ("key", |v| *slot(v, "/key") = json!("a".repeat(65))),
            ("name", |v| *slot(v, "/name") = json!(" padded")),
            ("publisher", |v| *slot(v, "/publisher") = json!("")),
            ("version", |v| *slot(v, "/version") = json!("1.0")),
            ("version", |v| {
                *slot(v, "/version") = json!(format!("1.0.0-{}", "a".repeat(64)))
            }),
        ];
        for (field, edit) in cases {
            let error = check(mutated(ui_json(), edit)).expect_err("field rejected");
            assert_eq!(detail(&error, "field"), json!(field));
            assert_eq!(reason(error), "manifest_field_invalid");
        }
        assert!(check(mutated(ui_json(), |v| *slot(v, "/key") = json!("a".repeat(64)))).is_ok());
    }

    #[test]
    fn m3_entry() {
        let cases: [fn(&mut Value); 5] = [
            |v| *slot(v, "/entry/path") = json!("other.html"),
            |v| *slot(v, "/entry/size") = json!(0),
            |v| *slot(v, "/entry/size") = json!(65537),
            |v| *slot(v, "/entry/sha256") = json!("ABCDEF"),
            |v| *slot(v, "/entry/sha256") = json!(HASH.to_uppercase()),
        ];
        for edit in cases {
            assert_reason(mutated(ui_json(), edit), "asset_invalid");
        }
        assert!(check(mutated(ui_json(), |v| *slot(v, "/entry/size") = json!(65536))).is_ok());
    }

    #[test]
    fn p1_ui_profile() {
        assert_reason(
            mutated(
                ui_json(),
                |v| *slot(v, "/wasm") = json!({"abi":"openpr.plugin.v1","path":"plugin.wasm","size":1,"sha256":HASH,"form_key":"order_line"}),
            ),
            "profile_mismatch",
        );
        assert_reason(
            mutated(ui_json(), |v| {
                *slot(v, "/capabilities") = json!([amount_capability(PREVIEW, "record.read")])
            }),
            "profile_mismatch",
        );
    }

    #[test]
    fn p2_wasm_profile() {
        let cases: [fn(&mut Value); 7] = [
            |v| *slot(v, "/wasm") = Value::Null,
            |v| *slot(v, "/wasm/abi") = json!("other"),
            |v| *slot(v, "/wasm/path") = json!("x.wasm"),
            |v| *slot(v, "/wasm/size") = json!(262_145),
            |v| *slot(v, "/wasm/form_key") = json!("Bad Key"),
            |v| *slot(v, "/capabilities") = json!([]),
            |v| *slot(v, "/capabilities/1/permission") = json!("record.read"),
        ];
        for edit in cases {
            assert_reason(mutated(wasm_json(), edit), "profile_mismatch");
        }
        assert_reason(
            mutated(wasm_json(), |v| *slot(v, "/wasm/sha256") = json!("zz")),
            "asset_invalid",
        );
        assert!(check(mutated(wasm_json(), |v| *slot(v, "/wasm/size") = json!(262_144))).is_ok());
    }

    #[test]
    fn p3_capability_target() {
        assert_reason(
            mutated(wasm_json(), |v| *slot(v, "/wasm/form_key") = json!("other_form")),
            "capability_target_missing",
        );
        // A list toolbar contribution alone does not satisfy the target rule.
        assert_reason(
            mutated(wasm_json(), |v| {
                *slot(v, "/contributions/1/slot") = json!("form.list.toolbar");
                *slot(v, "/contributions/1/kind") = json!("action");
                *slot(v, "/contributions/1/form_key") = json!("other_form");
            }),
            "capability_target_missing",
        );
        assert!(check(wasm_json()).is_ok());
    }

    fn contribution_error(edit: impl FnOnce(&mut Value)) -> ApiError {
        check(mutated(ui_json(), edit)).expect_err("contributions rejected")
    }

    #[test]
    fn c1_count() {
        let error = contribution_error(|v| *slot(v, "/contributions") = json!([]));
        assert_eq!(reason(error), "contributions_invalid");
        let nine = mutated(ui_json(), |v| {
            *slot(v, "/contributions") = Value::Array(
                (0..9)
                    .map(|i| {
                        json!({"id":format!("c{i}"),"kind":"action","slot":"form.list.toolbar","label":"L","view":"v","route":null,"form_key":format!("f{i}")})
                    })
                    .collect(),
            )
        });
        assert_reason(nine, "contributions_invalid");
        let eight = mutated(ui_json(), |v| {
            *slot(v, "/contributions") = Value::Array(
                (0..8)
                    .map(|i| {
                        json!({"id":format!("c{i}"),"kind":"action","slot":"form.list.toolbar","label":"L","view":"v","route":null,"form_key":format!("f{i}")})
                    })
                    .collect(),
            )
        });
        assert!(check(eight).is_ok());
    }

    #[test]
    fn c2_to_c8_contributions() {
        let cases: [(usize, fn(&mut Value)); 11] = [
            (1, |v| *slot(v, "/contributions/1/id") = json!("Bad")),
            (2, |v| *slot(v, "/contributions/2/id") = json!("board")),
            (0, |v| *slot(v, "/contributions/0/kind") = json!("panel")),
            (1, |v| *slot(v, "/contributions/1/slot") = json!("form.record.actions")),
            (1, |v| *slot(v, "/contributions/1/label") = json!("")),
            (1, |v| *slot(v, "/contributions/1/view") = json!("Bad View")),
            (0, |v| *slot(v, "/contributions/0/form_key") = json!("order_line")),
            (0, |v| *slot(v, "/contributions/0/route") = json!("x".repeat(33))),
            (1, |v| *slot(v, "/contributions/1/route") = json!("board")),
            (1, |v| *slot(v, "/contributions/1/form_key") = Value::Null),
            (2, |v| *slot(v, "/contributions/2/slot") = json!("form.record.detail")),
        ];
        for (index, edit) in cases {
            let error = contribution_error(edit);
            assert_eq!(detail(&error, "index"), json!(index));
            assert_eq!(reason(error), "contributions_invalid");
        }
    }

    #[test]
    fn c6_to_c8_views() {
        let view = |id: &str, route: &str| json!({"id":id,"kind":"view","slot":"project.view","label":"V","view":"v","route":route,"form_key":null});
        // C6: routes must be unique within the package.
        let dup = mutated(ui_json(), |v| {
            *slot(v, "/contributions") = json!([view("a", "same"), view("b", "same")])
        });
        assert_reason(dup, "contributions_invalid");
        // C7: at most two project views.
        let three = mutated(ui_json(), |v| {
            *slot(v, "/contributions") = json!([view("a", "one"), view("b", "two"), view("c", "three")])
        });
        assert_reason(three, "contributions_invalid");
        let two = mutated(ui_json(), |v| {
            *slot(v, "/contributions") = json!([view("a", "one"), view("b", "two")])
        });
        assert!(check(two).is_ok());
        // C6 positive: route of exactly 32 bytes.
        let long = mutated(ui_json(), |v| {
            *slot(v, "/contributions") = json!([view("a", &"r".repeat(32))])
        });
        assert!(check(long).is_ok());
    }

    #[test]
    fn c8_slot_target_unique() {
        let toolbar = |id: &str, form: &str| json!({"id":id,"kind":"action","slot":"form.list.toolbar","label":"T","view":"v","route":null,"form_key":form});
        let dup = mutated(ui_json(), |v| {
            *slot(v, "/contributions") = json!([toolbar("a", "f"), toolbar("b", "f")])
        });
        assert_reason(dup, "contributions_invalid");
        let distinct = mutated(ui_json(), |v| {
            *slot(v, "/contributions") = json!([toolbar("a", "f"), toolbar("b", "g")])
        });
        assert!(check(distinct).is_ok());
    }

    #[test]
    fn r1_to_r4_reads() {
        let read = |id: &str| json!({"id":id,"form_key":"f","fields":["a"]});
        let five = mutated(ui_json(), |v| {
            *slot(v, "/reads") = json!([read("a"), read("b"), read("c"), read("d"), read("e")])
        });
        assert_reason(five, "reads_invalid");
        let four = mutated(ui_json(), |v| {
            *slot(v, "/reads") = json!([read("a"), read("b"), read("c"), read("d")])
        });
        assert!(check(four).is_ok());
        assert!(check(mutated(ui_json(), |v| *slot(v, "/reads") = json!([]))).is_ok());
        let cases: [fn(&mut Value); 8] = [
            |v| *slot(v, "/reads/0/id") = json!("Bad"),
            |v| *slot(v, "/reads/0/id") = json!("i".repeat(33)),
            |v| {
                *slot(v, "/reads") =
                    json!([{"id":"a","form_key":"f","fields":["x"]},{"id":"a","form_key":"f","fields":["x"]}])
            },
            |v| *slot(v, "/reads/0/form_key") = json!("Bad Form"),
            |v| *slot(v, "/reads/0/fields") = json!([]),
            |v| *slot(v, "/reads/0/fields") = Value::Array((0..33).map(|i| json!(format!("f{i}"))).collect()),
            |v| *slot(v, "/reads/0/fields") = json!(["a", "a"]),
            |v| *slot(v, "/reads/0/fields") = json!(["Bad Field"]),
        ];
        for edit in cases {
            assert_reason(mutated(ui_json(), edit), "reads_invalid");
        }
        let max = mutated(ui_json(), |v| {
            *slot(v, "/reads/0/fields") = Value::Array((0..32).map(|i| json!(format!("f{i}"))).collect())
        });
        assert!(check(max).is_ok());
    }

    #[test]
    fn x1_malformed() {
        let cases: [fn(&mut Value); 5] = [
            |v| insert(v, "", "extra"),
            |v| {
                v.as_object_mut().map(|o| o.remove("reads"));
            },
            |v| *slot(v, "/bridge_version") = json!("2"),
            |v| *slot(v, "/contributions/0/slot") = json!("project.other"),
            |v| insert(v, "/entry", "unknown"),
        ];
        for edit in cases {
            let error = parse_manifest(mutated(ui_json(), edit)).expect_err("malformed manifest");
            assert_eq!(reason(error), "manifest_malformed");
        }
        let error = parse_manifest(mutated(ui_json(), |v| {
            v.as_object_mut().map(|o| o.remove("schema_version"));
        }))
        .expect_err("missing schema version");
        assert_eq!(reason(error), "manifest_malformed");
    }

    #[test]
    fn v2_sample_still_parses_and_validates() {
        // Manifest of the order-amount 3.0.0 sample package (protocol 2).
        let manifest: Value = serde_json::from_str(
            r#"{
    "schema_version": "openpr.plugin.v2",
    "key": "order_amount",
    "name": "Order amount controls",
    "version": "3.0.0",
    "publisher": "Sylvode examples",
    "host_protocol": {
        "min": 2,
        "max": 2
    },
    "wasm_abi": "openpr.plugin.v1",
    "wasm": {
        "path": "plugin.wasm",
        "size": 4492,
        "sha256": "b6e4451d420835fe6197ddca5d43962241b7595eca31757fb1c0106a99880dc2"
    },
    "ui": {
        "entry": {
            "path": "panel.html",
            "size": 4674,
            "sha256": "2a99e572d6fd5c6443326d82b0589ec43b14edb1954633dfb58370cda580e856"
        },
        "slot": "form.record.detail",
        "bridge_version": 1,
        "form_key": "order_line",
        "contributions": [
            {
                "id": "detail_panel",
                "slot": "form.record.detail",
                "label": "Order amount panel"
            },
            {
                "id": "amount_action",
                "slot": "form.record.actions",
                "label": "Calculate order amount"
            }
        ]
    },
    "capabilities": [
        {
            "name": "amount.preview",
            "permission": "record.read",
            "input_schema": {
                "type": "object",
                "additionalProperties": false
            },
            "output_schema": {
                "type": "object",
                "properties": {
                    "amount_cents": {
                        "type": "integer",
                        "minimum": 0,
                        "maximum": 1000000000000
                    }
                },
                "required": [
                    "amount_cents"
                ],
                "additionalProperties": false
            }
        },
        {
            "name": "amount.apply",
            "permission": "record.update",
            "input_schema": {
                "type": "object",
                "additionalProperties": false
            },
            "output_schema": {
                "type": "object",
                "properties": {
                    "amount_cents": {
                        "type": "integer",
                        "minimum": 0,
                        "maximum": 1000000000000
                    }
                },
                "required": [
                    "amount_cents"
                ],
                "additionalProperties": false
            }
        }
    ]
}"#,
        )
        .expect("sample manifest is JSON");
        match parse_manifest(manifest) {
            Ok(AnyManifest::V2(manifest)) => assert!(manifest.validate().is_ok()),
            other => panic!("expected a v2 manifest, got {other:?}"),
        }
    }

    #[test]
    fn lookup_helpers() {
        let manifest = parse_v3(ui_json());
        assert_eq!(manifest.contribution("board").map(|c| c.slot), Some(Slot::ProjectView));
        assert!(manifest.contribution("missing").is_none());
        assert!(manifest.read("orders").is_some());
        assert!(manifest.read("missing").is_none());
    }

    fn grants_reason(manifest: Option<&ManifestV3>, grants: &[&str]) -> Option<String> {
        let mut grants: Vec<String> = grants.iter().map(|grant| (*grant).to_owned()).collect();
        validate_v3_grants(manifest, &mut grants).err().map(reason)
    }

    #[test]
    fn grants_rules() {
        let ui = parse_v3(ui_json());
        let wasm = parse_v3(wasm_json());
        // Positive, and sorted in place.
        let mut grants = vec!["read:orders".to_owned(), UI_DISPLAY.to_owned()];
        assert!(validate_v3_grants(Some(&ui), &mut grants).is_ok());
        assert_eq!(grants, ["read:orders", UI_DISPLAY]);
        assert_eq!(grants_reason(Some(&ui), &[]), None);
        assert_eq!(grants_reason(Some(&wasm), &[APPLY, PREVIEW, UI_DISPLAY]), None);
        assert_eq!(grants_reason(None, &[]), None);
        // 1: duplicates.
        assert_eq!(
            grants_reason(Some(&ui), &[UI_DISPLAY, UI_DISPLAY]).as_deref(),
            Some("binding_grants_invalid")
        );
        // 2: undeclared grants.
        for grants in [
            &[UI_DISPLAY, "read:other"][..],
            &[UI_DISPLAY, PREVIEW],
            &[UI_DISPLAY, "bogus"],
        ] {
            assert_eq!(
                grants_reason(Some(&ui), grants).as_deref(),
                Some("binding_grants_invalid")
            );
        }
        // 3: ui.display required.
        assert_eq!(
            grants_reason(Some(&ui), &["read:orders"]).as_deref(),
            Some("binding_grants_invalid")
        );
        // 4: apply requires preview.
        assert_eq!(
            grants_reason(Some(&wasm), &[APPLY, UI_DISPLAY]).as_deref(),
            Some("binding_grants_invalid")
        );
        // 5: no package, no grants.
        assert_eq!(
            grants_reason(None, &[UI_DISPLAY]).as_deref(),
            Some("binding_grants_invalid")
        );
    }

    fn placement(id: &str, enabled: bool, position: u8) -> Placement {
        Placement {
            id: id.to_owned(),
            enabled,
            position,
        }
    }

    #[test]
    fn placements_rules() {
        let manifest = parse_v3(ui_json());
        let mut complete = vec![
            placement("record_summary", true, 99),
            placement("board", false, 0),
            placement("list_tool", true, 50),
        ];
        assert!(normalize_v3_placements(&manifest, &mut complete).is_ok());
        assert_eq!(
            complete.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
            ["board", "list_tool", "record_summary"]
        );
        let bad: [Vec<Placement>; 5] = [
            vec![placement("board", true, 1), placement("list_tool", true, 1)],
            vec![
                placement("board", true, 1),
                placement("board", true, 2),
                placement("list_tool", true, 1),
            ],
            vec![
                placement("board", true, 1),
                placement("list_tool", true, 1),
                placement("record_summary", true, 100),
            ],
            vec![
                placement("board", true, 1),
                placement("list_tool", true, 1),
                placement("unknown", true, 1),
            ],
            vec![],
        ];
        for mut placements in bad {
            let error = normalize_v3_placements(&manifest, &mut placements).expect_err("placements rejected");
            assert_eq!(reason(error), "placements_invalid");
        }
    }

    #[test]
    fn merge_layout_defaults_and_sorting() {
        let manifest = parse_v3(ui_json());
        let stored = [
            placement("record_summary", false, 10),
            placement("undeclared_row", true, 1),
        ];
        let merged = merge_v3_layout(&manifest, &stored);
        let summary: Vec<_> = merged.iter().map(|p| (p.id.as_str(), p.enabled, p.position)).collect();
        assert_eq!(
            summary,
            [
                ("board", true, 50),
                ("list_tool", true, 50),
                ("record_summary", false, 10)
            ]
        );
        assert_eq!(merge_v3_layout(&manifest, &[]).len(), 3);
    }

    #[test]
    fn plugin_error_envelope() {
        let error = plugin_error(ApiErrorKind::Forbidden, "data_read_not_granted", "denied");
        assert_eq!(error.kind(), ApiErrorKind::Forbidden);
        assert_eq!(reason(error), "data_read_not_granted");
    }
}
