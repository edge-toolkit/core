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

#![expect(
    clippy::expect_used,
    reason = "build-script code: a panic is the only failure channel cargo gives, and expect names the invariant"
)]

fn main() {
    // cc emits rerun-if-env-changed directives, which switches cargo off its rerun-on-any-file default --
    // so the shim sources must be declared explicitly or edits to them silently don't rebuild.
    println!("cargo:rerun-if-changed=mingw-shim/msvc_crt_shim.c");
    println!("cargo:rerun-if-changed=mingw-shim/msvc_crt_ops.s");
    println!("cargo:rerun-if-changed=mingw-shim/msvc_crt_locale.c");

    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    let target_abi = std::env::var("CARGO_CFG_TARGET_ABI").unwrap_or_default();
    if target_os != "windows" || target_env != "gnu" || target_abi == "llvm" {
        return;
    }

    cc::Build::new()
        .file("mingw-shim/msvc_crt_shim.c")
        .file("mingw-shim/msvc_crt_ops.s")
        .compile("msvc_crt_shim");

    let out_dir = std::env::var("OUT_DIR").expect("cargo always sets OUT_DIR");

    // msvc_crt_locale.c must be a standalone OBJECT on the link line, not an archive member: its strong
    // definitions have to intercept names that -lmsvcrt (earlier in the default-libs block) would
    // otherwise satisfy first, and linkers only extract archive members left-to-right.
    let gcc = cc::Build::new().get_compiler();
    let locale_obj = format!("{out_dir}/msvc_crt_locale.o");
    let locale_args = ["-c", "-O2", "-o", &locale_obj, "mingw-shim/msvc_crt_locale.c"];
    run(std::process::Command::new(gcc.path()).args(locale_args));

    // The msvc archive embeds `/defaultlib:libcmt` + `/defaultlib:oldnames` directives. lld honours them
    // (ld.bfd ignores directives) and errors when the libs don't exist; MSVC's static CRT has no mingw
    // equivalent -- the shim + import libs below stand in for it. Satisfy the directives with empty
    // archives ("!<arch>\n" is a valid zero-member ar file) in OUT_DIR, which cc put on the search path.
    for name in ["liblibcmt.a", "liboldnames.a"] {
        fs_err::write(format!("{out_dir}/{name}"), b"!<arch>\n").expect("OUT_DIR is writable during build scripts");
    }

    // The archive's std::exception_ptr internals (__ExceptionPtr*) are exported by msvcp140.dll, which
    // winlibs ships no import lib for -- generate one from the system DLL with winlibs' gendef + dlltool
    // (both on PATH via the mingw mise env).
    let system32 = format!(
        "{}\\System32",
        std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into())
    );
    run(std::process::Command::new("gendef")
        .arg(format!("{system32}\\msvcp140.dll"))
        .current_dir(&out_dir));
    let dlltool_args = ["-d", "msvcp140.def", "-l", "libmsvcp140.a", "-D", "msvcp140.dll"];
    run(std::process::Command::new("dlltool")
        .args(dlltool_args)
        .current_dir(&out_dir));

    println!("cargo:rustc-link-arg={locale_obj}");
    println!("cargo:rustc-link-arg={out_dir}/libmsvc_crt_shim.a");
    println!("cargo:rustc-link-arg=-lvcruntime140");
    println!("cargo:rustc-link-arg=-lucrtbase");
    println!("cargo:rustc-link-arg=-lmsvcp140");
    println!("cargo:rustc-link-arg=-fuse-ld=lld");
}

fn run(command: &mut std::process::Command) {
    let status = command
        .status()
        .expect("gendef/dlltool come from winlibs, on PATH in the mingw mise env");
    assert!(status.success(), "{command:?} exited with {status}");
}
