# Markdown editor test fixtures

This directory contains files for manual Markdown editor validation.

## Images

The `images/` directory contains sample images and `image_test.md`. The Markdown file covers:

- Relative paths such as `./sample1.jpg`.
- Parent directory references such as `../parent_test.jpg`.
- Absolute paths.
- JPEG images, including one file with a `.png` suffix.
- Images in lists.
- Empty alt text.

Open `images/image_test.md` in NeoWarp to test image rendering. Replace the sample absolute path with a valid path
on the test machine before following that case.

## Table of contents navigation

`toc_anchor_test.md` covers Markdown fragment navigation, case-insensitive heading matching, and scrolling.

Open `toc_anchor_test.md` in NeoWarp's Markdown view and select the table of contents links.
