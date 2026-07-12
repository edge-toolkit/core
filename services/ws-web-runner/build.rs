//! Link support for the x86_64-pc-windows-gnu (mingw) build; a no-op on every other target.
//!
//! `rusty_v8` ships no windows-gnu prebuilt, so the mingw lane links the MSVC archive (selected via
//! `RUSTY_V8_ARCHIVE` / `RUSTY_V8_SRC_BINDING_PATH` in `.mise/config.mingw.toml`). x64 COFF objects and
//! the C-ABI binding surface are toolchain-portable, but three gaps remain, closed here:
//!
//! - MSVC static-CRT symbols (stack cookie, `_Init_thread_*`, operator new/delete, RTTI statics) that
//!   MSVC links from msvcrt.lib/libvcruntime.lib -- supplied by the mingw-shim/ sources.
//! - Symbols exported by vcruntime140.dll / ucrtbase.dll -- supplied by winlibs' import libs.
//! - The archive's absl weak externals, which GNU ld.bfd leaves undefined -- resolved by linking with
//!   llvm-mingw's ld.lld (COFF weak-external support), found on PATH in the mingw mise env.
//!
//! Everything is emitted as `cargo:rustc-link-arg` so it lands at the END of the link line, after the
//! rlib that carries the `rusty_v8` archive -- GNU linkers resolve archives left-to-right, so the same
//! libs emitted as `rustc-link-lib` from this crate would precede the archive and satisfy nothing.

// std::env::var reads below take named constants, not inline literals.
// A mistyped name is then a compile error rather than a silently-failing lookup.
#[cfg(windows)]
const CARGO_CFG_TARGET_OS: &str = "CARGO_CFG_TARGET_OS";
#[cfg(windows)]
const CARGO_CFG_TARGET_ENV: &str = "CARGO_CFG_TARGET_ENV";
#[cfg(windows)]
const CARGO_CFG_TARGET_ABI: &str = "CARGO_CFG_TARGET_ABI";
#[cfg(windows)]
const OUT_DIR: &str = "OUT_DIR";
#[cfg(windows)]
const SYSTEM_ROOT: &str = "SystemRoot";

fn main() {
    // cc emits rerun-if-env-changed directives, which switches cargo off its rerun-on-any-file default --
    // so the shim sources must be declared explicitly or edits to them silently don't rebuild.
    println!("cargo:rerun-if-changed=mingw-shim/msvc_crt_shim.c");
    println!("cargo:rerun-if-changed=mingw-shim/msvc_crt_ops.s");
    println!("cargo:rerun-if-changed=mingw-shim/msvc_crt_locale.c");
    println!("cargo:rerun-if-changed=mingw-shim/msvc_crt_locale_cache.c");
    println!("cargo:rerun-if-changed=mingw-shim/msvc_crt_alloc.c");

    // The shim is only linked for the windows-gnu target, which is only ever built on a Windows host, so the
    // whole branch compiles there and nowhere else -- keeping it out of the Linux coverage build entirely.
    #[cfg(windows)]
    link_mingw_shim();
}

