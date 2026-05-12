#!/usr/bin/env python3
"""Prepare the demo sample pack from public COCO 2014 validation images.

The Demo page only loads an already-prepared same-origin sample pack. This
script is the setup-time bridge: download/cache COCO annotations, choose a
person/non-person split, download only those images, cover-crop them to 128x128,
and write pkg/demo/sample-pack/manifest.json.
"""

from __future__ import annotations

import argparse
import json
import random
import shutil
import urllib.error
import urllib.request
import zipfile
from pathlib import Path

from PIL import Image, ImageOps


DEFAULT_SOURCE = "coco2014-val-public"
DEFAULT_ANNOTATIONS_URL = "http://images.cocodataset.org/annotations/annotations_trainval2014.zip"
DEFAULT_IMAGE_BASE_URL = "http://images.cocodataset.org/val2014"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", default=DEFAULT_SOURCE)
    parser.add_argument("--annotations-url", default=DEFAULT_ANNOTATIONS_URL)
    parser.add_argument("--image-base-url", default=DEFAULT_IMAGE_BASE_URL)
    parser.add_argument("--cache", default=".sample-pack-cache")
    parser.add_argument("--out", default="pkg/demo/sample-pack")
    parser.add_argument("--seed", type=int, default=42)
    parser.add_argument("--train-person", type=int, default=40)
    parser.add_argument("--train-scene", type=int, default=40)
    parser.add_argument("--val-person", type=int, default=10)
    parser.add_argument("--val-scene", type=int, default=10)
    parser.add_argument("--force-download", action="store_true")
    parser.add_argument("--jpeg-quality", type=int, default=85)
    return parser.parse_args()


def download_file(url: str, dst: Path, force: bool = False, prefix: str = "") -> bool:
    if dst.exists() and not force:
        return False
    dst.parent.mkdir(parents=True, exist_ok=True)
    tmp = dst.with_suffix(dst.suffix + ".tmp")
    print(f"{prefix}downloading {url}")
    try:
        with urllib.request.urlopen(url, timeout=60) as response, tmp.open("wb") as out:
            shutil.copyfileobj(response, out)
    except urllib.error.URLError as err:
        tmp.unlink(missing_ok=True)
        raise SystemExit(f"download failed for {url}: {err}") from err
    tmp.replace(dst)
    return True


def ensure_coco_annotations(cache: Path, annotations_url: str, force: bool) -> Path:
    if force:
        shutil.rmtree(cache, ignore_errors=True)
    cache.mkdir(parents=True, exist_ok=True)

    annotations_json = cache / "annotations" / "instances_val2014.json"
    if annotations_json.exists() and not force:
        print(f"✓ using existing COCO annotations at {annotations_json}")
        return annotations_json

    archive = cache / "annotations_trainval2014.zip"
    download_file(annotations_url, archive, force=force)
    print(f"→ extracting {annotations_json.name}")
    with zipfile.ZipFile(archive) as zf:
        zf.extract("annotations/instances_val2014.json", cache)
    return annotations_json


def labels_from_coco_instances(annotations_json: Path, image_dir: Path) -> tuple[set[Path], set[Path]]:
    data = json.loads(annotations_json.read_text(encoding="utf-8"))
    images_by_id: dict[int, Path] = {}
    for image in data.get("images", []) or []:
        if not isinstance(image, dict) or "id" not in image:
            continue
        file_name = str(image.get("file_name", "")).split("/")[-1]
        images_by_id[int(image["id"])] = image_dir / file_name

    person_category_ids = {
        int(cat["id"])
        for cat in data.get("categories", []) or []
        if isinstance(cat, dict) and str(cat.get("name", "")).lower() == "person" and "id" in cat
    }
    if not person_category_ids:
        raise SystemExit(f"no person category found in {annotations_json}")

    person_image_ids: set[int] = set()
    for ann in data.get("annotations", []) or []:
        if not isinstance(ann, dict):
            continue
        try:
            if int(ann.get("category_id", -1)) in person_category_ids:
                person_image_ids.add(int(ann["image_id"]))
        except Exception:
            continue

    person = {images_by_id[image_id] for image_id in person_image_ids if image_id in images_by_id}
    scene = {path for image_id, path in images_by_id.items() if image_id not in person_image_ids}
    return person, scene


def download_images(paths: list[Path], image_base_url: str, force: bool) -> None:
    base = image_base_url.rstrip("/")
    total = len(paths)
    for idx, path in enumerate(paths, 1):
        prefix = f"[{idx:03d}/{total:03d}] "
        url = f"{base}/{path.name}"
        downloaded = download_file(url, path, force=force, prefix=prefix)
        status = "done" if downloaded else "cached"
        print(f"{prefix}{status} {path.name}")


def choose(paths: set[Path], count: int, rng: random.Random, label: str) -> list[Path]:
    seq = sorted(paths)
    rng.shuffle(seq)
    if len(seq) < count:
        raise SystemExit(f"not enough {label} images: need {count}, found {len(seq)}")
    return seq[:count]


def write_image(src: Path, dst: Path, quality: int) -> None:
    dst.parent.mkdir(parents=True, exist_ok=True)
    with Image.open(src) as img:
        img = ImageOps.exif_transpose(img).convert("RGB")
        img = ImageOps.fit(img, (128, 128), method=Image.Resampling.BILINEAR, centering=(0.5, 0.5))
        img.save(dst, "JPEG", quality=quality, optimize=True)


def populate_pack(out: Path, source: str, splits: dict[str, list[Path]], quality: int) -> None:
    tmp = out.with_name(out.name + ".tmp")
    shutil.rmtree(tmp, ignore_errors=True)
    manifest = {
        "version": 1,
        "source": source,
        "image_size": [128, 128],
        "preprocess": "center cover-crop to 128x128 RGB",
        "datasets": {"person-vs-scene": {"train": {"person": [], "scene": []}, "test": {"person": [], "scene": []}}},
    }
    for key, paths in splits.items():
        split, label = key.split("/")
        for idx, src in enumerate(paths, 1):
            name = f"{idx:03d}-{src.stem}.jpg"
            rel = f"{split}/{label}/{name}"
            write_image(src, tmp / rel, quality)
            manifest["datasets"]["person-vs-scene"][split][label].append({"path": rel, "name": name})

    (tmp / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    shutil.rmtree(out, ignore_errors=True)
    tmp.rename(out)


def main() -> int:
    args = parse_args()
    cache = Path(args.cache)
    out = Path(args.out)
    annotations_json = ensure_coco_annotations(cache, args.annotations_url, args.force_download)
    image_dir = cache / "val2014"
    person, scene = labels_from_coco_instances(annotations_json, image_dir)

    rng = random.Random(args.seed)
    train_person = choose(person, args.train_person, rng, "person training")
    person_remaining = person - set(train_person)
    val_person = choose(person_remaining, args.val_person, rng, "person validation")

    train_scene = choose(scene, args.train_scene, rng, "scene training")
    scene_remaining = scene - set(train_scene)
    val_scene = choose(scene_remaining, args.val_scene, rng, "scene validation")
    selected = train_person + val_person + train_scene + val_scene
    download_images(selected, args.image_base_url, args.force_download)

    populate_pack(
        out,
        args.source,
        {
            "train/person": train_person,
            "train/scene": train_scene,
            "test/person": val_person,
            "test/scene": val_scene,
        },
        args.jpeg_quality,
    )
    print(f"✓ wrote {out}")
    print(f"  train/person={len(train_person)} train/scene={len(train_scene)}")
    print(f"  test/person={len(val_person)} test/scene={len(val_scene)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
