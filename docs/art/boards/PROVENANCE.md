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
