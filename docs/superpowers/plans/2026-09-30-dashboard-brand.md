# The dashboard in blyte's own look — Plan

The dashboard page (`crates/blyte-cli/assets/dashboard/index.html`) still wears the palette and
type of the design mockup (Climate X greys, red, jade, Graphik). It should look like the rest of
blyte: the palette, type and mark of `docs/stylesheets/blyte.css` and `docs/assets/`. The page stays
self-contained: no request leaves the machine, so the fonts are bundled in the binary and served by
the dashboard's own server.

## Global Constraints

- Base branch `feat/dashboard-brand`. It already holds the font files and their licences in
  `crates/blyte-cli/assets/dashboard/fonts/` (`archivo.woff2`, a variable font with weight 400–900
  and width 62–125%; `plex-mono-400.woff2`; `plex-mono-500.woff2`; `OFL-Archivo.txt`;
  `OFL-IBMPlexMono.txt`) and `crates/blyte-cli/assets/dashboard/favicon.svg`. Do not change them.
- Change only the look: no change to the page's behaviour, data handling, element ids, classes the
  script uses, or the `/api/jobs` contract. Keep the phrase "What blyte is working on." and every
  string the existing page tests look for.
- The page must still contain no `http://` or `https://` (the page test checks). Inline SVG in HTML
  needs no `xmlns`, so leave it out.
