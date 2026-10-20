This private decoder is adapted from `lzxd` 0.2.7 by Lonami, under the MIT
license reproduced in LICENSE-MIT. Upstream: https://codeberg.org/Lonami/lzxd

CHM archives with only SpanInfo have no frame-offset table. The local adaptation
returns consumed input bytes for each decoded 32 KiB frame, caps raw reads at the
frame boundary, and consumes frame alignment/padding. Decoding remains sequential;
the Huffman tables, dictionary, and unfinished blocks survive between frames.
The E8 zero-target case is corrected, and dictionary resets preserve the absolute
E8 output position. Invalid match offsets return errors before copying window
bytes. The original decoder tests are retained.
