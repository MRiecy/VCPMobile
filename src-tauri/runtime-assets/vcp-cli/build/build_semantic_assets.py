#!/usr/bin/env python3
"""Build the frozen compact tokenizer pack and verify the shipped static model.

The script is a build-time utility only. Android never ships or invokes Python.
"""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import shutil
import statistics
import struct
import sys
import tempfile


MAGIC = b"VCPBPE1\0"
PRETOKEN_PATTERN = r"[^\r\n\p{L}\p{N}]?[\p{Lu}\p{Lt}\p{Lm}\p{Lo}\p{M}]*[\p{Ll}\p{Lm}\p{Lo}\p{M}]+(?i:'s|'t|'re|'ve|'m|'ll|'d)?|[^\r\n\p{L}\p{N}]?[\p{Lu}\p{Lt}\p{Lm}\p{Lo}\p{M}]+[\p{Ll}\p{Lm}\p{Lo}\p{M}]*(?i:'s|'t|'re|'ve|'m|'ll|'d)?|\p{N}{1,3}| ?[^\s\p{L}\p{N}]+[\r\n/]*|\s*[\r\n]+|\s+(?!\S)|\s+"
PROFILE = Path(__file__).resolve().parents[1] / "semantic-profile.json"
OUTPUT_ROOT = Path(__file__).resolve().parents[1]


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        while chunk := source.read(64 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


def require_asset(path: Path, expected_bytes: int, expected_hash: str, label: str) -> None:
    if not path.is_file() or path.is_symlink():
        raise ValueError(f"{label} must be a direct regular file")
    if path.stat().st_size != expected_bytes:
        raise ValueError(f"{label} byte size does not match semantic-profile.json")
    if sha256(path) != expected_hash:
        raise ValueError(f"{label} SHA-256 does not match semantic-profile.json")


def build_pack(source: Path, destination: Path, expected_median_token_length: int) -> None:
    tokenizer = json.loads(source.read_text(encoding="utf-8"))
    expected_pretokenizer = {
        "type": "Sequence",
        "pretokenizers": [
            {
                "type": "Split",
                "pattern": {"Regex": PRETOKEN_PATTERN},
                "behavior": "Isolated",
                "invert": False,
            },
            {
                "type": "ByteLevel",
                "add_prefix_space": True,
                "trim_offsets": True,
                "use_regex": False,
            },
        ],
    }
    expected_added_tokens = [
        {
            "id": 179934,
            "content": "<|endoftext|>",
            "single_word": True,
            "lstrip": True,
            "rstrip": True,
            "normalized": True,
            "special": True,
        }
    ]
    if (
        tokenizer.get("version") != "1.0"
        or tokenizer.get("normalizer") is not None
        or tokenizer.get("pre_tokenizer") != expected_pretokenizer
        or tokenizer.get("post_processor") is not None
        or tokenizer.get("decoder")
        != {
            "type": "ByteLevel",
            "add_prefix_space": True,
            "trim_offsets": True,
            "use_regex": True,
        }
        or tokenizer.get("added_tokens") != expected_added_tokens
        or tokenizer.get("truncation")
        != {
            "direction": "Right",
            "max_length": 32768,
            "strategy": "LongestFirst",
            "stride": 0,
        }
        or tokenizer.get("padding")
        != {
            "strategy": "BatchLongest",
            "direction": "Right",
            "pad_to_multiple_of": None,
            "pad_id": 179934,
            "pad_type_id": 0,
            "pad_token": "<|endoftext|>",
        }
    ):
        raise ValueError("unsupported tokenizer pre-tokenizer or added-token contract")
    model = tokenizer["model"]
    if (
        model.get("type") != "BPE"
        or model.get("ignore_merges") is not True
        or model.get("byte_fallback") is not False
        or model.get("unk_token") is not None
        or model.get("continuing_subword_prefix") is not None
        or model.get("end_of_word_suffix") is not None
        or model.get("dropout") is not None
        or model.get("fuse_unk") is not False
    ):
        raise ValueError("unsupported tokenizer BPE contract")

    vocab = sorted(model["vocab"].items(), key=lambda row: row[0].encode("utf-8"))
    if int(statistics.median(len(token) for token, _ in vocab)) != expected_median_token_length:
        raise ValueError("tokenizer median token length does not match semantic-profile.json")
    vocab_by_token = dict(vocab)
    if vocab_by_token.get("<|endoftext|>") != 179934:
        raise ValueError("tokenizer added token ID does not match the frozen contract")
    string_blob = bytearray()
    vocab_rows: list[tuple[int, int, int]] = []
    for token, token_id in vocab:
        encoded = token.encode("utf-8")
        vocab_rows.append((len(string_blob), len(encoded), int(token_id)))
        string_blob.extend(encoded)

    seen_pairs: set[int] = set()
    merge_rows: list[tuple[int, int, int]] = []
    for rank, pair in enumerate(model["merges"]):
        if not isinstance(pair, list) or len(pair) != 2:
            raise ValueError("invalid tokenizer merge row")
        left, right = pair
        left_id = int(vocab_by_token[left])
        right_id = int(vocab_by_token[right])
        new_id = int(vocab_by_token[left + right])
        key = (left_id << 32) | right_id
        if key in seen_pairs:
            raise ValueError("duplicate tokenizer merge pair")
        seen_pairs.add(key)
        merge_rows.append((key, rank, new_id))
    merge_rows.sort(key=lambda row: row[0])

    destination.parent.mkdir(parents=True, exist_ok=True)
    with destination.open("wb") as output:
        output.write(MAGIC)
        output.write(struct.pack("<IIII", len(vocab_rows), len(merge_rows), len(string_blob), 0))
        for row in vocab_rows:
            output.write(struct.pack("<III", *row))
        for row in merge_rows:
            output.write(struct.pack("<QII", *row))
        output.write(string_blob)
        output.flush()
        os.fsync(output.fileno())


def atomic_copy(source: Path, destination: Path) -> None:
    destination.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(dir=destination.parent, prefix=f".{destination.name}.", delete=False) as staging:
        staging_path = Path(staging.name)
        with source.open("rb") as input_file:
            shutil.copyfileobj(input_file, staging, length=64 * 1024)
        staging.flush()
        os.fsync(staging.fileno())
    os.replace(staging_path, destination)


def main() -> None:
    if len(sys.argv) != 2:
        raise SystemExit("usage: build_semantic_assets.py <upstream-model-directory>")
    source_root = Path(sys.argv[1]).resolve(strict=True)
    profile = json.loads(PROFILE.read_text(encoding="utf-8"))
    model_profile = profile["model"]
    tokenizer_profile = profile["tokenizerPack"]
    config_profile = profile["upstreamConfig"]

    source_model = source_root / "model.safetensors"
    source_tokenizer = source_root / "tokenizer.json"
    source_config = source_root / "config.json"
    require_asset(source_model, model_profile["bytes"], model_profile["sha256"], "model.safetensors")
    require_asset(
        source_tokenizer,
        tokenizer_profile["sourceBytes"],
        tokenizer_profile["sourceSha256"],
        "tokenizer.json",
    )
    require_asset(
        source_config,
        config_profile["bytes"],
        config_profile["sha256"],
        "config.json",
    )
    config = json.loads(source_config.read_text(encoding="utf-8"))
    if (
        config.get("model_type") != "model2vec"
        or config.get("hidden_dim") != profile["dimension"]
        or config.get("embedding_dtype") != "float16"
        or config.get("normalize") is not True
        or config.get("pooling") != "mean"
    ):
        raise ValueError("config.json does not match the frozen Model2Vec contract")

    model_destination = OUTPUT_ROOT / model_profile["asset"]
    tokenizer_destination = OUTPUT_ROOT / tokenizer_profile["asset"]
    atomic_copy(source_model, model_destination)
    with tempfile.NamedTemporaryFile(dir=tokenizer_destination.parent, prefix=".tokenizer.", delete=False) as staging:
        staging_path = Path(staging.name)
    try:
        build_pack(source_tokenizer, staging_path, profile["medianTokenLength"])
        os.replace(staging_path, tokenizer_destination)
    finally:
        staging_path.unlink(missing_ok=True)

    require_asset(model_destination, model_profile["bytes"], model_profile["sha256"], "shipped model")
    require_asset(
        tokenizer_destination,
        tokenizer_profile["bytes"],
        tokenizer_profile["sha256"],
        "shipped tokenizer pack",
    )
    print(f"verified {profile['modelId']}")


if __name__ == "__main__":
    main()
