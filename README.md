# partifactory

A self-hosted home for the things you and your AI agents make: pages, React tools, documents, decks and diagrams. Each one is a **part**, with its own origin and a name that means something (`photofinder-part.example.dev`).

Status: **design stage**. The plan is in [PLAN.md](PLAN.md). There's no code yet.

## Credit

partifactory is a reimagining of **[shartifacts](https://github.com/LaneBirmingham/shartifacts) by Lane Birmingham (birmo)**. Lane's project showed the shape of the idea: agents publishing over MCP, immutable versions, Claude-artifact compatibility, comments that agents act on, and a share surface kept apart from the workspace. partifactory takes that as its reference spec and rebuilds it for a different setup: Cloudflare Tunnel, an OIDC identity provider with in-app invites, a per-part origin for every part, and a single Rust binary. See [NOTICE](NOTICE).

## Site configuration

Everything specific to one deployment (hostnames, IdP inventory, flow names, deploy target) lives in a private site config. It isn't in this repository. Secrets live only in the CI secret store.

## License

[MIT](LICENSE).
