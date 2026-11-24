#!/usr/bin/env python3
"""Generates tools/imij-wasm/web/site/spec.html from SPEC.md and imij-set/FORMAT.md
with inline nested-rectangle wire diagrams and literate spec-proof badges.

Unified spec: diagrams are injected at the relevant spec sections, not in a
separate atlas. Wire examples and proof banner stay at the top as quick reference."""
import html, os, re, sys
from pathlib import Path

root = Path(__file__).resolve().parents[4]
spec_md = (root / "SPEC.md").read_text(encoding="utf-8")
v4_md = (root / "SPEC_V4.md").read_text(encoding="utf-8")
set_md = (root / "imij-set" / "FORMAT.md").read_text(encoding="utf-8")
# Canonical page origin of the published site (pages are separate from the tile/data origin).
page_origin = os.environ.get("IMI_PAGE_ORIGIN", "https://imi.imazen.org").rstrip("/")
out_path = Path(sys.argv[1]) if len(sys.argv) > 1 else Path(__file__).resolve().parent / "spec.html"

toc = [
    (2, "Quick Reference", "quick-ref"),
    (3, "Wire Examples by Image Type", "diag-examples"),
]


def slugify(text):
    return re.sub(r"[^a-z0-9]+", "-", text.lower()).strip("-")


def inline_fmt(text):
    esc = html.escape(text)
    esc = re.sub(r"`([^`]+)`", r"<code>\1</code>", esc)
    esc = re.sub(r"\*\*([^*]+)\*\*", r"<strong>\1</strong>", esc)
    esc = re.sub(
        r"\[([^\]]+)\]\(([^)]+)\)",
        lambda m: f'<a href="{html.escape(m.group(2))}">{m.group(1)}</a>',
        esc,
    )
    return esc


def source_links(rendered):
    """Keep repository Markdown/source references usable in the static site."""
    return re.sub(r'href="((?:SPEC[^"#]*\.md|docs/[^"#]+|imij/[^"#]+|imij-set/[^"#]+|tools/[^"#]+)(?:#[^"]*)?)"',
                  lambda m: 'href="https://github.com/imazen/zqi/blob/main/' + m[1] + '"', rendered)


# ═══════════════════════════════════════════════════════════════════════
# Diagram blocks — injected inline at the matching spec heading
# ═══════════════════════════════════════════════════════════════════════

DIAGRAM_CONTAINER = r"""
<div class="wire-box b-file" style="margin:16px 0 24px">
  <div class="wire-label">.imi File</div>
  <div class="wire-row">
    <div class="wire-field b-sig" style="flex:0 0 120px">
      <span class="f-label">Signature</span>
      <span class="f-size">8 bytes</span>
      <code class="f-hex">49 4D 49 4A 0D 0A 87 0A</code>
    </div>
    <div class="wire-field b-len" style="flex:0 0 80px">
      <span class="f-label">header_len</span>
      <span class="f-size">varint</span>
    </div>
    <div class="wire-box b-hdr" style="flex:1 1 260px">
      <a class="wire-label wire-link" href="#imi-1-0-header">Header (§1.0)</a>
      <div class="wire-row">
        <div class="wire-field b-meta" style="flex:0 0 42px">
          <span class="f-label">ver</span>
          <span class="f-size">u8</span>
          <span class="f-val">= 3</span>
        </div>
        <div class="wire-field b-feat" style="flex:0 0 56px">
          <span class="f-label">present</span>
          <span class="f-size">varint</span>
        </div>
        <div class="wire-field b-feat" style="flex:1 1 auto">
          <span class="f-label">fields by present bits</span>
          <span class="f-size">features, coding, sample, channels, ...</span>
        </div>
        <div class="wire-field b-geom" style="flex:0 0 52px">
          <span class="f-label">width</span>
          <span class="f-size">varint</span>
        </div>
        <div class="wire-field b-geom" style="flex:0 0 52px">
          <span class="f-label">height</span>
          <span class="f-size">varint</span>
        </div>
        <div class="wire-field b-rec" style="flex:0 0 62px">
          <span class="f-label">records</span>
          <span class="f-size">id+len*</span>
        </div>
      </div>
    </div>
    <div class="wire-box b-chunk" style="flex:1 1 200px">
      <a class="wire-label wire-link" href="#imi-1-1-chunks">Chunks (§1.1)</a>
      <div class="wire-row">
        <div class="wire-field b-meta" style="flex:0 0 auto">
          <span class="f-label">iCCP</span>
        </div>
        <div class="wire-field b-meta" style="flex:0 0 auto">
          <span class="f-label">eXIf</span>
        </div>
        <div class="wire-field b-data" style="flex:1 1 auto">
          <a class="f-label wire-link" href="#imi-2-data-chunk" style="color:#bbf7d0">DATA</a>
          <span class="f-size">pixel payload</span>
        </div>
        <div class="wire-field b-meta" style="flex:0 0 auto">
          <span class="f-label">uIDX</span>
        </div>
      </div>
    </div>
  </div>
</div>
"""