#[cfg(windows)]
#[expect(
    clippy::single_call_fn,
    clippy::unwrap_used,
    reason = "windows-gnu link setup: single call site, and unwraps assert build invariants"
)]
fn link_mingw_shim() {
    let target_os = std::env::var(CARGO_CFG_TARGET_OS).unwrap_or_default();
    let target_env = std::env::var(CARGO_CFG_TARGET_ENV).unwrap_or_default();
    let target_abi = std::env::var(CARGO_CFG_TARGET_ABI).unwrap_or_default();
    if target_os != "windows" || target_env != "gnu" || target_abi == "llvm" {
        return;
    }

    cc::Build::new()
        .file("mingw-shim/msvc_crt_shim.c")
        .file("mingw-shim/msvc_crt_ops.s")
        .compile("msvc_crt_shim");

    let out_dir = std::env::var(OUT_DIR).unwrap();

    // msvc_crt_locale.c must be a standalone OBJECT on the link line, not an archive member: its strong
    // definitions have to intercept names that -lmsvcrt (earlier in the default-libs block) would
    // otherwise satisfy first, and linkers only extract archive members left-to-right.
    let gcc = cc::Build::new().get_compiler();
    let locale_obj = format!("{out_dir}/msvc_crt_locale.o");
    let locale_args = ["-c", "-O2", "-o", &locale_obj, "mingw-shim/msvc_crt_locale.c"];
    run(std::process::Command::new(gcc.path()).args(locale_args));

    // The locale wrappers (setlocale, ...) are split into their own standalone object so Codacy can path-exclude
    // just their write-once symbol caches; they intercept -lmsvcrt exactly as msvc_crt_locale.c does, so they too
    // must be a link-line object rather than an archive member. They call ucrt_resolve_once from msvc_crt_locale.o.
    let locale_cache_obj = format!("{out_dir}/msvc_crt_locale_cache.o");
    let locale_cache_args = [
        "-c",
        "-O2",
        "-o",
        &locale_cache_obj,
        "mingw-shim/msvc_crt_locale_cache.c",
    ];
    run(std::process::Command::new(gcc.path()).args(locale_cache_args));

    // msvc_crt_alloc.c (operator new/delete + _dupenv_s) is a standalone object for the same reason: _dupenv_s
    // must intercept -lmsvcrt, and the operator-new symbols resolve the msvc_crt_ops.s jumps in the archive.
    let alloc_obj = format!("{out_dir}/msvc_crt_alloc.o");
    let alloc_args = ["-c", "-O2", "-o", &alloc_obj, "mingw-shim/msvc_crt_alloc.c"];
    run(std::process::Command::new(gcc.path()).args(alloc_args));

    // The msvc archive embeds `/defaultlib:libcmt` + `/defaultlib:oldnames` directives. lld honours them
    // (ld.bfd ignores directives) and errors when the libs don't exist; MSVC's static CRT has no mingw
    // equivalent -- the shim + import libs below stand in for it. Satisfy the directives with empty
    // archives ("!<arch>\n" is a valid zero-member ar file) in OUT_DIR, which cc put on the search path.
    for name in ["liblibcmt.a", "liboldnames.a"] {
        fs_err::write(format!("{out_dir}/{name}"), b"!<arch>\n").unwrap();
    }

    // The archive's std::exception_ptr internals (__ExceptionPtr*) are exported by msvcp140.dll, which
    // winlibs ships no import lib for -- generate one from the system DLL with winlibs' gendef + dlltool
    // (both on PATH via the mingw mise env).
    let system32 = format!(
        "{}\\System32",
        std::env::var(SYSTEM_ROOT).unwrap_or_else(|_| "C:\\Windows".into())
    );
    run(std::process::Command::new("gendef")
        .arg(format!("{system32}\\msvcp140.dll"))
        .current_dir(&out_dir));
    let dlltool_args = ["-d", "msvcp140.def", "-l", "libmsvcp140.a", "-D", "msvcp140.dll"];
    run(std::process::Command::new("dlltool")
        .args(dlltool_args)
        .current_dir(&out_dir));

    println!("cargo:rustc-link-arg={locale_obj}");
    println!("cargo:rustc-link-arg={locale_cache_obj}");
    println!("cargo:rustc-link-arg={alloc_obj}");
    println!("cargo:rustc-link-arg={out_dir}/libmsvc_crt_shim.a");
    println!("cargo:rustc-link-arg=-lvcruntime140");
    println!("cargo:rustc-link-arg=-lucrtbase");
    println!("cargo:rustc-link-arg=-lmsvcp140");
    println!("cargo:rustc-link-arg=-fuse-ld=lld");
}

#[cfg(windows)]
#[expect(
    clippy::unwrap_used,
    reason = "build script: a panic is cargo's only failure channel for a failed command"
)]
fn run(command: &mut std::process::Command) {
    let status = command.status().unwrap();
    assert!(status.success(), "{command:?} exited with {status}");
}
