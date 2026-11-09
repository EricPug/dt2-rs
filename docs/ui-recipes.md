# UI Recipes

These recipes pin the known panel navigation for the OS 1.17 section-3
diagnostic run. They use the saved machine snapshots and card image under
ignored `out/`; they are deterministic emulator inputs, not hardware behavior.
Each run writes a UART8 capture, log, resulting snapshot, and rendered final
frame under its output prefix. The script compares the rendered frame's SHA-256
with the expected value supplied on the command line.

## Input and timing conventions

Panel messages are comma-separated hexadecimal byte pairs:

| Message | Meaning |
|---|---|
| `2102,2100` | YES press, then release |
| `2104,2100` | UP press, then release |
| `2120,2100` | DOWN press, then release |
| `2140,2100` | RIGHT press, then release |
| `2180,2100` | TRK press, then release |
| `2201`, `2601,2600`, `2200` | FUNC down; SAMPLING press/release; FUNC release |
| `2208,2200` | PLAY press, then release |
| `2002,2000` | SRC / SAMPLE page |
| `2004,2000` | FLTR page |
| `3005,3000` | encoder channel 0 (A), +5 then zero delta |

`UART8_RX_PERIODIC` spaces messages by the recipe's instruction interval.
Encoder and page recipes use 3,000,000 instructions; track, browser, load, and
trigger recipes use 6,000,000. `UART8_RX_ONCE` ensures that the list is delivered
only once. Every key is released before the next unless explicitly stated.

The one latched modifier is FUNC in the browser chord: send FUNC down (`2201`),
SAMPLING down (`2601`), release SAMPLING (`2600`), then release FUNC (`2200`).
Do not insert `2200` between `2201` and `2601`. No other recipe latches a key.

## Recipes and screen assertions

| Gesture | Starting state | Sequence and interval | Final screen assertion |
|---|---|---|---|
| Encoder input fault check | `out/shots/main-after-load.snap` and `.png` | A/Tune: `3010`; E/Start: `3410`; D: `3310`, each +16 in a separate run | A and E match the no-turn hash `3c2c3309fff62686bd275285a587692c0edcc260e7419c283bf2d29b6aa7ae6a`; D leaves SAMPLE for Incoming. No parameter change confirmed; stop parameter work. |
| Change page to FLTR | same sample-loaded state | `2004,2000`, 3M | frame SHA-256 `20b7b62611a13133779f3da6e452591182fc7ff5bc37c2b9b3ada5e16b7133c3` |
| Return to SAMPLE | snapshot and rendered frame from the FLTR recipe | `2002,2000`, 3M | frame SHA-256 `18f5fff0850f13a36134f16346e12e56a00edd62adfb78eb434badcae0aede7b` |
| Select track 2 | sample-loaded main page | `2180,2100,2302,2300`, 3M | the `SMP 2` tile is selected; frame SHA-256 `e8940fe5181d7672ffe24a7f153f5fd7fcd0274c83da803fb21bf5c8602f4c04` |
| Browse to a card sample | sample-loaded main page | `2201,2601,2600,2200,2102,2100,2120,2100,2102,2100,2102,2100,2140,2100`; 6M | `incoming` action menu; frame SHA-256 `09fe754ea9c36a7826fc0a0aaec3260c89a5ae03df41d4e3a854c8030e279830` |
| Choose `LOAD TO TRK 01` | selected card sample's action menu | `2104,2100` x4, `2102,2100`, `-` x4, `2102,2100`; 6M | incoming list shows `008 kick verb` selected; frame SHA-256 `16430d9a6149ca1b7535333e3a111a1dfc54372b66d1f88cca4ca025c68d42f2` |
| Trigger 1 and PLAY | sample-loaded main page | `2301,2300,2208,2200`, 6M | sample page remains visible; frame SHA-256 `3c2c3309fff62686bd275285a587692c0edcc260e7419c283bf2d29b6aa7ae6a` |

The track-selection parameter is 1–16 and maps to the matching TRIG key after
the TRK tap; 1–8 use row `23` and 9–16 use row `24`. The trigger parameter
selects a TRIG key; it does not assert which firmware track receives the note.
For example, the tested trigger recipe's screen assertion is unchanged UI state
and does not prove audio. Only the modeled-DSP runner's captured note edge and
sample-matching WAV establish that modeled playback path.

