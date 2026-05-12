#!/usr/bin/env python3
"""Build a local TVM 0.11.1 environment for macOS arm64.

The normal Pixi lockfile is intentionally linux-64 because apache-tvm 0.11.1
does not publish macOS arm64 wheels. This helper keeps the exceptional path
local and gitignored: Pixi supplies build tools, TVM is built from source, and
the Python package is installed into .tvm-macos/venv.
"""

from __future__ import annotations

import argparse
import json
import os
import platform
import shutil
import subprocess
import sys
from pathlib import Path


TVM_REPO = "https://github.com/apache/tvm.git"
TVM_REF = "v0.11.1"

PYTHON_REQUIREMENTS = [
    "synr==0.6.0",
    "cloudpickle==3.1.2",
    "decorator==5.3.1",
    "attrs==26.1.0",
    "psutil==7.2.2",
    "tornado==6.5.5",
    "torch==1.13.1",
    "torchvision==0.14.1",
    "numpy==1.26.4",
    "scipy==1.13.1",
    "tqdm==4.67.3",
    "pyyaml==6.0.3",
    "pillow==11.3.0",
    "easydict==1.13",
    "graphviz==0.21",
    "pandas==2.3.3",
    "scikit-learn==1.6.1",
    "dask==2024.8.0",
    "matplotlib",
]


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", default=".tvm-macos", help="Local output directory")
    parser.add_argument("--force", action="store_true", help="Delete and rebuild the local TVM environment")
    parser.add_argument("--jobs", type=int, default=os.cpu_count() or 4)
    parser.add_argument(
        "--without-llvm",
        action="store_true",
        help="Disable LLVM. This is only useful for import diagnostics; triplets:regen needs target.build.llvm.",
    )
    return parser.parse_args()


def run(argv: list[str], *, cwd: Path | None = None, env: dict[str, str] | None = None) -> None:
    print("→", " ".join(argv))
    subprocess.run(argv, cwd=cwd, env=env, check=True)


def require_darwin_arm64() -> None:
    if platform.system() != "Darwin" or platform.machine() != "arm64":
        raise SystemExit("setup_tvm_macos.py only supports native macOS arm64")
    if shutil.which("xcode-select") is None:
        raise SystemExit("xcode-select not found. Install Xcode Command Line Tools first: xcode-select --install")
    subprocess.run(["xcode-select", "-p"], check=True, stdout=subprocess.DEVNULL)


def ensure_tvm_source(src: Path) -> None:
    if (src / ".git").is_dir():
        print(f"✓ using existing TVM checkout at {src}")
        run(["git", "fetch", "--depth=1", "origin", f"refs/tags/{TVM_REF}:refs/tags/{TVM_REF}"], cwd=src)
        run(["git", "checkout", "--quiet", TVM_REF], cwd=src)
        run(["git", "submodule", "update", "--init", "--recursive", "--depth=1"], cwd=src)
        return

    src.parent.mkdir(parents=True, exist_ok=True)
    run(["git", "clone", "--recursive", "--depth=1", "--branch", TVM_REF, TVM_REPO, str(src)])


def patch_config(src: Path, build: Path, *, with_llvm: bool) -> None:
    build.mkdir(parents=True, exist_ok=True)
    config_src = src / "cmake" / "config.cmake"
    config_dst = build / "config.cmake"
    text = config_src.read_text(encoding="utf-8")
    llvm_value = "OFF"
    if with_llvm:
        llvm_config = shutil.which("llvm-config")
        if llvm_config is None:
            raise SystemExit(
                "llvm-config not found. Run this helper through tools/tvm-macos-env/pixi.toml "
                "or set PATH to a compatible LLVM."
            )
        llvm_value = f'"{llvm_config}"'
    replacements = {
        "USE_LLVM": llvm_value,
        "USE_CUDA": "OFF",
        "USE_OPENCL": "OFF",
        "USE_METAL": "OFF",
        "USE_VULKAN": "OFF",
        "USE_ROCM": "OFF",
    }
    lines: list[str] = []
    for line in text.splitlines():
        stripped = line.strip()
        replaced = False
        for key, value in replacements.items():
            if stripped.startswith(f"set({key} "):
                lines.append(f"set({key} {value})")
                replaced = True
                break
        if not replaced:
            lines.append(line)
    config_dst.write_text("\n".join(lines) + "\n", encoding="utf-8")


