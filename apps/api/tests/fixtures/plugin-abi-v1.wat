;; Source of plugin-abi-v1.wasm, the compiled fixture that pins the openpr.plugin.v1 ABI.
;;
;; The .wasm file is what the test loads; this file documents it. The binary is checked in on
;; purpose: a rename of the ABI names across the source tree cannot rewrite it consistently, so
;; the runtime has to keep looking up exactly these exports.
;;
;; Exports, with the signatures the runtime requires:
;;   memory                                  the guest's linear memory
;;   openpr_plugin_abi_version () -> i32     returns 1 and records that it was called
;;   openpr_alloc (len: i32) -> i32          bump allocator for the input buffer
;;   openpr_invoke (ptr: i32, len: i32) -> i64
;;       traps unless openpr_plugin_abi_version was called first, then returns its input
;;       unchanged as (ptr << 32) | len, so the host reads back exactly the JSON it wrote.
(module
  (memory (export "memory") 1)
  (global $heap (mut i32) (i32.const 1024))
  (global $version_checked (mut i32) (i32.const 0))
  (func (export "openpr_plugin_abi_version") (result i32)
    i32.const 1
    global.set $version_checked
    i32.const 1)
  (func (export "openpr_alloc") (param $len i32) (result i32)
    (local $ptr i32)
    global.get $heap
    local.set $ptr
    global.get $heap
    local.get $len
    i32.add
    global.set $heap
    local.get $ptr)
  (func (export "openpr_invoke") (param $ptr i32) (param $len i32) (result i64)
    global.get $version_checked
    i32.eqz
    if
      unreachable
    end
    local.get $ptr
    i64.extend_i32_u
    i64.const 32
    i64.shl
    local.get $len
    i64.extend_i32_u
    i64.or))
