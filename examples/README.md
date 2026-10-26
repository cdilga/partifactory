# Four parts

Three self-contained HTML pages using the normal part runtime, with no external libraries, and one Markdown part.

- `about.html`: an explanation page, intended for an administrator-created public Can-view link.
- `workshop.html`: a household discussion page. Share it with the household group as Comment (`part_share`), so members can open it and use the Comments rail; its idea board uses shared storage and live events, and displays every entry as text.
- `household-notes.md`: shared notes for the household, created as a `markdown` part and shared with the household group as Edit, so anyone can change it in the formatted view (task list, table, quote) or comment on a selection.
- `orbit-relay.html`: a cooperative light-routing game. Every pilot edits the same mirror board. Grant household members Can use; an administrator may also create a Can-contribute guest link.

Create each as an `html` part with `live: after_check`, check its first render and interactions,
and share it through the normal access controls. The discussion page should remain private.
The game can be completed by routing coral through `(3,1)` and `(8,4)` to `(11,1)`, and ice through
`(7,6)` to `(0,6)`; coordinates are zero-based. All three pages call `partifactory.ready()`.
