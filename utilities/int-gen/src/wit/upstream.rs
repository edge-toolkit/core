//! Provenance for every WIT package under `generated/specs/wit/deps/` that
//! isn't generated from `WsMessage`.
//!
//! Five WASI packages (`wasi-clocks`, `wasi-io`, `wasi-keyvalue`,
//! `wasi-logging`, `wasi-nn`) are fetched verbatim from upstream
//! `WebAssembly/<repo>` at pinned tags or commit SHAs.
//!
//! `wasi-webgpu` is fetched from upstream `WebAssembly/wasi-gfx`, parsed
//! with `wit-parser`, and post-processed by `strip_webgpu` using the
//! allowlist `WEBGPU_KEEP_NAMES`: only those 72 top-level items survive,
//! and methods / fields / variant cases referencing other upstream names
//! are removed from inside them. The output is the compute-only subset the
//! wgpu-backed host impl in
//! `services/ws-wasi-runner/src/host/wasi_webgpu.rs` actually supports.
//!
//! `mise run fetch-wit-deps` re-runs everything; `gen-specs-check` flags
//! drift.

use std::collections::HashSet;
use std::fs;
use std::path::Path;

use wit_encoder::{Interface, InterfaceItem, PackageItem, ResourceFuncKind, Type, TypeDefKind};

use crate::Error;

/// One file within an upstream package.
struct File {
    name: &'static str,
}

/// An upstream WASI WIT package pinned to a tag or commit SHA.
struct UpstreamPackage {
    /// Directory name under `generated/specs/wit/deps/`.
    local_dir: &'static str,
    /// `WebAssembly/<repo>` (always under the WebAssembly org).
    repo: &'static str,
    /// Tag (e.g. `v0.2.6`) or commit SHA. wasi-logging has no releases, so
    /// it pins by SHA; everything else by release tag.
    git_ref: &'static str,
    files: &'static [File],
}

const PACKAGES: &[UpstreamPackage] = &[
    UpstreamPackage {
        local_dir: "wasi-clocks",
        repo: "wasi-clocks",
        git_ref: "v0.2.6",
        files: &[
            File {
                name: "monotonic-clock.wit",
            },
            File { name: "timezone.wit" },
            File { name: "wall-clock.wit" },
            File { name: "world.wit" },
        ],
    },
    UpstreamPackage {
        local_dir: "wasi-io",
        repo: "wasi-io",
        git_ref: "v0.2.6",
        files: &[
            File { name: "error.wit" },
            File { name: "poll.wit" },
            File { name: "streams.wit" },
            File { name: "world.wit" },
        ],
    },
    UpstreamPackage {
        local_dir: "wasi-keyvalue",
        repo: "wasi-keyvalue",
        git_ref: "v0.2.0-draft",
        files: &[
            File { name: "atomic.wit" },
            File { name: "batch.wit" },
            File { name: "store.wit" },
            File { name: "watch.wit" },
            File { name: "world.wit" },
        ],
    },
    UpstreamPackage {
        local_dir: "wasi-logging",
        repo: "wasi-logging",
        // No release tags exist; pinned to a known-good commit.
        git_ref: "d31c41d0d9eed81aabe02333d0025d42acf3fb75",
        files: &[File { name: "logging.wit" }, File { name: "world.wit" }],
    },
    UpstreamPackage {
        local_dir: "wasi-nn",
        repo: "wasi-nn",
        git_ref: "0.2.0-rc-2024-10-28",
        files: &[File { name: "wasi-nn.wit" }],
    },
];

/// Pinned commit of `WebAssembly/wasi-gfx`. wasi-gfx is pre-publication so
/// it has no release tags; we anchor to a specific commit until upstream
/// stabilises.
const WEBGPU_GIT_REF: &str = "6c0d2244daf997cae7aed19cb1c2b38df011a41c";

