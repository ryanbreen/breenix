// SPDX-License-Identifier: GPL-2.0-or-later
//
// Minimal Linux xHCI harness for Breenix (C-based, GPL).
// This is a targeted, trimmed port of Linux xHCI logic for init,
// context construction, and endpoint setup. It is not a full USB stack.

#include "linux_xhci.h"

#include <stdint.h>
#include <stddef.h>
#include <stdbool.h>

// ---------------------------------------------------------------------------
// Rust-provided helpers (C ABI)
// ---------------------------------------------------------------------------
extern void breenix_xhci_trace_raw_c(uint8_t op, uint8_t slot, uint8_t dci, const uint8_t *data, size_t len);
extern void breenix_xhci_trace_note_c(uint8_t slot, const uint8_t *data, size_t len);
extern uint64_t breenix_virt_to_phys_c(uint64_t addr);
extern void breenix_dma_cache_clean_c(const uint8_t *ptr, size_t len);
extern void breenix_dma_cache_invalidate_c(const uint8_t *ptr, size_t len);

// ---------------------------------------------------------------------------
// Small libc helpers (no libc available)
// ---------------------------------------------------------------------------
static void *bmemset(void *dst, int value, size_t len) {
    uint8_t *p = (uint8_t *)dst;
    for (size_t i = 0; i < len; i++) {
        p[i] = (uint8_t)value;
    }
    return dst;
}

static void *bmemcpy(void *dst, const void *src, size_t len) {
    uint8_t *d = (uint8_t *)dst;
    const uint8_t *s = (const uint8_t *)src;
    for (size_t i = 0; i < len; i++) {
        d[i] = s[i];
    }
    return dst;
}

static size_t c_strlen(const char *s) {
    size_t n = 0;
    while (s && s[n] != '\0') {
        n++;
    }
    return n;
}

static void trace_note(uint8_t slot, const char *s) {
    size_t len = c_strlen(s);
    if (len == 0) {
        return;
    }
    breenix_xhci_trace_note_c(slot, (const uint8_t *)s, len);
}

static void trace_port_found(uint8_t port_num) {
    char buf[16];
    size_t idx = 0;
    buf[idx++] = 'p';
    buf[idx++] = 'o';
    buf[idx++] = 'r';
    buf[idx++] = 't';
    buf[idx++] = '_';

    unsigned int val = port_num;
    char digits[3];
    unsigned int dlen = 0;
    if (val == 0) {
        digits[dlen++] = '0';
    } else {
        while (val > 0 && dlen < sizeof(digits)) {
            digits[dlen++] = (char)('0' + (val % 10u));
            val /= 10u;
        }
    }
    for (int i = (int)dlen - 1; i >= 0; i--) {
        buf[idx++] = digits[i];
    }

    const char suffix[] = "_found";
    for (size_t i = 0; i < sizeof(suffix) - 1u; i++) {
        buf[idx++] = suffix[i];
    }
    buf[idx] = '\0';
    trace_note(0, buf);
}

// ---------------------------------------------------------------------------
// Trace op codes (match Rust XhciTraceOp)
// ---------------------------------------------------------------------------
#define TRACE_MMIO_W32 1
#define TRACE_MMIO_W64 2
#define TRACE_CMD_SUBMIT 10
#define TRACE_CMD_COMPLETE 11
#define TRACE_XFER_SUBMIT 12
#define TRACE_XFER_EVENT 13
#define TRACE_DOORBELL 14
#define TRACE_INPUT_CTX 20
#define TRACE_OUTPUT_CTX 21
#define TRACE_CACHE_OP 30
#define TRACE_NOTE 50

static void trace_mmio_w32(uint64_t addr, uint32_t val) {
    uint8_t buf[12];
    bmemcpy(buf, &addr, 8);
    bmemcpy(buf + 8, &val, 4);
    breenix_xhci_trace_raw_c(TRACE_MMIO_W32, 0, 0, buf, sizeof(buf));
}

static void trace_mmio_w64(uint64_t addr, uint64_t val) {
    uint8_t buf[16];
    bmemcpy(buf, &addr, 8);
    bmemcpy(buf + 8, &val, 8);
    breenix_xhci_trace_raw_c(TRACE_MMIO_W64, 0, 0, buf, sizeof(buf));
}

static void trace_input_ctx(uint8_t slot, const uint8_t *base, size_t ctx_size, uint8_t max_dci) {
    size_t total = (2 + (size_t)max_dci) * ctx_size;
    if (total > 256) {
        total = 256;
    }
    breenix_xhci_trace_raw_c(TRACE_INPUT_CTX, slot, max_dci, base, total);
}

static void trace_output_ctx(uint8_t slot, const uint8_t *base, size_t ctx_size, uint8_t max_dci) {
    size_t total = (1 + (size_t)max_dci) * ctx_size;
    if (total > 256) {
        total = 256;
    }
    breenix_xhci_trace_raw_c(TRACE_OUTPUT_CTX, slot, max_dci, base, total);
}

static void trace_trb(uint8_t op, uint8_t slot, uint8_t dci, const void *trb) {
    breenix_xhci_trace_raw_c(op, slot, dci, (const uint8_t *)trb, 16);
}

// ---------------------------------------------------------------------------
// MMIO helpers
// ---------------------------------------------------------------------------
static inline uint32_t read32(uint64_t addr) {
    return *(volatile uint32_t *)addr;
}

static inline void write32(uint64_t addr, uint32_t val) {
    *(volatile uint32_t *)addr = val;
    trace_mmio_w32(addr, val);
}

static inline uint64_t read64(uint64_t addr) {
    return *(volatile uint64_t *)addr;
}

static inline void write64(uint64_t addr, uint64_t val) {
    *(volatile uint64_t *)addr = val;
    trace_mmio_w64(addr, val);
}

// ---------------------------------------------------------------------------
// USB descriptor structures
// ---------------------------------------------------------------------------
struct usb_config_descriptor {
    uint8_t bLength;
    uint8_t bDescriptorType;
    uint16_t wTotalLength;
    uint8_t bNumInterfaces;
    uint8_t bConfigurationValue;
    uint8_t iConfiguration;
    uint8_t bmAttributes;
    uint8_t bMaxPower;
} __attribute__((packed));

struct usb_interface_descriptor {
    uint8_t bLength;
    uint8_t bDescriptorType;
    uint8_t bInterfaceNumber;
    uint8_t bAlternateSetting;
    uint8_t bNumEndpoints;
    uint8_t bInterfaceClass;
    uint8_t bInterfaceSubClass;
    uint8_t bInterfaceProtocol;
    uint8_t iInterface;
} __attribute__((packed));

struct usb_endpoint_descriptor {
    uint8_t bLength;
    uint8_t bDescriptorType;
    uint8_t bEndpointAddress;
    uint8_t bmAttributes;
    uint16_t wMaxPacketSize;
    uint8_t bInterval;
} __attribute__((packed));

struct usb_ss_ep_comp_descriptor {
    uint8_t bLength;
    uint8_t bDescriptorType;
    uint8_t bMaxBurst;
    uint8_t bmAttributes;
    uint16_t wBytesPerInterval;
} __attribute__((packed));

struct usb_hid_descriptor {
    uint8_t bLength;
    uint8_t bDescriptorType;
    uint16_t bcdHID;
    uint8_t bCountryCode;
    uint8_t bNumDescriptors;
    uint8_t bReportDescriptorType;
    uint16_t wDescriptorLength;
} __attribute__((packed));

#define USB_DT_INTERFACE 4
#define USB_DT_ENDPOINT 5
#define USB_DT_HID 0x21
#define USB_DT_SS_ENDPOINT_COMP 0x30
#define USB_CLASS_HID 0x03

#define USB_ENDPOINT_XFER_CONTROL 0
#define USB_ENDPOINT_XFER_ISOC 1
#define USB_ENDPOINT_XFER_BULK 2
#define USB_ENDPOINT_XFER_INT 3

static inline uint8_t usb_endpoint_type(const struct usb_endpoint_descriptor *d) {
    return d->bmAttributes & 0x3;
}

static inline bool usb_endpoint_xfer_int(const struct usb_endpoint_descriptor *d) {
    return usb_endpoint_type(d) == USB_ENDPOINT_XFER_INT;
}

static inline bool usb_endpoint_xfer_control(const struct usb_endpoint_descriptor *d) {
    return usb_endpoint_type(d) == USB_ENDPOINT_XFER_CONTROL;
}

static inline bool usb_endpoint_xfer_bulk(const struct usb_endpoint_descriptor *d) {
    return usb_endpoint_type(d) == USB_ENDPOINT_XFER_BULK;
}

static inline bool usb_endpoint_xfer_isoc(const struct usb_endpoint_descriptor *d) {
    return usb_endpoint_type(d) == USB_ENDPOINT_XFER_ISOC;
}

static inline bool usb_endpoint_dir_in(const struct usb_endpoint_descriptor *d) {
    return (d->bEndpointAddress & 0x80u) != 0;
}

static inline uint8_t usb_endpoint_num(const struct usb_endpoint_descriptor *d) {
    return d->bEndpointAddress & 0x0Fu;
}

static inline uint16_t le16_to_cpu_u16(uint16_t v) {
    return v;
}

static inline unsigned int usb_endpoint_maxp(const struct usb_endpoint_descriptor *d) {
    return le16_to_cpu_u16(d->wMaxPacketSize) & 0x7ffu;
}

