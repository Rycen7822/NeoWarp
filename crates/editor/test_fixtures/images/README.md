# Markdown image fixtures

These images support manual Markdown image rendering checks.

## Files

- `image_test.md` contains the image references and rendering cases.
- `sample1.jpg` is a 400 by 300 JPEG photograph of water and the horizon.
- `sample2.jpg` is a 600 by 400 grayscale JPEG photograph of musicians on a stage.
- `sample3.png` is a 300 by 300 photograph of palm trees. The file contains JPEG data despite its `.png` suffix.
- `parent_test.jpg` is a 300 by 200 JPEG photograph of a covered dock and water. It is in the parent directory to
  test relative path resolution.

## Coverage

The fixtures provide several image sizes, a parent directory reference, and a mismatch between a filename suffix
and the image data. Every image fixture contains JPEG data. They do not provide PNG decoding or alpha transparency
coverage.
