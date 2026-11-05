//! What a guest built for a Wasm 1.0 target links: the `et` imports themselves, its heap, and its panic handler.
//!
//! None of it builds for a host, where nothing provides the imports. Each import is one call forwarded, and everything
//! that decides anything is in [`crate::Guest`], which takes any [`Imports`].

#![expect(unsafe_code, reason = "the `et` imports are declared in an `unsafe extern` block")]

use core::panic::PanicInfo;

use dlmalloc::GlobalDlmalloc;

use crate::{Guest, GuestError, Imports};

/// The heap every allocation the guest makes comes from, growing the guest's linear memory as it runs out.
///
/// No fixed size, so a guest takes what its input needs, as one built for WASI does; the host bounds the memory.
#[global_allocator]
static HEAP: GlobalDlmalloc = GlobalDlmalloc;

// The `et` imports, as the host provides them, each named here for the import it is.
//
// Each is declared `safe`: it reads or writes only the bytes its pointer and length name, and the host traps the guest
// on a pointer that is out of bounds rather than touching anything else.
#[link(wasm_import_module = "et")]
unsafe extern "C" {
    #[link_name = "connect"]
    safe fn et_connect() -> i32;
    #[link_name = "agent_id"]
    safe fn et_agent_id() -> i32;
    #[link_name = "send"]
    safe fn et_send(message: *const u8, message_len: usize) -> i32;
    #[link_name = "recv"]
    safe fn et_recv(timeout_ms: u32) -> i32;
    #[link_name = "take"]
    safe fn et_take(buffer: *mut u8, capacity: usize) -> i32;
    #[link_name = "disconnect"]
    safe fn et_disconnect();
    #[link_name = "kv_get"]
    safe fn et_kv_get(bucket: *const u8, bucket_len: usize, key: *const u8, key_len: usize) -> i32;
    #[link_name = "kv_set"]
    safe fn et_kv_set(
        bucket: *const u8,
        bucket_len: usize,
        key: *const u8,
        key_len: usize,
        value: *const u8,
        value_len: usize,
    ) -> i32;
    #[link_name = "log"]
    safe fn et_log(level: i32, context: *const u8, context_len: usize, message: *const u8, message_len: usize);
}

/// The `et` imports the host provides.
#[derive(Debug)]
#[non_exhaustive]
pub struct Host;

impl Imports for Host {
    fn connect(&mut self) -> i32 {
        et_connect()
    }

    fn agent_id(&mut self) -> i32 {
        et_agent_id()
    }

    fn send(&mut self, message: &[u8]) -> i32 {
        et_send(message.as_ptr(), message.len())
    }

    fn recv(&mut self, timeout_ms: u32) -> i32 {
        et_recv(timeout_ms)
    }

    fn take(&mut self, buffer: &mut [u8]) -> i32 {
        et_take(buffer.as_mut_ptr(), buffer.len())
    }

    fn disconnect(&mut self) {
        et_disconnect();
    }

    fn kv_get(&mut self, bucket: &str, key: &str) -> i32 {
        et_kv_get(bucket.as_ptr(), bucket.len(), key.as_ptr(), key.len())
    }

    fn kv_set(&mut self, bucket: &str, key: &str, value: &[u8]) -> i32 {
        et_kv_set(
            bucket.as_ptr(),
            bucket.len(),
            key.as_ptr(),
            key.len(),
            value.as_ptr(),
            value.len(),
        )
    }

    fn log(&mut self, level: i32, context: &str, message: &str) {
        et_log(level, context.as_ptr(), context.len(), message.as_ptr(), message.len());
    }
}

/// A panic traps the guest, which the host reports as the guest's failure.
#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    core::arch::wasm32::unreachable()
}

/// What the guest's `run` returns: `body` run on a [`Guest`] logging under `context`.
pub fn run<F>(context: &str, body: F) -> i32
where
    F: FnOnce(&mut Guest<Host>) -> Result<(), GuestError>,
{
    let mut guest = Guest::new(Host, context);
    let result = body(&mut guest);
    guest.finish(result)
}

/// Define the guest's `run` export as `body`, a function taking the guest's [`Guest`] and returning its result.
#[macro_export]
macro_rules! export {
    ($body:path) => {
        /// The guest's `run`, which the host calls once: [`et_core_guest::wasm::run`] of the guest's work.
        #[expect(unsafe_code, reason = "the host finds `run` by its unmangled export name")]
        #[unsafe(no_mangle)]
        pub extern "C" fn run() -> i32 {
            $crate::wasm::run(env!("CARGO_PKG_NAME"), $body)
        }
    };
}
