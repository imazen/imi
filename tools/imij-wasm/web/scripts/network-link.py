#!/usr/bin/env python3
"""Shape real HTTPS traffic through a loopback CONNECT proxy."""
import argparse
import asyncio
import math
import re

PROFILES = {
    "wifi": (30, 40, 10),
    "lte": (85, 12, 3),
    "weak-lte": (180, 2.5, 0.75),
}
CHUNK = 8192


class Link:
    def __init__(self, mbps, latency):
        self.rate = mbps * 1_000_000 / 8
        self.latency = latency
        self.available = 0.0

    async def send(self, reader, writer):
        loop = asyncio.get_running_loop()
        queue = asyncio.Queue(maxsize=32)

        async def receive():
            try:
                while data := await reader.read(CHUNK):
                    await queue.put((loop.time() + self.latency, data))
            except OSError:
                pass
            await queue.put((0, b""))

        producer = asyncio.create_task(receive())
        try:
            while True:
                received, data = await queue.get()
                if not data:
                    writer.write_eof()
                    break
                start = max(self.available, received, loop.time())
                self.available = start + len(data) / self.rate
                await asyncio.sleep(max(0, self.available - loop.time()))
                writer.write(data)
                await writer.drain()
        finally:
            producer.cancel()
            await asyncio.gather(producer, return_exceptions=True)


async def serve(args):
    rtt, down, up = PROFILES[args.profile]
    rtt = rtt if args.rtt_ms is None else args.rtt_ms
    down = down if args.down_mbps is None else args.down_mbps
    up = up if args.up_mbps is None else args.up_mbps
    if not all(math.isfinite(x) and x > 0 for x in (down, up)) or not math.isfinite(rtt) or rtt < 0:
        raise ValueError("latency must be nonnegative and rates must be positive")
    upload, download = Link(up, rtt / 2000), Link(down, rtt / 2000)
    allowed = set(args.allow)
    tunnels = asyncio.Semaphore(32)

    async def tunnel(reader, client):
        upstream = None
        async with tunnels:
            try:
                head = await asyncio.wait_for(reader.readuntil(b"\r\n\r\n"), 10)
                request = head.split(b"\r\n", 1)[0].decode("ascii")
                match = re.fullmatch(r"CONNECT ([A-Za-z0-9.-]+):443 HTTP/1\.[01]", request)
                if not match or match[1].lower() not in allowed:
                    client.write(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n")
                    await client.drain()
                    return
                source, upstream = await asyncio.wait_for(asyncio.open_connection(match[1], 443), 12)
                await asyncio.sleep(rtt / 1000)
                client.write(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                await client.drain()
                streams = [asyncio.create_task(upload.send(reader, upstream)),
                           asyncio.create_task(download.send(source, client))]
                try:
                    await asyncio.gather(*streams)
                finally:
                    for stream in streams:
                        stream.cancel()
                    await asyncio.gather(*streams, return_exceptions=True)
            except (OSError, EOFError, ValueError, asyncio.LimitOverrunError, asyncio.TimeoutError):
                pass
            finally:
                client.close()
                if upstream is not None:
                    upstream.close()

    server = await asyncio.start_server(tunnel, "127.0.0.1", args.port, limit=16 * 1024)
    print(f"ready 127.0.0.1:{args.port} {rtt:g} ms {down:g}/{up:g} Mbps", flush=True)
    async with server:
        await server.serve_forever()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--profile", choices=PROFILES, default="lte")
    parser.add_argument("--port", type=int, default=3480)
    parser.add_argument("--allow", action="append", required=True, help="HTTPS hostname (repeatable)")
    parser.add_argument("--rtt-ms", type=float, help="additional round-trip latency")
    parser.add_argument("--down-mbps", type=float, help="shared downstream rate")
    parser.add_argument("--up-mbps", type=float, help="shared upstream rate")
    args = parser.parse_args()
    if not 3000 <= args.port <= 3999:
        parser.error("choose a port between 3000 and 3999")
    try:
        asyncio.run(serve(args))
    except KeyboardInterrupt:
        pass
    except (OSError, ValueError) as error:
        parser.exit(1, f"network-link: {error}\n")


if __name__ == "__main__":
    main()
