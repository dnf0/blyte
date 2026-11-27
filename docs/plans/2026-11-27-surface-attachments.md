# Dropping and pasting images, files and long text into the surface

The user's request (2026-11-27): drag a screenshot or a file onto the surface, or paste one, and
have the conversation model see it; a long pasted text shows as `[Pasted text #1 +42 lines]` and
images as `[Image #1]`, `[Image #2]`, instead of the full content.

Facts this rests on (research run DDR1, checked against the code): a user message is a plain
`String` from `/api/session/messages` to every provider (`Message::User(String)` in
`crates/blyte-provider/src/lib.rs`); the HTTP server refuses bodies over 1 MiB
(`MAX_BODY_BYTES` in `crates/blyte-cli/src/http.rs`); every `/api/*` request needs the
`X-Blyte-Token` header, so an `<img src>` cannot load an attachment; the surface log must not hold
base64; a message with empty text is refused in the page and in `Session::submit`.

## Decisions

- **Images** (`image/png`, `image/jpeg`, `image/webp`, `image/gif`, decided by the file's first
  bytes, not by the name or the browser's claim) go to the model as inline image parts, and are
  also saved. **Every other file** is saved only, and the model is told its absolute path.
- Files are saved under `<surface record directory>/attachments/`, never in the user's working
  tree outside `.wt/`.
- One file is at most 10 MiB (10 × 1024 × 1024 bytes); a larger one, or an empty one, is refused
  with a message that names the file and the limit. Nothing is resized or dropped silently.
- A paste of 5 lines or more, or of more than 800 characters, becomes a marker; shorter pastes
  stay as typed text.
- No fallbacks anywhere: an attachment id the server does not know, a file that has gone missing,
  or a part a provider cannot express is an error that names it.

## Contract between the page and the server

- `POST /api/session/attachments?name=<percent-encoded file name>`; body: the raw bytes; token
  header as for every route. `200` with
  `{"id":"a1","name":"shot.png","mime_type":"image/png","size":148290,"image":true}`.
  `413` with a plain-text reason over 10 MiB; `400` for an empty body or a missing name.
  `mime_type` is the detected image type, or `application/octet-stream` for anything else.
- `GET /api/session/attachments/<id>`: the bytes, with that `mime_type`. `404` for an unknown id.
- `POST /api/session/messages` body gains two optional arrays:
  `"attachments": ["a1","a2"]` (ids, in the order attached) and
  `"pastes": [{"n":1,"text":"…"}]`. `text` may be empty when `attachments` is not.
  In `text`, the page has already written the markers: `[Image #1]`, `[File shot.pdf]`,
  `[Pasted text #1 +42 lines]`.
- The `user_message` log entry gains `attachments` (the metadata objects above, without bytes) and
  `pastes` (as sent), both left out when empty.
- What the model receives for such a message: the text, with each `[Pasted text #n +k lines]`
  marker replaced by the paste's full text; then one line per attachment,
  `[Image #1] shot.png (image/png, 148290 bytes) saved at <absolute path>` or
  `[File shot.pdf] (application/octet-stream, 52011 bytes) saved at <absolute path>`; then, for
  each image, an inline image part, in order.

### Task DD1: a user message can carry images, in the provider and agent crates

**Depends on:** none. **Siblings:** DD2 (server) and DD3 (page) are separate and not in this
worktree; do not add routes, storage or page code here.

**Files:** `crates/blyte-provider/src/lib.rs`, `gemini.rs`, `anthropic.rs`, `openai.rs`,
`chatgpt.rs`, `ollama.rs` in the same folder; `crates/blyte-agent/src/state.rs`, `run.rs`,
`context.rs`; any other file only where an exhaustive `match` on `Message` no longer compiles
(for example `crates/blyte-agent/src/claim_check.rs`, `crates/blyte-cli/src/model_review.rs`),
with the smallest change that treats the new variant by its text parts.

**What to build.**

1. In `blyte-provider/src/lib.rs`:
   ```rust
   pub enum UserPart {
       Text(String),
       /// An image: its MIME type and its bytes, base64-encoded (standard alphabet, padded).
       Image { mime_type: String, data: std::sync::Arc<str> },
   }
   ```
   and a new variant `Message::UserParts(Vec<UserPart>)` beside `Message::User(String)`, which
   stays as it is. Derive what `Message` derives. Add `Message::user_text(&self) -> Option<String>`
   returning the text of `User`, or the `Text` parts of `UserParts` joined with `\n`.
2. Each provider's request body sends a `UserParts` message as one user turn:
   - Gemini (AI Studio and Vertex): `parts` with `{"text": …}` and
     `{"inlineData": {"mimeType": …, "data": …}}`, in order.
   - Anthropic: `content` array with `{"type":"text","text":…}` and
     `{"type":"image","source":{"type":"base64","media_type":…,"data":…}}`.
   - OpenAI chat completions: `content` array with `{"type":"text","text":…}` and
     `{"type":"image_url","image_url":{"url":"data:<mime>;base64,<data>"}}`.
   - ChatGPT (Responses): `content` with `{"type":"input_text","text":…}` and
     `{"type":"input_image","image_url":"data:<mime>;base64,<data>"}`.
   - Ollama: `content` is the text parts joined with `\n`; `images` is the array of base64
     strings.
   Wherever a provider merges or caches consecutive user messages, `UserParts` follows the same
   rule as `User`.
3. In `blyte-agent`: a field `pending_user_parts: Vec<UserPart>` on the runtime state next to the
   conversation (in `state.rs`; not serialised into any saved state if the struct is saved). In
   `Runner::run_until`, where the incoming user prompt is pushed as `Message::User`, push
   `Message::UserParts([Text(prompt), …pending])` instead when `pending_user_parts` is not empty,
   taking them with `std::mem::take` so they are sent with that one prompt only (they stay in the
   conversation history as part of that message).
4. `message_chars` in `run.rs` and `approx_tokens` in `context.rs` count a `UserParts` message as
   its text parts plus a fixed 1,000 tokens (4,000 characters) for each image; name the constant
   `IMAGE_TOKEN_ESTIMATE` with a comment that it is an estimate used only for folding and sizing,
   and that billing uses the provider's reported usage. Never count the base64 length.
5. `digest` in `context.rs`: a folded `UserParts` message contributes its text, plus
   `[image <mime_type>]` for each image. No base64 in a digest.

**Tests.** For each provider, in the place its existing body tests live: a request whose history
has `UserParts([Text("look"), Image{mime_type:"image/png", data:"QUJD"}])` produces exactly the
JSON above for that provider (assert the user turn's JSON value in full). In `blyte-agent`:
`pending_user_parts_are_sent_with_the_next_prompt_only` (two prompts with a scripted provider: the
first request's last user message is `UserParts` with the image, the second prompt's own message
is a plain `User`, and the first is still in the history with its image);
`approx_tokens_counts_an_image_as_the_fixed_estimate` (a 1 MiB base64 string gives the text
estimate plus `IMAGE_TOKEN_ESTIMATE`); `digest_names_an_image_without_its_data` (the digest
contains `[image image/png]` and not the data string).

Run `cargo test -p blyte-provider && cargo test -p blyte-agent`.

**Constraints.** `Message::User(String)` and every existing call site keep working unchanged.
No new dependency. No fallbacks. Existing tests pass unmodified.

**Commit message:** `feat(provider): a user message can carry images`

### Task DD2: the surface server takes, stores and forwards attachments and pastes

**Depends on:** DD1, ST1 (same file `session.rs`). **Siblings:** DD3 builds the page; do not
change `assets/surface/index.html` here.

**Files:** `crates/blyte-cli/src/http.rs`, `session.rs`, `serve.rs`; `repl.rs` and
`task_runner.rs` only if the parts cannot reach the runner otherwise; a new
`crates/blyte-cli/src/attachments.rs`; tests in `crates/blyte-cli/tests/test_attachments.rs`.

**What to build.** The contract at the top of this plan, exactly.

1. `http.rs`: the body limit stays 1 MiB for every route except
   `POST /api/session/attachments`, whose limit is 10 MiB; over it, `413` with
   `"<name> is larger than the 10 MiB limit for one file"` when the name is known, without reading
   the body into memory first. Responses can carry a content type decided at run time.
2. `attachments.rs`: `detect_image(bytes) -> Option<&'static str>` from the first bytes (PNG,
   JPEG, GIF87a/GIF89a, WebP `RIFF….WEBP`); `store(dir, name, bytes) -> Result<Meta, String>`
   writing `<dir>/attachments/<id>-<sanitised name>` with ids `a1`, `a2`, … continuing after the
   highest id already in the directory; the name is reduced to its final path component and to
   `[A-Za-z0-9._-]` (other characters become `_`), and a name that is empty after that is an
   error; `load(dir, id) -> Result<(Meta, Vec<u8>), String>`, an error naming the id when it is
   unknown or its file is gone; a small standard base64 encoder with tests against known vectors.
3. `serve.rs`: the two attachment routes, token-checked like the others; the attachments directory
   is created under the surface's record directory, and is added to the read roots and to the
   command sandbox's readable paths, so the model's `read` tool and its shell can read a saved
   file in a git repository and in a folder that is not one. `/api/session/messages` accepts
   `attachments` and `pastes`; an unknown attachment id is `400` naming it.
4. `session.rs`: `UserMessage` entries (both body and wire types) gain `attachments` and `pastes`
   with `#[serde(default, skip_serializing_if = "Vec::is_empty")]`; `Session::submit` accepts
   empty text when there is at least one attachment; the queued message carries both; the runner
   builds the model's text as the contract says and sets `pending_user_parts` to the images
   (base64 of the stored bytes) before the prompt runs. A paste marker in the text with no
   matching `pastes` entry, or a `pastes` entry whose marker is not in the text, is `400` naming
   it. The log entry keeps the text with markers, not the expanded text.
5. Resumable and idempotent: a stored file is written to a temporary name and renamed.

**Tests** (`test_attachments.rs`, with the scripted model and server helpers the existing surface
server tests use; read `test_session.rs` and the serve tests first):
`upload_then_message_sends_the_image_to_the_model` (the scripted provider's request holds a
`UserParts` message whose image data is the base64 of the uploaded bytes, and whose text ends
with the `[Image #1] … saved at …` line); `a_non_image_file_is_saved_and_named_but_not_inlined`;
`a_png_named_txt_is_detected_as_an_image_and_a_text_file_named_png_is_not`;
`an_upload_over_10_mib_is_refused_with_413_naming_the_file`; `an_empty_upload_is_refused`;
`a_message_with_an_unknown_attachment_id_is_refused_naming_it`;
`a_message_with_only_an_attachment_and_no_text_is_accepted`;
`a_paste_marker_is_expanded_for_the_model_and_kept_as_a_marker_in_the_log`;
`a_paste_marker_without_its_text_is_refused`;
`an_old_user_message_entry_without_attachments_still_reads`;
`get_attachment_returns_the_bytes_and_type_and_404_for_an_unknown_id`;
`the_file_name_is_sanitised` (`../../x y.png` is stored as `a1-x_y.png` inside the directory);
`a_saved_attachment_is_inside_the_read_roots`.

Run `cargo test -p blyte --test test_attachments`.

**Constraints.** Existing tests pass unmodified. No fallbacks. No new dependency.

**Commit message:** `feat(surface): the server takes dropped files and pasted text`

### Task DD3: drop, paste and markers in the surface page

**Depends on:** DD2, AT1 (same file). **Siblings:** none.

**Files:** `crates/blyte-cli/assets/surface/index.html`, `crates/blyte-cli/tests/test_surface_page.rs`.

**What to build**, against the contract at the top of this plan.

1. Dropping files anywhere on the Design view, and pasting files or images into the message box,
   uploads each with `POST /api/session/attachments` (through the page's existing authenticated
   fetch helper) and adds a chip above the message box, next to the attached-report chips:
   `[Image #1]`, `[Image #2]` for images (numbered in the order attached, within the message being
   written) and `[File <name>]` for others, each with a remove button. While files are dragged
   over the page, the conversation area shows a drop outline. A refused upload shows the server's
   reason in the page's existing error place, and adds no chip.
2. Pasting text of 5 lines or more, or more than 800 characters, into the message box inserts
   `[Pasted text #n +k lines]` at the cursor instead of the text (k is the number of lines; n
   counts up within the message), and keeps the text for sending. Deleting the marker from the box
   drops that paste. Shorter pastes behave as now.
3. Sending: allowed with empty text when there is at least one attachment. The body carries
   `text` (image and file markers appended, each on its own line, when not already in the text),
   `attachments` and `pastes`. After a successful send the chips and pastes are cleared.
4. In the conversation, a user message shows its markers as small buttons: a paste marker expands
   in place to the paste's text and collapses again; an image marker shows the image under the
   message (fetched with the authenticated fetch helper into a blob object URL, revoked when
   replaced); a file marker shows the file's name and size. A message from an old log without
   these fields draws as now.
5. Keep the functions that build markers pure and named: `pasteMarker(n, text)`,
   `needsPasteMarker(text)`, `imageMarker(n)`.

**Tests** in `test_surface_page.rs`, in its existing style: the page contains
`/api/session/attachments`, `pasteMarker`, `needsPasteMarker`, `imageMarker`, a `drop` listener
and a `paste` listener; if the file can run page functions, `needsPasteMarker` is false for 4
lines of 10 characters, true for 5 lines, true for one line of 801 characters, and
`pasteMarker(1, "a\nb\nc\nd\ne")` is `[Pasted text #1 +5 lines]`.

Run `cargo test -p blyte --test test_surface_page`.

**Constraints.** Existing tests pass. Touch only the two files. No fallbacks.

**Commit message:** `feat(surface): drop or paste images, files and long text`