/// Top-level items (`resource`, `record`, `variant`, `enum`, `type`, free
/// `func`) we keep from upstream `webgpu.wit`. Anything else is dropped.
/// 72 items — the compute-only subset the wgpu-backed host actually
/// supports. Anything new upstream adds is excluded by default; opt in by
/// listing it here.
const WEBGPU_KEEP_NAMES: &[&str] = &[
    "create-pipeline-error",
    "create-pipeline-error-kind",
    "get-gpu",
    "get-mapped-range-error",
    "get-mapped-range-error-kind",
    "gpu",
    "gpu-adapter",
    "gpu-adapter-info",
    "gpu-bind-group",
    "gpu-bind-group-descriptor",
    "gpu-bind-group-entry",
    "gpu-bind-group-layout",
    "gpu-bind-group-layout-descriptor",
    "gpu-bind-group-layout-entry",
    "gpu-binding-resource",
    "gpu-buffer",
    "gpu-buffer-binding",
    "gpu-buffer-binding-layout",
    "gpu-buffer-binding-type",
    "gpu-buffer-descriptor",
    "gpu-buffer-dynamic-offset",
    "gpu-buffer-map-state",
    "gpu-buffer-usage",
    "gpu-buffer-usage-flags",
    "gpu-command-buffer",
    "gpu-command-buffer-descriptor",
    "gpu-command-encoder",
    "gpu-command-encoder-descriptor",
    "gpu-compute-pass-descriptor",
    "gpu-compute-pass-encoder",
    "gpu-compute-pipeline",
    "gpu-compute-pipeline-descriptor",
    "gpu-device",
    "gpu-device-descriptor",
    "gpu-feature-name",
    "gpu-flags-constant",
    "gpu-index32",
    "gpu-layout-mode",
    "gpu-map-mode",
    "gpu-map-mode-flags",
    "gpu-pipeline-constant-value",
    "gpu-pipeline-error-reason",
    "gpu-pipeline-layout",
    "gpu-pipeline-layout-descriptor",
    "gpu-power-preference",
    "gpu-programmable-stage",
    "gpu-queue",
    "gpu-queue-descriptor",
    "gpu-request-adapter-options",
    "gpu-shader-module",
    "gpu-shader-module-compilation-hint",
    "gpu-shader-module-descriptor",
    "gpu-shader-stage",
    "gpu-shader-stage-flags",
    "gpu-size32",
    "gpu-size32-out",
    "gpu-size64",
    "gpu-size64-out",
    "gpu-supported-features",
    "gpu-supported-limits",
    "map-async-error",
    "map-async-error-kind",
    "record-gpu-pipeline-constant-value",
    "record-option-gpu-size64",
    "request-device-error",
    "request-device-error-kind",
    "set-bind-group-error",
    "set-bind-group-error-kind",
    "unmap-error",
    "unmap-error-kind",
    "write-buffer-error",
    "write-buffer-error-kind",
];

/// Methods on kept resources whose signatures use only kept types yet still
/// sit outside our compute-only subset. Identified by method name so the
/// post-parse walker drops them by direct name match.
const WEBGPU_DROP_METHODS: &[&str] = &[
    "create-compute-pipeline-async", // sync compute creation is enough
    "clear-buffer",                  // write-buffer-with-copy serves init
    "push-debug-group",              // debug markers unused
    "pop-debug-group",
    "insert-debug-marker",
    "dispatch-workgroups-indirect", // no indirect buffers
    "on-submitted-work-done",       // no completion pollables
];

/// Minimal stub packages for the cross-package `use` clauses in upstream
/// `webgpu.wit`. wit-parser needs every referenced package available to
/// resolve; we don't actually ship these — the trim drops all methods that
/// reference these types, then we clear the `use` clauses entirely.
const WASI_IO_STUB: &str = concat!(
    "package wasi:io@0.2.0;\n",
    "interface poll {\n",
    "  resource pollable {}\n",
    "}\n",
);

const WASI_GRAPHICS_CONTEXT_STUB: &str = concat!(
    "package wasi:graphics-context@0.0.1;\n",
    "interface graphics-context {\n",
    "  resource context {}\n",
    "  resource abstract-buffer {}\n",
    "}\n",
);