DIAGRAM_HEADER = r"""
<div class="atlas-note" style="margin:12px 0 16px">The <code>present</code> bitmask says which fields are written. A clear bit means the field takes its immutable default &mdash; <strong>0 bytes on the wire</strong>. An 8-bit sRGB RGB lossless image header is just 4 bytes: <code>02 00 W H</code>.</div>
<div class="field-table" style="margin:0 0 24px">
  <div class="ft-row ft-header">
    <span class="ft-bit">Bit</span>
    <span class="ft-name">Field</span>
    <span class="ft-type">Type</span>
    <span class="ft-default">Default</span>
    <span class="ft-desc">Values</span>
  </div>
  <div class="ft-row">
    <span class="ft-bit">&mdash;</span>
    <span class="ft-name"><code>version</code></span>
    <span class="ft-type">u8</span>
    <span class="ft-default">always</span>
    <span class="ft-desc">Writers use <strong>3</strong>; readers also accept 2</span>
  </div>
  <div class="ft-row">
    <span class="ft-bit">&mdash;</span>
    <span class="ft-name"><code>present</code></span>
    <span class="ft-type">varint</span>
    <span class="ft-default">always</span>
    <span class="ft-desc">Bitmask of which fields follow</span>
  </div>
  <div class="ft-row b-feat-row">
    <span class="ft-bit">2</span>
    <span class="ft-name"><code>features</code></span>
    <span class="ft-type">LEB128</span>
    <span class="ft-default">none</span>
    <span class="ft-desc">Capability mask, bits 0..26: <a href="#imi-0-1-features">signed_samples, wide_samples, lossy, quick_pixels, ...</a></span>
  </div>
  <div class="ft-row b-cod-row">
    <span class="ft-bit">0</span>
    <span class="ft-name"><code>coding</code></span>
    <span class="ft-type">u8</span>
    <span class="ft-default">0</span>
    <span class="ft-desc"><strong>0</strong> lossless &middot; <strong>1</strong> lossy XYB &middot; <strong>2</strong> lossy YCoCg</span>
  </div>
  <div class="ft-row b-pix-row">
    <span class="ft-bit">1</span>
    <span class="ft-name"><code>sample</code></span>
    <span class="ft-type">u8</span>
    <span class="ft-default">0</span>
    <span class="ft-desc">
      <span class="st-baseline">u8<sup>0</sup></span>
      <span class="st-baseline">u16<sup>1</sup></span>
      <span class="st-baseline">f16<sup>2</sup></span>
      <span class="st-baseline">f32<sup>5</sup></span>
      <span class="st-signed">i8<sup>6</sup></span>
      <span class="st-signed">i16<sup>7</sup></span>
      <span class="st-ext">u32<sup>3</sup></span>
      <span class="st-ext">i32<sup>4</sup></span>
      <span class="st-wide">u64<sup>8</sup></span>
      <span class="st-wide">i64<sup>9</sup></span>
      <span class="st-wide">f64<sup>10</sup></span>
    </span>
  </div>
  <div class="ft-row b-pix-row">
    <span class="ft-bit">3</span>
    <span class="ft-name"><code>channels</code></span>
    <span class="ft-type">u8</span>
    <span class="ft-default">3</span>
    <span class="ft-desc">
      <span class="ch-base">Gray<sup>1</sup></span>
      <span class="ch-base">GrayA<sup>2</sup></span>
      <span class="ch-base">RGB<sup>3</sup></span>
      <span class="ch-base">RGBA<sup>4</sup></span>
      <span class="ch-ext">CMYK<sup>5</sup></span>
      <span class="ch-ext">Oklab<sup>6</sup></span>
      <span class="ch-ext">OklabA<sup>7</sup></span>
    </span>
  </div>
  <div class="ft-row">
    <span class="ft-bit">4</span>
    <span class="ft-name"><code>orientation</code></span>
    <span class="ft-type">u8</span>
    <span class="ft-default">1</span>
    <span class="ft-desc">EXIF 1..8</span>
  </div>
  <div class="ft-row">
    <span class="ft-bit">5</span>
    <span class="ft-name"><code>decode_speed</code></span>
    <span class="ft-type">u8</span>
    <span class="ft-default">1</span>
    <span class="ft-desc"><strong>0</strong> compact &middot; <strong>1</strong> balanced &middot; <strong>2</strong> fast &middot; <strong>3</strong> fastest</span>
  </div>
  <div class="ft-row b-col-row">
    <span class="ft-bit">6</span>
    <span class="ft-name"><code>cicp</code></span>
    <span class="ft-type">4&times;u8</span>
    <span class="ft-default">sRGB</span>
    <span class="ft-desc">primaries, transfer, matrix, full_range</span>
  </div>
  <div class="ft-row">
    <span class="ft-bit">7</span>
    <span class="ft-name"><code>sig_bits</code></span>
    <span class="ft-type">C&times;u8</span>
    <span class="ft-default">all</span>
    <span class="ft-desc">Significant bits per channel</span>
  </div>
  <div class="ft-row">
    <span class="ft-bit">8</span>
    <span class="ft-name"><code>alpha_mode</code></span>
    <span class="ft-type">u8</span>
    <span class="ft-default">0</span>
    <span class="ft-desc"><strong>0</strong> straight &middot; <strong>1</strong> premul &middot; <strong>2</strong> opaque &middot; <strong>3</strong> padding</span>
  </div>
</div>
"""

DIAGRAM_CHUNKS = r"""
<div class="grammar-grid" style="margin:16px 0 20px">
  <div class="grammar-card">
    <h4>Chunk Framing</h4>
    <div class="wire-box b-chunk">
      <div class="wire-label">Chunk</div>
      <div class="wire-row">
        <div class="wire-field b-sig" style="flex:0 0 70px">
          <span class="f-label">tag</span>
          <span class="f-size">4 ASCII</span>
        </div>
        <div class="wire-field b-len" style="flex:0 0 70px">
          <span class="f-label">length</span>
          <span class="f-size">varint</span>
        </div>
        <div class="wire-field b-data" style="flex:1">
          <span class="f-label">payload</span>
          <span class="f-size">length bytes</span>
        </div>
      </div>
    </div>
    <div class="tag-case">
      <span class="tc"><strong>1st letter:</strong> <span class="tc-crit">UPPER</span> = critical &middot; <span class="tc-anc">lower</span> = ancillary</span>
      <span class="tc"><strong>2nd:</strong> <span class="tc-crit">UPPER</span> = public &middot; <span class="tc-anc">lower</span> = private</span>
      <span class="tc"><strong>4th:</strong> <span class="tc-crit">UPPER</span> = unsafe to copy &middot; <span class="tc-anc">lower</span> = safe</span>
    </div>
  </div>
  <div class="grammar-card">
    <h4>Record Framing</h4>
    <div class="wire-box b-rec-box">
      <div class="wire-label">Record</div>
      <div class="wire-row">
        <div class="wire-field b-feat" style="flex:0 0 60px">
          <span class="f-label">id</span>
          <span class="f-size">varint</span>
        </div>
        <div class="wire-field b-len" style="flex:0 0 60px">
          <span class="f-label">len</span>
          <span class="f-size">varint</span>
        </div>
        <div class="wire-field b-data" style="flex:1">
          <span class="f-label">payload</span>
          <span class="f-size">len bytes</span>
        </div>
      </div>
    </div>
    <div class="tag-case">
      <span class="tc"><strong>id parity:</strong> <span class="tc-crit">odd</span> = critical (unknown &rarr; Unsupported)</span>
      <span class="tc"><strong>id parity:</strong> <span class="tc-anc">even</span> = ancillary (unknown &rarr; skipped)</span>
      <span class="tc">IDs strictly increase within each list</span>
    </div>
  </div>
</div>
<div class="chunk-catalog" style="margin:0 0 24px">
  <h4>Known Chunk Tags</h4>
  <div class="cc-grid">
    <div class="cc-item cc-crit"><code>DATA</code> <span>Pixel payload (exactly once)</span></div>
    <div class="cc-item cc-anc"><code>iCCP</code> <span>ICC color profile</span></div>
    <div class="cc-item cc-anc"><code>eXIf</code> <span>Exif metadata</span></div>
    <div class="cc-item cc-anc"><code>xMPp</code> <span>XMP packet</span></div>
    <div class="cc-item cc-anc"><code>mDCv</code> <span>ST 2086 mastering display</span></div>
    <div class="cc-item cc-anc"><code>dWHI</code> <span>Diffuse white luminance</span></div>
    <div class="cc-item cc-anc"><code>cLLI</code> <span>Content light levels</span></div>
    <div class="cc-item cc-crit"><code>pRVW</code> <span>Preview (nested .imi)</span></div>
    <div class="cc-item cc-crit"><code>gMAP</code> <span><a href="#imi-1-2-gain-map-gmap">Gain map</a> (ISO 21496-1)</span></div>
    <div class="cc-item cc-crit"><code>aNIM</code> <span><a href="#imi-1-3-frames-and-pages-anim-fram">Animation / pages</a> control</span></div>
    <div class="cc-item cc-crit"><code>fRAM</code> <span>Further frame / page</span></div>
    <div class="cc-item cc-anc"><code>cSUM</code> <span><a href="#imi-1-4-file-checksum-csum">File checksum</a> (XXH3-128)</span></div>
    <div class="cc-item cc-anc"><code>pSUM</code> <span><a href="#imi-1-4-1-decoded-bitmap-checksum-psum">Decoded bitmap</a> XXH3-128 digest &mdash; verifies exact pixel decode</span></div>
    <div class="cc-item cc-anc"><code>cALB</code> <span><a href="#imi-1-5-science-records-calb-axes-cord-smdt-catg">Calibration</a> &amp; units</span></div>
    <div class="cc-item cc-crit"><code>uIDX</code> <span>Trailing unit index</span></div>
  </div>
</div>
"""

