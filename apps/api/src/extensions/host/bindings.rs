//! Bindings of the two worlds the host runs. The generated code is the one place that needs the
//! relaxed lints of macro output; everything the host writes by hand is in the sibling modules.
#![allow(clippy::all, clippy::pedantic, clippy::nursery, missing_docs, unreachable_pub)]

/// The `action` world: imports `ctx`, `records`, `catalogue`, `log`, `clock`; exports `run`.
pub mod action {
    wasmtime::component::bindgen!({
        path: "../../wit/sv-plugin.wit",
        world: "action",
        imports: { default: async | trappable },
        exports: { default: async },
    });
}

/// The `field-rule` world: no imports; exports `apply`.
pub mod rule {
    wasmtime::component::bindgen!({
        path: "../../wit/sv-plugin.wit",
        world: "field-rule",
        exports: { default: async },
    });
}
