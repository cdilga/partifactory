# partifactory

A self-hosted home for the things you and your AI agents make: pages, React tools, documents, decks and diagrams. Each one is a **part**, with its own origin and a name that means something (`photofinder-part.example.dev`).

Status: **early, and in daily use by one household.** The first two slices of [PLAN.md](PLAN.md) are built, tested in CI on desktop and mobile browser engines, and deployed:

- **Parts:** HTML, Markdown, text, SVG, code and multi-file parts, each sandboxed on its own host with no cookies. Every version is kept, and a server-side render check catches pages that crash or come up blank.
- **Agents:** connect over MCP, an HTTPS JSON API or the `part` CLI, with OAuth (including Client ID Metadata Documents) or a device code. Each agent gets its own scoped, revocable connection. The contract agents read is [docs/agent-contract.md](docs/agent-contract.md), served as `/skill.md`.
- **People:** OIDC sign-in, sharing with people and groups as View, Comment, Edit or Manage, a household Family space, live comments, a formatted Markdown editor, shared data inside parts, and admin-made public links.

Still to come: React (`tsx`) parts, backends, invites, notifications and full-text search. The plan has the order.

## Credit

partifactory is a reimagining of **[shartifacts](https://github.com/LaneBirmingham/shartifacts) by Lane Birmingham (birmo)**. Lane's project showed the shape of the idea: agents publishing over MCP, immutable versions, Claude-artifact compatibility, comments that agents act on, and a share surface kept apart from the workspace. partifactory takes that as its reference spec and rebuilds it for a different setup: Cloudflare Tunnel, an OIDC identity provider with in-app invites, a per-part origin for every part, and a single Rust binary. See [NOTICE](NOTICE).

## Site configuration

Everything specific to one deployment (hostnames, IdP inventory, flow names, deploy target) lives in a private site config. It isn't in this repository. Secrets live only in the CI secret store.

## License

[MIT](LICENSE).
