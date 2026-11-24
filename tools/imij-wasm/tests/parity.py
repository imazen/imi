#!/usr/bin/env python3
"""imij-wasm parity harness: native vs wasmtime byte-identical encode/trace
output, plus lossless decode pixel-exactness.

Usage: parity.py NATIVE_BIN WASM_BIN FIXTUREDIR [--wasmtime-args "..."]
"""

import json
import subprocess
import sys
import tempfile
from pathlib import Path

WASMTIME = ["wasmtime", "--dir", "/"]

# (name, fixture, encode argv). Fixtures: file under FIXTUREDIR; raw files
# carry their --raw args inline.
CASES = [
    # --- every fixture through a default-mode encode ---
    ("gray8-ll", "gray8.png", "--lossless"),
    ("ga8-ll", "ga8.png", "--lossless"),
    ("rgb8-ll", "rgb8.png", "--lossless"),
    ("rgba8-ll", "rgba8.png", "--lossless"),
    ("odd-ll", "odd61x37.png", "--lossless"),
    ("gray16-ll", "gray16.png", "--lossless"),
    ("rgb16pq-ll", "rgb16-pq.png", "--lossless"),
    ("dither16-ll", "dither16.png", "--lossless --effort 10"),
    ("dither16-ll-e5", "dither16.png", "--lossless"),
    ("sparse16-ll", "sparse16.png", "--lossless"),
    ("bilevel-ll", "bilevel.png", "--lossless"),
    ("bilevel-ll-nopal", "bilevel.png", "--lossless --palette off"),
    # --- quick pixels (SPEC 2.13) and bit-smart byte planes (2.13.1) ---
    ("quick", "rgb8.png", "--quick"),
    ("quick-rgba", "rgba8.png", "--quick"),
    ("quick-bits-packed", "gray16.png", "--quick --quick-bits packed"),
    ("quick-bits-packets-shift", "rgb8.png", "--quick --quick-bits packets --sample u16 --expand shift"),
    ("quick-bits-packets-repl", "rgb8.png", "--quick --quick-bits packets --sample u16 --expand replicate"),
    ("rgb8-ly", "rgb8.png", "--lossy --quality 90"),
    ("rgba8-ly", "rgba8.png", "--lossy --quality 90"),
    ("odd-ly", "odd61x37.png", "--lossy --quality 90"),
    ("rgb16pq-ly", "rgb16-pq.png", "--lossy --quality 90"),
    # --- lossless option sweep ---
    ("ll-effort", "rgb8.png", "--lossless --effort 10"),
    ("ll-effort-lo", "rgb8.png", "--lossless --effort 0"),
    ("ll-stripes", "rgb8.png", "--lossless --stripe-rows 8"),
    ("ll-lanes", "rgb8.png", "--lossless --lanes 4"),
    ("ll-gcols", "rgb8.png", "--lossless --column-groups 24"),
    ("ll-nopal", "rgb8.png", "--lossless --palette off"),
    ("ll-decor", "rgb8.png", "--lossless --decor-mask 1"),
    ("ll-pred", "rgb8.png", "--lossless --predictor-mask 1"),
    # --- conversion matrix: channels ---
    ("cv-gray", "rgb8.png", "--lossless --channels gray"),
    ("cv-ga", "rgb8.png", "--lossless --channels ga"),
    ("cv-rgba", "rgb8.png", "--lossless --channels rgba"),
    ("cv-rgb-from-ga", "ga8.png", "--lossless --channels rgb"),
    # --- conversion: samples x expand ---
    ("cv-u16-shift", "rgb8.png", "--lossless --sample u16 --expand shift"),
    ("cv-u16-repl", "rgb8.png", "--lossless --sample u16 --expand replicate"),
    ("cv-u16-scale", "rgb8.png", "--lossless --sample u16 --expand scale"),
    ("cv-u32", "gray8.png", "--lossless --sample u32"),
    ("cv-i32", "gray8.png", "--lossless --sample i32"),
    ("cv-f16", "rgb8.png", "--lossless --sample f16"),
    ("cv-f32", "gray8.png", "--lossless --sample f32"),
    ("cv-u16-to-u8", "gray16.png", "--lossless --sample u8 --expand scale"),
    ("cv-u16-to-u8-sh", "gray16.png", "--lossless --sample u8 --expand shift"),
    # --- sbits / premultiplied / cicp ---
    ("sbits-10", "gray16.png", "--lossless --sbits 10 --sample u16 --expand shift"),
    ("sbits-12", "rgb16-pq.png", "--lossless --sbits 12,12,12"),
    ("sbits-broadcast", "rgba8.png", "--lossless --channels rgba --sample u16 --expand replicate --sbits 10"),
    ("sbits-broadcast-gray", "rgb8.png", "--lossless --channels gray --sample u16 --expand shift --sbits 12"),
    ("sbits-list-mismatch", "rgb8.png", "--lossless --channels rgba --sbits 10,10"),
    ("premult", "rgba8.png", "--lossless --premultiplied"),
    ("cicp-pq", "rgb16-pq.png", "--lossless --cicp 9,16"),
    # --- lossy option sweep ---
    ("ly-q30", "rgb8.png", "--lossy --quality 30"),
    ("ly-q100", "rgb8.png", "--lossy --quality 100"),
    ("ly-ycocg", "rgb8.png", "--lossy --quality 85 --color ycocg"),
    # Retired entropy modes: identical rejection on both tiers.
    ("ly-ent-pk", "rgb8.png", "--lossy --quality 85 --entropy packed"),
    ("ly-ent-sp", "rgb8.png", "--lossy --quality 85 --entropy split"),
    ("ly-ent-sh", "rgb8.png", "--lossy --quality 85 --entropy splithuf"),
    ("ly-ds-compact", "rgb8.png", "--lossy --quality 85 --decode-speed compact"),
    ("ly-ds-fast", "rgb8.png", "--lossy --quality 85 --decode-speed fast"),
    ("ly-ds-fastest", "rgb8.png", "--lossy --quality 85 --decode-speed fastest"),
    ("ly-nocompact", "rgb8.png", "--lossy --quality 85 --compact-streams off"),
    ("ly-sb4", "rgb8.png", "--lossy --quality 85 --stripe-block-rows 4"),
    ("ly-gcols", "rgb8.png", "--lossy --quality 85 --column-groups 32"),
    ("ly-aq", "rgb8.png", "--lossy --quality 85 --aq-gamma 0.3"),
    ("ly-layout", "rgb8.png",
     "--lossy --quality 85 --layout-2 dct4,h --layout-3 dct2,both"),
    ("ly-adaptive", "rgb8.png", "--lossy --quality 85 --layout-1 adaptive4"),
    ("ly-matrix", "rgb8.png", "--lossy --quality 85 --color ycocg --matrix 0"),
    ("ly-chanmul", "rgb8.png", "--lossy --quality 85 --channel-mul 1,1,1"),
    ("ly-round", "rgb8.png", "--lossy --quality 85 --round 0.5"),
    ("ly-16bit", "gray16.png", "--lossy --quality 85"),
    # --- raw-input paths ---
    ("raw-u32", "u32.rgba.raw", "--raw 48 32 rgba u32 --lossless"),
    ("raw-i32", "i32.rgb.raw", "--raw 48 32 rgb i32 --lossless"),
    ("raw-f16", "f16.rgb.raw", "--raw 64 48 rgb f16 --lossless"),
    ("raw-f32", "f32.rgb.raw", "--raw 48 32 rgb f32 --lossless"),
    ("raw-u32-cv", "u32.rgba.raw", "--raw 48 32 rgba u32 --lossless --sample u16 --expand shift"),
    # --- error paths: same rejection on both tiers ---
    ("err-badopt", "rgb8.png", "--lossy --quality 999"),
    ("err-sbits", "rgb8.png", "--lossless --sbits 9"),
    ("err-nofmt", "rgb8.png", "--lossy --channels rgb --sample u32 --quality 50"),
]


