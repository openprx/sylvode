//! Bad fixture: an `action` that makes 300 host calls and ignores every answer. The host must stop
//! it at the 201st call with `plugin_host_call_limit`. The calls name an operation that does not
//! exist, so they are refused by the host without touching the database.

wit_bindgen::generate!({
    path: "../../../../wit/sv-plugin.wit",
    world: "action",
});

use sv::plugin::catalogue;

struct Plugin;

impl Guest for Plugin {
    fn run(_action: String, _input: String) -> Result<String, ActionError> {
        for _ in 0..300 {
            let _ = catalogue::call("no.such.operation", "{}");
        }
        Ok("{\"calls\":300}".to_string())
    }
}

export!(Plugin);