The hardware owner's layout maps the grid's top-left Tune control to encoder A
and bottom-left Start control to encoder E. The +16 test did not change either
visible value; encoder D instead navigated to the Incoming sample list. The
former `ui-turn-encoder.sh` hash is also produced by the no-turn control, so it
is not a value-change assertion and remains excluded from `ui-smoke.sh`.

The card route is split into `ui-browse-sample.sh` and `ui-load-sample.sh`.
The load recipe is pinned to `out/shots/action-down.snap`, where the selected
sample's action menu is open. The known checkpoint shows `008 kick verb`
selected, `LOAD TO TRK 01` in the menu, and the cursor on `RENAME`. Its
cumulative instruction counter is 8,400,000,000. The original boot/input
transcript that created this snapshot was not retained, so this is known
state-level provenance, not a fully reproducible snapshot-generation recipe.
It depends on the settled incoming-card browser state and its selected sample;
a visually similar menu is not sufficient.

The continuous browse-to-load route was replayed from
`out/shots/main-after-load.snap` with the browser/load inputs spaced 6,000,000
instructions apart and a 2,000,000,000-instruction budget. The final screen
still showed `LOADING 1 SAMPLE`; its frame SHA-256 was
`bcc8d1f1ad4f05c2c73cc22413c1f77ff589508cf4fcee4d6b8ed5566a1c4626`. A second
attempt selected Track 2 first and reached a menu labeled `LOAD TO TRK 02`,
but running the load gestures from that visually similar state produced zero
RGPIO events and left the watched sample-selection word unchanged. Neither
continuous route is a verified load.

Keep the pinned load recipe until a replacement is shown to produce the RGPIO
page stream. Its screen assertion checks the resulting incoming-list selection,
while the modeled-DSP runner verifies the RGPIO upload and played waveform. Its
combined load-plus-trigger sequence remains in one emulator run so the modeled
DSP retains its uploaded sample.

## Run the scripts

Each runner takes its pinned starting snapshot, base frame, output prefix,
expected final-frame SHA-256, and optional `dt2-cli` binary:

```sh
tools/ui-change-page.sh FLTR \
  out/shots/main-after-load.snap out/shots/main-after-load.png \
  out/ui-recipes/fltr \
  20b7b62611a13133779f3da6e452591182fc7ff5bc37c2b9b3ada5e16b7133c3

tools/ui-open-sample-page.sh \
  out/ui-recipes/fltr.snap out/ui-recipes/fltr-frame-final.png \
  out/ui-recipes/sample-page \
  18f5fff0850f13a36134f16346e12e56a00edd62adfb78eb434badcae0aede7b

tools/ui-select-track.sh 2 \
  out/shots/main-after-load.snap out/shots/main-after-load.png \
  out/ui-recipes/track-2 \
  e8940fe5181d7672ffe24a7f153f5fd7fcd0274c83da803fb21bf5c8602f4c04

tools/ui-browse-sample.sh \
  out/shots/main-after-load.snap out/shots/main-after-load.png \
  out/ui-recipes/browse-sample \
  09fe754ea9c36a7826fc0a0aaec3260c89a5ae03df41d4e3a854c8030e279830

tools/ui-load-sample.sh \
  out/shots/action-down.snap out/shots/action-down.png \
  out/ui-recipes/load-sample \
  16430d9a6149ca1b7535333e3a111a1dfc54372b66d1f88cca4ca025c68d42f2

tools/ui-trigger-track.sh 1 \
  out/shots/main-after-load.snap out/shots/main-after-load.png \
  out/ui-recipes/trigger-1 \
  3c2c3309fff62686bd275285a587692c0edcc260e7419c283bf2d29b6aa7ae6a
```

`ui-load-sample.sh --messages` and `ui-trigger-track.sh --messages <n>` emit
their encoded sequences for a caller that must combine them in one emulator
run. `tools/ui-smoke.sh` uses the page-change and SAMPLE-return recipes; its encoder
mutation check is skipped until a visible change is confirmed.
`tools/modeled-dsp-sample.sh` uses the load and trigger recipes.

No repository agent-instructions file was present when these recipes were
added.
