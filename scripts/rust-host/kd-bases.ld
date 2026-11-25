/* AMD64 psABI §4.4.1 and LSB .eh_frame pointer encodings.
 * These symbols supply the local, static unwind fallback. The hosted
 * iterator also reports each object's own .text and .got addresses. */
SECTIONS {
    HIDDEN(__breenix_text_start = ADDR(.text));
    HIDDEN(__breenix_got_start = SIZEOF(.got) ? ADDR(.got) : 0);
}
INSERT BEFORE .text;
