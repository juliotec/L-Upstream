# build-assets

The finish stage's files: what the game folder holds beside the compiled game, made from the player's romfs and the
repository's assets. The driver calls `Finish` after the compile; it reads the romfs (through the index, or a
folder) and writes the texts, the button prompts, the shader list, the pipeline lists and the icons, each cached by
exactly its inputs.

## Main parts

- [`src/lib.rs`](src/lib.rs): `Finish`, what the driver calls; `title_res`, the exe's name for Windows.
- [`src/texts.rs`](src/texts.rs): the text that names the console, reworded for a PC in every language (`texts/`).
- [`src/prompts/`](src/prompts/mod.rs): other controllers' button prompts: the icon font redrawn per face, the
  textures, the layouts' places (`prompts/`, served by `host/prompts.c`); with [`src/font/`](src/font/mod.rs) (an
  OpenType CFF font with glyphs added) and [`src/texture/`](src/texture/mod.rs) (the game's ASTC and BC4 textures read
  and written back).
- [`src/census.rs`](src/census.rs): every shader program in the game's archives (`longshot_shaders.bin`).
- [`src/pipelines.rs`](src/pipelines.rs): the pipeline lists a release ships (`longshot_pipeline_states_shipped.bin`);
  [`src/share.rs`](src/share.rs): a player's list zipped for the "Submit your pipelines" issue (`longshot-builder share`).
- [`src/icon.rs`](src/icon.rs): the window's icon (`longshot.png`) and the exe's `.ico` and `.res`.
- [`src/cache.rs`](src/cache.rs): the cache; [`src/romfs.rs`](src/romfs.rs): `Romfs`, where the files are read from.
- [`src/sarc.rs`](src/sarc.rs), [`src/zs.rs`](src/zs.rs): the game's archives and its dictionary zstd frames;
  [`src/raster.rs`](src/raster.rs), [`src/draw.rs`](src/draw.rs): pictures, drawn and resampled as Pillow does.

## Used by / uses

Used by `build-driver` and `longshot-builder` (`share`). Uses `build-core`, `game-files`, ab_glyph_rasterizer, crc32fast, flate2,
lzma-rs, png, ruzstd, serde, serde_json, sha1_smol, ttf-parser and zune-jpeg (all pure Rust).

## Rules it keeps

- **Each output is cached by exactly what it is made from**: the files it reads by content, the options that change
  it, and its recipe's version, a constant beside the code that makes it, raised when that code would make something
  else of the same inputs. One changed text archive remakes that language alone.
- The pictures follow Pillow's operations to the rounding where it matters, so they look as they did when Pillow made them.
- What is made from the player's romfs (the shader list, the prompts' textures, the texts) stays on their PC. An asset
  that fails is a warning (`asset_not_made`), not a failed build.

## Tests

```
cargo test -p build-assets
```

Each module has its own tests on synthetic inputs: the cache's keys, the font read and written, ASTC and the textures,
the drawing and the pictures, the prompts' layouts and guides, the census, the pipeline lists and the zip, SARC and
the dictionary frames, the texts, the icons. `cargo run --release -p build-assets --example assets -- <step> <romfs>
<out folder>` makes one output to compare with another build's.

See [a build](../../../docs/dev/reference/builder.md#a-build) (step 5, finish) and
[the game folder](../../../docs/dev/reference/builder.md#the-game-folder).
