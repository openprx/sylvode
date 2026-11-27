//! Bad fixture: an `action` that never returns. The host must stop it (fuel, then the epoch
//! deadline) with `plugin_fuel_exhausted` or `plugin_deadline_exceeded`.

wit_bindgen::generate!({
    path: "../../../../wit/sv-plugin.wit",
    world: "action",
});

struct Plugin;

impl Guest for Plugin {
    fn run(_action: String, _input: String) -> Result<String, ActionError> {
        let mut counter: u64 = 0;
        loop {
            counter = std::hint::black_box(counter.wrapping_add(1));
        }
    }
}

export!(Plugin);
