#!/usr/bin/env python3
"""Mac harness plumbing: fixed TCP destinations; no general-purpose proxy or TLS termination."""
import asyncio
import os


async def forward(reader, writer, host, port):
    remote_writer = None
    try:
        remote_reader, remote_writer = await asyncio.open_connection(host, port)
        async def copy(source, destination):
            while chunk := await source.read(65536):
                destination.write(chunk)
                await destination.drain()
        tasks = [asyncio.create_task(copy(reader, remote_writer)), asyncio.create_task(copy(remote_reader, writer))]
        _, pending = await asyncio.wait(tasks, return_when=asyncio.FIRST_COMPLETED)
        for task in pending:
            task.cancel()
        await asyncio.gather(*tasks, return_exceptions=True)
    except (OSError, asyncio.CancelledError):
        pass
    finally:
        writer.close()
        if remote_writer:
            remote_writer.close()


async def main():
    host, port = os.environ.get("PF_RENDER_HOST_SERVER", "host.docker.internal:18445").rsplit(":", 1)
    endpoints = [(8443, host, int(port)), (9222, "partifactory-render", 9222), (9223, "partifactory-render", 9223)]
    servers = [await asyncio.start_server(lambda r, w, h=h, p=p: forward(r, w, h, p), "0.0.0.0", listen) for listen, h, p in endpoints]
    await asyncio.gather(*(server.serve_forever() for server in servers))


asyncio.run(main())