def build_tvm(src: Path, build: Path, jobs: int, *, with_llvm: bool) -> None:
    if (build / "CMakeCache.txt").exists() and not (
        (build / "libtvm.dylib").exists() and (build / "libtvm_runtime.dylib").exists()
    ):
        print(f"→ removing incomplete TVM build directory {build}")
        shutil.rmtree(build)
    patch_config(src, build, with_llvm=with_llvm)
    run(["cmake", "-S", str(src), "-B", str(build), "-G", "Ninja"])
    run(["cmake", "--build", str(build), "--target", "tvm", "tvm_runtime", "-j", str(jobs)])


def create_venv(venv: Path, tvm_src: Path, tvm_build: Path, *, require_llvm: bool) -> None:
    py = venv / "bin" / "python"
    if not py.exists():
        run([sys.executable, "-m", "venv", str(venv)])

    env = os.environ.copy()
    env["TVM_LIBRARY_PATH"] = str(tvm_build.resolve())
    env["DYLD_LIBRARY_PATH"] = f"{tvm_build.resolve()}:{env.get('DYLD_LIBRARY_PATH', '')}"

    run([str(py), "-m", "pip", "install", "--upgrade", "pip", "setuptools", "wheel"], env=env)
    run([str(py), "-m", "pip", "install", *PYTHON_REQUIREMENTS], env=env)
    run([str(py), "-m", "pip", "install", "-e", str(tvm_src / "python")], env=env)

    verify = "import tvm, torch, numpy; "
    if require_llvm:
        verify += "assert tvm.get_global_func('target.build.llvm', True), 'target.build.llvm is not enabled'; "
    verify += (
        "print(f'  tvm     {tvm.__version__}'); "
        "print(f'  torch   {torch.__version__}'); "
        "print(f'  numpy   {numpy.__version__}')"
    )
    run([str(py), "-c", verify], env=env)


def build_config(root: Path, *, with_llvm: bool) -> dict[str, object]:
    return {
        "tvm_ref": TVM_REF,
        "with_llvm": with_llvm,
    }


def config_matches(root: Path, expected: dict[str, object]) -> bool:
    path = root / "build-config.json"
    if not path.exists():
        return False
    try:
        return json.loads(path.read_text(encoding="utf-8")) == expected
    except Exception:
        return False


def write_build_config(root: Path, config: dict[str, object]) -> None:
    root.mkdir(parents=True, exist_ok=True)
    (root / "build-config.json").write_text(json.dumps(config, indent=2) + "\n", encoding="utf-8")


def write_env_file(root: Path, tvm_build: Path, venv: Path) -> None:
    env_file = root / "env.sh"
    env_file.write_text(
        "\n".join(
            [
                f'export TVM_LIBRARY_PATH="{tvm_build.resolve()}"',
                f'export DYLD_LIBRARY_PATH="{tvm_build.resolve()}:${{DYLD_LIBRARY_PATH:-}}"',
                f'export PYTHON="{(venv / "bin" / "python").resolve()}"',
                "",
            ]
        ),
        encoding="utf-8",
    )


def main() -> int:
    args = parse_args()
    require_darwin_arm64()

    root = Path(args.root)
    src = root / "tvm"
    build = src / "build"
    venv = root / "venv"
    with_llvm = not args.without_llvm
    expected_config = build_config(root, with_llvm=with_llvm)

    if args.force:
        shutil.rmtree(root, ignore_errors=True)

    py = venv / "bin" / "python"
    if (
        py.exists()
        and (build / "libtvm.dylib").exists()
        and (build / "libtvm_runtime.dylib").exists()
        and config_matches(root, expected_config)
    ):
        print(f"✓ local macOS TVM environment already exists at {root}")
        write_env_file(root, build, venv)
        return 0

    if root.exists() and not config_matches(root, expected_config):
        print(f"→ rebuilding {root} because the TVM build config changed")
        shutil.rmtree(root)

    ensure_tvm_source(src)
    build_tvm(src, build, args.jobs, with_llvm=with_llvm)
    create_venv(venv, src, build, require_llvm=with_llvm)
    write_build_config(root, expected_config)
    write_env_file(root, build, venv)
    print(f"✓ wrote local macOS TVM environment to {root}")
    print("  use with: source .tvm-macos/env.sh")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