def run(bin_argv, data):
    return subprocess.run(
        bin_argv, input=data, capture_output=True, timeout=300
    )


def summary_no_time(stderr):
    line = stderr.decode(errors="replace").strip().splitlines()
    if not line:
        return ""
    try:
        d = json.loads(line[-1])
        d.pop("encode_ms", None)
        return json.dumps(d, sort_keys=True)
    except Exception:
        return line[-1]


def main():
    native, wasm, fxdir = sys.argv[1], sys.argv[2], Path(sys.argv[3])
    fails = []
    with tempfile.TemporaryDirectory() as td:
        for name, fixture, argstr in CASES:
            argv = argstr.split()
            data = (fxdir / fixture).read_bytes()
            rn = run([native, "encode", *argv], data)
            rw = run([*WASMTIME, wasm, "encode", *argv], data)
            same_out = rn.stdout == rw.stdout
            same_rc = (rn.returncode == 0) == (rw.returncode == 0)
            same_err = summary_no_time(rn.stderr) == summary_no_time(rw.stderr)
            ok = same_out and same_rc and same_err
            # Lossless pixel-exactness: the decoded pixels must equal the
            # converted bitmap byte-for-byte (convert-raw is the conversion
            # pipeline standalone).
            decode_ok = True
            if ok and rn.returncode == 0 and ("--lossless" in argstr or "--quick" in argstr):
                rd = run([native, "decode-raw"], rn.stdout)
                rdw = run([*WASMTIME, wasm, "decode-raw"], rn.stdout)
                conv = run([native, "convert-raw", *argv], data)
                convw = run([*WASMTIME, wasm, "convert-raw", *argv], data)
                decode_ok = (
                    rd.returncode == 0
                    and rd.stdout == rdw.stdout
                    and rd.stdout == conv.stdout == convw.stdout
                )
            # Trace parity.
            trace_ok = True
            if ok and rn.returncode == 0:
                tn = run([native, "trace"], rn.stdout)
                tw = run([*WASMTIME, wasm, "trace"], rn.stdout)
                trace_ok = tn.stdout == tw.stdout and tn.returncode == 0
            status = "ok" if (ok and decode_ok and trace_ok) else "FAIL"
            detail = []
            if not same_out:
                detail.append(
                    f"out {len(rn.stdout)}B vs {len(rw.stdout)}B")
            if not same_rc:
                detail.append(f"rc {rn.returncode} vs {rw.returncode}")
            if not same_err:
                detail.append(
                    f"stderr {summary_no_time(rn.stderr)!r} vs "
                    f"{summary_no_time(rw.stderr)!r}")
            if not decode_ok:
                detail.append("decode-raw mismatch")
            if not trace_ok:
                detail.append("trace mismatch")
            print(f"{status} {name}: {'; '.join(detail) if detail else ''}")
            if status == "FAIL":
                fails.append(name)
    print(f"\n{len(CASES) - len(fails)}/{len(CASES)} cases pass")
    return 1 if fails else 0


if __name__ == "__main__":
    sys.exit(main())
