# Presenting IMI

IMI is an experimental image format. V4 is implemented and remains unreleased.

## Live demonstration

https://gigapixels.imazen.org/

Start in Chrome with a visible tab, **Auto** resolution and **Lossy · target 70**.
Let the first view settle before discussing its decode speed: worker startup,
WASM compilation and the first download are part of the first experience.
Pan, zoom, switch quality, and enter fullscreen. The gallery is available while
WASM loads. Half-resolution steps keep the view manageable; **Original** can
require decoding billions of pixels when zoomed out, even though retained output
is bounded.

What to point out:

- These are static files accessed with ordinary HTTPS range requests. There is
  no image-rendering server.
- **WASM decoding** is the decoder's measured work for the last update. It is
  separate from HTTP/cache throughput and is not the entire view latency.
- Worker counts report workers producing output. More workers need not be
  faster; Auto learns from eligible real views.
- Compression compares encoded bytes with the uncompressed bitmap. A lossy
  quality target is an encoder setting, not a measured perceptual score for
  every gallery image.
- Displayed memory covers WASM capacity and the reported retained buffers and
  caches. It is not total browser or system memory.

The browser decode test is opt-in and pauses map work. It compares one NASA crop
at 4K and 8K, with three trials, separate lossless/lossy tables and IMI worker
counts 1/2/4/8/16. Input download, identity checks and decode are separate.
Lossy comparison fixtures have a measured SSIMULACRA2 match within the stated
0.5-point tolerance. An unsupported browser format stays a rejection; it does
not become a speed result. Read the displayed implementation/source notes.

Keep the tab visible. The test downloads hundreds of MiB and deliberately
bypasses HTTP caches for its input trials; do not run it over a metered
connection without considering that cost. Stop restores the map. Its results
are this browser's observations, not a general codec ranking.

## Before taking the stage

1. Use the actual public origin, with normal browser security enabled. Check
   gallery selection, a pan and zoom, quality changes and fullscreen.
2. Run the decode test once on the presentation machine. Confirm both tables
   populate, required formats decode, and stopping returns control to the map.
3. Record browser version, display density and selected worker count with any
   screenshots or quoted numbers. Distinguish first view from cached-input work.
4. Keep an already-loaded tab for the live pan demonstration, but describe it
   as warm. Save a screenshot or recording as a network-outage fallback and
   identify it as recorded if used.

The [viewer guide](../tools/imij-wasm/web/gigapixel/README.md) describes controls
and counters. The [shipping checklist](SHIPPING_STATUS.md) separates implemented
interfaces from remaining work. The deployed bundle's WASM and JavaScript may
predate repository main; a new API on main is not evidence that the live demo
uses it.

[Codestream Atlas](https://imazen.github.io/zqi/) is a second demonstration of
the byte layout and decoder read order. The explorer currently stores ordinary
independent IMI tiles in imij-set and uses a prepared tile directory. It is not
a demonstration of a single native gigapixel lossy image or cross-entry
prediction. Range access, a bounded retained raster, bounded encoding and
inter-image prediction are separate capabilities.
