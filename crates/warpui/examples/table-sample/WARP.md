# Table example screenshot checks

Capture each table demo as a PNG and inspect the images for blank regions, missing content, misalignment, and
clipping. These visual checks do not replace pixel comparisons.

## Build the example

From the repository root, build the example with the pinned Rust toolchain:

```bash
cargo +1.92.0 build --locked -p warpui --example table-sample
```

The checked-in [.cargo/config.toml](../../../../.cargo/config.toml) sets `build.target-dir` to `.work/target`, so
the build and capture commands use the same executable path. Run the capture commands in Bash, including Git Bash
on Windows. They run the executable from `.work/table-screenshots` so that generated images stay under `.work/`.

## Capture baseline images

Run the built executable with `--capture-baseline`:

```bash
mkdir -p .work/table-screenshots
(
	cd .work/table-screenshots
	../target/debug/examples/table-sample --capture-baseline
)
```

On Windows, use `../target/debug/examples/table-sample.exe` as the executable path.

The example writes the images to `.work/table-screenshots/screenshots/baseline/`. It exits after capturing all
demos.

## Capture current images

After changing the table implementation, run the build command again. Then run the rebuilt example with
`--capture-screenshots`:

```bash
mkdir -p .work/table-screenshots
(
	cd .work/table-screenshots
	../target/debug/examples/table-sample --capture-screenshots
)
```

On Windows, use the `.exe` path shown above. The example writes the images to
`.work/table-screenshots/screenshots/current/` and exits after capturing all demos.

## Inspect the images

Open every PNG in the baseline and current directories with an image viewer or an image-capable file reader.
Compare the matching demos and check for:

- Blank, black, or solid-color regions where the UI should appear.
- Missing headers, rows, or columns.
- Misaligned row bands, headers, and body cells.
- Text that is clipped or unreadable because of low contrast.

Report the image path and the visible defect for each failure. To inspect one demo without capturing images, run
the executable without either capture flag:

```bash
mkdir -p .work/table-screenshots
(
	cd .work/table-screenshots
	../target/debug/examples/table-sample
)
```

On Windows, use the `.exe` path shown above. Use the Left Arrow and Right Arrow keys to move between demos. Close
the window to exit interactive mode.
