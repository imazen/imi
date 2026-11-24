# Archive HTTP qualification tools

Build the existing native range client with `just archive-network-build`.
Run builds and studies through the shared resource wrapper on an eligible host.
The standalone manifest uses the root's pinned codec dependencies and enables
HTTPS without adding dependencies to the product crates.

`iqa_http_access` accepts an immutable URL or `@/absolute/path/to/url-file`.
Use a private mode-600 file for signed URLs; keep it outside result directories.
Responses must be HTTP 206 with consistent Content-Range, length and strong
ETag. Later requests use If-Match. Source pixel checks are mandatory.

Independent `--image` and `--roi` operations default to a bounded exact-range
cache: at most 1 MiB / 128 retained ranges, without speculative read-ahead.
Indexed-sequence and metadata modes retain the original 256 KiB read-ahead
window. `IMI_HTTP_CACHE=exact|prefetch` selects either policy explicitly.
Exact caching saves transferred bytes in the measured independent-image cases;
it can add cold sequence requests. Both original cache tests remain controls.

- `just archive-network-study CONFIG OUTPUT` runs the separate-host HTTP
  controller and reconciles its server ledger with client counters.
- `just archive-network-remote-probe CONFIG OUTPUT` probes existing remote
  objects, interleaving trials, cases and policies. Its JSON configuration has
  `remote`, `remote_prefix` (resource-wrapper argv), `client`, `trials`,
  `policies` and `cases`. Each case has a stable `name`, an absolute `url_file`
  on the client host, and client `args`. URLs are not copied into results.
  These are client counters, without an independent remote server ledger.
- `just archive-network-wire-census INPUT OUTPUT` counts the baseline numeric
  V4 witness at byte zero, including a one-entry archive beginning with that
  image. It requires prior native validation. It supports a front directory,
  integrity mode 0/1 and numeric main/tile-boundary objects only. It is neither
  a decoder nor an integrity verifier; unsupported shapes fail explicitly.

Preserve immutable inputs, binaries, raw successes and failures with XXH3-128
identities. Signed access files and credentials do not belong in receipts.
The [current report](../../docs/ARCHIVE_CURRENT_WIRE_2026-11-17.md) distinguishes
LAN controls, real R2 reads and the separately deployed public browser witness.

`just web-directory-network-probe WEB FRESH_OUTPUT CONFIG.json` records actual
CDN protocol/cache/timing observations in Chromium. Jobs fetch a full sidecar,
or explicit range lists with a stage number: equal-stage requests run in
parallel; later stages depend on completion. Range bodies are checked against
paired publication or a verified full index. This does not implement a paged
codec or replay display. `just archive-directory-network-report ROOT OUTPUT
TABLE` audits the retained study, including cache wire diagnostics and all slow
controls. See [the round-trip report](../../docs/DIRECTORY_CDN_ROUNDTRIPS_2026-11-17.md).