static inline unsigned int usb_endpoint_maxp_mult(const struct usb_endpoint_descriptor *d) {
    return ((le16_to_cpu_u16(d->wMaxPacketSize) >> 11) & 0x3u) + 1u;
}

// ---------------------------------------------------------------------------
// USB device/endpoint minimal structs
// ---------------------------------------------------------------------------
struct usb_host_endpoint {
    struct usb_endpoint_descriptor desc;
    struct usb_ss_ep_comp_descriptor ss_ep_comp;
    uint8_t iface_num;
    uint8_t iface_subclass;
    uint8_t iface_protocol;
    uint16_t report_len;
};

struct usb_device_min {
    uint8_t speed;
    uint8_t slot_id;
    uint8_t portnum;
    uint32_t route;
};

// USB speeds (match Linux values)
#define USB_SPEED_LOW        1
#define USB_SPEED_FULL       2
#define USB_SPEED_HIGH       3
#define USB_SPEED_SUPER      4
#define USB_SPEED_SUPER_PLUS 5

// ---------------------------------------------------------------------------
// xHCI structures and macros (subset from Linux xhci.h)
// ---------------------------------------------------------------------------
#define MAX_SLOTS 32
#define MAX_HID_EPS 4
#define MAX_INTR_ENDPOINTS (MAX_SLOTS * MAX_HID_EPS)
#define MAX_PORTS 256
#define TRBS_PER_SEGMENT 256
#define SEGMENT_POOL_COUNT 64

// TRB types
#define TRB_TYPE_NORMAL 1
#define TRB_TYPE_SETUP 2
#define TRB_TYPE_DATA 3
#define TRB_TYPE_STATUS 4
#define TRB_TYPE_LINK 6
#define TRB_TYPE_NOOP 8
#define TRB_TYPE_ENABLE_SLOT 9
#define TRB_TYPE_ADDRESS_DEVICE 11
#define TRB_TYPE_CONFIGURE_ENDPOINT 12
#define TRB_TYPE_STOP_ENDPOINT 15
#define TRB_TYPE_SET_TR_DEQ 16
#define TRB_TYPE_TRANSFER_EVENT 32
#define TRB_TYPE_COMMAND_COMPLETION 33

// TRB control bits
#define TRB_CYCLE (1u << 0)
#define TRB_TC (1u << 1)
#define TRB_ISP (1u << 2)
#define TRB_IOC (1u << 5)
#define TRB_IDT (1u << 6)

#define TRB_DIR_IN (1u << 16)
#define TRB_TRT_SHIFT 16
#define TRB_SLOT_ID_SHIFT 24
#define TRB_EP_ID_SHIFT 16

// Slot/EP context fields
#define LAST_CTX(p) ((p) << 27)
#define ROOT_HUB_PORT(p) (((p) & 0xff) << 16)
#define EP_MULT(p) (((p) & 0x3) << 8)
#define EP_INTERVAL(p) (((p) & 0xff) << 16)
#define EP_TYPE(p) ((p) << 3)
#define ERROR_COUNT(p) (((p) & 0x3) << 1)
#define MAX_BURST(p) (((p) & 0xff) << 8)
#define MAX_PACKET(p) (((p) & 0xffff) << 16)
#define EP_AVG_TRB_LENGTH(p) ((p) & 0xffff)
#define EP_MAX_ESIT_PAYLOAD_LO(p) (((p) & 0xffff) << 16)
#define EP_MAX_ESIT_PAYLOAD_HI(p) ((((p) >> 16) & 0xff) << 24)

#define SLOT_SPEED_SS (4u << 20)
#define SLOT_SPEED_SSP (5u << 20)
#define SLOT_SPEED_HS (3u << 20)
#define SLOT_SPEED_FS (2u << 20)
#define SLOT_SPEED_LS (1u << 20)

#define EP0_FLAG (1u << 1)
#define SLOT_FLAG (1u << 0)

#define CTRL_EP 4
#define INT_IN_EP 7

// xHCI command completion codes
#define CC_SUCCESS 1
#define CC_SHORT_PACKET 13

// Registers
#define USBCMD 0x00
#define USBSTS 0x04
#define DNCTRL 0x14
#define CRCR 0x18
#define DCBAAP 0x30
#define CONFIG 0x38

// PORTSC bits
#define PORTSC_CCS (1u << 0)
#define PORTSC_PED (1u << 1)
#define PORTSC_PR (1u << 4)
#define PORTSC_PRC (1u << 21)
#define PORTSC_SPEED_SHIFT 10
#define PORTSC_SPEED_MASK (0xFu << PORTSC_SPEED_SHIFT)

// Interrupter registers
#define IMAN 0x00
#define IMOD 0x04
#define ERSTSZ 0x08
#define ERSTBA 0x10
#define ERDP 0x18

struct xhci_trb {
    uint32_t field[4];
};

struct xhci_segment {
    struct xhci_trb *trbs;
    uint64_t dma;
    struct xhci_segment *next;
};

enum xhci_ring_type {
    TYPE_COMMAND = 0,
    TYPE_EVENT = 1,
    TYPE_CTRL = 2,
    TYPE_INTR = 3,
};

struct xhci_ring {
    struct xhci_segment *first_seg;
    struct xhci_segment *last_seg;
    struct xhci_segment *enq_seg;
    struct xhci_trb *enqueue;
    uint32_t cycle_state;
    unsigned int num_segs;
    enum xhci_ring_type type;
};

struct xhci_slot_ctx {
    uint32_t dev_info;
    uint32_t dev_info2;
    uint32_t tt_info;
    uint32_t dev_state;
    uint32_t reserved[4];
};

struct xhci_ep_ctx {
    uint32_t ep_info;
    uint32_t ep_info2;
    uint64_t deq;
    uint32_t tx_info;
    uint32_t reserved[3];
};

struct xhci_input_control_ctx {
    uint32_t drop_flags;
    uint32_t add_flags;
    uint32_t rsvd2[6];
};

struct xhci_erst_entry {
    uint64_t seg_addr;
    uint32_t seg_size;
    uint32_t rsvd;
};

struct xhci_virt_device {
    uint8_t slot_id;
    uint8_t ctx_size;
    uint8_t *in_ctx;
    uint8_t *reconfig_in_ctx;
    uint8_t *out_ctx;
    struct xhci_ring *ep_rings[32];
};

struct xhci_hcd {
    uint64_t base;
    uint64_t op_base;
    uint64_t rt_base;
    uint64_t db_base;
    uint16_t hci_version;
    uint8_t max_slots;
    uint8_t max_ports;
    uint8_t ctx_size;
};

struct intr_ep_queue {
    uint8_t slot_id;
    uint8_t dci;
    struct xhci_ring *ep_ring;
    uint32_t max_packet;
};

static struct xhci_hcd g_xhci;
static struct xhci_erst_entry g_erst[1] __attribute__((aligned(64)));
static uint64_t g_dcbaa[256] __attribute__((aligned(64)));
static uint8_t g_input_ctx[MAX_SLOTS][4096] __attribute__((aligned(4096)));
static uint8_t g_reconfig_input_ctx[MAX_SLOTS][4096] __attribute__((aligned(4096)));
static uint8_t g_output_ctx[MAX_SLOTS][4096] __attribute__((aligned(4096)));
static struct xhci_virt_device g_virt_devs[MAX_SLOTS];

static struct xhci_ring g_cmd_ring;
static struct xhci_ring g_event_ring;
static struct xhci_segment g_segments[SEGMENT_POOL_COUNT];
static struct xhci_trb g_segment_trbs[SEGMENT_POOL_COUNT][TRBS_PER_SEGMENT] __attribute__((aligned(4096)));
static size_t g_segment_alloc_idx = 0;

// Event ring dequeue state
static struct xhci_segment *g_event_deq_seg;
static struct xhci_trb *g_event_dequeue;
static uint32_t g_event_cycle = 1;

// Control transfer buffer
static uint8_t g_ctrl_data_buf[256] __attribute__((aligned(64)));
static uint8_t g_intr_bufs[MAX_INTR_ENDPOINTS][1024] __attribute__((aligned(64)));

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------
static struct xhci_input_control_ctx *get_input_control_ctx(uint8_t *ctx) {
    return (struct xhci_input_control_ctx *)ctx;
}

static struct xhci_slot_ctx *get_slot_ctx_in(uint8_t *ctx, size_t ctx_size) {
    return (struct xhci_slot_ctx *)(ctx + ctx_size);
}

static struct xhci_slot_ctx *get_slot_ctx_out(uint8_t *ctx) {
    return (struct xhci_slot_ctx *)ctx;
}

static struct xhci_ep_ctx *get_ep_ctx_in(uint8_t *ctx, size_t ctx_size, uint8_t dci) {
    return (struct xhci_ep_ctx *)(ctx + (1u + (size_t)dci) * ctx_size);
}

static struct xhci_ep_ctx *get_ep_ctx_out(uint8_t *ctx, size_t ctx_size, uint8_t dci) {
    return (struct xhci_ep_ctx *)(ctx + (size_t)dci * ctx_size);
}

static uint32_t clamp_val_u32(uint32_t v, uint32_t lo, uint32_t hi) {
    if (v < lo) return lo;
    if (v > hi) return hi;
    return v;
}

