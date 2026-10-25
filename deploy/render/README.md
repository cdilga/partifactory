The optional render container starts Chrome for `part_check` and stops it after five minutes
without a job. Its only network is an internal Docker network shared with partifactory.
Chrome uses the server's per-start SPKI pin; the private certificate key stays in the server.

The browser runs without its nested Linux namespace sandbox because Docker's default
seccomp profile blocks that namespace setup. The container supplies the isolation boundary:
non-root user, no capabilities, no new privileges, read-only filesystem, bounded tmpfs,
CPU/memory/process limits, and no external network. Never attach this service to another network
or publish its CDP/controller ports in a deployment. The harness publishes them to loopback
only when its server runs natively on the Mac.
