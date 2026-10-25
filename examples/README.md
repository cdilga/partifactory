# Three parts

These self-contained HTML pages use the normal part runtime, with no external libraries.

- `about.html`: an explanation page, intended for an administrator-created public Can-view link.
- `workshop.html`: a household discussion page. Put it in the family space and grant household members Can use. Comments use shared storage and live events; the page itself displays every comment as text.
- `orbit-relay.html`: a cooperative light-routing game. Every pilot edits the same mirror board. Grant household members Can use; an administrator may also create a Can-contribute guest link.

Create each as an `html` part with `live: after_check`, check its first render and interactions,
and share it through the normal access controls. The discussion page should remain private.
The game can be completed by routing coral through `(3,1)` and `(8,4)` to `(11,1)`, and ice through
`(7,6)` to `(0,6)`; coordinates are zero-based. All three pages call `partifactory.ready()`.