/// Refresh every upstream WIT package and write them all under
/// `generated/specs/wit/deps/<pkg>/`. Triggered by `mise run fetch-wit-deps`.
pub fn run(project_root: &Path) -> Result<(), Error> {
    let deps_root = project_root.join("generated/specs/wit/deps");
    for pkg in PACKAGES {
        fetch_one(&deps_root, pkg)?;
    }
    fetch_and_trim_webgpu(&deps_root)?;
    Ok(())
}

fn fetch_one(deps_root: &Path, pkg: &UpstreamPackage) -> Result<(), Error> {
    let dest = deps_root.join(pkg.local_dir);
    // Wipe the destination first — guards against orphan files left over
    // from an old pin that the new pin no longer ships.
    if dest.exists() {
        fs::remove_dir_all(&dest)?;
    }
    fs::create_dir_all(&dest)?;
    for file in pkg.files {
        let url = format!(
            "https://raw.githubusercontent.com/WebAssembly/{repo}/{git_ref}/wit/{file}",
            repo = pkg.repo,
            git_ref = pkg.git_ref,
            file = file.name,
        );
        let body = ureq::get(&url).call()?.into_string()?;
        let target = dest.join(file.name);
        fs::write(&target, body)?;
        println!("wrote {}", target.display());
    }
    Ok(())
}

fn fetch_and_trim_webgpu(deps_root: &Path) -> Result<(), Error> {
    let url = format!("https://raw.githubusercontent.com/WebAssembly/wasi-gfx/{WEBGPU_GIT_REF}/webgpu/webgpu.wit");
    let raw = ureq::get(&url).call()?.into_string()?;
    let stripped = strip_webgpu(&raw)?;
    let dest = deps_root.join("wasi-webgpu");
    if dest.exists() {
        fs::remove_dir_all(&dest)?;
    }
    fs::create_dir_all(&dest)?;
    let target = dest.join("webgpu.wit");
    fs::write(&target, stripped)?;
    println!("wrote {}", target.display());
    Ok(())
}

/// Parse the upstream `webgpu.wit` via `wit-parser`, filter the parsed AST
/// down to our compute-only subset, and re-emit using `wit-encoder`.
fn strip_webgpu(raw: &str) -> Result<String, Error> {
    // wit-parser returns `anyhow::Error` which doesn't impl `std::error::Error`
    // (coherence), so transparent thiserror propagation isn't possible. Panic
    // on parse failure — this is an internal CLI; if the upstream WIT can't be
    // parsed the bump should be reverted, not error-handled.
    let mut resolve = wit_parser::Resolve::default();
    resolve.push_str("wasi-io-stub.wit", WASI_IO_STUB).unwrap();
    resolve
        .push_str("wasi-graphics-context-stub.wit", WASI_GRAPHICS_CONTEXT_STUB)
        .unwrap();
    resolve.push_str("webgpu.wit", raw).unwrap();

    let mut webgpu = wit_encoder::packages_from_parsed(&resolve)
        .into_iter()
        .find(|pkg| pkg.name().namespace() == "wasi" && pkg.name().name().raw_name() == "webgpu")
        .expect("upstream webgpu.wit declared a non-`wasi:webgpu` package");

    let keep: HashSet<&str> = WEBGPU_KEEP_NAMES.iter().copied().collect();
    let drop_methods: HashSet<&str> = WEBGPU_DROP_METHODS.iter().copied().collect();

    for package_item in webgpu.items_mut() {
        if let PackageItem::Interface(iface) = package_item {
            mutate_interface(iface, &keep, &drop_methods);
        }
    }

    Ok(webgpu.to_string())
}