static unsigned int fls_u32(uint32_t v) {
    unsigned int r = 0;
    while (v) {
        v >>= 1;
        r++;
    }
    return r;
}

static unsigned int xhci_parse_exponent_interval(const struct usb_host_endpoint *ep) {
    unsigned int bi = ep->desc.bInterval;
    if (bi < 1) bi = 1;
    if (bi > 16) bi = 16;
    return bi - 1;
}

static unsigned int xhci_microframes_to_exponent(unsigned int desc_interval,
                                                 unsigned int min_exp,
                                                 unsigned int max_exp) {
    unsigned int interval = fls_u32(desc_interval) - 1;
    interval = clamp_val_u32(interval, min_exp, max_exp);
    return interval;
}

static unsigned int xhci_parse_microframe_interval(const struct usb_host_endpoint *ep) {
    if (ep->desc.bInterval == 0) return 0;
    return xhci_microframes_to_exponent(ep->desc.bInterval, 0, 15);
}

static unsigned int xhci_parse_frame_interval(const struct usb_host_endpoint *ep) {
    return xhci_microframes_to_exponent((unsigned int)ep->desc.bInterval * 8u, 3, 10);
}

static unsigned int xhci_get_endpoint_interval(const struct usb_device_min *udev,
                                               const struct usb_host_endpoint *ep) {
    unsigned int interval = 0;
    switch (udev->speed) {
    case USB_SPEED_HIGH:
        if (usb_endpoint_xfer_control(&ep->desc) ||
            usb_endpoint_xfer_bulk(&ep->desc)) {
            interval = xhci_parse_microframe_interval(ep);
            break;
        }
        // fallthrough: HS isoc/int use exponent interval
    case USB_SPEED_SUPER_PLUS:
    case USB_SPEED_SUPER:
        if (usb_endpoint_xfer_int(&ep->desc) ||
            usb_endpoint_xfer_isoc(&ep->desc)) {
            interval = xhci_parse_exponent_interval(ep);
        }
        break;
    case USB_SPEED_FULL:
        if (usb_endpoint_xfer_isoc(&ep->desc)) {
            interval = xhci_parse_exponent_interval(ep);
            break;
        }
        // fallthrough: FS interrupt uses frame interval like LS
    case USB_SPEED_LOW:
        if (usb_endpoint_xfer_int(&ep->desc) ||
            usb_endpoint_xfer_isoc(&ep->desc)) {
            interval = xhci_parse_frame_interval(ep);
        }
        break;
    default:
        break;
    }
    return interval;
}

static unsigned int usb_endpoint_max_periodic_payload(const struct usb_device_min *udev,
                                                      const struct usb_host_endpoint *ep) {
    if (usb_endpoint_xfer_control(&ep->desc) ||
        usb_endpoint_xfer_bulk(&ep->desc)) {
        return 0;
    }
    if (udev->speed >= USB_SPEED_SUPER) {
        unsigned int bytes = le16_to_cpu_u16(ep->ss_ep_comp.wBytesPerInterval);
        if (bytes == 0) {
            unsigned int max_packet = usb_endpoint_maxp(&ep->desc);
            unsigned int max_burst = ep->ss_ep_comp.bMaxBurst;
            unsigned int mult = (ep->ss_ep_comp.bmAttributes & 0x3u) + 1u;
            bytes = max_packet * (max_burst + 1u) * mult;
        }
        return bytes;
    }
    return usb_endpoint_maxp(&ep->desc) * usb_endpoint_maxp_mult(&ep->desc);
}

static unsigned int xhci_get_endpoint_mult(const struct usb_device_min *udev,
                                           const struct usb_host_endpoint *ep) {
    (void)udev;
    (void)ep;
    return 0;
}

static unsigned int xhci_get_endpoint_max_burst(const struct usb_device_min *udev,
                                                const struct usb_host_endpoint *ep) {
    if (udev->speed >= USB_SPEED_SUPER) {
        return ep->ss_ep_comp.bMaxBurst;
    }
    if (udev->speed == USB_SPEED_HIGH && usb_endpoint_xfer_int(&ep->desc)) {
        return usb_endpoint_maxp_mult(&ep->desc) - 1u;
    }
    return 0;
}

static unsigned int xhci_get_endpoint_type(const struct usb_host_endpoint *ep) {
    int in = usb_endpoint_dir_in(&ep->desc);
    if (usb_endpoint_type(&ep->desc) == USB_ENDPOINT_XFER_INT) {
        return in ? INT_IN_EP : 3;
    }
    return 0;
}

// ---------------------------------------------------------------------------
// Ring allocation and operations
// ---------------------------------------------------------------------------
static struct xhci_segment *xhci_segment_alloc(void) {
    if (g_segment_alloc_idx >= SEGMENT_POOL_COUNT) {
        return NULL;
    }
    struct xhci_segment *seg = &g_segments[g_segment_alloc_idx];
    struct xhci_trb *trbs = g_segment_trbs[g_segment_alloc_idx];
    g_segment_alloc_idx++;

    bmemset(trbs, 0, sizeof(g_segment_trbs[0]));
    seg->trbs = trbs;
    seg->dma = breenix_virt_to_phys_c((uint64_t)(uintptr_t)trbs);
    seg->next = NULL;
    breenix_dma_cache_clean_c((const uint8_t *)trbs, sizeof(g_segment_trbs[0]));
    return seg;
}

static void xhci_link_segment(struct xhci_segment *seg, struct xhci_segment *next, bool toggle_cycle) {
    struct xhci_trb *link = &seg->trbs[TRBS_PER_SEGMENT - 1];
    bmemset(link, 0, sizeof(*link));
    link->field[0] = (uint32_t)(next->dma & 0xFFFFFFFFu);
    link->field[1] = (uint32_t)((next->dma >> 32) & 0xFFFFFFFFu);
    link->field[3] = (TRB_TYPE_LINK << 10) | TRB_CYCLE | (toggle_cycle ? TRB_TC : 0);
    breenix_dma_cache_clean_c((const uint8_t *)link, sizeof(*link));
}

static int xhci_ring_init(struct xhci_ring *ring, unsigned int num_segs, enum xhci_ring_type type) {
    ring->num_segs = num_segs;
    ring->type = type;
    ring->cycle_state = 1;
    ring->first_seg = NULL;
    ring->last_seg = NULL;
    ring->enq_seg = NULL;
    ring->enqueue = NULL;

    if (num_segs == 0) {
        return 0;
    }

    struct xhci_segment *first = NULL;
    struct xhci_segment *prev = NULL;
    for (unsigned int i = 0; i < num_segs; i++) {
        struct xhci_segment *seg = xhci_segment_alloc();
        if (!seg) {
            return -1;
        }
        if (!first) {
            first = seg;
        }
        if (prev) {
            prev->next = seg;
        }
        prev = seg;
    }
    prev->next = first;

    ring->first_seg = first;
    ring->last_seg = prev;
    ring->enq_seg = first;
    ring->enqueue = first->trbs;

    // Link TRBs
    struct xhci_segment *cur = first;
    for (unsigned int i = 0; i < num_segs; i++) {
        struct xhci_segment *next = cur->next;
        bool toggle = (cur == ring->last_seg);
        xhci_link_segment(cur, next, toggle);
        cur = next;
    }

    return 0;
}

static void xhci_ring_enqueue_trb(struct xhci_ring *ring, const struct xhci_trb *src) {
    struct xhci_trb *trb = ring->enqueue;
    bmemcpy(trb, src, sizeof(*trb));
    if (ring->cycle_state) {
        trb->field[3] |= TRB_CYCLE;
    } else {
        trb->field[3] &= ~TRB_CYCLE;
    }
    breenix_dma_cache_clean_c((const uint8_t *)trb, sizeof(*trb));

    // Advance enqueue pointer
    if (trb == &ring->enq_seg->trbs[TRBS_PER_SEGMENT - 2]) {
        // next is link TRB slot, move to next segment start
        ring->enq_seg = ring->enq_seg->next;
        ring->enqueue = ring->enq_seg->trbs;
        ring->cycle_state ^= 1u;
    } else {
        ring->enqueue = trb + 1;
    }
}

// ---------------------------------------------------------------------------
// Event ring handling
// ---------------------------------------------------------------------------
static int xhci_wait_for_event(struct xhci_hcd *xhci, struct xhci_trb *out, uint32_t expected_type) {
    unsigned int timeout = 2000000;
    while (timeout--) {
        breenix_dma_cache_invalidate_c((const uint8_t *)g_event_dequeue, sizeof(*g_event_dequeue));
        struct xhci_trb trb = *g_event_dequeue;
        uint32_t cycle = trb.field[3] & TRB_CYCLE;
        if ((cycle ? 1u : 0u) == g_event_cycle) {
            // advance dequeue
            if (g_event_dequeue == &g_event_deq_seg->trbs[TRBS_PER_SEGMENT - 1]) {
                g_event_deq_seg = g_event_deq_seg->next;
                g_event_dequeue = g_event_deq_seg->trbs;
                g_event_cycle ^= 1u;
            } else {
                g_event_dequeue++;
            }
            // update ERDP (clear EHB via W1C on bit 3)
            uint64_t erdp = breenix_virt_to_phys_c((uint64_t)(uintptr_t)g_event_dequeue);
            write64(xhci->rt_base + 0x20 + ERDP, erdp | (1ull << 3));
            // Clear IMAN.IP (W1C bit 0, preserve IE bit 1) — matches Linux irq handler
            uint32_t iman = read32(xhci->rt_base + 0x20 + IMAN);
            write32(xhci->rt_base + 0x20 + IMAN, iman | 0x1u);
            // Clear USBSTS.EINT (W1C bit 3) — matches Linux irq handler
            uint32_t sts = read32(xhci->op_base + USBSTS);
            write32(xhci->op_base + USBSTS, sts | (1u << 3));

            uint32_t trb_type = (trb.field[3] >> 10) & 0x3f;
            if (expected_type == 0 || trb_type == expected_type) {
                *out = trb;
                return 0;
            } else {
                trace_trb(TRACE_XFER_EVENT,
                          (uint8_t)((trb.field[3] >> TRB_SLOT_ID_SHIFT) & 0xff),
                          (uint8_t)((trb.field[3] >> TRB_EP_ID_SHIFT) & 0x1f),
                          &trb);
            }
        }
    }
    return -1;
}

