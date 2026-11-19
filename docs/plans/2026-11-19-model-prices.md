# Prices for the Gemini 4 models

Date: 2026-11-19. Source: Google's Gemini API pricing page, read 2026-11-19. USD per 1M tokens.

| Model id contains | Period | Input | Cached input | Output |
|---|---|---|---|---|
| `gemini-4-hydrogen` | any date | 1.50 | 0.075 | 7.50 |
| `gemini-4-neon` | any date | 3.00 | 0.15 | 15.00 |
| `gemini-4-argon` | through 2026-11-30 | 2.00 | 0.10 | 10.00 |
| `gemini-4-argon` | from 2026-12-01 | 4.00 | 0.20 | 20.00 |
| `gemini-4-carbon` | through 2026-11-30 | 2.00 | 0.10 | 10.00 |
| `gemini-4-carbon` | from 2026-12-01 | 4.00 | 0.20 | 20.00 |

### Task MP1: price Hydrogen, Neon and Carbon, and correct Argon's cached rate

**Goal.** `blyte` reports no cost for runs on `gemini-4-hydrogen`, `gemini-4-neon` or `gemini-4-carbon`
because `pricing_on` has no entry for them, and Argon's cached rate from 2026-12-01 is an assumed 0.40
where Google now lists 0.20.

**Depends on:** none. **Siblings:** none.

**Files.** `crates/blyte-provider/src/lib.rs` (the function `pricing_on` only) and a new
`crates/blyte-provider/tests/test_gemini4_prices.rs`.

**What to build.** Make `pricing_on(model_id, date)` return exactly the values in the table above
for a model id containing each name (ids arrive with prefixes, such as `gemini/gemini-4-neon` and
`vertex/gemini-4-argon`). The `gemini-3.8-flash` entry is unchanged. Any other model id still returns
`None`: no default price. Delete the comments inside `pricing_on`, including the one that says the
cached rate is an assumption; the test names carry those facts. Add no comments.

**Tests** (`cargo test -p blyte-provider --test test_gemini4_prices`), each asserting all three
fields exactly:

1. `hydrogen_price`: `pricing_on("gemini/gemini-4-hydrogen", (2026, 11, 19))` and the same on
   `(2027, 1, 1)` are 1.50 / 0.075 / 7.50.
2. `neon_price`: `gemini-4-neon` on both dates is 3.00 / 0.15 / 15.00.
3. `argon_introductory_price_ends_2026_11_30`: on `(2026, 11, 30)` 2.00 / 0.10 / 10.00; on
   `(2026, 12, 1)` 4.00 / 0.20 / 20.00.
4. `carbon_introductory_price_ends_2026_11_30`: the same two dates and values for
   `vertex/gemini-4-carbon`.
5. `flash_price_unchanged`: `gemini-3.8-flash-high` is 0.75 / 0.075 / 3.75.
6. `unknown_model_has_no_price`: `pricing_on("gemini-4-xenon", (2026, 11, 19))` is `None`.

If an existing test asserts Argon's cached rate of 0.40 from December, change that value to 0.20
and say so in the report. Otherwise existing tests pass unmodified.

**Constraints.** No fallbacks: an unknown model has no price. No new dependencies.

**Commit message:** `fix(cost): price Hydrogen, Neon and Carbon, and correct Argon's cached rate from December`