DIAGRAM_LOSSLESS = r"""
<div class="wire-box b-data-outer" style="margin:16px 0 20px">
  <div class="wire-label">DATA Chunk &mdash; Lossless</div>
  <div class="wire-row" style="flex-wrap:wrap; gap:6px">
    <div class="wire-box b-head" style="flex:1 1 100%">
      <div class="wire-label">Payload Head</div>
      <div class="wire-row">
        <div class="wire-field b-meta" style="flex:0 0 auto">
          <span class="f-label">records_len</span>
        </div>
        <div class="wire-field b-rec" style="flex:1">
          <span class="f-label">head records</span>
          <span class="f-size">unit table, near-lossless bounds, bilevel, template palette, ...</span>
        </div>
      </div>
    </div>
    <div class="wire-box b-stripe" style="flex:1 1 30%">
      <div class="wire-label">Stripe 0</div>
      <div class="wire-row" style="flex-wrap:wrap;gap:4px">
        <div class="wire-field b-pred" style="flex:1 1 100%">
          <a class="f-label wire-link" href="#imi-2-1-channel-maps" style="color:#c7d2fe">channel maps &amp; predictors</a>
          <span class="f-size">per-channel: mode, predictor, decorrelation</span>
        </div>
        <div class="wire-field b-rans" style="flex:1 1 100%">
          <a class="f-label wire-link" href="#imi-3-entropy-coding-interleaved-rans" style="color:#f5d0fe">16-lane rANS stream</a>
          <span class="f-size">SIMD-parallel entropy decode</span>
        </div>
      </div>
    </div>
    <div class="wire-box b-stripe" style="flex:1 1 30%">
      <div class="wire-label">Stripe 1</div>
      <div class="wire-row" style="flex-wrap:wrap;gap:4px">
        <div class="wire-field b-pred" style="flex:1 1 100%">
          <span class="f-label">channel maps</span>
        </div>
        <div class="wire-field b-rans" style="flex:1 1 100%">
          <span class="f-label">16-lane rANS</span>
        </div>
      </div>
    </div>
    <div class="wire-field b-stripe-etc" style="flex:0 0 auto; align-self:stretch">
      <span class="f-label">&hellip;</span>
    </div>
    <div class="wire-box b-stripe" style="flex:1 1 20%">
      <div class="wire-label">Stripe N</div>
      <div class="wire-row" style="flex-wrap:wrap;gap:4px">
        <div class="wire-field b-pred" style="flex:1 1 100%">
          <span class="f-label">maps</span>
        </div>
        <div class="wire-field b-rans" style="flex:1 1 100%">
          <span class="f-label">rANS</span>
        </div>
      </div>
    </div>
  </div>
</div>
"""

DIAGRAM_LOSSY = r"""
<div class="wire-box b-data-outer b-lossy-outer" style="margin:16px 0 24px">
  <div class="wire-label">DATA Chunk &mdash; Lossy</div>
  <div class="wire-row" style="flex-wrap:wrap; gap:6px">
    <div class="wire-box b-head" style="flex:1 1 100%">
      <div class="wire-label">Lossy Payload Head</div>
      <div class="wire-row">
        <div class="wire-field b-meta" style="flex:0 0 auto">
          <span class="f-label">records_len</span>
        </div>
        <div class="wire-field b-cod" style="flex:0 0 auto">
          <span class="f-label">entropy</span>
        </div>
        <div class="wire-field b-cod" style="flex:0 0 auto">
          <span class="f-label">matrix</span>
        </div>
        <div class="wire-field b-rec" style="flex:1">
          <span class="f-label">head records: AQ, exact, bias, transform, ...</span>
        </div>
      </div>
    </div>
    <div class="wire-box b-dc-nest" style="flex:1 1 45%">
      <div class="wire-label">DC Units (nested lossless .imi)</div>
      <div class="wire-row" style="flex-wrap:wrap;gap:4px">
        <div class="wire-field b-pred" style="flex:1 1 100%">
          <span class="f-label">8&times;8 block means &rarr; lossless image</span>
        </div>
        <div class="wire-field b-rans" style="flex:1 1 100%">
          <span class="f-label">16-lane rANS coded DC</span>
        </div>
      </div>
    </div>
    <div class="wire-box b-ac" style="flex:1 1 45%">
      <div class="wire-label">AC Units</div>
      <div class="wire-row" style="flex-wrap:wrap;gap:4px">
        <div class="wire-field b-cod" style="flex:1 1 100%">
          <span class="f-label">coefficients: RunLevel rANS or SplitHuf</span>
        </div>
        <div class="wire-field b-feat" style="flex:1 1 100%">
          <span class="f-label">per-block 8&times;8 / adaptive 4&times;4 transform</span>
        </div>
      </div>
    </div>
    <div class="wire-box b-alpha-nest" style="flex:1 1 40%">
      <div class="wire-label">Alpha Units (if RGBA/GrayA/OklabA)</div>
      <div class="wire-row">
        <div class="wire-field b-pred" style="flex:1">
          <span class="f-label">nested lossless alpha image</span>
        </div>
      </div>
    </div>
    <div class="wire-box b-exact-nest" style="flex:1 1 40%">
      <div class="wire-label">Exact Units (if exact_blocks)</div>
      <div class="wire-row">
        <div class="wire-field b-feat" style="flex:1">
          <span class="f-label">lossless recovery of selected blocks</span>
        </div>
      </div>
    </div>
  </div>
</div>
"""

DIAGRAM_RANS = r"""
<div class="detail-box" style="margin:16px 0 24px">
  <h4>16-Lane Interleaved rANS</h4>
  <p>16 independent rANS state machines are interleaved so a single <strong>AVX-512 register</strong> (or two AVX2 / four NEON registers) decodes all 16 lanes in lockstep. Each lane refills from the bitstream independently &mdash; no serial bit-level dependencies.</p>
  <div class="wire-box b-rans-detail">
    <div class="wire-label">rANS Stream Layout</div>
    <div class="wire-row">
      <div class="wire-field b-rans" style="flex:0 0 80px">
        <span class="f-label">table</span>
        <span class="f-size">CDF entries</span>
      </div>
      <div class="wire-field b-rans-lane" style="flex:1">
        <span class="f-label">lane 0</span>
      </div>
      <div class="wire-field b-rans-lane" style="flex:1">
        <span class="f-label">lane 1</span>
      </div>
      <div class="wire-field b-rans-lane" style="flex:1">
        <span class="f-label">lane 2</span>
      </div>
      <div class="wire-field b-rans-lane-etc" style="flex:0 0 30px">
        <span class="f-label">&hellip;</span>
      </div>
      <div class="wire-field b-rans-lane" style="flex:1">
        <span class="f-label">lane 15</span>
      </div>
    </div>
  </div>
</div>
"""

