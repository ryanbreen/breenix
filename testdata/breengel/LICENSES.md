# Image fixture licences

- `png/*.png`: PngSuite, copyright Willem van Schaik, 1996, 2011;
  permission to use, copy, modify and distribute for any purpose without fee.
  The full notice is in `png/LICENSE`. Source and data hashes: `png/PROVENANCE.json`.
- `png/*.bgra`: reference pixel conversions of those images, under the same permission.
- `inflate/*.deflate`, `inflate/*.raw`: original generated fixtures, dedicated to the
  public domain under CC0-1.0; see `inflate/LICENSE` and `inflate/LICENSE-CC0.txt`.
  Encoder version and data hashes: `inflate/PROVENANCE.json`.

External codecs were used only at authoring time to record fixture bytes. The suite
reads these pinned files and never invokes an external encoder or decoder.
