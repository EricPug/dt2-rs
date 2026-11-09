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
| `3001,3000` | encoder channel 0 (A), +1 then zero delta |

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
| Turn A/Tune by one count, 16 times | `out/shots/main-after-load.snap` and `.png` | `3001,3000` repeated 16 times; 3M between messages | Temporary Tune-label readout shows `-3.00`; frame SHA-256 `0121a4cf0880c5892dabd094dbc7cf4caf6d14fd9d93cfd5b276d37981a74368`. No-turn and single `+16` hash: `3c2c3309fff62686bd275285a587692c0edcc260e7419c283bf2d29b6aa7ae6a`. |
| Turn A/Tune once by 16 counts | same sample-loaded state | `3010`, 3M | Same hash as no-turn; no visible value change. |
| Rotate D once by 16 counts | same sample-loaded state | `3310`, 3M | D is a rotation, not a press; the observed screen changed to the Incoming sample list. Sample selection is menu-driven, so this is not a sample-selection gesture; the reason for the screen transition is unclassified. |
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
and bottom-left Start control to encoder E. One A `+16` rotation did not change
the Tune readout, while sixteen separate A `+1` rotations did. The latter is
the verified encoder assertion used by `tools/ui-smoke.sh`; the `+16` result
remains distinct and inconclusive about why combined deltas behave differently.
Matched SRAM and DSPI2 captures did not change, so this visible UI response does
not yet establish parameter state or justify modeled-DSP changes.

The card route is split into `ui-browse-sample.sh` and `ui-load-sample.sh`.
The load recipe is pinned to `out/shots/action-down.snap`, where the selected
sample's action menu is open. The known checkpoint shows `008 kick verb`
selected, `LOAD TO TRK 01` in the menu, and the cursor on `RENAME`. Its
cumulative instruction counter is 8,400,000,000. The original boot/input
transcript that created this snapshot was not retained, so this is known
state-level provenance, not a fully reproducible snapshot-generation recipe.
It depends on the settled incoming-card browser state and its selected sample;
a visually similar menu is not sufficient.

For a manual attempt from the sample-loaded main page: hold FUNC, press and
release SAMPLING, then release FUNC; select `+DRIVE` with YES, move DOWN to
`incoming` and press YES. Highlight the desired sample, press YES to check it,
then RIGHT for its action menu. In the pinned kick checkpoint the cursor is on
`RENAME`: UP four times selects `LOAD TO TRK 01`. In a later replay from that
selected-menu state, one YES press and release was enough to load the sample;
the left-side `DESTIN.: RAM BANK ANY` is visible in the menu itself, not proof
of a separate confirmation dialog. The confirmed kick upload starts from that
pinned action-menu snapshot, not from an uninterrupted main-page replay.
Repeating the sequence by hand is not guaranteed to load a sample.
An uninterrupted first-load run from a fresh card-attached boot subsequently
loaded the kick and accepted DOWN afterward; see the fresh-boot comparison and
exact reproduction in the canonical handoff. This does not resolve the
previously loaded or manual `LOAD TO PROJECT` cases.
Run `tools/ui-fresh-kick.sh target/release/dt2-cli` to check this first-load
route, its post-load browser frame, and the uploaded PCM. The full UI smoke
gate also runs it.

Select by the visible label rather than blindly sending four UP presses:
`LOAD TO PROJECT` is above `LOAD TO TRK 01` and is a different action. In a
manual attempt `LOAD TO PROJECT` remained on `LOADING 1 SAMPLE`; the app
recorded no additional card commands or RGPIO events. See the handoff's manual
attempt section for the preserved evidence.

The continuous browse-to-load route was replayed from
`out/shots/main-after-load.snap` with the browser/load inputs spaced 6,000,000
instructions apart and a 2,000,000,000-instruction budget. The final screen
still showed `LOADING 1 SAMPLE`; its frame SHA-256 was
`bcc8d1f1ad4f05c2c73cc22413c1f77ff589508cf4fcee4d6b8ed5566a1c4626`. A second
attempt selected Track 2 first and reached a menu labeled `LOAD TO TRK 02`,
but running the load gestures from that state produced zero RGPIO events. One
retry from its saved menu snapshot again produced zero RGPIO events and no UART
display updates; the captured word at `0x80005428` remained `0x012c` (300).
Neither continuous route is a verified load. Captures:
`out/modeled-dsp/two-track-loaded.log` and
`out/modeled-dsp/two-track-loaded-retry.log`.
In a later label-matched Track 1 kick comparison, the continuous route
produced no upload, while the pinned pre-upload state did; repeating the kick
after a successful pinned upload also produced no upload. The reason is
unconfirmed; see the matched comparison in the canonical handoff.