static inline uint64_t read_cntvct(void) {
    uint64_t val;
    __asm__ volatile("mrs %0, cntvct_el0" : "=r"(val));
    return val;
}

static inline uint64_t read_cntfrq(void) {
    uint64_t val;
    __asm__ volatile("mrs %0, cntfrq_el0" : "=r"(val));
    return val;
}

static void wait_ms(uint64_t timeout_ms) {
    uint64_t start = read_cntvct();
    uint64_t freq = read_cntfrq();
    uint64_t ticks_per_ms = freq / 1000u;
    uint64_t deadline = start + (ticks_per_ms * timeout_ms);

    while ((int64_t)(read_cntvct() - deadline) < 0) {
    }
}

static int xhci_wait_for_event_ms(struct xhci_hcd *xhci, struct xhci_trb *out,
                                  uint32_t expected_type, uint64_t timeout_ms) {
    uint64_t start = read_cntvct();
    uint64_t freq = read_cntfrq();
    uint64_t ticks_per_ms = freq / 1000u;
    uint64_t deadline = start + (ticks_per_ms * timeout_ms);

    while ((int64_t)(read_cntvct() - deadline) < 0) {
        breenix_dma_cache_invalidate_c((const uint8_t *)g_event_dequeue, sizeof(*g_event_dequeue));
        struct xhci_trb trb = *g_event_dequeue;
        uint32_t cycle = trb.field[3] & TRB_CYCLE;
        if ((cycle ? 1u : 0u) == g_event_cycle) {
            if (g_event_dequeue == &g_event_deq_seg->trbs[TRBS_PER_SEGMENT - 1]) {
                g_event_deq_seg = g_event_deq_seg->next;
                g_event_dequeue = g_event_deq_seg->trbs;
                g_event_cycle ^= 1u;
            } else {
                g_event_dequeue++;
            }
            uint64_t erdp = breenix_virt_to_phys_c((uint64_t)(uintptr_t)g_event_dequeue);
            write64(xhci->rt_base + 0x20 + ERDP, erdp | (1ull << 3));
            // Clear IMAN.IP (W1C bit 0, preserve IE bit 1)
            uint32_t iman = read32(xhci->rt_base + 0x20 + IMAN);
            write32(xhci->rt_base + 0x20 + IMAN, iman | 0x1u);
            // Clear USBSTS.EINT (W1C bit 3)
            uint32_t sts = read32(xhci->op_base + USBSTS);
            write32(xhci->op_base + USBSTS, sts | (1u << 3));

            uint32_t trb_type = (trb.field[3] >> 10) & 0x3f;
            if (expected_type == 0 || trb_type == expected_type) {
                *out = trb;
                return 0;
            }
        }
    }
    return -1;
}

// ---------------------------------------------------------------------------
// Doorbell
// ---------------------------------------------------------------------------
static void ring_doorbell(struct xhci_hcd *xhci, uint8_t slot, uint8_t target) {
    uint64_t addr = xhci->db_base + ((uint64_t)slot) * 4u;
    write32(addr, target);
    uint8_t buf[12];
    bmemcpy(buf, &xhci->db_base, 8);
    buf[8] = slot;
    buf[9] = target;
    breenix_xhci_trace_raw_c(TRACE_DOORBELL, slot, target, buf, 12);
}

// ---------------------------------------------------------------------------
// Command submission helpers
// ---------------------------------------------------------------------------
static int xhci_cmd(struct xhci_hcd *xhci, struct xhci_trb *cmd_trb, struct xhci_trb *ev_out) {
    trace_trb(TRACE_CMD_SUBMIT, (cmd_trb->field[3] >> TRB_SLOT_ID_SHIFT) & 0xff, 0, cmd_trb);
    xhci_ring_enqueue_trb(&g_cmd_ring, cmd_trb);
    ring_doorbell(xhci, 0, 0);
    if (xhci_wait_for_event(xhci, ev_out, TRB_TYPE_COMMAND_COMPLETION) != 0) {
        return -1;
    }
    trace_trb(TRACE_CMD_COMPLETE, (ev_out->field[3] >> TRB_SLOT_ID_SHIFT) & 0xff, 0, ev_out);
    return 0;
}

// ---------------------------------------------------------------------------
// Control transfers (EP0)
// ---------------------------------------------------------------------------
struct usb_setup_packet {
    uint8_t bmRequestType;
    uint8_t bRequest;
    uint16_t wValue;
    uint16_t wIndex;
    uint16_t wLength;
} __attribute__((packed));

static int control_transfer(struct xhci_hcd *xhci, uint8_t slot_id, struct xhci_ring *ring,
                            const struct usb_setup_packet *setup,
                            uint64_t data_phys, uint16_t data_len, bool dir_in) {
    struct xhci_trb trb;
    // Setup Stage (IDT)
    bmemset(&trb, 0, sizeof(trb));
    uint64_t setup_data = 0;
    bmemcpy(&setup_data, setup, sizeof(*setup));
    trb.field[0] = (uint32_t)(setup_data & 0xFFFFFFFFu);
    trb.field[1] = (uint32_t)((setup_data >> 32) & 0xFFFFFFFFu);
    trb.field[2] = 8;
    uint32_t trt = 0;
    if (data_len == 0) {
        trt = 0;
    } else if (dir_in) {
        trt = 3;
    } else {
        trt = 2;
    }
    trb.field[3] = (TRB_TYPE_SETUP << 10) | TRB_IDT | (trt << TRB_TRT_SHIFT);
    xhci_ring_enqueue_trb(ring, &trb);

    if (data_len > 0) {
        bmemset(&trb, 0, sizeof(trb));
        trb.field[0] = (uint32_t)(data_phys & 0xFFFFFFFFu);
        trb.field[1] = (uint32_t)((data_phys >> 32) & 0xFFFFFFFFu);
        trb.field[2] = data_len;
        trb.field[3] = (TRB_TYPE_DATA << 10) | (dir_in ? TRB_DIR_IN : 0);
        xhci_ring_enqueue_trb(ring, &trb);
    }

    bmemset(&trb, 0, sizeof(trb));
    trb.field[3] = (TRB_TYPE_STATUS << 10) | TRB_IOC | (dir_in ? 0 : TRB_DIR_IN);
    xhci_ring_enqueue_trb(ring, &trb);

    ring_doorbell(xhci, slot_id, 1);

    struct xhci_trb ev;
    if (xhci_wait_for_event(xhci, &ev, TRB_TYPE_TRANSFER_EVENT) != 0) {
        return -1;
    }
    trace_trb(TRACE_XFER_EVENT, (ev.field[3] >> TRB_SLOT_ID_SHIFT) & 0xff,
              (ev.field[3] >> TRB_EP_ID_SHIFT) & 0x1f, &ev);
    return 0;
}

// ---------------------------------------------------------------------------
// Endpoint setup (Linux-style)
// ---------------------------------------------------------------------------
static int xhci_endpoint_init(struct xhci_virt_device *virt_dev,
                              const struct usb_device_min *udev,
                              const struct usb_host_endpoint *ep) {
    uint8_t ep_num = usb_endpoint_num(&ep->desc);
    uint8_t dci = (uint8_t)(ep_num * 2u + (usb_endpoint_dir_in(&ep->desc) ? 1u : 0u));
    struct xhci_ep_ctx *ep_ctx = get_ep_ctx_in(virt_dev->in_ctx, virt_dev->ctx_size, dci);

    unsigned int endpoint_type = xhci_get_endpoint_type(ep);
    if (!endpoint_type) {
        return -1;
    }

    unsigned int max_esit_payload = usb_endpoint_max_periodic_payload(udev, ep);
    unsigned int interval = xhci_get_endpoint_interval(udev, ep);
    unsigned int mult = xhci_get_endpoint_mult(udev, ep);
    unsigned int max_packet = usb_endpoint_maxp(&ep->desc);
    unsigned int max_burst = xhci_get_endpoint_max_burst(udev, ep);
    unsigned int avg_trb_len = max_esit_payload;
    unsigned int err_count = 3;

    // allocate ring for this endpoint
    struct xhci_ring *ring = (struct xhci_ring *)0;
    static struct xhci_ring ring_pool[16];
    static unsigned int ring_pool_idx = 0;
    if (ring_pool_idx >= 16) {
        return -1;
    }
    ring = &ring_pool[ring_pool_idx++];
    if (xhci_ring_init(ring, 2, TYPE_INTR) != 0) {
        return -1;
    }
    virt_dev->ep_rings[dci] = ring;

    ep_ctx->ep_info = EP_MAX_ESIT_PAYLOAD_HI(max_esit_payload) |
                      EP_INTERVAL(interval) |
                      EP_MULT(mult);
    ep_ctx->ep_info2 = EP_TYPE(endpoint_type) |
                       MAX_PACKET(max_packet) |
                       MAX_BURST(max_burst) |
                       ERROR_COUNT(err_count);
    ep_ctx->deq = ring->first_seg->dma | ring->cycle_state;
    ep_ctx->tx_info = EP_MAX_ESIT_PAYLOAD_LO(max_esit_payload) |
                      EP_AVG_TRB_LENGTH(avg_trb_len);

    return 0;
}

