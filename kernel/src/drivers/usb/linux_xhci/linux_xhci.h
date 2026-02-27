#ifndef BREENIX_LINUX_XHCI_H
#define BREENIX_LINUX_XHCI_H

#include <stdint.h>
#include <stddef.h>

struct breenix_xhci_state {
    uint64_t base;
    uint64_t op_base;
    uint64_t rt_base;
    uint64_t db_base;
    uint8_t cap_length;
    uint8_t max_slots;
    uint8_t max_ports;
    uint8_t context_size;
};

int linux_xhci_init(struct breenix_xhci_state *state);

#endif