## Shared-pool, two-track load flow (known bad in emulator)

The attempted shared-pool flow selected a destination track with a latched
TRK+TRIG chord, then used SRC encoder D to choose an empty slot and YES to open
the assignment browser. Selecting the already-loaded kick with YES left the
browser unresponsive in matched DTIM3 = 4 and 16 replays. A later hand test
reported the same symptom with clap and Track 2. Treat SRC-slot-YES as a known
bad path in this emulator, not as a verified way to load samples; see
[the handoff](./section3-handoff.md#slot-number-correlation-and-routing-limit).
A plain TRIG tap is not a track selection.

The emulator's Track 1 chord input was sent as `2180,2301,2300,2100,-`; the
display showed `SMP 1`, but the starting frame already showed `SMP 1`, so the
chord is untested, not failed. The starting frame was already on SRC; the SRC
input left the sample controls visible. One D `+1`, one DOWN, and four separate
spaced D `+4` turns all left the screen on `SMP 1`; no empty slot was visibly
reached. Screens: `menu-track1-src.png`, `menu-track1-slot-step1.png`,
`menu-track1-down.png`, and `menu-track1-d-plus4.png` through
`menu-track1-d-plus4-step4.png`.

The earlier D `+16` image shows a list of sample names, but its `Tune Max
EncoderD` heading does not prove that it is the +Drive browser. The verified
+Drive route starts from `out/shots/close-browser.snap`: its screen shows
`SAMPLES` with `+DRIVE` selected. Entering +Drive shows `Factory A` and
`incoming`; entering `incoming` shows `008 kick verb` and `303 clap real`.
Per Elektron's FAQ, factory samples do not occupy +Drive space, so the Factory
tree appearing empty is expected; do not investigate it as an emulator issue.
One DOWN from that listing moved the white highlight to the clap; a subsequent
YES in the earlier attempt left that frame unchanged, produced zero RGPIO
events, and left the observed `0xe0` word at `0x012c`.
The fallback did not load or replace a sample. Frames:
`plusdrive-list.png`, `plusdrive-incoming-folder.png`,
`plusdrive-clap-selected.png`, and `plusdrive-clap-load-result.png`.

A bounded Route A retry started from
`out/modeled-dsp/plusdrive-incoming-folder.snap` and sent DOWN as `2120` held
for 3M instructions, then release `2100`. The white highlight moved from kick
to clap and stayed there through 51M instructions in 3M chunks. The `C044`
marker on the kick row is separate from the highlight; the previous report
mistook it for the cursor. No YES/RIGHT or action-menu/load input was sent in
that run, and it reported zero RGPIO events. Frames and logs are under
`out/ui-frames/` (`00-start.png`, `01-down.png`, and `02-wait.png` through
`16-wait.png`).

Continuing from the clap-highlighted snapshot, YES (`2102`, release `2100`)
added a check mark beside the clap, and RIGHT (`2140`, release `2100`) opened
the action menu. TRK+TRIG 2 (`2180,2302,2300,2100`, with each held for 3M
instructions) changed its entry from `LOAD TO TRK 01` to `LOAD TO TRK 02`.
DOWN (`2120,2100`) moved the white highlight to that entry; YES was then sent
and released. The display showed `LOADING 1 SAMPLE` after 69M instructions and
remained there through 300M. All 100 post-confirmation 3M chunks reported zero
RGPIO events. The popup obscures the clap row, so a loaded marker cannot be
checked; no upload header, page count, or length was captured. The SRAM word
corresponding to candidate DSPI2 offset `0xe0` remained `0x012c`; this is not
a direct capture of a DSPI2 frame. It is inconclusive whether the kick's slot
would be replaced.
Frames: `out/ui-frames/17-yes-press.png` through
`out/ui-frames/26-down-release.png`, then `27-load-yes-press.png`,
`28-load-yes-release.png`, and `29-load-wait.png` through `126-load-wait.png`.

This remains inconclusive: no second upload, replacement, or WAV was confirmed.
Do not compare sample headers or claim two-track playback until the sample load
completes and a distinct upload is captured. Keep the existing pinned load
recipe for the one-sample regression only; its combined load-plus-trigger run
preserves the modeled DSP's uploaded sample.

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