// ---------------------------------------------------------------------------
// xHCI init and enumeration
// ---------------------------------------------------------------------------
static int xhci_setup_rings(struct xhci_hcd *xhci) {
    // DCBAA
    bmemset(g_dcbaa, 0, sizeof(g_dcbaa));
    breenix_dma_cache_clean_c((const uint8_t *)g_dcbaa, sizeof(g_dcbaa));
    write64(xhci->op_base + DCBAAP, breenix_virt_to_phys_c((uint64_t)(uintptr_t)g_dcbaa));

    // Command ring
    if (xhci_ring_init(&g_cmd_ring, 1, TYPE_COMMAND) != 0) {
        return -1;
    }
    uint64_t crcr = g_cmd_ring.first_seg->dma | 1u;
    write64(xhci->op_base + CRCR, crcr);

    // Event ring
    if (xhci_ring_init(&g_event_ring, 1, TYPE_EVENT) != 0) {
        return -1;
    }
    g_event_deq_seg = g_event_ring.first_seg;
    g_event_dequeue = g_event_ring.first_seg->trbs;
    g_event_cycle = 1;

    g_erst[0].seg_addr = g_event_ring.first_seg->dma;
    g_erst[0].seg_size = TRBS_PER_SEGMENT;
    g_erst[0].rsvd = 0;
    breenix_dma_cache_clean_c((const uint8_t *)g_erst, sizeof(g_erst));

    uint64_t ir0 = xhci->rt_base + 0x20;
    write32(ir0 + IMOD, 0x000000a0);
    write32(ir0 + ERSTSZ, 1);
    // xHCI spec §4.9.4: ERDP must be set BEFORE ERSTBA (ERSTBA triggers HW read)
    write64(ir0 + ERDP, g_event_ring.first_seg->dma);
    write64(ir0 + ERSTBA, breenix_virt_to_phys_c((uint64_t)(uintptr_t)g_erst));

    return 0;
}

static uint8_t xhci_enable_slot(struct xhci_hcd *xhci) {
    struct xhci_trb trb;
    bmemset(&trb, 0, sizeof(trb));
    trb.field[3] = (TRB_TYPE_ENABLE_SLOT << 10);
    struct xhci_trb ev;
    if (xhci_cmd(xhci, &trb, &ev) != 0) {
        return 0;
    }
    uint8_t slot = (uint8_t)((ev.field[3] >> TRB_SLOT_ID_SHIFT) & 0xff);
    return slot;
}

static int xhci_address_device(struct xhci_hcd *xhci, struct xhci_virt_device *vdev,
                               const struct usb_device_min *udev) {
    uint8_t *in_ctx = vdev->in_ctx;
    bmemset(in_ctx, 0, 4096);

    struct xhci_input_control_ctx *ctrl = get_input_control_ctx(in_ctx);
    ctrl->add_flags = SLOT_FLAG | EP0_FLAG;
    ctrl->drop_flags = 0;

    struct xhci_slot_ctx *slot_ctx = get_slot_ctx_in(in_ctx, vdev->ctx_size);
    uint32_t speed_bits = 0;
    switch (udev->speed) {
    case USB_SPEED_SUPER_PLUS: speed_bits = SLOT_SPEED_SSP; break;
    case USB_SPEED_SUPER: speed_bits = SLOT_SPEED_SS; break;
    case USB_SPEED_HIGH: speed_bits = SLOT_SPEED_HS; break;
    case USB_SPEED_FULL: speed_bits = SLOT_SPEED_FS; break;
    case USB_SPEED_LOW: speed_bits = SLOT_SPEED_LS; break;
    default: speed_bits = SLOT_SPEED_SS; break;
    }
    slot_ctx->dev_info = speed_bits | LAST_CTX(1) | (udev->route & 0xfffff);
    slot_ctx->dev_info2 = ROOT_HUB_PORT(udev->portnum);

    struct xhci_ep_ctx *ep0 = get_ep_ctx_in(in_ctx, vdev->ctx_size, 1);
    ep0->ep_info = 0;
    ep0->ep_info2 = EP_TYPE(CTRL_EP) | MAX_PACKET(512) | MAX_BURST(0) | ERROR_COUNT(3);
    ep0->deq = vdev->ep_rings[1]->first_seg->dma | vdev->ep_rings[1]->cycle_state;
    ep0->tx_info = EP_AVG_TRB_LENGTH(8);

    breenix_dma_cache_clean_c(in_ctx, 4096);
    trace_input_ctx(udev->slot_id, in_ctx, vdev->ctx_size, 1);

    struct xhci_trb trb;
    bmemset(&trb, 0, sizeof(trb));
    uint64_t in_phys = breenix_virt_to_phys_c((uint64_t)(uintptr_t)in_ctx);
    trb.field[0] = (uint32_t)(in_phys & 0xFFFFFFFFu);
    trb.field[1] = (uint32_t)((in_phys >> 32) & 0xFFFFFFFFu);
    trb.field[3] = (TRB_TYPE_ADDRESS_DEVICE << 10) | ((uint32_t)udev->slot_id << TRB_SLOT_ID_SHIFT);
    struct xhci_trb ev;
    if (xhci_cmd(xhci, &trb, &ev) != 0) {
        return -1;
    }
    return 0;
}

