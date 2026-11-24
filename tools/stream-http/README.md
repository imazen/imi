# stream-http

End-to-end streaming encode over HTTP. It compares zencodec's streaming
output (`Encoder::begin` / `push_rows` / `pending_output`, chunked transfer)
with one-shot `encode` (`Content-Length`), for every codec that adopted it.
It does so from a Rust hyper server and from an ASP.NET Core server that
drives the same encoders through a C ABI and writes into
`Response.BodyWriter`.

| Name | Encoder |
|---|---|
| `imij-quick` | IMIJ quick pixel packets (QOI-style) in one-row groups; on `src=noise` every unit falls back to raw samples |
| `imij-group` | IMIJ MED prediction, fixed rANS tables, one-row groups |
| `jpeg-fixed` | zenjpeg q85 4:2:0, fixed Huffman tables, sequential (streams) |
| `jpeg-default` | zenjpeg q85 4:2:0 default: progressive, optimized tables (`AT_FINISH`) |
| `png-e0`, `png-e1`, `png-e5` | zenpng at effort 0, 1 and 5, downcasts off, sequential |
| `ppm`, `qoi` | zenbitmaps P6 and QOI |

```
cargo build --release
target/release/server 3901 photo.jpg          # Rust
dotnet run -c Release --project dotnet -- 3902 photo.jpg target/release/libimij_stream_http.so
target/release/client http://127.0.0.1:3901 rust photo all 1024x1024 stream,buffer 1,8 20
./run.sh photo.jpg out/ 20                    # the full matrix plus peak RSS (Linux)
```

Requests crop the top-left `w` x `h` of the source with rows left strided,
and push 16 rows at a time (`strip=`). `src=noise` swaps in incompressible
noise of the same size. Client rows report time to the first body byte and
to the last (p50/p95), the output size and throughput.

`src/ffi.rs` is this tool's only `unsafe` code: the C ABI must build slices
from the pointers .NET passes.
