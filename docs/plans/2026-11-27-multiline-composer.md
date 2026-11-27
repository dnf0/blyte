# A multi-line chat box in the surface

The surface's chat box is a single-line `<input>`, so a message cannot contain a new line and the
box is small. This plan makes it a multi-line box.

### Task MC1: the chat box is multi-line, sends on Return and grows as you type

**Depends on:** none. **Siblings:** none.

**Goal.** In the surface page, the chat box (`#conversation-input` in
`crates/blyte-cli/assets/surface/index.html`) becomes a `<textarea>`: Return sends the message,
Shift+Return adds a new line, and the box is bigger and grows with its content. **Why:** the user
writes multi-line messages and finds the one-line box too small.

**Files.**
- `crates/blyte-cli/assets/surface/index.html`
- `crates/blyte-cli/tests/test_surface_page.rs` (new tests; existing tests change only where they
  construct or assume an `<input type="text">` for `conversation-input`, and then only that part)

**What to build.**
1. **Element.** Replace the `<input type="text" id="conversation-input" ...>` with
   `<textarea id="conversation-input" rows="3" placeholder="Message the conversation model... (Shift+Return for a new line)" autocomplete="off"></textarea>`.
   Keep the id, so every existing `getElementById("conversation-input")` keeps working. Check each
   of those call sites still behaves with a textarea (value, focus, disabled, selection, paste and
   input handlers).
2. **Keys.** A `keydown` handler on the textarea, in a named function
   `handleComposerKeydown(e)` so it can be tested:
   - `Enter` with no modifier, and not during text composition (`e.isComposing` false and
     `e.keyCode !== 229`): `preventDefault()` and submit the form the same way the existing submit
     handler is reached (`form.requestSubmit()`), so there is one send path.
   - `Enter` with Shift: do nothing (the browser inserts the new line).
   - `Enter` with Ctrl, Meta or Alt: do nothing.
   - Every other key: do nothing.
3. **What is sent.** The message text keeps its inner new lines exactly. Whatever trimming the
   submit handler does today stays; a message that is only white space (including only new lines)
   is not sent, as an empty one is not sent today.
4. **Size.** A named function `resizeComposer()` sets the textarea's height to fit its content:
   set `style.height = "auto"`, then `style.height = Math.min(scrollHeight, cap) + "px"` where
   `cap` is 40% of `window.innerHeight`; above the cap the textarea scrolls (`overflow-y: auto`),
   below it shows no scroll bar. Call it on `input`, after the box is cleared on send, after any
   code sets the box's value (find them: drafts restored, paste markers inserted), and on window
   `resize`. CSS: replace the `.composer input[type="text"]` rule with one for
   `.composer textarea` with the same colours and border, `font-size: 14px`,
   `line-height: 1.45`, `font-family: inherit`, `resize: none`, `min-height` of three lines,
   `padding: 9px 12px`; set `.composer { align-items: flex-end; }` so the send button and any
   attach control sit at the bottom as the box grows.
5. **Showing sent messages.** A user message with new lines shows them as typed in the
   conversation (`white-space: pre-wrap` on the element that holds the user's message text, if it
   is not already). The existing paste, image and file markers inside messages keep rendering as
   they do.
6. **Unchanged.** Dropping and pasting images, files and long text into the box; the paste
   markers; the attachment chips; the disabled state while a reply is in progress; every other
   form and input on the page.

**Tests** (`crates/blyte-cli/tests/test_surface_page.rs`, using the existing script-harness
approach in that file; run with `cargo test -p blyte --test test_surface_page`):
- the page contains `<textarea id="conversation-input"` and no `<input type="text" id="conversation-input"`;
- `handleComposerKeydown` with `{key: "Enter", shiftKey: false}`: `preventDefault` was called and
  the form's `requestSubmit` was called once;
- with `{key: "Enter", shiftKey: true}`: neither was called;
- with `{key: "Enter", isComposing: true}` and with `{key: "Enter", keyCode: 229}`: neither was called;
- with `{key: "Enter", metaKey: true}`, `ctrlKey` and `altKey`: neither was called;
- with `{key: "a"}`: neither was called;
- submitting with the box holding `"line one\nline two"` sends a request body whose message is
  exactly `"line one\nline two"`, and the box is empty afterwards;
- submitting with the box holding `"\n \n"` sends nothing;
- `resizeComposer` with `scrollHeight` 100 and `window.innerHeight` 1000 sets height `100px`;
  with `scrollHeight` 900 sets `400px`;
- a rendered user message `"a\nb"` keeps the new line (assert on the rendered HTML and on the
  CSS rule giving that element `white-space: pre-wrap`);
- the existing paste, drop and attachment tests pass.

**Constraints.** No fallbacks: a missing element or handler is an error, not a silent skip. No
`window.confirm`, `alert` or `prompt`. No new dependencies or external assets. Nothing here is
long-running, so "resumable" does not apply. Existing tests pass; change one only where it
depends on the old `<input>` element, and say which in your final message. No home-directory
paths in code or tests.

**Commit message:** `feat(surface): a multi-line chat box that sends on Return and grows as you type`