DIAGRAM_IMIJSET = r"""
<div class="wire-box b-archive" style="margin:16px 0 16px">
  <div class="wire-label">.imijset File</div>
  <div class="wire-row">
    <div class="wire-box b-btree" style="flex:1 1 60%">
      <div class="wire-label">B-Tree Pages</div>
      <div class="wire-row">
        <div class="wire-field b-data" style="flex:1">
          <span class="f-label">leaf pages</span>
          <span class="f-size">.imi blobs by key</span>
        </div>
        <div class="wire-field b-meta" style="flex:0 0 auto">
          <span class="f-label">internal pages</span>
        </div>
      </div>
    </div>
    <div class="wire-field b-commit" style="flex:0 0 140px">
      <span class="f-label">Tail Commit</span>
      <span class="f-size">136 bytes @ commit end</span>
      <span class="f-val">root, count, gen</span>
      <span class="f-label" style="color:#4ade80;font-weight:700">XXH3-128</span>
    </div>
  </div>
</div>
<div class="detail-box" style="margin:0 0 16px;padding:10px 16px">
  <strong style="color:#4ade80">XXH3-128</strong> integrity: the footer checks its own fields with an accidental-corruption checksum and names the root page digest. Pages and blobs are checked as they are read; opening the archive does not verify every descendant. Version 2 footers are 136 bytes; version 1 footers (80 bytes, XXH3-64) remain readable.
</div>
<div class="wire-box b-morton" style="margin:0 0 24px">
  <div class="wire-label">Key 0: Pyramid Catalog (IMIJPAT\x01)</div>
  <div class="wire-row">
    <div class="wire-field b-sig" style="flex:0 0 100px">
      <span class="f-label">magic</span>
      <code class="f-hex">IMIJPAT\x01</code>
    </div>
    <div class="wire-field b-geom" style="flex:1">
      <span class="f-label">level descriptors</span>
      <span class="f-size">divisor, dims, tile size, order (Row/Morton), base_key, key_count</span>
    </div>
  </div>
  <p class="atlas-note" style="margin:8px 0 0;padding:0 8px">Spatial access: <code>base_key + morton_2d(tx, ty)</code> clusters 2D viewports into contiguous leaf ranges for O(log n) lookups.</p>
</div>
"""

# Heading slug → diagram HTML to inject after it
HEADING_DIAGRAMS = {
    "imi-1-container": DIAGRAM_CONTAINER,
    "imi-1-0-header": DIAGRAM_HEADER,
    "imi-1-1-chunks": DIAGRAM_CHUNKS,
    "imi-2-data-chunk": DIAGRAM_LOSSLESS,
    "imi-2-8-lossy-payload-head": DIAGRAM_LOSSY,
    "imi-3-entropy-coding-interleaved-rans": DIAGRAM_RANS,
}

SET_HEADING_DIAGRAMS = {
    "set-imij-set-version-2-experimental-not-frozen": DIAGRAM_IMIJSET,
}


def render_markdown(md_text, prefix="", diagram_map=None):
    if diagram_map is None:
        diagram_map = {}
    lines = md_text.splitlines()
    in_code = False
    in_table = False
    in_list = False
    para_buf = []
    out = []

    def flush_para():
        nonlocal para_buf
        if para_buf:
            out.append(f"<p>{' '.join(para_buf)}</p>")
            para_buf = []

    for line in lines:
        if line.startswith("```"):
            flush_para()
            if in_list:
                out.append("</ul>")
                in_list = False
            if in_table:
                out.append("</tbody></table>")
                in_table = False
            if not in_code:
                in_code = True
                out.append("<pre><code>")
            else:
                in_code = False
                out.append("</code></pre>")
            continue
        if in_code:
            out.append(html.escape(line))
            continue

        m = re.match(r"^(#{1,4})\s+(.*)$", line)
        if m:
            flush_para()
            if in_list:
                out.append("</ul>")
                in_list = False
            if in_table:
                out.append("</tbody></table>")
                in_table = False
            level = len(m.group(1))
            title = m.group(2).strip()
            anchor = prefix + slugify(title)
            if level in (2, 3):
                toc.append((level, title, anchor))
            tag = f"h{min(level + 1, 4)}"
            out.append(f'<{tag} id="{anchor}">{inline_fmt(title)}</{tag}>')
            if anchor in diagram_map:
                out.append(diagram_map[anchor])
            continue

        if line.strip().startswith("|") and line.strip().endswith("|"):
            flush_para()
            if in_list:
                out.append("</ul>")
                in_list = False
            cells = [c.strip() for c in line.strip()[1:-1].split("|")]
            if all(re.match(r"^:?-+:?$", c) for c in cells if c):
                continue
            if not in_table:
                in_table = True
                out.append(
                    "<table><thead><tr>"
                    + "".join(f"<th>{inline_fmt(c)}</th>" for c in cells)
                    + "</tr></thead><tbody>"
                )
            else:
                out.append(
                    "<tr>"
                    + "".join(f"<td>{inline_fmt(c)}</td>" for c in cells)
                    + "</tr>"
                )
            continue
        else:
            if in_table:
                flush_para()
                out.append("</tbody></table>")
                in_table = False

        if line.startswith("- ") or line.startswith("* "):
            flush_para()
            if not in_list:
                in_list = True
                out.append("<ul>")
            out.append(f"<li>{inline_fmt(line[2:].strip())}</li>")
            continue
        else:
            if in_list and not line.strip():
                flush_para()
                out.append("</ul>")
                in_list = False

        if not line.strip():
            flush_para()
            continue

        para_buf.append(inline_fmt(line))

    flush_para()
    if in_list:
        out.append("</ul>")
    if in_table:
        out.append("</tbody></table>")
    if in_code:
        out.append("</code></pre>")
    return "\n".join(out)


v4_html = source_links(render_markdown(v4_md, "v4-"))
v4_html = re.sub(r'href="#([^"]+)"', r'href="#v4-\1"', v4_html)
spec_html = source_links(render_markdown(spec_md, "imi-", HEADING_DIAGRAMS).replace('href="docs/VALIDATION.md"', 'href="./validation.html"'))
set_html = source_links(render_markdown(set_md, "set-", SET_HEADING_DIAGRAMS))
set_html = re.sub(r'href="#([^"]+)"', r'href="#set-\1"', set_html)

V4_WIRE = '''
<section id="v4-overview" aria-label="V4 wire overview">
  <h2>V4: independent groups in contiguous spatial tiles</h2>
  <p>The header declares interpretation and required capabilities. DATA carries
  immutable setup, then framed objects. A spatial tile stores several independent
  decode groups together; one byte range can supply multiple worker jobs.</p>
  <div class="wire-box b-file"><span class="wire-label">V4 file</span>
    <div class="wire-row">
      <div class="wire-field b-sig"><span class="f-label">Signature + header length</span></div>
      <div class="wire-field b-meta"><span class="f-label">Header: version 4</span><span>profile, interpretation, capabilities, dimensions</span></div>
    </div>
    <div class="wire-box"><span class="wire-label">DATA</span>
      <div class="wire-field b-meta"><span class="f-label">Setup + directory placement</span><span>coding geometry, tile layout/order, defaults, banks, integrity policy</span></div>
      <div class="wire-box"><span class="wire-label">Tile in declared disk order</span>
        <div class="wire-row">
          <div class="wire-field b-feat"><span class="f-label">Begin</span><span>identity + optional exact-block plan</span></div>
          <div class="wire-field b-meta"><span class="f-label">Typed model definitions</span><span>immutable IDs; payload or next-main lifetime</span></div>
          <div class="wire-field b-geom"><span class="f-label">DC / alpha / AC / exact groups</span><span>framed identity, resolved models and local entropy</span></div>
          <div class="wire-field b-feat"><span class="f-label">End</span><span>tile completion + optional digest</span></div>
        </div>
      </div>
      <p>Numeric images use the same tile framing with independent numeric groups.
      Each object can carry an XXH3-128 digest. Lossy numeric dependencies have
      explicit geometry; AC contexts do not depend on decoding an earlier group.</p>
    </div>
    <div class="wire-row">
      <div class="wire-field b-meta"><span class="f-label">Optional outer records</span><span>metadata, collections, encoded and bitmap checksums</span></div>
      <div class="wire-field b-feat"><span class="f-label">Tail directory, when declared</span><span>final chunk; a front directory is also supported</span></div>
    </div>
  </div>
  <p>Settings resolve image defaults → selected reusable settings → group changes
  → cell changes. Short forms inherit defaults; they do not inherit mutable
  entropy state. Directory offsets are checked against physical object framing.
  Pixel delivery and final validation remain distinct.</p>
</section>
'''