- Checks: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`.
- Conventional commit, `--no-gpg-sign`.

### Task 1: palette, type, mark, favicon and bundled fonts

**Files:** `crates/blyte-cli/src/dashboard.rs` (routes), `crates/blyte-cli/assets/dashboard/index.html`
(styles and header), a new test file `crates/blyte-cli/tests/test_dashboard_brand.rs` (exactly as
below), and `docs/usage.md` (one sentence in the Dashboard section: the page uses blyte's bundled
Archivo and IBM Plex Mono fonts, under the SIL Open Font License, served locally).

**Routes.** In `route`, add, each with `include_bytes!` / `include_str!`:

| Path | Content-Type | Body |
|---|---|---|
| `/favicon.svg` | `image/svg+xml` | `assets/dashboard/favicon.svg` |
| `/fonts/archivo.woff2` | `font/woff2` | `assets/dashboard/fonts/archivo.woff2` |
| `/fonts/plex-mono-400.woff2` | `font/woff2` | `assets/dashboard/fonts/plex-mono-400.woff2` |
| `/fonts/plex-mono-500.woff2` | `font/woff2` | `assets/dashboard/fonts/plex-mono-500.woff2` |

Everything else stays a 404. `route` returns `(u16, &'static str, Vec<u8>)` already, so the bytes
go in as `.to_vec()`.

**Fonts.** At the top of the page's `<style>`, three `@font-face` rules with `font-display: swap`:
`"Archivo"` from `/fonts/archivo.woff2` (`format("woff2")`, `font-weight: 400 900`,
`font-stretch: 62% 125%`), and `"IBM Plex Mono"` at 400 and 500 from the two Plex files. Then:

```css
--font-sans: "Archivo", "Helvetica Neue", Helvetica, Arial, sans-serif;
--font-mono: "IBM Plex Mono", ui-monospace, SFMono-Regular, Menlo, monospace;
```

**Palette.** Replace the token values (keep the token names; add the two `--brand-*` tokens):

| Token | Light | Dark |
|---|---|---|
| `--color-page` | `#ECEFF0` | `#0F1315` |
| `--color-card` | `#F4F6F7` | `#171D20` |
| `--color-line` | `#C9D0D4` | `#283136` |
| `--color-line-dark` | `#B3BCC1` | `#36414A` |
| `--color-text` | `#2A3238` | `#C9D1D5` |
| `--color-strong` | `#141A1E` | `#E2E7E9` |
| `--color-muted` | `#56616A` | `#93A0A7` |
| `--color-faint` | `#8A959C` | `#5F6C73` |
| `--status-running` | `#A6441A` | `#E27A44` |
| `--status-amber` | `#8A6A12` | `#D9B45A` |
| `--status-jade` | `#2E7A4D` | `#6CC08E` |
| `--status-jade-label` | `#2E7A4D` | `#6CC08E` |
| `--status-red` | `#A93A36` | `#E27B76` |
| `--status-red-label` | `#A93A36` | `#E27B76` |
| `--status-grey` | `#8A959C` | `#5F6C73` |
| `--focus-ring` | `#A6441A` | `#E27A44` |
| `--brand-rust` (new) | `#A6441A` | `#E27A44` |
| `--brand-rust-soft` (new) | `#E8C8B7` | `#4A2B1D` |

The dark values go where the dark values are now (the `prefers-color-scheme: dark` block). Any
colour written as a literal elsewhere in the styles (a hex or `rgb()` outside these tokens) becomes
the nearest token. The active view pill and the active tag pill use `--brand-rust-soft` as their
background and `--color-strong` as their text; links and hover accents use `--brand-rust`.

**Type.** The page title (`.page-title`) is Archivo 800 with `font-variation-settings: "wdth" 112`
and `letter-spacing: -0.02em`. The summary tiles' big numbers are Archivo 800 with
`font-variant-numeric: tabular-nums`. Column headers, the eyebrow and tag pills use
`--font-mono` at their current sizes, uppercase where they are uppercase now, with
`letter-spacing: 0.05em`. Everything else inherits `--font-sans`.

**Header.** Replace the eyebrow's text with the mark and the wordmark, keeping the element and
its class:

```html
<div class="eyebrow">
  <svg class="brand-mark" viewBox="0 0 100 100" width="22" height="22" aria-hidden="true">
    <rect class="m-i" x="10" y="10" width="24" height="24" rx="3"/>
    <rect class="m-i" x="38" y="10" width="24" height="24" rx="3"/>
    <rect class="m-i" x="66" y="10" width="24" height="24" rx="3"/>
    <rect class="m-i" x="10" y="38" width="24" height="24" rx="3"/>
    <rect class="m-s" x="38" y="38" width="24" height="24" rx="3"/>
    <rect class="m-r" x="66" y="38" width="24" height="24" rx="3"/>
    <rect class="m-i" x="10" y="66" width="24" height="24" rx="3"/>
    <rect class="m-r" x="38" y="66" width="24" height="24" rx="3"/>
    <rect class="m-r" x="66" y="66" width="24" height="24" rx="3"/>
  </svg>
  <span class="brand-word">blyte</span>
  <span class="brand-sub">dashboard · this machine</span>
</div>
```

with `.eyebrow { display: flex; align-items: center; gap: 10px; }`, `.m-i { fill: var(--color-strong); }`,
`.m-s { fill: var(--brand-rust-soft); }`, `.m-r { fill: var(--brand-rust); }`, `.brand-word` Archivo
900, `font-variation-settings: "wdth" 125`, `letter-spacing: -0.02em`, 20px, `--color-strong`, not
uppercase; `.brand-sub` in `--font-mono`, 11px, uppercase, `letter-spacing: 0.05em`, `--color-muted`.

**Favicon.** In `<head>`: `<link rel="icon" type="image/svg+xml" href="/favicon.svg">`.

- [ ] **Step 1: test.** Create `crates/blyte-cli/tests/test_dashboard_brand.rs` exactly:

```rust
//! The dashboard wears blyte's own palette, type and mark, with its fonts served locally
//! (2026-09-30).

use blyte_cli::dashboard::{route, Sources};

const PAGE: &str = include_str!("../assets/dashboard/index.html");

fn src() -> Sources {
    Sources { roots: vec![], runs_dirs: vec![], hours: 24 }
}

#[test]
fn the_favicon_and_fonts_are_served_locally() {
    let s = src();
    let (code, ctype, body) = route("/favicon.svg", &s);
    assert_eq!((code, ctype), (200, "image/svg+xml"));
    assert!(String::from_utf8_lossy(&body).contains("<svg"));
    for f in ["archivo", "plex-mono-400", "plex-mono-500"] {
        let (code, ctype, body) = route(&format!("/fonts/{f}.woff2"), &s);
        assert_eq!((code, ctype), (200, "font/woff2"), "{f}");
        assert_eq!(&body[..4], b"wOF2", "{f}");
    }
    assert_eq!(route("/fonts/other.woff2", &s).0, 404);
}

#[test]
fn the_page_uses_the_brand_and_only_local_fonts() {
    for needle in [
        "#A6441A",
        "#E27A44",
        "#ECEFF0",
        "#0F1315",
        "url(\"/fonts/archivo.woff2\")",
        "\"IBM Plex Mono\"",
        "href=\"/favicon.svg\"",
        "class=\"brand-mark\"",
        "class=\"brand-word\">blyte<",
    ] {
        assert!(PAGE.contains(needle), "missing {needle}");
    }
    for gone in ["Graphik", "#e52321", "#2bad72"] {
        assert!(!PAGE.contains(gone), "still has {gone}");
    }
}
```

  Run `cargo test --workspace --test test_dashboard_brand`: it fails (no routes, old palette).
- [ ] **Step 2:** the routes, then the page, until the whole check passes.
- [ ] **Step 3: commit** `git commit --no-gpg-sign -m "feat(cli): the dashboard in blyte's palette, type and mark"`.
