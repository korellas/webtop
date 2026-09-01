# Credits

## Wandering cat sprite

The wandering cat overlay (sitting idle + 6-frame run cycle) is **original
artwork**, drawn procedurally as anti-aliased silhouettes by
`frontend/scripts/gen_cat.py` and tinted per cat/theme by
`frontend/scripts/bake_cats.py`. No third-party sprite assets are used.

To regenerate after editing the generator:

```bash
python3 frontend/scripts/gen_cat.py    # -> cat_run.png, cat_idle.png
python3 frontend/scripts/bake_cats.py  # -> cat_{run,idle}_{cpu,gpu}_{dark,light}.png
```

### History

Earlier versions used the **RunCat** runner frames by Takuto Asakura (Kyome22,
Apache-2.0, https://github.com/Kyome22/RunCat_for_windows). Those 5 pixel
frames have been fully replaced by the procedural silhouettes above, so no
external license now applies to the cat art.