toc_html = "\n".join(
    f'<a href="#{anchor}" style="padding-left:{(lvl - 2) * 12 + 6}px">{html.escape(title)}</a>'
    for lvl, title, anchor in toc
)

WIRE_EXAMPLES_HTML = r"""
<section id="quick-ref" class="wire-atlas">

  <div class="proof-banner">
    <span class="proof-badge">WIRE CHECKS</span>
    <div>
      <strong>Selected wire rules checked against the implementation</strong>
      <p>The registry and wire tests check assigned IDs, signature, header fields and unit ordinals. These checks are not a proof of the entire specification. <a href="./validation.html">Strict v3 validation</a> fully decodes every recognized image and verifies every declared checksum; missing checksums remain absent.</p>
    </div>
  </div>

  <h3 id="diag-examples">Wire Examples by Image Type</h3>
  <p class="atlas-note">Each example shows the exact bytes on the wire for common configurations. The <code>present</code> mask controls which <a href="#imi-1-0-header">header fields</a> appear; most fields take their defaults at zero cost.</p>

  <div class="examples-grid">
    <div class="example-card">
      <div class="ex-title">
        <span class="ex-tag ex-lossless">Lossless</span>
        <strong>RGB u8</strong>
        <span class="ex-subtitle">Simplest &mdash; 4-byte header</span>
      </div>
      <div class="ex-wire">
        <div class="wire-box b-mini">
          <div class="wire-row">
            <div class="wire-field b-sig"><span class="f-label">sig</span><span class="f-size">8B</span></div>
            <div class="wire-field b-len"><span class="f-label">hdr_len</span></div>
            <div class="wire-field b-meta"><span class="f-label">ver=2</span></div>
            <div class="wire-field b-feat"><span class="f-label">present=0</span></div>
            <div class="wire-field b-geom"><span class="f-label">W</span></div>
            <div class="wire-field b-geom"><span class="f-label">H</span></div>
            <div class="wire-field b-data" style="flex:1"><span class="f-label">DATA: lossless rANS stripes</span></div>
          </div>
        </div>
      </div>
      <div class="ex-hex"><code>49 4D 49 4A 0D 0A 87 0A <em>04</em> 02 00 <em>W H</em> &middot; DATA ...</code></div>
      <p class="ex-note">present=0x00: all defaults (coding=lossless, sample=u8, channels=RGB, cicp=sRGB).</p>
    </div>

    <div class="example-card">
      <div class="ex-title">
        <span class="ex-tag ex-lossless">Lossless</span>
        <strong>RGBA u16</strong>
        <span class="ex-subtitle">6 header bytes</span>
      </div>
      <div class="ex-wire">
        <div class="wire-box b-mini">
          <div class="wire-row">
            <div class="wire-field b-sig"><span class="f-label">sig</span><span class="f-size">8B</span></div>
            <div class="wire-field b-len"><span class="f-label">hdr_len</span></div>
            <div class="wire-field b-meta"><span class="f-label">ver=2</span></div>
            <div class="wire-field b-feat"><span class="f-label">present=0x0A</span></div>
            <div class="wire-field b-pix"><span class="f-label">sample=1</span><span class="f-size">u16</span></div>
            <div class="wire-field b-pix"><span class="f-label">chan=4</span><span class="f-size">RGBA</span></div>
            <div class="wire-field b-geom"><span class="f-label">W</span></div>
            <div class="wire-field b-geom"><span class="f-label">H</span></div>
            <div class="wire-field b-data" style="flex:1"><span class="f-label">DATA</span></div>
          </div>
        </div>
      </div>
      <p class="ex-note">present bits 1+3 set (sample, channels differ from defaults).</p>
    </div>

    <div class="example-card">
      <div class="ex-title">
        <span class="ex-tag ex-lossy">Lossy</span>
        <strong>RGB u8 YCoCg</strong>
        <span class="ex-subtitle">Lossy with nested DC</span>
      </div>
      <div class="ex-wire">
        <div class="wire-box b-mini">
          <div class="wire-row">
            <div class="wire-field b-sig"><span class="f-label">sig</span></div>
            <div class="wire-field b-len"><span class="f-label">hdr_len</span></div>
            <div class="wire-field b-meta"><span class="f-label">ver=2</span></div>
            <div class="wire-field b-feat"><span class="f-label">present=0x01</span></div>
            <div class="wire-field b-cod"><span class="f-label">coding=2</span><span class="f-size">YCoCg</span></div>
            <div class="wire-field b-geom"><span class="f-label">W H</span></div>
            <div class="wire-field b-data" style="flex:1"><span class="f-label">DATA: <a href="#imi-2-8-lossy-payload-head" class="wire-link" style="color:#bbf7d0">lossy head</a> + DC/AC units</span></div>
          </div>
        </div>
      </div>
      <p class="ex-note">present bit 0 only (coding=YCoCg). <code>lossy</code> feature is implied by coding, not in mask.</p>
    </div>

    <div class="example-card">
      <div class="ex-title">
        <span class="ex-tag ex-lossless">Lossless</span>
        <strong>F32 Gray HDR</strong>
        <span class="ex-subtitle">Scientific float + CICP PQ</span>
      </div>
      <div class="ex-wire">
        <div class="wire-box b-mini">
          <div class="wire-row">
            <div class="wire-field b-sig"><span class="f-label">sig</span></div>
            <div class="wire-field b-len"><span class="f-label">hdr_len</span></div>
            <div class="wire-field b-meta"><span class="f-label">ver=2</span></div>
            <div class="wire-field b-feat"><span class="f-label">present=0x4A</span></div>
            <div class="wire-field b-pix"><span class="f-label">sample=5</span><span class="f-size">f32</span></div>
            <div class="wire-field b-pix"><span class="f-label">chan=1</span><span class="f-size">Gray</span></div>
            <div class="wire-field b-col"><span class="f-label">cicp</span><span class="f-size">9,16,0,1</span></div>
            <div class="wire-field b-geom"><span class="f-label">W H</span></div>
            <div class="wire-field b-data" style="flex:1"><span class="f-label">DATA</span></div>
          </div>
        </div>
      </div>
      <p class="ex-note">present bits 1+3+6 (sample=f32, channels=Gray, cicp=PQ). <code>samples_32</code> implied by f32.</p>
    </div>

    <div class="example-card">
      <div class="ex-title">
        <span class="ex-tag ex-ext">Extended</span>
        <strong>Oklab f16</strong>
        <span class="ex-subtitle">Perceptual color space</span>
      </div>
      <div class="ex-wire">
        <div class="wire-box b-mini">
          <div class="wire-row">
            <div class="wire-field b-sig"><span class="f-label">sig</span></div>
            <div class="wire-field b-len"><span class="f-label">hdr_len</span></div>
            <div class="wire-field b-meta"><span class="f-label">ver=2</span></div>
            <div class="wire-field b-feat"><span class="f-label">present=0x0E</span></div>
            <div class="wire-field b-feat"><span class="f-label">features</span><span class="f-size">pixel_models</span></div>
            <div class="wire-field b-pix"><span class="f-label">sample=2</span><span class="f-size">f16</span></div>
            <div class="wire-field b-pix"><span class="f-label">chan=6</span><span class="f-size">Oklab</span></div>
            <div class="wire-field b-geom"><span class="f-label">W H</span></div>
            <div class="wire-field b-data" style="flex:1"><span class="f-label">DATA</span></div>
          </div>
        </div>
      </div>
      <p class="ex-note">Channels 5-7 require feature <code>pixel_models</code> (bit 21) in the <a href="#imi-0-1-features">feature mask</a>.</p>
    </div>

    <div class="example-card">
      <div class="ex-title">
        <span class="ex-tag ex-ext">Extended</span>
        <strong>i64 signed wide</strong>
        <span class="ex-subtitle">64-bit integer detector data</span>
      </div>
      <div class="ex-wire">
        <div class="wire-box b-mini">
          <div class="wire-row">
            <div class="wire-field b-sig"><span class="f-label">sig</span></div>
            <div class="wire-field b-len"><span class="f-label">hdr_len</span></div>
            <div class="wire-field b-meta"><span class="f-label">ver=2</span></div>
            <div class="wire-field b-feat"><span class="f-label">present=0x06</span></div>
            <div class="wire-field b-feat"><span class="f-label">features</span><span class="f-size">wide_samples</span></div>
            <div class="wire-field b-pix"><span class="f-label">sample=9</span><span class="f-size">i64</span></div>
            <div class="wire-field b-geom"><span class="f-label">W H</span></div>
            <div class="wire-field b-data" style="flex:1"><span class="f-label">DATA</span></div>
          </div>
        </div>
      </div>
      <p class="ex-note">64-bit samples coded as two 32-bit words (<a href="#imi-2-1-channel-maps">§2.1</a>). <code>wide_samples</code> + <code>samples_32</code> required.</p>
    </div>
  </div>

</section>
"""

