# Switching between surfaces from inside a surface

## Problem

A surface is a page in a browser. With several surfaces running (one per folder), each is a
separate tab or window with nothing linking them, so the one you want is hard to find, and from
inside one surface there is no way to see the others. `blyte surfaces` and `blyte open <name>`
do this from a terminal only.

## Decisions

1. **The name in the header is the switcher.** The box next to the wordmark (today a plain label
   holding the folder name) becomes a button. It opens a list of the surfaces running on this
   machine. Each row shows the surface's name, the full path of its folder (two folders can share
   a name), its branch when it has one, and what it is doing. Picking a row takes this window to
   that surface.
2. **Surfaces stay independent.** Each is still its own process with its own conversation, port
   and token, as decided in `2026-11-25-independent-surfaces.md`. The switcher only navigates one
   window between them; it is not tabs and holds no shared state.
3. **The list comes from the surface registry**, the same source as `blyte surfaces`, through a
   new `GET /api/surfaces` on each surface. It needs that surface's token like every other route.
   Each row carries the address of the surface it describes, built the way `blyte open` builds it.
   The answer therefore contains other surfaces' tokens: it goes to the page only, and is never
   logged, written to a transcript or given to the model.
4. **A surface that cannot be opened is shown, not dropped.** If a registered surface's token
   cannot be read, its row is listed, cannot be picked, and says why.
5. **The window title says what the surface is doing**: `<name>: working - blyte` while a turn is
   running, `<name> - blyte` otherwise, so the window can be found from the window list.

## Later, not in this change

- "Waiting on you" as a state in the list and the title (a pending approval or question).
- Starting a new surface in a chosen folder from the list.
- Opening the surface in its own application window instead of a browser tab.
