# Declared window-client input textures

The 256 RGB PNG inputs hold section 24's raw-client pattern for seeds 0 through 255,
one 256 by 256 tile per seed. At (x,y), B=(7x+13s) mod 256, G=(5y+29s) mod 256,
and R=(x xor y xor s) mod 256. Larger coordinates and seed values repeat modulo 256.
`generate.py` authors the source inputs with PNG Sub filtering and zlib compression.

B13 permits declared client input textures. The guest decodes these assets through
`libimg::Decoded`, uploads them through `bgl::Texture::from_decoded` as Image content,
and copies the selected regions on the GPU. Window replay remains a kernel GPU copy.
The cases keep their independent scalar pixel formulas for comparison; these files
are client inputs, not the expected scene or scanout artifacts.
