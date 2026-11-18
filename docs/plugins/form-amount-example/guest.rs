#![no_std]

use core::{panic::PanicInfo, ptr, slice};

const INPUT_BYTES: usize = 256;
const OUTPUT_BYTES: usize = 64;
static mut INPUT: [u8; INPUT_BYTES] = [0; INPUT_BYTES];
static mut OUTPUT: [u8; OUTPUT_BYTES] = [0; OUTPUT_BYTES];

#[panic_handler]
fn panic(_: &PanicInfo<'_>) -> ! {
    core::arch::wasm32::unreachable()
}

#[unsafe(no_mangle)]
pub extern "C" fn openpr_plugin_abi_version() -> i32 {
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn openpr_alloc(length: i32) -> i32 {
    if !(1..=INPUT_BYTES as i32).contains(&length) {
        core::arch::wasm32::unreachable()
    }
    ptr::addr_of_mut!(INPUT) as i32
}

#[unsafe(no_mangle)]
pub extern "C" fn openpr_invoke(input_ptr: i32, length: i32) -> i64 {
    if input_ptr != openpr_alloc(length) {
        core::arch::wasm32::unreachable()
    }
    // The runtime creates a fresh instance per call and writes only this allocated buffer.
    let input = unsafe { slice::from_raw_parts(ptr::addr_of!(INPUT).cast::<u8>(), length as usize) };
    let quantity = unsigned_field(input, b"\"quantity\":");
    let unit_price = unsigned_field(input, b"\"unit_price_cents\":");
    let mut cents = quantity * unit_price;
    #[cfg(extra_fee)]
    {
        cents += 100;
    }
    let output_ptr = ptr::addr_of_mut!(OUTPUT).cast::<u8>();
    // The input and output arrays are disjoint and the instance is single-threaded.
    let output = unsafe { slice::from_raw_parts_mut(output_ptr, OUTPUT_BYTES) };
    let prefix = b"{\"amount_cents\":";
    output[..prefix.len()].copy_from_slice(prefix);
    let digits_start = prefix.len();
    let mut cursor = digits_start;
    loop {
        output[cursor] = b'0' + (cents % 10) as u8;
        cursor += 1;
        cents /= 10;
        if cents == 0 {
            break;
        }
    }
    output[digits_start..cursor].reverse();
    output[cursor] = b'}';
    ((output_ptr as i64) << 32) | (cursor as i64 + 1)
}

fn unsigned_field(input: &[u8], field: &[u8]) -> u64 {
    let start = input.windows(field.len()).position(|window| window == field);
    let Some(start) = start else {
        core::arch::wasm32::unreachable()
    };
    let mut digits = input[start + field.len()..]
        .iter()
        .take_while(|byte| byte.is_ascii_digit());
    let Some(first) = digits.next() else {
        core::arch::wasm32::unreachable()
    };
    digits.fold(u64::from(first - b'0'), |value, digit| {
        value.saturating_mul(10).saturating_add(u64::from(digit - b'0'))
    })
}