static int xhci_configure_endpoints(struct xhci_hcd *xhci, struct xhci_virt_device *vdev,
                                    const struct usb_device_min *udev,
                                    const struct usb_host_endpoint *eps, unsigned int ep_count) {
    uint8_t *in_ctx = vdev->in_ctx;
    bmemset(in_ctx, 0, 4096);

    struct xhci_input_control_ctx *ctrl = get_input_control_ctx(in_ctx);
    ctrl->drop_flags = 0;
    ctrl->add_flags = SLOT_FLAG;

    uint8_t max_dci = 1;
    for (unsigned int i = 0; i < ep_count; i++) {
        uint8_t dci = (uint8_t)(usb_endpoint_num(&eps[i].desc) * 2u + (usb_endpoint_dir_in(&eps[i].desc) ? 1u : 0u));
        ctrl->add_flags |= (1u << dci);
        if (dci > max_dci) {
            max_dci = dci;
        }
    }
    uint32_t add_flags = ctrl->add_flags;

    // Copy slot context from output context, zero dev_state
    breenix_dma_cache_invalidate_c(vdev->out_ctx, 4096);
    struct xhci_slot_ctx *slot_ctx = get_slot_ctx_in(in_ctx, vdev->ctx_size);
    struct xhci_slot_ctx *out_slot = get_slot_ctx_out(vdev->out_ctx);
    bmemcpy(slot_ctx, out_slot, sizeof(*slot_ctx));
    slot_ctx->dev_state = 0;
    slot_ctx->dev_info &= ~(0x1f << 27);
    slot_ctx->dev_info |= LAST_CTX(max_dci);

    for (unsigned int i = 0; i < ep_count; i++) {
        if (xhci_endpoint_init(vdev, udev, &eps[i]) != 0) {
            return -1;
        }
    }

    breenix_dma_cache_clean_c(in_ctx, 4096);
    trace_input_ctx(udev->slot_id, in_ctx, vdev->ctx_size, max_dci);

    uint64_t in_phys = breenix_virt_to_phys_c((uint64_t)(uintptr_t)in_ctx);
    struct xhci_trb trb;
    bmemset(&trb, 0, sizeof(trb));
    trb.field[0] = (uint32_t)(in_phys & 0xFFFFFFFFu);
    trb.field[1] = (uint32_t)((in_phys >> 32) & 0xFFFFFFFFu);
    trb.field[3] = (TRB_TYPE_CONFIGURE_ENDPOINT << 10) | ((uint32_t)udev->slot_id << TRB_SLOT_ID_SHIFT);

    struct xhci_trb ev;
    if (xhci_cmd(xhci, &trb, &ev) != 0) {
        return -1;
    }

    breenix_dma_cache_invalidate_c(vdev->out_ctx, 4096);
    trace_output_ctx(udev->slot_id, vdev->out_ctx, vdev->ctx_size, max_dci);

    // Linux-style bandwidth dance: Stop Endpoint + re-ConfigureEndpoint per ep
    const bool run_bw_dance = true;
    if (run_bw_dance) {
        for (unsigned int i = 0; i < ep_count; i++) {
            uint8_t *reconfig_ctx = vdev->reconfig_in_ctx;
            uint8_t dci = (uint8_t)(usb_endpoint_num(&eps[i].desc) * 2u +
                                    (usb_endpoint_dir_in(&eps[i].desc) ? 1u : 0u));
            struct xhci_trb stop_trb;
            bmemset(&stop_trb, 0, sizeof(stop_trb));
            stop_trb.field[3] = (TRB_TYPE_STOP_ENDPOINT << 10) |
                                ((uint32_t)udev->slot_id << TRB_SLOT_ID_SHIFT) |
                                ((uint32_t)dci << TRB_EP_ID_SHIFT);
            struct xhci_trb stop_ev;
            if (xhci_cmd(xhci, &stop_trb, &stop_ev) != 0) {
                return -1;
            }

            // Rebuild input ctx from output ctx, then re-configure
            bmemset(reconfig_ctx, 0, 4096);
            struct xhci_input_control_ctx *rctrl = get_input_control_ctx(reconfig_ctx);
            rctrl->drop_flags = 0;
            rctrl->add_flags = add_flags;

            breenix_dma_cache_invalidate_c(vdev->out_ctx, 4096);
            struct xhci_slot_ctx *rc_slot = get_slot_ctx_in(reconfig_ctx, vdev->ctx_size);
            struct xhci_slot_ctx *out_slot = get_slot_ctx_out(vdev->out_ctx);
            bmemcpy(rc_slot, out_slot, sizeof(*rc_slot));
            rc_slot->dev_state = 0;
            rc_slot->dev_info &= ~(0x1f << 27);
            rc_slot->dev_info |= LAST_CTX(max_dci);

            for (unsigned int j = 0; j < ep_count; j++) {
                uint8_t ep_dci = (uint8_t)(usb_endpoint_num(&eps[j].desc) * 2u +
                                           (usb_endpoint_dir_in(&eps[j].desc) ? 1u : 0u));
                struct xhci_ep_ctx *rc_ep = get_ep_ctx_in(reconfig_ctx, vdev->ctx_size, ep_dci);
                struct xhci_ep_ctx *out_ep = get_ep_ctx_out(vdev->out_ctx, vdev->ctx_size, ep_dci);
                bmemcpy(rc_ep, out_ep, sizeof(*rc_ep));
                rc_ep->ep_info &= ~0x7u; // clear state bits
            }

            breenix_dma_cache_clean_c(reconfig_ctx, 4096);
            trace_input_ctx(udev->slot_id, reconfig_ctx, vdev->ctx_size, max_dci);

            uint64_t rc_phys = breenix_virt_to_phys_c((uint64_t)(uintptr_t)reconfig_ctx);
            struct xhci_trb rc_trb;
            bmemset(&rc_trb, 0, sizeof(rc_trb));
            rc_trb.field[0] = (uint32_t)(rc_phys & 0xFFFFFFFFu);
            rc_trb.field[1] = (uint32_t)((rc_phys >> 32) & 0xFFFFFFFFu);
            rc_trb.field[3] = (TRB_TYPE_CONFIGURE_ENDPOINT << 10) |
                              ((uint32_t)udev->slot_id << TRB_SLOT_ID_SHIFT);
            struct xhci_trb rc_ev;
            if (xhci_cmd(xhci, &rc_trb, &rc_ev) != 0) {
                return -1;
            }
            breenix_dma_cache_invalidate_c(vdev->out_ctx, 4096);
            trace_output_ctx(udev->slot_id, vdev->out_ctx, vdev->ctx_size, max_dci);
            breenix_dma_cache_invalidate_c(vdev->out_ctx, 4096);
            breenix_xhci_trace_raw_c(TRACE_OUTPUT_CTX, vdev->slot_id, max_dci, vdev->out_ctx,
                                     (size_t)(1u + max_dci) * xhci->ctx_size);
        }
    }

    return 0;
}

static int xhci_init_controller(struct xhci_hcd *xhci) {
    // Stop controller if running
    uint32_t usbcmd = read32(xhci->op_base + USBCMD);
    if (usbcmd & 1u) {
        write32(xhci->op_base + USBCMD, usbcmd & ~1u);
        // wait for HCH (USBSTS bit0)
        for (unsigned int i = 0; i < 100000; i++) {
            if (read32(xhci->op_base + USBSTS) & 1u) break;
        }
    }
    // Reset
    write32(xhci->op_base + USBCMD, read32(xhci->op_base + USBCMD) | 2u);
    for (unsigned int i = 0; i < 100000; i++) {
        if ((read32(xhci->op_base + USBCMD) & 2u) == 0) break;
    }
    // Wait for Controller Not Ready to clear (xHCI spec §4.2, Linux xhci_reset)
    for (unsigned int i = 0; i < 100000; i++) {
        if ((read32(xhci->op_base + USBSTS) & (1u << 11)) == 0) break;
    }

    // MaxSlotsEn
    write32(xhci->op_base + CONFIG, xhci->max_slots);
    write32(xhci->op_base + DNCTRL, 0x02);

    if (xhci_setup_rings(xhci) != 0) {
        return -1;
    }

    // Enable interrupter 0
    uint64_t ir0 = xhci->rt_base + 0x20;
    uint32_t iman = read32(ir0 + IMAN);
    write32(ir0 + IMAN, iman | 2u);

    // Run
    usbcmd = read32(xhci->op_base + USBCMD);
    write32(xhci->op_base + USBCMD, usbcmd | 1u | (1u << 2));

    return 0;
}

// Parse config descriptor and collect interrupt IN endpoints.
static unsigned int parse_hid_endpoints(const uint8_t *buf, unsigned int len,
                                        struct usb_host_endpoint *out_eps, unsigned int max_eps) {
    unsigned int offset = 0;
    unsigned int count = 0;
    uint8_t current_iface = 0;
    uint8_t current_subclass = 0;
    uint8_t current_protocol = 0;
    uint16_t current_report_len = 0;
    bool in_hid = false;
    while (offset + 2 <= len) {
        uint8_t dlen = buf[offset];
        uint8_t dtype = buf[offset + 1];
        if (dlen == 0) break;
        if (offset + dlen > len) break;

        if (dtype == USB_DT_INTERFACE && dlen >= sizeof(struct usb_interface_descriptor)) {
            const struct usb_interface_descriptor *iface = (const struct usb_interface_descriptor *)(buf + offset);
            in_hid = iface->bInterfaceClass == USB_CLASS_HID;
            current_iface = iface->bInterfaceNumber;
            current_subclass = iface->bInterfaceSubClass;
            current_protocol = iface->bInterfaceProtocol;
            current_report_len = 0;
        } else if (in_hid && dtype == USB_DT_HID && dlen >= sizeof(struct usb_hid_descriptor)) {
            const struct usb_hid_descriptor *hid = (const struct usb_hid_descriptor *)(buf + offset);
            current_report_len = le16_to_cpu_u16(hid->wDescriptorLength);
        } else if (in_hid && dtype == USB_DT_ENDPOINT && dlen >= sizeof(struct usb_endpoint_descriptor)) {
            const struct usb_endpoint_descriptor *epd = (const struct usb_endpoint_descriptor *)(buf + offset);
            if (usb_endpoint_xfer_int(epd) && usb_endpoint_dir_in(epd)) {
                if (count < max_eps) {
                    bmemcpy(&out_eps[count].desc, epd, sizeof(*epd));
                    bmemset(&out_eps[count].ss_ep_comp, 0, sizeof(out_eps[count].ss_ep_comp));
                    out_eps[count].iface_num = current_iface;
                    out_eps[count].iface_subclass = current_subclass;
                    out_eps[count].iface_protocol = current_protocol;
                    out_eps[count].report_len = current_report_len;
                    // SS companion descriptor immediately following
                    unsigned int ss_off = offset + dlen;
                    if (ss_off + 2 <= len) {
                        uint8_t ss_len = buf[ss_off];
                        uint8_t ss_type = buf[ss_off + 1];
                        if (ss_type == USB_DT_SS_ENDPOINT_COMP && ss_len >= sizeof(struct usb_ss_ep_comp_descriptor)) {
                            const struct usb_ss_ep_comp_descriptor *ss = (const struct usb_ss_ep_comp_descriptor *)(buf + ss_off);
                            bmemcpy(&out_eps[count].ss_ep_comp, ss, sizeof(*ss));
                        }
                    }
                    count++;
                }
            }
        }
        offset += dlen;
    }
    return count;
}

static unsigned int build_hid_interfaces(const struct usb_host_endpoint *eps, unsigned int ep_count,
                                         uint8_t *ifaces, uint16_t *reports,
                                         uint8_t *subclass, uint8_t *protocol,
                                         unsigned int max_ifaces) {
    unsigned int count = 0;
    for (unsigned int i = 0; i < ep_count; i++) {
        uint8_t iface = eps[i].iface_num;
        bool seen = false;
        for (unsigned int j = 0; j < count; j++) {
            if (ifaces[j] == iface) {
                seen = true;
                break;
            }
        }
        if (seen) {
            continue;
        }
        if (count < max_ifaces) {
            ifaces[count] = iface;
            reports[count] = eps[i].report_len;
            subclass[count] = eps[i].iface_subclass;
            protocol[count] = eps[i].iface_protocol;
            count++;
        }
    }
    return count;
}

