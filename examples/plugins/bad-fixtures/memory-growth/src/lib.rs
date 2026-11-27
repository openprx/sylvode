//! Bad fixture: an `action` that keeps allocating. The host must stop it with `plugin_memory_limit`.

wit_bindgen::generate!({
    path: "../../../../wit/sv-plugin.wit",
    world: "action",
});

struct Plugin;

impl Guest for Plugin {
    fn run(_action: String, _input: String) -> Result<String, ActionError> {
        let mut held: Vec<Vec<u8>> = Vec::new();
        loop {
            // Four megabytes of capacity at a time, with one byte written so the allocation is
            // real: the allocator asks the host for pages, which is what the memory limit counts,
            // and the loop stays cheap in fuel.
            let mut chunk: Vec<u8> = Vec::with_capacity(4 << 20);
            chunk.push(1);
            held.push(std::hint::black_box(chunk));
        }
    }
}

export!(Plugin);
