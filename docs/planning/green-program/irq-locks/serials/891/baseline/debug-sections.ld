SECTIONS {
.debug_abbrev 0 : { *(.debug_abbrev) }
.debug_info 0 : { *(.debug_info .debug_info.*) }
.debug_aranges 0 : { *(.debug_aranges) }
.debug_ranges 0 : { *(.debug_ranges) }
.debug_str 0 : { *(.debug_str) }
.debug_line 0 : { *(.debug_line .debug_line.*) }
.debug_loc 0 : { *(.debug_loc) }
.debug_pubnames 0 : { *(.debug_pubnames) }
.debug_pubtypes 0 : { *(.debug_pubtypes) }
.debug_frame 0 : { *(.debug_frame) }
.debug_line_str 0 : { *(.debug_line_str) }
.debug_rnglists 0 : { *(.debug_rnglists) }
.debug_loclists 0 : { *(.debug_loclists) }
.debug_str_offsets 0 : { *(.debug_str_offsets) }
.debug_addr 0 : { *(.debug_addr) }
}