static void add_intr_endpoints(const struct usb_host_endpoint *eps, unsigned int ep_count,
                               struct xhci_virt_device *vdev,
                               struct intr_ep_queue *intr_eps,
                               unsigned int *intr_count,
                               unsigned int max_intr) {
    for (unsigned int i = 0; i < ep_count; i++) {
        if (*intr_count >= max_intr) {
            break;
        }
        uint8_t dci = (uint8_t)(usb_endpoint_num(&eps[i].desc) * 2u + 1u);
        struct xhci_ring *ep_ring = vdev->ep_rings[dci];
        if (!ep_ring) {
            continue;
        }
        intr_eps[*intr_count].slot_id = vdev->slot_id;
        intr_eps[*intr_count].dci = dci;
        intr_eps[*intr_count].ep_ring = ep_ring;
        intr_eps[*intr_count].max_packet = usb_endpoint_maxp(&eps[i].desc);
        (*intr_count)++;
    }
}

static bool enumerate_port(struct xhci_hcd *xhci,
                           uint8_t port,
                           bool *port_enumerated,
                           struct intr_ep_queue *intr_eps,
                           unsigned int *intr_count,
                           unsigned int max_intr) {
    uint64_t portsc_addr = xhci->op_base + 0x400 + ((uint64_t)port) * 0x10;
    uint32_t portsc = read32(portsc_addr);
    if ((portsc & PORTSC_CCS) == 0) {
        return false;
    }
    trace_port_found((uint8_t)(port + 1u));
    if (port_enumerated[port]) {
        return false;
    }

    // Reset port if it is not already enabled.
    if ((portsc & PORTSC_PED) == 0) {
        write32(portsc_addr, portsc | PORTSC_PR);
        for (unsigned int i = 0; i < 100000; i++) {
            portsc = read32(portsc_addr);
            if ((portsc & PORTSC_PR) == 0 && (portsc & PORTSC_PED)) {
                break;
            }
        }
    }
    portsc = read32(portsc_addr);

    // Enable slot
    uint8_t slot_id = xhci_enable_slot(xhci);
    if (slot_id == 0 || slot_id > MAX_SLOTS) {
        return false;
    }

    struct xhci_virt_device *vdev = &g_virt_devs[slot_id - 1];
    // Use preallocated contexts for this slot
    vdev->slot_id = slot_id;
    vdev->ctx_size = xhci->ctx_size;
    vdev->in_ctx = g_input_ctx[slot_id - 1];
    vdev->reconfig_in_ctx = g_reconfig_input_ctx[slot_id - 1];
    vdev->out_ctx = g_output_ctx[slot_id - 1];
    bmemset(vdev->ep_rings, 0, sizeof(vdev->ep_rings));
    bmemset(vdev->in_ctx, 0, 4096);
    bmemset(vdev->reconfig_in_ctx, 0, 4096);
    bmemset(vdev->out_ctx, 0, 4096);

    // Ep0 ring
    static struct xhci_ring ep0_ring_pool[MAX_SLOTS];
    if (xhci_ring_init(&ep0_ring_pool[slot_id - 1], 2, TYPE_CTRL) != 0) {
        return false;
    }
    vdev->ep_rings[1] = &ep0_ring_pool[slot_id - 1];

    // Point DCBAA to output context
    g_dcbaa[slot_id] = breenix_virt_to_phys_c((uint64_t)(uintptr_t)vdev->out_ctx);
    breenix_dma_cache_clean_c((const uint8_t *)g_dcbaa, sizeof(g_dcbaa));

    // Build a minimal usb_device_min
    struct usb_device_min udev;
    udev.slot_id = slot_id;
    udev.portnum = port + 1;
    udev.route = 0;
    uint32_t speed_val = (portsc & PORTSC_SPEED_MASK) >> PORTSC_SPEED_SHIFT;
    switch (speed_val) {
    case 5: udev.speed = USB_SPEED_SUPER_PLUS; break;
    case 4: udev.speed = USB_SPEED_SUPER; break;
    case 3: udev.speed = USB_SPEED_HIGH; break;
    case 2: udev.speed = USB_SPEED_FULL; break;
    case 1: udev.speed = USB_SPEED_LOW; break;
    default: udev.speed = USB_SPEED_SUPER; break;
    }

    if (xhci_address_device(xhci, vdev, &udev) != 0) {
        return false;
    }

    // GET CONFIG descriptor header
    struct usb_setup_packet setup;
    setup.bmRequestType = 0x80;
    setup.bRequest = 0x06;
    setup.wValue = (0x02 << 8);
    setup.wIndex = 0;
    setup.wLength = 9;

    bmemset(g_ctrl_data_buf, 0, sizeof(g_ctrl_data_buf));
    breenix_dma_cache_clean_c(g_ctrl_data_buf, sizeof(g_ctrl_data_buf));
    uint64_t buf_phys = breenix_virt_to_phys_c((uint64_t)(uintptr_t)g_ctrl_data_buf);

    if (control_transfer(xhci, slot_id, vdev->ep_rings[1], &setup, buf_phys, 9, true) != 0) {
        return false;
    }
    breenix_dma_cache_invalidate_c(g_ctrl_data_buf, 9);
    struct usb_config_descriptor *cfg = (struct usb_config_descriptor *)g_ctrl_data_buf;
    uint16_t total_len = cfg->wTotalLength;
    if (total_len > sizeof(g_ctrl_data_buf)) {
        total_len = sizeof(g_ctrl_data_buf);
    }

    setup.wLength = total_len;
    bmemset(g_ctrl_data_buf, 0, sizeof(g_ctrl_data_buf));
    breenix_dma_cache_clean_c(g_ctrl_data_buf, sizeof(g_ctrl_data_buf));
    if (control_transfer(xhci, slot_id, vdev->ep_rings[1], &setup, buf_phys, total_len, true) != 0) {
        return false;
    }
    breenix_dma_cache_invalidate_c(g_ctrl_data_buf, total_len);

    struct usb_host_endpoint eps[MAX_HID_EPS];
    unsigned int ep_count = parse_hid_endpoints(g_ctrl_data_buf, total_len, eps, MAX_HID_EPS);
    uint8_t config_value = cfg->bConfigurationValue;

    if (ep_count > 0) {
        if (xhci_configure_endpoints(xhci, vdev, &udev, eps, ep_count) != 0) {
            return false;
        }

        // SET_CONFIGURATION after endpoint config (matches Linux order)
        struct usb_setup_packet set_cfg;
        set_cfg.bmRequestType = 0x00;
        set_cfg.bRequest = 0x09;
        set_cfg.wValue = config_value;
        set_cfg.wIndex = 0;
        set_cfg.wLength = 0;
        control_transfer(xhci, slot_id, vdev->ep_rings[1], &set_cfg, 0, 0, false);

        // HID class setup: Set Idle + Get Report Descriptor per interface (Linux-like)
        uint8_t hid_ifaces[MAX_HID_EPS];
        uint16_t hid_reports[MAX_HID_EPS];
        uint8_t hid_subclass[MAX_HID_EPS];
        uint8_t hid_protocol[MAX_HID_EPS];
        unsigned int hid_count = build_hid_interfaces(eps, ep_count, hid_ifaces, hid_reports,
                                                      hid_subclass, hid_protocol, MAX_HID_EPS);
        for (unsigned int i = 0; i < hid_count; i++) {
            // Standard SET_INTERFACE (alt 0) to match Linux enumeration
            struct usb_setup_packet set_iface;
            set_iface.bmRequestType = 0x01;
            set_iface.bRequest = 0x0B;
            set_iface.wValue = 0;
            set_iface.wIndex = hid_ifaces[i];
            set_iface.wLength = 0;
            control_transfer(xhci, slot_id, vdev->ep_rings[1], &set_iface, 0, 0, false);

            // HID SET_PROTOCOL (boot) for boot-class devices
            if (hid_subclass[i] == 1) {
                struct usb_setup_packet set_proto;
                set_proto.bmRequestType = 0x21;
                set_proto.bRequest = 0x0B;
                set_proto.wValue = 0;
                set_proto.wIndex = hid_ifaces[i];
                set_proto.wLength = 0;
                control_transfer(xhci, slot_id, vdev->ep_rings[1], &set_proto, 0, 0, false);
            }

            struct usb_setup_packet set_idle;
            set_idle.bmRequestType = 0x21;
            set_idle.bRequest = 0x0A;
            set_idle.wValue = 0;
            set_idle.wIndex = hid_ifaces[i];
            set_idle.wLength = 0;
            control_transfer(xhci, slot_id, vdev->ep_rings[1], &set_idle, 0, 0, false);

            uint16_t report_len = hid_reports[i];
            if (report_len > 0) {
                if (report_len > sizeof(g_ctrl_data_buf)) {
                    report_len = sizeof(g_ctrl_data_buf);
                }
                struct usb_setup_packet get_report;
                get_report.bmRequestType = 0x81;
                get_report.bRequest = 0x06;
                get_report.wValue = (0x22 << 8);
                get_report.wIndex = hid_ifaces[i];
                get_report.wLength = report_len;
                bmemset(g_ctrl_data_buf, 0, sizeof(g_ctrl_data_buf));
                breenix_dma_cache_clean_c(g_ctrl_data_buf, sizeof(g_ctrl_data_buf));
                uint64_t rep_phys = breenix_virt_to_phys_c((uint64_t)(uintptr_t)g_ctrl_data_buf);
                control_transfer(xhci, slot_id, vdev->ep_rings[1], &get_report,
                                 rep_phys, report_len, true);
                breenix_dma_cache_invalidate_c(g_ctrl_data_buf, report_len);
            }

            // Linux-style feature report GET/SET (report IDs 0x11/0x12) for mouse-class HID
            if (hid_protocol[i] == 2) {
                uint8_t feature_id = 0;
                if (hid_ifaces[i] == 0) {
                    feature_id = 0x11;
                } else if (hid_ifaces[i] == 1) {
                    feature_id = 0x12;
                }
                if (feature_id != 0) {
                    struct usb_setup_packet get_feat;
                    get_feat.bmRequestType = 0xA1;
                    get_feat.bRequest = 0x01;
                    get_feat.wValue = (uint16_t)((0x03 << 8) | feature_id);
                    get_feat.wIndex = hid_ifaces[i];
                    get_feat.wLength = 64;
                    bmemset(g_ctrl_data_buf, 0, sizeof(g_ctrl_data_buf));
                    breenix_dma_cache_clean_c(g_ctrl_data_buf, sizeof(g_ctrl_data_buf));
                    uint64_t feat_phys = breenix_virt_to_phys_c((uint64_t)(uintptr_t)g_ctrl_data_buf);
                    control_transfer(xhci, slot_id, vdev->ep_rings[1], &get_feat,
                                     feat_phys, 64, true);
                    breenix_dma_cache_invalidate_c(g_ctrl_data_buf, 64);

                    struct usb_setup_packet set_feat;
                    set_feat.bmRequestType = 0x21;
                    set_feat.bRequest = 0x09;
                    set_feat.wValue = (uint16_t)((0x03 << 8) | feature_id);
                    set_feat.wIndex = hid_ifaces[i];
                    set_feat.wLength = 2;
                    bmemset(g_ctrl_data_buf, 0, sizeof(g_ctrl_data_buf));
                    g_ctrl_data_buf[0] = feature_id;
                    g_ctrl_data_buf[1] = feature_id;
                    breenix_dma_cache_clean_c(g_ctrl_data_buf, 2);
                    control_transfer(xhci, slot_id, vdev->ep_rings[1], &set_feat,
                                     breenix_virt_to_phys_c((uint64_t)(uintptr_t)g_ctrl_data_buf), 2, false);
                }
            }

            // LED/output report for keyboards (boot protocol)
            if (hid_protocol[i] == 1) {
                struct usb_setup_packet set_led;
                set_led.bmRequestType = 0x21;
                set_led.bRequest = 0x09;
                set_led.wValue = 0x0200;
                set_led.wIndex = hid_ifaces[i];
                set_led.wLength = 1;
                bmemset(g_ctrl_data_buf, 0, sizeof(g_ctrl_data_buf));
                g_ctrl_data_buf[0] = 0;
                breenix_dma_cache_clean_c(g_ctrl_data_buf, 1);
                control_transfer(xhci, slot_id, vdev->ep_rings[1], &set_led,
                                 breenix_virt_to_phys_c((uint64_t)(uintptr_t)g_ctrl_data_buf), 1, false);
            }
        }

        add_intr_endpoints(eps, ep_count, vdev, intr_eps, intr_count, max_intr);
    }

    port_enumerated[port] = true;
    return true;
}

