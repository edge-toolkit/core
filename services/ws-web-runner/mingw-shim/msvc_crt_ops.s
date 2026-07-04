# MSVC-mangled and TLS shim symbols for linking the msvc-built rusty_v8 archive on x86_64-pc-windows-gnu.
# GAS accepts `?` in symbol names only in quoted form, which C asm-labels can't emit -- hence this
# hand-written file. MSVC x64 and mingw x64 share the Microsoft x64 calling convention, so bare jmps
# forward arguments untouched. The plain-named impls live in msvc_crt_shim.c.

# Per-thread ints MSVC objects bind with SECREL relocations, so they must be real .tls$ symbols (a gcc
# __thread variable is emutls and can't satisfy them). _Init_thread_epoch stays INT_MIN forever (see the
# _Init_thread_header comment in msvc_crt_shim.c); __tls_guard is pre-set so on-demand TLS init is
# skipped -- the mingw-w64 crt's TLS callback already runs the .CRT$XD* initializers.
    .section .tls$,"dw"
    .globl _Init_thread_epoch
_Init_thread_epoch:
    .long 0x80000000
    .globl __tls_guard
__tls_guard:
    .byte 1

# MSVC RTTI statics from libvcruntime's static portion. type_info's vtable has a single virtual slot (the
# scalar deleting destructor); type_info objects are never deleted through that vtable here, so a stub
# that aborts is safe. The root node anchors the SLIST that __std_type_info_name (vcruntime140.dll)
# chains undecorated-name allocations onto: an SLIST_HEADER, which x64 REQUIRES 16-byte aligned
# (cmpxchg16b). At 8-mod-16 it access-violates inside ntdll!ExpInterlockedPopEntrySListEnd the first time
# type_info::name() runs -- observed via Intl.DateTimeFormat -> v8/ICU LocaleCacheKey::hashCode ->
# __std_type_info_name in the dotnet-data1 web-runner test, the only module whose JS touches Intl.
    .text
type_info_stub_dtor:
    jmp abort

    .data
    .balign 8
    .globl "??_7type_info@@6B@"
"??_7type_info@@6B@":
    .quad type_info_stub_dtor

    .balign 16
    .globl "?__type_info_root_node@@3U__type_info_node@@A"
"?__type_info_root_node@@3U__type_info_node@@A":
    .quad 0
    .quad 0

    .text

    .globl "?__libcpp_verbose_abort@__Cr@std@@YAXPEBDZZ"  # Chromium libc++ __Cr::std::__libcpp_verbose_abort
"?__libcpp_verbose_abort@__Cr@std@@YAXPEBDZZ":
    jmp shim_libcpp_verbose_abort

    .globl "??2@YAPEAX_K@Z"                     # operator new(size_t)
"??2@YAPEAX_K@Z":
    jmp shim_op_new

    .globl "??2@YAPEAX_KAEBUnothrow_t@std@@@Z"  # operator new(size_t, nothrow_t const&)
"??2@YAPEAX_KAEBUnothrow_t@std@@@Z":
    jmp shim_op_new_nothrow

    .globl "??2@YAPEAX_KW4align_val_t@std@@@Z"  # operator new(size_t, align_val_t)
"??2@YAPEAX_KW4align_val_t@std@@@Z":
    jmp shim_op_new_aligned

    .globl "??_U@YAPEAX_K@Z"                    # operator new[](size_t)
"??_U@YAPEAX_K@Z":
    jmp shim_op_new

    .globl "??_U@YAPEAX_KAEBUnothrow_t@std@@@Z" # operator new[](size_t, nothrow_t const&)
"??_U@YAPEAX_KAEBUnothrow_t@std@@@Z":
    jmp shim_op_new_nothrow

    .globl "??_U@YAPEAX_KW4align_val_t@std@@@Z" # operator new[](size_t, align_val_t)
"??_U@YAPEAX_KW4align_val_t@std@@@Z":
    jmp shim_op_new_aligned

    .globl "??3@YAXPEAX@Z"                      # operator delete(void*)
"??3@YAXPEAX@Z":
    jmp shim_op_delete

    .globl "??3@YAXPEAX_K@Z"                    # operator delete(void*, size_t)
"??3@YAXPEAX_K@Z":
    jmp shim_op_delete_sized

    .globl "??3@YAXPEAXW4align_val_t@std@@@Z"   # operator delete(void*, align_val_t)
"??3@YAXPEAXW4align_val_t@std@@@Z":
    jmp shim_op_delete_aligned

    .globl "??3@YAXPEAX_KW4align_val_t@std@@@Z" # operator delete(void*, size_t, align_val_t)
"??3@YAXPEAX_KW4align_val_t@std@@@Z":
    jmp shim_op_delete_sized_aligned

    .globl "??_V@YAXPEAX@Z"                     # operator delete[](void*)
"??_V@YAXPEAX@Z":
    jmp shim_op_delete

    .globl "??_V@YAXPEAX_K@Z"                   # operator delete[](void*, size_t)
"??_V@YAXPEAX_K@Z":
    jmp shim_op_delete_sized

    .globl "??_V@YAXPEAXW4align_val_t@std@@@Z"  # operator delete[](void*, align_val_t)
"??_V@YAXPEAXW4align_val_t@std@@@Z":
    jmp shim_op_delete_aligned
