# Boards: provenance

Every board here is a curated, size-capped JPEG (≤ 400 KB). The full sets, with a per-image
provenance record (model, version, weights licence, seed, prompt, workflow, date), are in the
review app's `manifest.json` (outside the repo; see [../README.md](../README.md#review-app)).

**Licence of the boards:** CC0-1.0 (roadmap §9.5: AI-generated output is CC0 in the repo; the
other boards are made by the project's own scripts). They are **concept and reference
material**, not shipped game assets, so they don't go through the `content-check` licence gate.

| Board | What it shows | How it was made | Inputs and licences |
|---|---|---|---|
| `ember-moods.jpg` | Ember Crossing under the four candidate lighting moods (gameplay camera) | Real engine captures (Compat renderer, Ultra, 1920×1080) by the review app's `tools/moods.sh`, after `env-bake` with each mood's inputs; montage with ImageMagick | Today's `content/` (project-original); no third-party inputs |
| `palettes.jpg` | Team, faction and map palettes with OKLCH values; the hue wheel of reserved team-colour zones | `tools/boards.py palettes` (Pillow), from the values in [../palette-and-value.md](../palette-and-value.md) | Noto Sans font for labels (SIL OFL 1.1, rendered into the image only) |
| `scale.jpg` | Every size class drawn at its true pixel height at 1080p, 1440p and 4K, heroes plain and with ally and enemy rims | `tools/boards.py scale` (Pillow), from the table in [../silhouettes-and-scale.md](../silhouettes-and-scale.md) | Noto Sans font for labels (SIL OFL 1.1) |
| `factions.jpg` | The three readings of each faction: architecture (seeds 1100/2100) and tower (1101/2101) | AI-generated, Z-Image-Turbo; contact sheet by `tools/boards.py sheet` | Text prompts only (no image inputs); prompts in the review app's `prompts/round1-factions.json` |
| `heroes.jpg` | The three slice-hero concepts: full body (seeds 3100, 3103, 3105) and a portrait (3200) | As above | Prompts in `prompts/round1-heroes.json` |
| `units.jpg` | Minions (seed 4100) and structures (seed 5101) for both factions | As above | Prompts in `prompts/round1-minions.json` and `round1-structures.json` |
| `ember-concepts.jpg` | Ember Crossing in the four moods: aerial (seed 6101) and vista (6200) | As above. These are concept images, not engine captures | Prompts in `prompts/round1-ember.json` |

**AI model for every generated board:** Z-Image-Turbo (Tongyi-MAI), weights **Apache-2.0**
(<https://huggingface.co/Tongyi-MAI/Z-Image-Turbo>), run locally in ComfyUI from the
Comfy-Org repack. Components:
- `z_image_turbo_bf16.safetensors`, SHA-256 `2407613050b8…74a6` (matches Hugging Face);
- the Qwen3-4B text encoder (Apache-2.0);
- the FLUX.1-schnell autoencoder (Apache-2.0).

Sampler: `res_multistep`/`simple`, 9 steps, CFG 1.0, AuraFlow shift 3.0, no negative
prompt. Every image's full record (prompt, seed, workflow JSON, date) is in the review app's
`manifest.json`. No image inputs, LoRAs, or real people's likenesses were used.
