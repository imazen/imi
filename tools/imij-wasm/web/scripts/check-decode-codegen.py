#!/usr/bin/env python3
"""Compare the Explorer's linked Wasm hot-path instruction counts."""

import argparse
from collections import Counter
from difflib import unified_diff
from pathlib import Path
import re
import subprocess
import sys

KERNELS = (
    "dct::idct8x8",
    "simd::recon_block_row",
    "recon_stripe_mode::<false>",
    "recon_stripe_mode::<true>",
    "recon_unit::<false>",
    "simd::xyb_row_linear",
    "color_chunk_row",
    "recon_portable::go16::<3usize, false>",
    "v4::decode::parsed_into::<fn() -> bool>",
)
LEGACY_TILE_KERNELS = ("recon_stripe_mode::<false>", "recon_unit::<false>")
WIDE_ROWS = (
    "StripeCore<u32>",
    "StripeState<u32>",
    "OwnedStripe<u32>",
    "wide::out_wasm128::write_row32",
)
NEAR_ROWS = re.compile(r"::go(?:16|32)::<[1-4]usize, true>")
SYMBOL = re.compile(r'  \(func \$"imij\[[^\]]+\]::([^"]+)"')
COLUMNS = (
    "variant", "kernel", "instructions", "f32x4", "scalar_f32", "shuffle",
    "vector_load", "vector_store", "calls", "opcode_counts",
)


def disassemble(module, wasm_tools, baseline, exact_models, v4_only):
    functions = {}
    wide_rows = []
    near_rows = []
    counter = None
    with subprocess.Popen([wasm_tools, "print", str(module)], text=True,
                          stdout=subprocess.PIPE, encoding="utf-8") as process:
        for line in process.stdout:
            if line.startswith("  (func "):
                if baseline and any(row in line for row in WIDE_ROWS):
                    wide_rows.append(line.strip())
                if exact_models and NEAR_ROWS.search(line):
                    near_rows.append(line.strip())
                name = SYMBOL.match(line)
                counter = None
                kernel = name[1].removeprefix("lossy::") if name else None
                if kernel in KERNELS:
                    if kernel in functions:
                        raise ValueError(f"duplicate {kernel} in {module}")
                    counter = functions[kernel] = Counter()
            elif line.startswith("  )"):
                counter = None
            elif counter is not None and line.startswith("    "):
                instruction = line.strip().split(maxsplit=1)
                if instruction and not instruction[0].startswith(("(", ";")):
                    counter[instruction[0]] += 1
        if process.wait():
            raise ValueError(f"wasm-tools print failed for {module}")
    if wide_rows:
        raise ValueError(f"wide row bodies in baseline {module}: {', '.join(wide_rows)}")
    if near_rows:
        raise ValueError(f"near-lossless row bodies in exact-model {module}: {', '.join(near_rows)}")
    expected = set(KERNELS)
    if v4_only:
        legacy = functions.keys() & set(LEGACY_TILE_KERNELS)
        if legacy:
            raise ValueError(f"legacy tile row bodies in V4-only {module}: {', '.join(sorted(legacy))}")
        expected.difference_update(LEGACY_TILE_KERNELS)
    missing = expected - functions.keys()
    if missing:
        raise ValueError(f"missing hot paths in {module}: {', '.join(sorted(missing))}")
    return functions


def signature(ops):
    return (
        sum(ops.values()),
        sum(n for op, n in ops.items() if op.startswith("f32x4.")),
        sum(n for op, n in ops.items() if op.startswith("f32.")),
        ops["i8x16.shuffle"],
        sum(n for op, n in ops.items() if op.startswith("v128.load")),
        sum(n for op, n in ops.items() if op.startswith("v128.store")),
        ops["call"] + ops["call_indirect"],
        " ".join(f"{op}={n}" for op, n in sorted(ops.items())),
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("dist", type=Path, help="directory with pkg-simd and pkg-relaxed")
    parser.add_argument("--wasm-tools", default="wasm-tools")
    parser.add_argument("--wasm-opt", type=Path,
                        default=Path(__file__).resolve().parents[1] / "node_modules/.bin/wasm-opt")
    parser.add_argument("--baseline", action="store_true", help="reject wide row bodies")
    parser.add_argument("--exact-models", action="store_true", help="reject near-lossless row bodies")
    parser.add_argument("--v4-only", action="store_true", help="reject legacy tile row bodies")
    parser.add_argument("--print-signatures", action="store_true", help="emit a TSV for review")
    args = parser.parse_args()
    kernels = tuple(k for k in KERNELS if not args.v4_only or k not in LEGACY_TILE_KERNELS)
    lines = ["\t".join(COLUMNS)]
    for variant in ("simd", "relaxed"):
        package = args.dist / f"pkg-{variant}"
        named = package / "imij_profile.wasm"
        shipped = package / "imij_bg.wasm"
        stripped = subprocess.check_output([args.wasm_opt, "--strip-debug", named, "-o", "-"])
        if stripped != shipped.read_bytes():
            raise ValueError(f"stripping the profile does not reproduce the shipped {variant} module")
        functions = disassemble(named, args.wasm_tools, args.baseline, args.exact_models, args.v4_only)
        for name in kernels:
            lines.append("\t".join((variant, name, *map(str, signature(functions[name])))))
    actual = "\n".join(lines) + "\n"
    if args.print_signatures:
        print(actual, end="")
        return 0
    snapshot = "decode-codegen-v4.tsv" if args.v4_only else "decode-codegen.tsv"
    path = Path(__file__).with_name(snapshot)
    expected = path.read_text()
    if actual != expected:
        sys.stderr.writelines(unified_diff(expected.splitlines(keepends=True),
                                          actual.splitlines(keepends=True),
                                          fromfile=str(path), tofile=str(args.dist)))
        return 1
    print(f"Verified {len(kernels)} hot paths in both shipped Explorer SIMD packages")
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        sys.exit(str(error))