int linux_xhci_init(struct breenix_xhci_state *state) {
    trace_note(0, "linux_xhci_begin");

    // Initialize global controller info
    g_xhci.base = state->base;
    g_xhci.op_base = state->op_base;
    g_xhci.rt_base = state->rt_base;
    g_xhci.db_base = state->db_base;
    g_xhci.max_slots = state->max_slots;
    g_xhci.max_ports = state->max_ports;
    g_xhci.ctx_size = state->context_size;

    uint32_t cap_word = read32(g_xhci.base);
    g_xhci.hci_version = (uint16_t)((cap_word >> 16) & 0xffff);

    if (xhci_init_controller(&g_xhci) != 0) {
        trace_note(0, "linux_xhci_init_fail");
        return -1;
    }

    bool port_enumerated[MAX_PORTS];
    bmemset(port_enumerated, 0, sizeof(port_enumerated));
    struct intr_ep_queue intr_eps[MAX_INTR_ENDPOINTS];
    unsigned int intr_count = 0;

    // First pass: enumerate all currently connected devices.
    for (uint8_t port = 0; port < g_xhci.max_ports; port++) {
        enumerate_port(&g_xhci, port, port_enumerated, intr_eps, &intr_count, MAX_INTR_ENDPOINTS);
    }

    // Second pass: wait for late connections, then re-scan for new devices.
    wait_ms(2000);
    for (uint8_t port = 0; port < g_xhci.max_ports; port++) {
        enumerate_port(&g_xhci, port, port_enumerated, intr_eps, &intr_count, MAX_INTR_ENDPOINTS);
    }

    trace_note(0, "all_devices_enumerated");

    for (uint8_t i = 0; i < MAX_SLOTS; i++) {
        struct xhci_virt_device *vdev = &g_virt_devs[i];
        if (vdev->slot_id == 0) {
            continue;
        }
        breenix_dma_cache_invalidate_c(vdev->out_ctx, 4096);
        trace_output_ctx(vdev->slot_id, vdev->out_ctx, vdev->ctx_size, 5);
    }

    bool intr_slot[MAX_SLOTS + 1];
    bmemset(intr_slot, 0, sizeof(intr_slot));
    for (unsigned int i = 0; i < intr_count; i++) {
        uint8_t slot_id = intr_eps[i].slot_id;
        if (slot_id > 0 && slot_id <= MAX_SLOTS) {
            intr_slot[slot_id] = true;
        }
    }

    // Queue interrupt transfers on all interrupt IN endpoints.
    for (unsigned int i = 0; i < intr_count; i++) {
        struct intr_ep_queue *info = &intr_eps[i];
        struct xhci_ring *ep_ring = info->ep_ring;
        if (!ep_ring) {
            continue;
        }

        uint32_t xfer_len = info->max_packet;
        if (xfer_len == 0) {
            xfer_len = 64;
        }
        if (xfer_len > sizeof(g_intr_bufs[i])) {
            xfer_len = sizeof(g_intr_bufs[i]);
        }

        bmemset(g_intr_bufs[i], 0xDE, sizeof(g_intr_bufs[i]));
        breenix_dma_cache_clean_c(g_intr_bufs[i], sizeof(g_intr_bufs[i]));
        struct xhci_trb trb;
        bmemset(&trb, 0, sizeof(trb));
        uint64_t data_phys = breenix_virt_to_phys_c((uint64_t)(uintptr_t)g_intr_bufs[i]);
        trb.field[0] = (uint32_t)(data_phys & 0xFFFFFFFFu);
        trb.field[1] = (uint32_t)((data_phys >> 32) & 0xFFFFFFFFu);
        trb.field[2] = xfer_len;
        trb.field[3] = (TRB_TYPE_NORMAL << 10) | TRB_IOC | TRB_ISP;
        trace_trb(TRACE_XFER_SUBMIT, info->slot_id, info->dci, &trb);
        xhci_ring_enqueue_trb(ep_ring, &trb);
        ring_doorbell(&g_xhci, info->slot_id, info->dci);
    }

    if (intr_count > 0) {
        struct xhci_trb noop;
        bmemset(&noop, 0, sizeof(noop));
        noop.field[3] = (TRB_TYPE_NOOP << 10);
        struct xhci_trb noop_ev;
        int rc = xhci_cmd(&g_xhci, &noop, &noop_ev);
        if (rc == 0) {
            trace_note(0, "cmd_ring_alive");
        } else {
            trace_note(0, "cmd_ring_dead");
        }

        struct xhci_trb ev;
        trace_note(0, "linux_xhci_wait_intr");
        bool got_event = false;
        for (unsigned int attempt = 0; attempt < 6; attempt++) {
            if (xhci_wait_for_event_ms(&g_xhci, &ev, TRB_TYPE_TRANSFER_EVENT, 5000) == 0) {
                trace_trb(TRACE_XFER_EVENT, (uint8_t)((ev.field[3] >> TRB_SLOT_ID_SHIFT) & 0xff),
                          (uint8_t)((ev.field[3] >> TRB_EP_ID_SHIFT) & 0x1f), &ev);
                got_event = true;
                break;
            }

            uint64_t usbsts_addr = g_xhci.op_base + USBSTS;
            uint32_t usbsts = read32(usbsts_addr);
            trace_mmio_w32(usbsts_addr, usbsts);
            uint64_t iman_addr = g_xhci.rt_base + 0x20 + IMAN;
            uint32_t iman = read32(iman_addr);
            trace_mmio_w32(iman_addr, iman);

            for (uint8_t slot_id = 1; slot_id <= MAX_SLOTS; slot_id++) {
                if (!intr_slot[slot_id]) {
                    continue;
                }
                struct xhci_virt_device *vdev = &g_virt_devs[slot_id - 1];
                breenix_dma_cache_invalidate_c(vdev->out_ctx, 4096);
                trace_output_ctx(slot_id, vdev->out_ctx, vdev->ctx_size, 5);
            }
        }
        if (!got_event) {
            trace_note(0, "linux_xhci_intr_timeout");
        }
    }

    trace_note(0, "linux_xhci_done");
    return 0;
}