page = f"""<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width,initial-scale=1">
  <title>IMI &amp; IMIJ-Set Wire Specification</title>
  <link rel="canonical" href="{page_origin}/spec/">
  <script src="../imi/site-origin.js"></script>
  <link rel="stylesheet" href="./style.css">
  <style>
    /* ─── Wire Diagram Styles (inline, not in style.css) ─── */
    .wire-atlas {{
      margin: 24px 0 48px;
    }}
    .wire-atlas h3 {{
      font: 700 20px/1.3 var(--sans);
      color: var(--ink);
      margin: 48px 0 8px;
      padding-bottom: 8px;
      border-bottom: 2px solid var(--line);
    }}
    .wire-atlas h3:first-of-type {{ margin-top: 24px; }}
    .wire-atlas h4 {{
      font: 600 16px/1.3 var(--sans);
      color: var(--ink);
      margin: 20px 0 8px;
    }}
    .atlas-note {{
      font: 400 14.5px/1.65 var(--sans);
      color: var(--muted);
      margin: 0 0 16px;
    }}

    /* Override style.css 80ch cap */
    .spec-body p,
    .spec-body li {{
      max-width: none;
    }}

    /* ─── Proof Banner ─── */
    .proof-banner {{
      display: flex;
      gap: 16px;
      align-items: center;
      padding: 14px 18px;
      background: linear-gradient(135deg, #1a2e1a 0%, #1a2518 100%);
      border: 1px solid #3d6b3d;
      border-radius: 10px;
      margin-bottom: 28px;
    }}
    .proof-banner p {{ margin: 4px 0 0; font: 400 13px/1.5 var(--sans); color: var(--muted); }}
    .proof-badge {{
      font: 700 11px var(--mono);
      background: #4ade80;
      color: #0a1f0a;
      padding: 6px 12px;
      border-radius: 6px;
      letter-spacing: 1px;
      flex: none;
      white-space: nowrap;
    }}

    /* ─── Nested Box Diagrams ─── */
    .wire-box {{
      border: 2px solid var(--line);
      border-radius: 8px;
      padding: 4px;
      background: var(--bg);
      position: relative;
    }}
    .wire-label {{
      font: 700 11px/1 var(--mono);
      letter-spacing: 0.5px;
      padding: 3px 8px;
      border-radius: 4px;
      margin: 2px 2px 4px;
      display: inline-block;
      color: #fff;
      background: #374151;
    }}
    a.wire-link {{
      text-decoration: none;
      cursor: pointer;
    }}
    a.wire-link:hover {{
      text-decoration: underline;
    }}
    .wire-row {{
      display: flex;
      gap: 4px;
      padding: 2px;
      align-items: stretch;
    }}
    .wire-field {{
      display: flex;
      flex-direction: column;
      align-items: center;
      justify-content: center;
      gap: 1px;
      padding: 6px 8px;
      border-radius: 5px;
      border: 1.5px solid;
      min-width: 40px;
      text-align: center;
      transition: transform 0.1s, box-shadow 0.1s;
    }}
    .wire-field:hover {{
      transform: translateY(-1px);
      box-shadow: 0 3px 12px #0006;
    }}
    .f-label {{
      font: 600 11.5px/1.2 var(--mono);
      color: var(--ink);
      white-space: nowrap;
    }}
    .f-size {{
      font: 400 10px/1.1 var(--mono);
      color: var(--muted);
      white-space: nowrap;
    }}
    .f-val {{
      font: 700 10px/1.1 var(--mono);
      white-space: nowrap;
    }}
    .f-hex {{
      font: 400 9.5px/1.1 var(--mono);
      white-space: nowrap;
    }}

    /* ─── Color Palette (vibrant, distinct) ─── */
    .b-file {{ border-color: #64748b; background: #0f172a; }}
    .b-file > .wire-label {{ background: #475569; }}

    .b-sig {{ background: #312e81; border-color: #6366f1; color: #c7d2fe; }}
    .b-sig .f-label {{ color: #e0e7ff; }}
    .b-sig .f-hex {{ color: #a5b4fc; }}

    .b-len {{ background: #1e1b4b; border-color: #818cf8; }}
    .b-len .f-label {{ color: #c7d2fe; }}

    .b-hdr {{ border-color: #0ea5e9; background: #0c1929; }}
    .b-hdr > .wire-label {{ background: #0284c7; }}

    .b-meta {{ background: #1a2332; border-color: #38bdf8; }}
    .b-meta .f-label {{ color: #bae6fd; }}
    .b-meta .f-val {{ color: #7dd3fc; }}

    .b-feat {{ background: #2a1a0a; border-color: #f59e0b; }}
    .b-feat .f-label {{ color: #fde68a; }}
    .b-feat .f-size {{ color: #d4a04a; }}

    .b-geom {{ background: #0a2a1a; border-color: #34d399; }}
    .b-geom .f-label {{ color: #a7f3d0; }}

    .b-rec {{ background: #1a0a2a; border-color: #a78bfa; }}
    .b-rec .f-label {{ color: #ddd6fe; }}
    .b-rec .f-size {{ color: #a78bfa; }}

    .b-chunk {{ border-color: #64748b; background: #111827; }}
    .b-chunk > .wire-label {{ background: #4b5563; }}

    .b-data {{ background: #0a2818; border-color: #22c55e; }}
    .b-data .f-label {{ color: #bbf7d0; }}
    .b-data .f-size {{ color: #6ee7b7; }}

    .b-pix {{ background: #1a0a2a; border-color: #c084fc; }}
    .b-pix .f-label {{ color: #e9d5ff; }}
    .b-pix .f-size {{ color: #c084fc; }}

    .b-col {{ background: #0a1a30; border-color: #60a5fa; }}
    .b-col .f-label {{ color: #bfdbfe; }}
    .b-col .f-size {{ color: #93c5fd; }}

    .b-cod {{ background: #2a1a00; border-color: #eab308; }}
    .b-cod .f-label {{ color: #fef08a; }}
    .b-cod .f-size {{ color: #d4a04a; }}

    /* Payload specific */
    .b-data-outer {{ border-color: #22c55e; background: #071a0e; }}
    .b-data-outer > .wire-label {{ background: #16a34a; }}
    .b-lossy-outer {{ border-color: #f97316; background: #1a0f05; }}
    .b-lossy-outer > .wire-label {{ background: #ea580c; }}

    .b-head {{ border-color: #6366f1; background: #0f0f2a; }}
    .b-head > .wire-label {{ background: #4f46e5; }}

    .b-stripe {{ border-color: #06b6d4; background: #071a20; }}
    .b-stripe > .wire-label {{ background: #0891b2; }}
    .b-stripe-etc {{ background: #111; border-color: #444; display:flex; align-items:center; padding: 8px; }}

    .b-pred {{ background: #1a1a30; border-color: #818cf8; }}
    .b-pred .f-label {{ color: #c7d2fe; }}
    .b-pred .f-size {{ color: #a5b4fc; }}

    .b-rans {{ background: #1a0020; border-color: #e879f9; }}
    .b-rans .f-label {{ color: #f5d0fe; }}
    .b-rans .f-size {{ color: #d946ef; }}

    .b-rans-detail {{ margin-top: 8px; }}
    .b-rans-lane {{ background: #2a0030; border-color: #e879f9; }}
    .b-rans-lane .f-label {{ color: #f0abfc; font-size: 10px; }}
    .b-rans-lane-etc {{ background: #1a0020; border-color: #444; display:flex; align-items:center; }}

    .b-dc-nest {{ border-color: #3b82f6; background: #0a1530; }}
    .b-dc-nest > .wire-label {{ background: #2563eb; }}
    .b-ac {{ border-color: #f59e0b; background: #1a1505; }}
    .b-ac > .wire-label {{ background: #d97706; }}
    .b-alpha-nest {{ border-color: #a78bfa; background: #150a2a; }}
    .b-alpha-nest > .wire-label {{ background: #7c3aed; }}
    .b-exact-nest {{ border-color: #f43f5e; background: #1a0a0a; }}
    .b-exact-nest > .wire-label {{ background: #e11d48; }}

    .b-rec-box {{ border-color: #a78bfa; background: #0f0a20; }}
    .b-rec-box > .wire-label {{ background: #7c3aed; }}

    .b-archive {{ border-color: #8b5cf6; background: #0f0a20; }}
    .b-archive > .wire-label {{ background: #7c3aed; }}
    .b-btree {{ border-color: #a78bfa; background: #150f30; }}
    .b-btree > .wire-label {{ background: #6d28d9; }}
    .b-commit {{ background: #2a0a30; border-color: #d946ef; }}
    .b-commit .f-label {{ color: #f5d0fe; }}
    .b-commit .f-size {{ color: #e879f9; }}
    .b-commit .f-val {{ color: #d946ef; font-weight: 400; }}

    .b-morton {{ border-color: #6366f1; background: #0f0f2a; }}
    .b-morton > .wire-label {{ background: #4f46e5; }}

    .b-mini {{ border: 1.5px solid #334155; background: #0f172a; padding: 2px; }}
    .b-mini .wire-row {{ gap: 3px; }}
    .b-mini .wire-field {{ padding: 4px 6px; }}

    /* ─── Detail box ─── */
    .detail-box {{
      background: var(--panel);
      border: 1px solid var(--line);
      border-radius: 10px;
      padding: 16px 20px;
      margin: 16px 0 24px;
    }}
    .detail-box h4 {{ margin: 0 0 6px; }}
    .detail-box p {{ font: 400 14px/1.65 var(--sans); color: var(--muted); margin: 0 0 12px; }}

    /* ─── Grammar cards ─── */
    .grammar-grid {{
      display: grid;
      grid-template-columns: repeat(auto-fit, minmax(360px, 1fr));
      gap: 16px;
      margin: 16px 0 24px;
    }}
    .grammar-card {{
      background: var(--panel);
      border: 1px solid var(--line);
      border-radius: 10px;
      padding: 16px;
    }}
    .grammar-card h4 {{ margin: 0 0 10px; font-size: 15px; }}
    .tag-case {{
      margin-top: 10px;
      display: flex;
      flex-direction: column;
      gap: 3px;
    }}
    .tc {{ font: 400 12.5px/1.4 var(--sans); color: var(--muted); }}
    .tc-crit {{ color: #fbbf24; font-weight: 600; }}
    .tc-anc {{ color: #86efac; font-weight: 600; }}

    /* ─── Chunk catalog ─── */
    .chunk-catalog {{
      margin: 16px 0 24px;
      background: var(--panel);
      border: 1px solid var(--line);
      border-radius: 10px;
      padding: 16px;
    }}
    .chunk-catalog h4 {{ margin: 0 0 12px; font-size: 15px; }}
    .cc-grid {{
      display: grid;
      grid-template-columns: repeat(auto-fill, minmax(260px, 1fr));
      gap: 6px;
    }}
    .cc-item {{
      display: flex;
      align-items: baseline;
      gap: 8px;
      padding: 5px 8px;
      border-radius: 5px;
      font: 400 13px/1.4 var(--sans);
      color: var(--muted);
    }}
    .cc-item a {{ color: inherit; }}
    .cc-item a:hover {{ color: var(--accent); }}
    .cc-item code {{
      font: 700 12px var(--mono);
      flex: none;
    }}
    .cc-crit {{ background: #fbbf2410; }}
    .cc-crit code {{ color: #fbbf24; }}
    .cc-anc {{ background: #86efac10; }}
    .cc-anc code {{ color: #86efac; }}

    /* ─── Field table ─── */
    .field-table {{
      margin: 16px 0 28px;
      border: 1px solid var(--line);
      border-radius: 10px;
      overflow: hidden;
      background: var(--panel);
    }}
    .ft-row {{
      display: grid;
      grid-template-columns: 48px 120px 72px 72px 1fr;
      gap: 0;
      padding: 0;
      border-bottom: 1px solid var(--line);
      font: 400 13.5px/1.5 var(--sans);
      align-items: center;
    }}
    .ft-row:last-child {{ border-bottom: none; }}
    .ft-header {{
      background: var(--panel-alt);
      font-weight: 700;
      font-size: 11.5px;
      text-transform: uppercase;
      letter-spacing: 0.5px;
      color: var(--muted);
    }}
    .ft-row > span {{
      padding: 8px 10px;
    }}
    .ft-bit {{ font: 600 13px var(--mono); color: var(--accent); text-align: center; }}
    .ft-name {{ font-family: var(--mono); font-weight: 600; color: var(--ink); }}
    .ft-type {{ font: 400 12px var(--mono); color: var(--muted); }}
    .ft-default {{ font: 400 12px var(--mono); color: var(--muted); }}
    .ft-desc {{ color: var(--muted); font-size: 13px; }}
    .ft-desc a {{ color: var(--muted); }}
    .ft-desc a:hover {{ color: var(--accent); }}
    .ft-desc strong {{ color: var(--ink); }}

    .b-feat-row {{ background: #f59e0b08; }}
    .b-cod-row {{ background: #eab30808; }}
    .b-pix-row {{ background: #c084fc08; }}
    .b-col-row {{ background: #60a5fa08; }}

    /* Sample & channel type badges */
    .st-baseline, .st-signed, .st-ext, .st-wide,
    .ch-base, .ch-ext {{
      display: inline-block;
      padding: 1px 6px;
      border-radius: 3px;
      font: 600 11.5px/1.4 var(--mono);
      margin: 1px 2px;
    }}
    .st-baseline {{ background: #22c55e20; color: #86efac; border: 1px solid #22c55e44; }}
    .st-signed {{ background: #f59e0b18; color: #fde68a; border: 1px solid #f59e0b44; }}
    .st-ext {{ background: #3b82f618; color: #93c5fd; border: 1px solid #3b82f644; }}
    .st-wide {{ background: #e879f918; color: #f5d0fe; border: 1px solid #e879f944; }}
    .ch-base {{ background: #22c55e20; color: #86efac; border: 1px solid #22c55e44; }}
    .ch-ext {{ background: #e879f918; color: #f5d0fe; border: 1px solid #e879f944; }}

    .st-baseline sup, .st-signed sup, .st-ext sup, .st-wide sup,
    .ch-base sup, .ch-ext sup {{
      font-size: 9px;
      opacity: 0.7;
      margin-left: 1px;
    }}

    /* ─── Wire Examples ─── */
    .examples-grid {{
      display: grid;
      grid-template-columns: repeat(auto-fit, minmax(440px, 1fr));
      gap: 16px;
      margin: 16px 0 24px;
    }}
    .example-card {{
      background: var(--panel);
      border: 1px solid var(--line);
      border-radius: 10px;
      padding: 16px;
      display: flex;
      flex-direction: column;
      gap: 10px;
    }}
    .ex-title {{
      display: flex;
      align-items: baseline;
      gap: 8px;
      flex-wrap: wrap;
    }}
    .ex-title strong {{
      font: 700 16px var(--sans);
      color: var(--ink);
    }}
    .ex-subtitle {{
      font: 400 13px var(--sans);
      color: var(--muted);
    }}
    .ex-tag {{
      font: 700 10px var(--mono);
      padding: 3px 8px;
      border-radius: 4px;
      letter-spacing: 0.5px;
      text-transform: uppercase;
      flex: none;
    }}
    .ex-lossless {{ background: #22c55e20; color: #4ade80; border: 1px solid #22c55e55; }}
    .ex-lossy {{ background: #f9731620; color: #fb923c; border: 1px solid #f9731655; }}
    .ex-ext {{ background: #8b5cf620; color: #a78bfa; border: 1px solid #8b5cf655; }}
    .ex-wire {{ overflow-x: auto; }}
    .ex-hex {{
      font: 400 12px/1.4 var(--mono);
      color: var(--muted);
      padding: 6px 10px;
      background: var(--bg);
      border-radius: 5px;
      border: 1px solid var(--line);
      overflow-x: auto;
      white-space: nowrap;
    }}
    .ex-hex em {{ color: var(--accent); font-style: normal; font-weight: 600; }}
    .ex-note {{
      font: 400 12.5px/1.5 var(--sans);
      color: var(--muted);
      margin: 0;
    }}
    .ex-note a {{ color: var(--muted); }}
    .ex-note a:hover {{ color: var(--accent); }}

    @media (max-width: 860px) {{
      .wire-row {{ flex-wrap: wrap; }}
      .examples-grid {{ grid-template-columns: 1fr; }}
      .grammar-grid {{ grid-template-columns: 1fr; }}
      .ft-row {{ grid-template-columns: 36px 90px 56px 56px 1fr; font-size: 12px; }}
      .cc-grid {{ grid-template-columns: 1fr; }}
    }}
  </style>
</head>
<body>
  <header class="topbar">
    <a class="brand" href="./index.html"><span class="mark">i</span> IMI <span>FORMAT</span> <small>IMI v4 · retained v2/v3 · archive v2</small></a>
    <nav class="nav" aria-label="IMI site navigation">
      <a href="./index.html">IMI Home</a>
      <a href="./why.html">Why IMI?</a>
      <a href="./encode.html">Encoder Lab</a>
      <a href="./benchmarks.html">Benchmarks</a>
      <a href="../gigapixel/">Gigapixel Explorer</a>
      <a href="../codestream-atlas/">3D Codestream Atlas</a>
      <a href="./spec.html" aria-current="page">Format Spec</a>
    </nav>
  </header>

  <main class="container">
    <section class="hero">
      <p class="eyebrow">Wire Specification · V4 and retained formats</p>
      <h1>IMI (.imi) &amp; IMIJ-Set (.imijset) Wire Specification</h1>
      <p class="lede">
        V4 defines the current image grammar. Retained V2/V3 and archive grammars
        follow below. The production reader powers the <a href="../codestream-atlas/">Codestream Atlas</a>;
        the independent <code>imij-oracle</code> decoder covers V2/V3 lossless syntax.
        Implementation status and qualification limits appear in the V4 specification.
      </p>
    </section>

    <div class="spec-layout" style="margin-top:24px">
      <aside class="spec-toc" aria-label="Specification table of contents">
        <input id="toc-filter" type="search" placeholder="Filter sections…" aria-label="Filter sections">
        <div id="toc-links">{toc_html}</div>
      </aside>
      <div class="spec-body">
        {V4_WIRE}
        {v4_html}
        <hr style="border:0;border-top:1px solid var(--line);margin:48px 0">
        <h2 id="legacy-spec">Retained V2/V3 grammar and historical wire examples</h2>
        {WIRE_EXAMPLES_HTML}
        {spec_html}
        <hr style="border:0;border-top:1px solid var(--line);margin:48px 0">
        {set_html}
      </div>
    </div>
  </main>
  <script>
    const input = document.getElementById('toc-filter');
    const links = [...document.querySelectorAll('#toc-links a')];
    input.addEventListener('input', () => {{
      const q = input.value.trim().toLowerCase();
      for (const a of links) a.style.display = (q && !a.textContent.toLowerCase().includes(q)) ? 'none' : '';
    }});
  </script>
</body>
</html>
"""

out_path.write_text(page, encoding="utf-8")
print(f"Wrote {out_path} ({len(page)} bytes, {len(toc)} TOC entries)")