fn mutate_interface(iface: &mut Interface, keep: &HashSet<&str>, drop_methods: &HashSet<&str>) {
    // Drop all `use` clauses (wit-encoder doesn't expose a clearer for them)
    // by rebuilding the interface from scratch. The kept items are moved over
    // verbatim, so this is a no-op for any field we care about preserving.
    // Cross-package types were either stubs we injected or are referenced
    // only by methods we'll filter out below.
    let items = std::mem::take(iface.items_mut());
    let mut rebuilt = Interface::new(iface.name().clone());
    rebuilt.set_docs(iface.docs().clone());
    for item in items {
        rebuilt.items_mut().push(item);
    }
    *iface = rebuilt;

    // Drop top-level items not on the keep list.
    iface.items_mut().retain(|item| match item {
        InterfaceItem::TypeDef(td) => keep.contains(td.name().raw_name()),
        InterfaceItem::Function(f) => keep.contains(f.name().raw_name()),
    });

    // For each kept TypeDef, drop methods / fields / variant cases that
    // reference a dropped name (either by their own name for methods, or
    // transitively through their Type signatures).
    for item in iface.items_mut() {
        let InterfaceItem::TypeDef(td) = item else { continue };
        match td.kind_mut() {
            TypeDefKind::Resource(res) => {
                res.funcs_mut()
                    .retain(|func| !should_drop_resource_func(func, keep, drop_methods));
            }
            TypeDefKind::Record(record) => {
                record
                    .fields_mut()
                    .retain(|field| !type_refs_dropped(field.type_(), keep));
            }
            TypeDefKind::Variant(variant) => {
                variant.cases_mut().retain(|case| match case.type_() {
                    Some(ty) => !type_refs_dropped(ty, keep),
                    None => true,
                });
            }
            _ => {} // enums, flags, type aliases: no inner refs to check
        }
    }
}

fn should_drop_resource_func(
    func: &wit_encoder::ResourceFunc,
    keep: &HashSet<&str>,
    drop_methods: &HashSet<&str>,
) -> bool {
    // Drop if the method's own name is on the explicit drop list.
    let func_name = match func.kind() {
        ResourceFuncKind::Method(name, _, _) | ResourceFuncKind::Static(name, _, _) => Some(name.raw_name()),
        ResourceFuncKind::Constructor(_) => None,
    };
    if let Some(name) = func_name
        && drop_methods.contains(name)
    {
        return true;
    }

    // Drop if any parameter or return type references a non-kept identifier.
    for (_pname, pty) in func.params().items() {
        if type_refs_dropped(pty, keep) {
            return true;
        }
    }
    let ret = match func.kind() {
        ResourceFuncKind::Method(_, _, r) | ResourceFuncKind::Static(_, _, r) => r.as_ref(),
        ResourceFuncKind::Constructor(r) => r.as_ref(),
    };
    if let Some(r) = ret
        && type_refs_dropped(r, keep)
    {
        return true;
    }
    false
}

/// `true` if walking `ty` finds any `Type::Named` / `Type::Borrow` whose
/// raw name is not in the keep set. Cross-package stub types
/// (`pollable`, `context`, `abstract-buffer`) and every upstream item we
/// stripped naturally land here because they're absent from
/// `WEBGPU_KEEP_NAMES`.
fn type_refs_dropped(ty: &Type, keep: &HashSet<&str>) -> bool {
    let mut refs = HashSet::new();
    collect_type_refs(ty, &mut refs);
    refs.iter().any(|r| !keep.contains(r.as_str()))
}

fn collect_type_refs(ty: &Type, refs: &mut HashSet<String>) {
    match ty {
        Type::Named(ident) | Type::Borrow(ident) => {
            refs.insert(ident.raw_name().to_string());
        }
        Type::Option(inner) | Type::List(inner) => collect_type_refs(inner, refs),
        Type::FixedLengthList(inner, _) => collect_type_refs(inner, refs),
        Type::Result(r) => {
            if let Some(ok) = r.get_ok() {
                collect_type_refs(ok, refs);
            }
            if let Some(err) = r.get_err() {
                collect_type_refs(err, refs);
            }
        }
        Type::Tuple(tup) => {
            for item in tup.types() {
                collect_type_refs(item, refs);
            }
        }
        Type::Map(k, v) => {
            collect_type_refs(k, refs);
            collect_type_refs(v, refs);
        }
        Type::Future(inner) | Type::Stream(inner) => {
            if let Some(t) = inner {
                collect_type_refs(t, refs);
            }
        }
        // primitives (Bool, U8/16/32/64, S8/16/32/64, F32/F64, Char, String, ErrorContext)
        _ => {}
    }
}
