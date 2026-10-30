# Recall: the R4 exit run at 150 ms RTT (protocol v42, logic v9)

Captured 2026-10-30 on the development workstation, not a CI runner, for the recall
amendment of [ADR-0012](../adr/0012-animation-runtime.md#amendment-recall-r4-exit-protocol-v42-simulation-logic-v9).

- Renderer: lavapipe (`llvmpipe`) from `/home/jaket/Projects/MOBA/.tools` with the bundled
  Khronos validation layer, `MOBA_VULKAN_VALIDATION=required`. No hardware GPU was used.
- CPU: AMD Ryzen 7 9800X3D; OS: Linux 7.3.0-2-cachyos, Wayland
- Build: release, this branch (based on `main` at `197857b`)

## What the roadmap asks
R4's exit: "a hero with idle, run, attack, three casts, death **and recall** plays correctly in a
networked match at 150 ms RTT". R4b ([perf/0018](0018-vat-anim-lod.md#exit-run-150-ms-rtt))
met everything but recall, which did not exist in the simulation. Recall now does (8 s
channel, `rules.recall_ticks`; cancelled by orders, casts, damage, stun and death; see the
`Simulation` docs), so this run repeats the exit with it.

## Setup
A real server with nine bot heroes, a link proxy at 150 ms RTT, 30 ms jitter and 5 % loss,
and one **player** client (blue slot 0, the skinned `sentinel-skinned` hero for every slot)
through the proxy. The player's orders are scripted with `MOBA_DEMO_ORDERS` (sent at the
first snapshot at or after each tick, so they travel the lossy link like a human's clicks):

```bash
moba-server --listen 127.0.0.1:18977 --seconds 150 --countdown 1
link-proxy --upstream 127.0.0.1:18977 --listen 127.0.0.1:18978 \
  --rtt 150 --jitter 30 --loss 5 --seconds 170
MOBA_SERVER=127.0.0.1:18978 MOBA_HERO_MODELS=sentinel-skinned MOBA_QUALITY=low \
  MOBA_WINDOW_SIZE=960x540 MOBA_VULKAN_VALIDATION=required MOBA_ANIM_LOG=anim.log \
  MOBA_WARMUP_TICKS=2700 MOBA_SMOKE_FRAMES=10 \
  MOBA_CAPTURE_SERIES=frame MOBA_CAPTURE_SERIES_TICKS=320,1100,1240,1250,1705,2420 \
  MOBA_DEMO_ORDERS="move:100:128@30,q@300,w@330,e@360,recall@600,move:70:128@640,\
recall@900,stop@960,recall@1000,move:129:128@1300,attack@1700,recall@1760,attack@1800,\
attack@1900,attack@2000" \
  moba-client      # plus the lavapipe VK_ICD_FILENAMES / VK_LAYER_PATH / LD_LIBRARY_PATH
```

The proxy forwarded 5,375 packets and dropped 246 (4.4 %); the server ran 2,716 ticks. The
client exited cleanly with no validation message. (This is the run after the review fixes:
the fog-of-war rule for recall ends, flag-driven recall effects, flag-confirmed prediction
and flag-based teleport snapping. The run before them, with the same orders, is described
under "Lost events" below.)

## The player's hero
`MOBA_ANIM_LOG` records each animated hero's action state and locomotion clip by render tick
(the interpolated view, about four ticks behind the newest snapshot); `[recall]` lines are
the recall events the client received, stamped with the newest snapshot tick then.

| Tick | Order sent | Server event received | Animation (render tick) |
|---:|---|---|---|
| 0 | — | — | idle at the spawn |
| 30 | move to (100, 128) | — | run from 37, idle at 244 |
| 300 / 330 / 360 | Q / W / E at the hero's feet | (casts) | `cast1` 302 (also while running at 306), `cast2` 332, `cast3` 364 |
| 600 | recall | started at (100, 128), tick 605 | `recall` loop from 607 |
| 640 | move to (70, 128) | **cancelled**, tick 644 (an obeyed move) | the loop fades out under the run from 648; idle at 775 |
| 900 | recall | started, tick 907 | `recall` from 909 |
| 960 | stop | **cancelled**, tick 966 (stop) | idle from 973 |
| 1000 | recall | started, tick 1006 | `recall` from 1008 |
| — | — | **completed**, tick 1244: (70, 128) → (52, 128) | idle from 1252, at the spawn |
| 1300 | move to (129, 128) beside the red tower | — | run from 1307, idle at 1639 |
| 1700 | attack the nearest enemy | — | `attack` at 1703 and 1729 |
| 1760 | recall | started, tick 1765 | `recall` from 1767 |
| 1800 | attack | **cancelled**, tick 1805 (an order) | idle from 1813 |
| — | — | (killed under the tower) | dead flag at 2406, `death` at 2411 |
| — | — | (respawned) | idle at 2705 |

So one skinned hero played idle, run, attack, three casts, death and recall — started four
times, cancelled by a move, a stop and an attack order, and completed — over a 150 ms /
30 ms / 5 % link. The completed recall shows both protections working:

- **Teleport snap.** The hero went from (70, 128) to the spawn (52, 128), 18 units, in one
  tick. The view held it at the channel spot until that snapshot's tick and then drew it at
  the spawn: no slide, and no run or turn on arrival (the log goes straight from `recall` to
  idle). The snap comes from the recall flag clearing between two snapshots, not from the
  distance: an earlier draft used a distance threshold, whose first value (16 units) missed
  this 18-unit recall and showed a one-frame `run`, and which a content pack's long dash or
  fast hero would have tripped.
- **Lost events.** This run received every recall edge. In the run before the review fixes
  (same orders, 246 of 5,638 packets dropped) the completion event was one of the dropped
  ones: the recall flag cleared in the next snapshot and ended the loop on the same tick an
  event would have (idle at 1252). The recall effect now follows the flag the same way, and
  also stops when the hero dies or leaves the snapshot.

![Player hero sequence](../images/r4-recall-sequence.png)

*Crops of the capture series (960×540 logical, Low): cast 1; recalling, with the HUD
channel bar and the recall effect's ring and light at the feet; the last channel tick; idle
at the spawn one tick after arrival; a basic attack beside the red tower; the death.*

## The other nine heroes
Over the same run the nine bot heroes started 476 `attack`, 48 `cast1`, 17 `cast2`, 19
`cast3` and 6 `death` actions (the player's hero: 2, 1, 1, 1, 1 and 4 `recall`), counted as
changes of each hero's action in the log. Bots do not recall, so no other recall event
reached the client.

## Fog of war
A client is told about an enemy's recall only while it sees that hero: the recall flag is
part of the entity (filtered with it), a start needs the hero in the viewer's vision in that
tick, and a cancel or completion needs it in the viewer's previous snapshot. Those report
where the hero channelled, which the viewer saw only if it saw the hero standing there; in
the tick a recall ends the hero may have teleported to its spawn, or a dash that cancelled it
may have carried it out of the fog, so seeing it *now* is not enough. Tests in
`crates/net/src/server/tests.rs`:

- `an_enemy_recall_out_of_vision_is_never_reported`: a red hero recalls deep in its own
  half for the whole 8 s; blue's client gets no event, no flagged snapshot and never even a
  net id for it.
- `a_recall_completed_in_hiding_is_not_reported_to_a_spawn_watcher`: the same, with blue's
  hero standing beside red's spawn. Blue sees red arrive, but no event (an earlier draft of
  this change sent it the completion, with the hidden channel spot in `x`/`y`).
- `a_dash_out_of_fog_does_not_reveal_where_a_recall_was_cancelled`: red channels unseen 19
  units from blue (vision 14) and dashes 7 units toward it, cancelling the recall at the
  hidden spot and landing in view: blue sees red, but no cancel.
- `a_visible_enemy_recall_is_reported_from_start_to_completion`: one beside blue's hero is
  flagged in 239 of the 240 channel snapshots (not the arrival's) and reported as started
  (`aux` 240) and completed (`x2`/`y2` = red's spawn), after which it is out of vision.
- `a_recall_start_needs_the_hero_in_view_and_its_end_needs_it_seen_before`: the rules above.

Before the fix both new tests failed: blue received `EVENT_RECALL { value: 2, x: 3200,
y: 960 }` (the hidden spot (200, 60), quantized) and a cancel at the pre-dash spot.

## Bandwidth
Recall costs nothing unless someone recalls: the flag is a spare bit of the existing entity
flags byte, and `EVENT_RECALL` is one 26-byte event per edge (at most three per recall). The
`soak` runs of [perf/0009](0009-presentation-events.md#protocol-v41-basic-attack-events), on
this branch (150 ms RTT, 5 % loss, 30 ms jitter; bots do not recall):

| Run | v41 down/client (perf/0009) | v42 down/client | v42 server sent |
|---|---:|---:|---:|
| `soak --seconds 60` (10 players) | 5.0–6.2 KiB/s | 5.0–6.2 KiB/s | 3,714 KiB |
| `soak --seconds 120 --clients 2` (8 bot heroes fight) | 7.2–7.3 KiB/s | 7.2 KiB/s | 1,848 KiB |

Upstream is 1.16–1.18 KiB/s (unchanged). A player recalling adds one event per edge to the
snapshots that carry it: under 0.1 KiB per recall.

## Goldens
Every `render-golden.yml` capture and comparison (default, networked, ember, outline probe,
VAT, Ultra, Low scaled, map overview/horizon, culling probe, environments, distortion) passes
unchanged on lavapipe with validation and sync validation, run through XWayland at scale 1 as
CI's Xvfb does: no golden was rebaselined. None of them shows a recall, a teleport or the
shop key.

## Reproduce
The server and proxy commands above, then the client with the environment shown. Two runs
with the same orders received the same recall edges within a tick; the animation log differs
only where packet loss moved a snapshot's arrival.
