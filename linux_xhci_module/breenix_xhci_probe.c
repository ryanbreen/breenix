// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * breenix_xhci_probe - Standalone xHCI probe module for Breenix validation.
 *
 * This is a plain PCI driver that claims an xHCI controller, does raw
 * register-level init, enumerates USB devices, configures HID interrupt
 * endpoints, and prints received HID reports to dmesg.
 *
 * It does NOT use the Linux USB HCD framework (usb_hcd / hc_driver).
 * Its purpose is to validate xHCI logic independently of Breenix kernel
 * infrastructure (DMA mapping, cache coherency, memory model).
 *
 * Ported from kernel/src/drivers/usb/linux_xhci/linux_xhci.c (Breenix).
 */

#include <linux/module.h>
#include <linux/pci.h>
#include <linux/dma-mapping.h>
#include <linux/delay.h>
#include <linux/interrupt.h>
#include <linux/slab.h>
#include <linux/io.h>
#include <linux/string.h>
#include <linux/iommu.h>
#include <linux/mm.h>
#include <linux/version.h>

/* -----------------------------------------------------------------------
 * Constants
 * ----------------------------------------------------------------------- */
#define MAX_SLOTS          32
#define MAX_HID_EPS         4
#define MAX_INTR_ENDPOINTS (MAX_SLOTS * MAX_HID_EPS)
#define MAX_PORTS         256
#define TRBS_PER_SEGMENT  256
#define SEGMENT_POOL_COUNT 64
#define RING_POOL_COUNT    16
#define CTRL_BUF_SIZE     512
#define INTR_BUF_SIZE    1024

/* TRB types */
#define TRB_TYPE_NORMAL             1
#define TRB_TYPE_SETUP              2
#define TRB_TYPE_DATA               3
#define TRB_TYPE_STATUS             4
#define TRB_TYPE_LINK               6
#define TRB_TYPE_NOOP               8
#define TRB_TYPE_ENABLE_SLOT        9
#define TRB_TYPE_ADDRESS_DEVICE    11
#define TRB_TYPE_CONFIGURE_ENDPOINT 12
#define TRB_TYPE_STOP_ENDPOINT     15
#define TRB_TYPE_SET_TR_DEQ        16
#define TRB_TYPE_TRANSFER_EVENT    32
#define TRB_TYPE_COMMAND_COMPLETION 33

/* TRB control bits */
#define TRB_CYCLE   (1u << 0)
#define TRB_TC      (1u << 1)
#define TRB_ISP     (1u << 2)
#define TRB_IOC     (1u << 5)
#define TRB_IDT     (1u << 6)
#define TRB_DIR_IN  (1u << 16)
#define TRB_TRT_SHIFT       16
#define TRB_SLOT_ID_SHIFT   24
#define TRB_EP_ID_SHIFT     16

/* Slot/EP context field macros */
#define LAST_CTX(p)               ((p) << 27)
#define ROOT_HUB_PORT(p)          (((p) & 0xffu) << 16)
#define EP_MULT(p)                (((p) & 0x3u) << 8)
#define EP_INTERVAL(p)            (((p) & 0xffu) << 16)
#define EP_TYPE(p)                ((p) << 3)
#define ERROR_COUNT(p)            (((p) & 0x3u) << 1)
#define MAX_BURST(p)              (((p) & 0xffu) << 8)
#define MAX_PACKET(p)             (((p) & 0xffffu) << 16)
#define EP_AVG_TRB_LENGTH(p)      ((p) & 0xffffu)
#define EP_MAX_ESIT_PAYLOAD_LO(p) (((p) & 0xffffu) << 16)
#define EP_MAX_ESIT_PAYLOAD_HI(p) ((((p) >> 16) & 0xffu) << 24)

#define SLOT_SPEED_SS   (4u << 20)
#define SLOT_SPEED_SSP  (5u << 20)
#define SLOT_SPEED_HS   (3u << 20)
#define SLOT_SPEED_FS   (2u << 20)
#define SLOT_SPEED_LS   (1u << 20)

#define EP0_FLAG   (1u << 1)
#define SLOT_FLAG  (1u << 0)
#define CTRL_EP    4
#define INT_IN_EP  7

#define CC_SUCCESS      1
#define CC_SHORT_PACKET 13

/* Operational registers (offsets from op_base) */
#define USBCMD_OFF  0x00
#define USBSTS_OFF  0x04
#define DNCTRL_OFF  0x14
#define CRCR_OFF    0x18
#define DCBAAP_OFF  0x30
#define CONFIG_OFF  0x38

/* PORTSC bits */
#define PORTSC_CCS         (1u << 0)
#define PORTSC_PED         (1u << 1)
#define PORTSC_PR          (1u << 4)
#define PORTSC_PRC         (1u << 21)
#define PORTSC_SPEED_SHIFT 10
#define PORTSC_SPEED_MASK  (0xFu << PORTSC_SPEED_SHIFT)

/* Interrupter registers (offsets from ir0_base) */
#define IMAN_OFF   0x00
#define IMOD_OFF   0x04
#define ERSTSZ_OFF 0x08
#define ERSTBA_OFF 0x10
#define ERDP_OFF   0x18

/* USB speeds */
#define USB_SPEED_LOW        1
#define USB_SPEED_FULL       2
#define USB_SPEED_HIGH       3
#define USB_SPEED_SUPER      4
#define USB_SPEED_SUPER_PLUS 5

/* USB descriptor types */
#define USB_DT_INTERFACE       4
#define USB_DT_ENDPOINT        5
#define USB_DT_HID          0x21
#define USB_DT_SS_EP_COMP   0x30
#define USB_CLASS_HID        0x03

/* USB endpoint transfer types */
#define USB_EP_XFER_CONTROL 0
#define USB_EP_XFER_ISOC    1
#define USB_EP_XFER_BULK    2
#define USB_EP_XFER_INT     3

/* -----------------------------------------------------------------------
 * Data structures
 * ----------------------------------------------------------------------- */

struct xhci_trb {
	u32 field[4];
};

struct xhci_segment {
	struct xhci_trb *trbs;     /* virtual */
	dma_addr_t       dma;      /* physical */
	struct xhci_segment *next;
};

enum xhci_ring_type {
	TYPE_COMMAND = 0,
	TYPE_EVENT   = 1,
	TYPE_CTRL    = 2,
	TYPE_INTR    = 3,
};

struct xhci_ring {
	struct xhci_segment *first_seg;
	struct xhci_segment *last_seg;
	struct xhci_segment *enq_seg;
	struct xhci_trb     *enqueue;
	u32                  cycle_state;
	unsigned int         num_segs;
	enum xhci_ring_type  type;
};

struct xhci_slot_ctx {
	u32 dev_info;
	u32 dev_info2;
	u32 tt_info;
	u32 dev_state;
	u32 reserved[4];
};

struct xhci_ep_ctx {
	u32 ep_info;
	u32 ep_info2;
	u64 deq;
	u32 tx_info;
	u32 reserved[3];
};

struct xhci_input_control_ctx {
	u32 drop_flags;
	u32 add_flags;
	u32 rsvd2[6];
};

struct xhci_erst_entry {
	u64 seg_addr;
	u32 seg_size;
	u32 rsvd;
};

struct xhci_virt_device {
	u8               slot_id;
	u8               ctx_size;
	u8              *in_ctx;
	dma_addr_t       in_ctx_dma;
	u8              *reconfig_in_ctx;
	dma_addr_t       reconfig_in_ctx_dma;
	u8              *out_ctx;
	dma_addr_t       out_ctx_dma;
	struct xhci_ring *ep_rings[32];
};

/* USB descriptor structs (packed) */
struct usb_config_desc {
	u8  bLength;
	u8  bDescriptorType;
	__le16 wTotalLength;
	u8  bNumInterfaces;
	u8  bConfigurationValue;
	u8  iConfiguration;
	u8  bmAttributes;
	u8  bMaxPower;
} __packed;

struct usb_iface_desc {
	u8  bLength;
	u8  bDescriptorType;
	u8  bInterfaceNumber;
	u8  bAlternateSetting;
	u8  bNumEndpoints;
	u8  bInterfaceClass;
	u8  bInterfaceSubClass;
	u8  bInterfaceProtocol;
	u8  iInterface;
} __packed;

struct usb_ep_desc {
	u8  bLength;
	u8  bDescriptorType;
	u8  bEndpointAddress;
	u8  bmAttributes;
	__le16 wMaxPacketSize;
	u8  bInterval;
} __packed;

struct usb_ss_ep_comp_desc {
	u8  bLength;
	u8  bDescriptorType;
	u8  bMaxBurst;
	u8  bmAttributes;
	__le16 wBytesPerInterval;
} __packed;

struct usb_hid_desc {
	u8  bLength;
	u8  bDescriptorType;
	__le16 bcdHID;
	u8  bCountryCode;
	u8  bNumDescriptors;
	u8  bReportDescriptorType;
	__le16 wDescriptorLength;
} __packed;

struct usb_setup_packet {
	u8  bmRequestType;
	u8  bRequest;
	__le16 wValue;
	__le16 wIndex;
	__le16 wLength;
} __packed;

struct host_endpoint {
	struct usb_ep_desc         desc;
	struct usb_ss_ep_comp_desc ss_ep_comp;
	u8  iface_num;
	u8  iface_subclass;
	u8  iface_protocol;
	u16 report_len;
};

struct usb_dev_min {
	u8  speed;
	u8  slot_id;
	u8  portnum;
	u32 route;
};

struct intr_ep_queue {
	u8               slot_id;
	u8               dci;
	struct xhci_ring *ep_ring;
	u32              max_packet;
};

/* -----------------------------------------------------------------------
 * Per-device probe state (replaces all globals from C harness)
 * ----------------------------------------------------------------------- */
struct probe_state {
	struct pci_dev   *pdev;
	void __iomem     *bar;        /* ioremapped BAR0 */
	size_t            bar_len;

	/* Capability/operational/runtime/doorbell base offsets from bar */
	u32 op_off;
	u32 rt_off;
	u32 db_off;
	u8  max_slots;
	u8  max_ports;
	u8  ctx_size;
	u16 hci_version;

	/* DCBAA */
	u64        *dcbaa;
	dma_addr_t  dcbaa_dma;

	/* ERST */
	struct xhci_erst_entry *erst;
	dma_addr_t              erst_dma;

	/* Segment pool */
	struct xhci_segment  segments[SEGMENT_POOL_COUNT];
	struct xhci_trb     *seg_trb_va[SEGMENT_POOL_COUNT];
	dma_addr_t            seg_trb_dma[SEGMENT_POOL_COUNT];
	unsigned int          seg_alloc_idx;

	/* Rings */
	struct xhci_ring cmd_ring;
	struct xhci_ring event_ring;

	/* Event ring dequeue state */
	struct xhci_segment *event_deq_seg;
	struct xhci_trb     *event_dequeue;
	u32                  event_cycle;

	/* Virtual devices */
	struct xhci_virt_device virt_devs[MAX_SLOTS];

	/* EP0 ring pool */
	struct xhci_ring ep0_ring_pool[MAX_SLOTS];

	/* Interrupt endpoint ring pool */
	struct xhci_ring ring_pool[RING_POOL_COUNT];
	unsigned int     ring_pool_idx;

	/* Control transfer buffer */
	u8         *ctrl_buf;
	dma_addr_t  ctrl_buf_dma;

	/* Interrupt transfer buffers */
	u8         *intr_bufs[MAX_INTR_ENDPOINTS];
	dma_addr_t  intr_bufs_dma[MAX_INTR_ENDPOINTS];

	/* Interrupt endpoint tracking */
	struct intr_ep_queue intr_eps[MAX_INTR_ENDPOINTS];
	unsigned int         intr_count;

	/* MSI IRQ */
	int irq;
};

/* -----------------------------------------------------------------------
 * MMIO write trace buffer (HCRST through first doorbell ring)
 * ----------------------------------------------------------------------- */
#define MMIO_TRACE_MAX 512

struct mmio_trace_entry {
	u32 offset;   /* offset from BAR base */
	u32 value;    /* 32-bit value written */
	u32 seq;      /* monotonically increasing sequence number */
};

static struct mmio_trace_entry mmio_trace_buf[MMIO_TRACE_MAX];
static u32  mmio_trace_idx;   /* next write index */
static u32  mmio_trace_seq;   /* sequence counter */
static bool mmio_trace_active;

static inline void mmio_trace_record(u32 offset, u32 value)
{
	if (!mmio_trace_active)
		return;
	if (mmio_trace_idx < MMIO_TRACE_MAX) {
		mmio_trace_buf[mmio_trace_idx].offset = offset;
		mmio_trace_buf[mmio_trace_idx].value  = value;
		mmio_trace_buf[mmio_trace_idx].seq    = mmio_trace_seq++;
		mmio_trace_idx++;
	}
}

/* -----------------------------------------------------------------------
 * MMIO helpers
 * ----------------------------------------------------------------------- */
static inline u32 xhci_read32(struct probe_state *st, u32 offset)
{
	return readl(st->bar + offset);
}

static inline void xhci_write32(struct probe_state *st, u32 offset, u32 val)
{
	mmio_trace_record(offset, val);
	writel(val, st->bar + offset);
}

static inline u64 xhci_read64(struct probe_state *st, u32 offset)
{
	u32 lo = readl(st->bar + offset);
	u32 hi = readl(st->bar + offset + 4);
	return ((u64)hi << 32) | lo;
}

static inline void xhci_write64(struct probe_state *st, u32 offset, u64 val)
{
	mmio_trace_record(offset, (u32)(val & 0xFFFFFFFF));
	mmio_trace_record(offset + 4, (u32)(val >> 32));
	writel((u32)(val & 0xFFFFFFFF), st->bar + offset);
	writel((u32)(val >> 32), st->bar + offset + 4);
}

/* Convenience: operational register access */
static inline u32 op_read32(struct probe_state *st, u32 reg)
{
	return xhci_read32(st, st->op_off + reg);
}

static inline void op_write32(struct probe_state *st, u32 reg, u32 val)
{
	xhci_write32(st, st->op_off + reg, val);
}

static inline u64 op_read64(struct probe_state *st, u32 reg)
{
	return xhci_read64(st, st->op_off + reg);
}

static inline void op_write64(struct probe_state *st, u32 reg, u64 val)
{
	xhci_write64(st, st->op_off + reg, val);
}

/* Convenience: interrupter 0 register access */
static inline u32 ir0_off(struct probe_state *st)
{
	return st->rt_off + 0x20;
}

static inline u32 ir0_read32(struct probe_state *st, u32 reg)
{
	return xhci_read32(st, ir0_off(st) + reg);
}

static inline void ir0_write32(struct probe_state *st, u32 reg, u32 val)
{
	xhci_write32(st, ir0_off(st) + reg, val);
}

static inline u64 ir0_read64(struct probe_state *st, u32 reg)
{
	return xhci_read64(st, ir0_off(st) + reg);
}

static inline void ir0_write64(struct probe_state *st, u32 reg, u64 val)
{
	xhci_write64(st, ir0_off(st) + reg, val);
}

/* Doorbell */
static inline void ring_doorbell(struct probe_state *st, u8 slot, u8 target)
{
	xhci_write32(st, st->db_off + (u32)slot * 4, target);
}

/* -----------------------------------------------------------------------
 * Milestone-based initialization outline
 *
 * The xHCI initialization is organized into numbered milestones. Each
 * milestone validates specific invariants. When comparing Linux vs Breenix,
 * the first milestone that diverges identifies the root cause.
 *
 * M1:  CONTROLLER_DISCOVERY  — BAR mapped, capabilities read
 * M2:  CONTROLLER_RESET      — HCRST done, CNR clear, HCH=1
 * M3:  DATA_STRUCTURES       — DCBAA, CMD ring, EVT ring, ERST programmed
 * M4:  CONTROLLER_RUNNING    — RS=1, INTE=1, IMAN.IE=1
 * M5:  PORT_DETECTION        — Connected ports identified, speed known
 * M6:  SLOT_ENABLE           — EnableSlot CC=1, slot ID allocated
 * M7:  DEVICE_ADDRESS        — Input ctx built, AddressDevice CC=1
 * M8:  ENDPOINT_CONFIG       — ConfigureEndpoint CC=1, BW dance done
 * M9:  HID_CLASS_SETUP       — SET_CONFIGURATION, SET_IDLE, descriptors
 * M10: INTERRUPT_TRANSFER    — Normal TRBs queued, doorbells rung
 * M11: EVENT_DELIVERY        — First transfer event (HID data received)
 * ----------------------------------------------------------------------- */

#define M_DISCOVERY  1
#define M_RESET      2
#define M_DATA_STRUC 3
#define M_RUNNING    4
#define M_PORT_DET   5
#define M_SLOT_EN    6
#define M_ADDR_DEV   7
#define M_EP_CONFIG  8
#define M_HID_SETUP  9
#define M_INTR_XFER 10
#define M_EVT_DELIV 11
#define M_TOTAL     11

static const char * const milestone_names[] = {
	[0]  = "UNUSED",
	[1]  = "CONTROLLER_DISCOVERY",
	[2]  = "CONTROLLER_RESET",
	[3]  = "DATA_STRUCTURES",
	[4]  = "CONTROLLER_RUNNING",
	[5]  = "PORT_DETECTION",
	[6]  = "SLOT_ENABLE",
	[7]  = "DEVICE_ADDRESS",
	[8]  = "ENDPOINT_CONFIG",
	[9]  = "HID_CLASS_SETUP",
	[10] = "INTERRUPT_TRANSFER",
	[11] = "EVENT_DELIVERY",
};

static void ms_begin(struct probe_state *st, int m)
{
	dev_info(&st->pdev->dev,
		 "=== MILESTONE %d/%d: %s ===\n", m, M_TOTAL,
		 milestone_names[m]);
}

static void ms_pass(struct probe_state *st, int m)
{
	dev_info(&st->pdev->dev,
		 "[M%d] RESULT: PASS\n", m);
}

static void ms_fail(struct probe_state *st, int m, const char *reason)
{
	dev_info(&st->pdev->dev,
		 "[M%d] RESULT: FAIL (%s)\n", m, reason);
}

/* Print a key=value pair under a milestone */
#define ms_kv(st, m, fmt, ...) \
	dev_info(&(st)->pdev->dev, "[M%d] " fmt "\n", (m), ##__VA_ARGS__)

/* Hex-dump a DMA buffer under a milestone */
static void ms_dump(struct probe_state *st, int m, const char *label,
		    const void *buf, dma_addr_t dma, size_t len)
{
	const u32 *p = buf;
	size_t i, n;

	dev_info(&st->pdev->dev, "[M%d] %s: dma=0x%llx len=%zu\n",
		 m, label, (u64)dma, len);
	n = len / 4;
	for (i = 0; i < n; i += 4) {
		if (i + 3 < n) {
			dev_info(&st->pdev->dev,
				 "[M%d]   +%03zx: %08x %08x %08x %08x\n",
				 m, i * 4, p[i], p[i+1], p[i+2], p[i+3]);
		} else if (n - i == 3) {
			dev_info(&st->pdev->dev,
				 "[M%d]   +%03zx: %08x %08x %08x\n",
				 m, i * 4, p[i], p[i+1], p[i+2]);
		} else if (n - i == 2) {
			dev_info(&st->pdev->dev,
				 "[M%d]   +%03zx: %08x %08x\n",
				 m, i * 4, p[i], p[i+1]);
		} else if (n - i == 1) {
			dev_info(&st->pdev->dev,
				 "[M%d]   +%03zx: %08x\n",
				 m, i * 4, p[i]);
		}
	}
}

/* Dump a TRB under a milestone */
static void ms_trb(struct probe_state *st, int m, const char *label,
		   const struct xhci_trb *trb)
{
	dev_info(&st->pdev->dev,
		 "[M%d] %s: %08x %08x %08x %08x\n",
		 m, label, trb->field[0], trb->field[1],
		 trb->field[2], trb->field[3]);
}

/* Dump all key registers under a milestone */
static void ms_regs(struct probe_state *st, int m)
{
	ms_kv(st, m, "USBCMD=0x%08x USBSTS=0x%08x",
	      op_read32(st, USBCMD_OFF), op_read32(st, USBSTS_OFF));
	ms_kv(st, m, "DCBAAP=0x%016llx CRCR=0x%016llx",
	      (u64)op_read64(st, DCBAAP_OFF), (u64)op_read64(st, CRCR_OFF));
	ms_kv(st, m, "IMAN=0x%08x IMOD=0x%08x ERSTSZ=0x%08x",
	      ir0_read32(st, IMAN_OFF), ir0_read32(st, IMOD_OFF),
	      ir0_read32(st, ERSTSZ_OFF));
	ms_kv(st, m, "ERDP=0x%016llx ERSTBA=0x%016llx",
	      (u64)ir0_read64(st, ERDP_OFF), (u64)ir0_read64(st, ERSTBA_OFF));
}

/* Dump full 256-byte PCI config space in milestone format for comparison */
static void ms_pci_config(struct probe_state *st, int m, const char *label)
{
	int offset;

	for (offset = 0; offset < 256; offset += 16) {
		u32 dw0, dw1, dw2, dw3;

		pci_read_config_dword(st->pdev, offset,      &dw0);
		pci_read_config_dword(st->pdev, offset + 4,  &dw1);
		pci_read_config_dword(st->pdev, offset + 8,  &dw2);
		pci_read_config_dword(st->pdev, offset + 12, &dw3);
		dev_info(&st->pdev->dev,
			 "[M%d] %s +%03x: %08x %08x %08x %08x\n",
			 m, label, offset, dw0, dw1, dw2, dw3);
	}
}

/* -----------------------------------------------------------------------
 * USB endpoint helpers
 * ----------------------------------------------------------------------- */
static inline u8 ep_type(const struct usb_ep_desc *d)
{
	return d->bmAttributes & 0x3;
}

static inline bool ep_is_int(const struct usb_ep_desc *d)
{
	return ep_type(d) == USB_EP_XFER_INT;
}

static inline bool ep_is_bulk(const struct usb_ep_desc *d)
{
	return ep_type(d) == USB_EP_XFER_BULK;
}

static inline bool ep_is_isoc(const struct usb_ep_desc *d)
{
	return ep_type(d) == USB_EP_XFER_ISOC;
}

static inline bool ep_is_ctrl(const struct usb_ep_desc *d)
{
	return ep_type(d) == USB_EP_XFER_CONTROL;
}

static inline bool ep_dir_in(const struct usb_ep_desc *d)
{
	return (d->bEndpointAddress & 0x80) != 0;
}

static inline u8 ep_num(const struct usb_ep_desc *d)
{
	return d->bEndpointAddress & 0x0F;
}

static inline unsigned int ep_maxp(const struct usb_ep_desc *d)
{
	return le16_to_cpu(d->wMaxPacketSize) & 0x7FF;
}

static inline unsigned int ep_maxp_mult(const struct usb_ep_desc *d)
{
	return ((le16_to_cpu(d->wMaxPacketSize) >> 11) & 0x3) + 1;
}

/* -----------------------------------------------------------------------
 * Context helpers
 * ----------------------------------------------------------------------- */
static inline struct xhci_input_control_ctx *get_input_control_ctx(u8 *ctx)
{
	return (struct xhci_input_control_ctx *)ctx;
}

static inline struct xhci_slot_ctx *get_slot_ctx_in(u8 *ctx, size_t ctx_size)
{
	return (struct xhci_slot_ctx *)(ctx + ctx_size);
}

static inline struct xhci_slot_ctx *get_slot_ctx_out(u8 *ctx)
{
	return (struct xhci_slot_ctx *)ctx;
}

static inline struct xhci_ep_ctx *get_ep_ctx_in(u8 *ctx, size_t ctx_size, u8 dci)
{
	return (struct xhci_ep_ctx *)(ctx + (1 + (size_t)dci) * ctx_size);
}

static inline struct xhci_ep_ctx *get_ep_ctx_out(u8 *ctx, size_t ctx_size, u8 dci)
{
	return (struct xhci_ep_ctx *)(ctx + (size_t)dci * ctx_size);
}

/* -----------------------------------------------------------------------
 * Interval calculation (matches Linux xhci-mem.c)
 * ----------------------------------------------------------------------- */
static unsigned int fls_u32(u32 v)
{
	unsigned int r = 0;

	while (v) {
		v >>= 1;
		r++;
	}
	return r;
}

static unsigned int xhci_parse_exponent_interval(const struct host_endpoint *ep)
{
	unsigned int bi = ep->desc.bInterval;

	if (bi < 1)
		bi = 1;
	if (bi > 16)
		bi = 16;
	return bi - 1;
}

static unsigned int xhci_microframes_to_exponent(unsigned int desc_interval,
						 unsigned int min_exp,
						 unsigned int max_exp)
{
	unsigned int interval = fls_u32(desc_interval) - 1;

	return clamp(interval, min_exp, max_exp);
}

static unsigned int xhci_parse_microframe_interval(const struct host_endpoint *ep)
{
	if (ep->desc.bInterval == 0)
		return 0;
	return xhci_microframes_to_exponent(ep->desc.bInterval, 0, 15);
}

static unsigned int xhci_parse_frame_interval(const struct host_endpoint *ep)
{
	return xhci_microframes_to_exponent((unsigned int)ep->desc.bInterval * 8, 3, 10);
}

static unsigned int xhci_get_endpoint_interval(const struct usb_dev_min *udev,
					       const struct host_endpoint *ep)
{
	unsigned int interval = 0;

	switch (udev->speed) {
	case USB_SPEED_HIGH:
		if (ep_is_ctrl(&ep->desc) || ep_is_bulk(&ep->desc)) {
			interval = xhci_parse_microframe_interval(ep);
			break;
		}
		fallthrough;
	case USB_SPEED_SUPER_PLUS:
	case USB_SPEED_SUPER:
		if (ep_is_int(&ep->desc) || ep_is_isoc(&ep->desc))
			interval = xhci_parse_exponent_interval(ep);
		break;
	case USB_SPEED_FULL:
		if (ep_is_isoc(&ep->desc)) {
			interval = xhci_parse_exponent_interval(ep);
			break;
		}
		fallthrough;
	case USB_SPEED_LOW:
		if (ep_is_int(&ep->desc) || ep_is_isoc(&ep->desc))
			interval = xhci_parse_frame_interval(ep);
		break;
	default:
		break;
	}
	return interval;
}

static unsigned int usb_ep_max_periodic_payload(const struct usb_dev_min *udev,
						const struct host_endpoint *ep)
{
	if (ep_is_ctrl(&ep->desc) || ep_is_bulk(&ep->desc))
		return 0;
	if (udev->speed >= USB_SPEED_SUPER) {
		unsigned int bytes = le16_to_cpu(ep->ss_ep_comp.wBytesPerInterval);

		if (bytes == 0) {
			unsigned int mp = ep_maxp(&ep->desc);
			unsigned int mb = ep->ss_ep_comp.bMaxBurst;
			unsigned int mult = (ep->ss_ep_comp.bmAttributes & 0x3) + 1;

			bytes = mp * (mb + 1) * mult;
		}
		return bytes;
	}
	return ep_maxp(&ep->desc) * ep_maxp_mult(&ep->desc);
}

static unsigned int xhci_get_endpoint_max_burst(const struct usb_dev_min *udev,
						const struct host_endpoint *ep)
{
	if (udev->speed >= USB_SPEED_SUPER)
		return ep->ss_ep_comp.bMaxBurst;
	if (udev->speed == USB_SPEED_HIGH && ep_is_int(&ep->desc))
		return ep_maxp_mult(&ep->desc) - 1;
	return 0;
}

static unsigned int xhci_get_endpoint_type(const struct host_endpoint *ep)
{
	int in = ep_dir_in(&ep->desc);

	if (ep_type(&ep->desc) == USB_EP_XFER_INT)
		return in ? INT_IN_EP : 3;
	return 0;
}

/* -----------------------------------------------------------------------
 * Ring allocation and operations
 * ----------------------------------------------------------------------- */
static struct xhci_segment *segment_alloc(struct probe_state *st)
{
	struct xhci_segment *seg;
	struct xhci_trb *trbs;
	dma_addr_t dma;

	if (st->seg_alloc_idx >= SEGMENT_POOL_COUNT) {
		dev_err(&st->pdev->dev, "segment pool exhausted\n");
		return NULL;
	}

	trbs = dma_alloc_coherent(&st->pdev->dev,
				  TRBS_PER_SEGMENT * sizeof(struct xhci_trb),
				  &dma, GFP_KERNEL);
	if (!trbs)
		return NULL;

	seg = &st->segments[st->seg_alloc_idx];
	st->seg_trb_va[st->seg_alloc_idx] = trbs;
	st->seg_trb_dma[st->seg_alloc_idx] = dma;
	st->seg_alloc_idx++;

	dev_info(&st->pdev->dev,
		 "DMA alloc: virt=%px dma=0x%llx size=%zu (segment)\n",
		 trbs, (u64)dma,
		 TRBS_PER_SEGMENT * sizeof(struct xhci_trb));

	memset(trbs, 0, TRBS_PER_SEGMENT * sizeof(struct xhci_trb));
	seg->trbs = trbs;
	seg->dma  = dma;
	seg->next = NULL;
	return seg;
}

static void xhci_link_segment(struct xhci_segment *seg,
			       struct xhci_segment *next,
			       bool toggle_cycle)
{
	struct xhci_trb *link = &seg->trbs[TRBS_PER_SEGMENT - 1];

	memset(link, 0, sizeof(*link));
	link->field[0] = lower_32_bits(next->dma);
	link->field[1] = upper_32_bits(next->dma);
	link->field[3] = (TRB_TYPE_LINK << 10) | TRB_CYCLE |
			 (toggle_cycle ? TRB_TC : 0);
}

static int xhci_ring_init(struct probe_state *st, struct xhci_ring *ring,
			   unsigned int num_segs, enum xhci_ring_type type)
{
	struct xhci_segment *first = NULL, *prev = NULL, *cur;
	unsigned int i;

	ring->num_segs    = num_segs;
	ring->type        = type;
	ring->cycle_state = 1;
	ring->first_seg   = NULL;
	ring->last_seg    = NULL;
	ring->enq_seg     = NULL;
	ring->enqueue     = NULL;

	if (num_segs == 0)
		return 0;

	for (i = 0; i < num_segs; i++) {
		struct xhci_segment *seg = segment_alloc(st);

		if (!seg)
			return -ENOMEM;
		if (!first)
			first = seg;
		if (prev)
			prev->next = seg;
		prev = seg;
	}
	prev->next = first;

	ring->first_seg = first;
	ring->last_seg  = prev;
	ring->enq_seg   = first;
	ring->enqueue   = first->trbs;

	cur = first;
	for (i = 0; i < num_segs; i++) {
		bool toggle = (cur == ring->last_seg);

		xhci_link_segment(cur, cur->next, toggle);
		cur = cur->next;
	}
	return 0;
}

static void xhci_ring_enqueue_trb(struct xhci_ring *ring, const struct xhci_trb *src)
{
	struct xhci_trb *trb = ring->enqueue;

	memcpy(trb, src, sizeof(*trb));
	if (ring->cycle_state)
		trb->field[3] |= TRB_CYCLE;
	else
		trb->field[3] &= ~TRB_CYCLE;

	/* Advance enqueue pointer */
	if (trb == &ring->enq_seg->trbs[TRBS_PER_SEGMENT - 2]) {
		/* Next is link TRB slot, move to next segment */
		ring->enq_seg = ring->enq_seg->next;
		ring->enqueue = ring->enq_seg->trbs;
		ring->cycle_state ^= 1;
	} else {
		ring->enqueue = trb + 1;
	}
}

/* -----------------------------------------------------------------------
 * Event ring handling
 * ----------------------------------------------------------------------- */
static void advance_event_dequeue(struct probe_state *st)
{
	if (st->event_dequeue == &st->event_deq_seg->trbs[TRBS_PER_SEGMENT - 1]) {
		st->event_deq_seg = st->event_deq_seg->next;
		st->event_dequeue = st->event_deq_seg->trbs;
		st->event_cycle ^= 1;
	} else {
		st->event_dequeue++;
	}
}

static void ack_event(struct probe_state *st)
{
	dma_addr_t erdp;

	/* Calculate DMA address of new dequeue pointer */
	/* event_dequeue points into event_deq_seg->trbs. Offset: */
	ptrdiff_t off = st->event_dequeue - st->event_deq_seg->trbs;

	erdp = st->event_deq_seg->dma + off * sizeof(struct xhci_trb);
	/* Write ERDP with EHB (bit 3) set to clear Event Handler Busy */
	ir0_write64(st, ERDP_OFF, erdp | (1ULL << 3));
	/* Clear IMAN.IP (W1C bit 0, preserve IE bit 1) */
	ir0_write32(st, IMAN_OFF, ir0_read32(st, IMAN_OFF) | 0x1);
	/* Clear USBSTS.EINT (W1C bit 3) */
	op_write32(st, USBSTS_OFF, op_read32(st, USBSTS_OFF) | (1u << 3));
}

static int xhci_wait_for_event(struct probe_state *st, struct xhci_trb *out,
				u32 expected_type)
{
	unsigned int timeout = 2000000;

	while (timeout--) {
		struct xhci_trb trb = *st->event_dequeue;
		u32 cycle = trb.field[3] & TRB_CYCLE;

		if ((cycle ? 1u : 0u) == st->event_cycle) {
			u32 trb_type;

			advance_event_dequeue(st);
			ack_event(st);

			trb_type = (trb.field[3] >> 10) & 0x3f;
			if (expected_type == 0 || trb_type == expected_type) {
				*out = trb;
				return 0;
			}
			/* Unexpected type — log and continue */
			dev_dbg(&st->pdev->dev,
				"skip event type=%u (expected %u)\n",
				trb_type, expected_type);
		}
		cpu_relax();
	}
	return -ETIMEDOUT;
}

static int xhci_wait_for_event_ms(struct probe_state *st, struct xhci_trb *out,
				   u32 expected_type, unsigned long timeout_ms)
{
	unsigned long deadline = jiffies + msecs_to_jiffies(timeout_ms);

	while (time_before(jiffies, deadline)) {
		struct xhci_trb trb = *st->event_dequeue;
		u32 cycle = trb.field[3] & TRB_CYCLE;

		if ((cycle ? 1u : 0u) == st->event_cycle) {
			u32 trb_type;

			advance_event_dequeue(st);
			ack_event(st);

			trb_type = (trb.field[3] >> 10) & 0x3f;
			if (expected_type == 0 || trb_type == expected_type) {
				*out = trb;
				return 0;
			}
		}
		usleep_range(100, 500);
	}
	return -ETIMEDOUT;
}

/* -----------------------------------------------------------------------
 * Command submission
 * ----------------------------------------------------------------------- */
static int xhci_cmd(struct probe_state *st, struct xhci_trb *cmd_trb,
		     struct xhci_trb *ev_out)
{
	xhci_ring_enqueue_trb(&st->cmd_ring, cmd_trb);
	ring_doorbell(st, 0, 0);
	return xhci_wait_for_event(st, ev_out, TRB_TYPE_COMMAND_COMPLETION);
}

/* -----------------------------------------------------------------------
 * Control transfers (EP0)
 * ----------------------------------------------------------------------- */
static int control_transfer(struct probe_state *st, u8 slot_id,
			    struct xhci_ring *ring,
			    const struct usb_setup_packet *setup,
			    dma_addr_t data_dma, u16 data_len, bool dir_in)
{
	struct xhci_trb trb, ev;
	u64 setup_data = 0;
	u32 trt;

	/* Setup Stage (IDT) */
	memset(&trb, 0, sizeof(trb));
	memcpy(&setup_data, setup, sizeof(*setup));
	trb.field[0] = lower_32_bits(setup_data);
	trb.field[1] = upper_32_bits(setup_data);
	trb.field[2] = 8;
	if (data_len == 0)
		trt = 0;
	else if (dir_in)
		trt = 3;
	else
		trt = 2;
	trb.field[3] = (TRB_TYPE_SETUP << 10) | TRB_IDT | (trt << TRB_TRT_SHIFT);
	xhci_ring_enqueue_trb(ring, &trb);

	/* Data Stage (if any) */
	if (data_len > 0) {
		memset(&trb, 0, sizeof(trb));
		trb.field[0] = lower_32_bits(data_dma);
		trb.field[1] = upper_32_bits(data_dma);
		trb.field[2] = data_len;
		trb.field[3] = (TRB_TYPE_DATA << 10) | (dir_in ? TRB_DIR_IN : 0);
		xhci_ring_enqueue_trb(ring, &trb);
	}

	/* Status Stage */
	memset(&trb, 0, sizeof(trb));
	trb.field[3] = (TRB_TYPE_STATUS << 10) | TRB_IOC |
		       (dir_in ? 0 : TRB_DIR_IN);
	xhci_ring_enqueue_trb(ring, &trb);

	ring_doorbell(st, slot_id, 1);

	if (xhci_wait_for_event(st, &ev, TRB_TYPE_TRANSFER_EVENT) != 0)
		return -ETIMEDOUT;

	{
		u32 cc = (ev.field[2] >> 24) & 0xFF;

		if (cc != CC_SUCCESS && cc != CC_SHORT_PACKET) {
			dev_warn(&st->pdev->dev,
				 "ctrl xfer slot=%u CC=%u\n", slot_id, cc);
		}
	}
	return 0;
}

/* -----------------------------------------------------------------------
 * Endpoint setup (Linux-style context programming)
 * ----------------------------------------------------------------------- */
static int xhci_endpoint_init(struct probe_state *st,
			      struct xhci_virt_device *virt_dev,
			      const struct usb_dev_min *udev,
			      const struct host_endpoint *ep)
{
	u8 epn = ep_num(&ep->desc);
	u8 dci = (u8)(epn * 2 + (ep_dir_in(&ep->desc) ? 1 : 0));
	struct xhci_ep_ctx *ep_ctx;
	unsigned int endpoint_type, max_esit, interval, max_packet, max_burst, avg_trb_len;
	struct xhci_ring *ring;

	ep_ctx = get_ep_ctx_in(virt_dev->in_ctx, virt_dev->ctx_size, dci);

	endpoint_type = xhci_get_endpoint_type(ep);
	if (!endpoint_type)
		return -EINVAL;

	max_esit   = usb_ep_max_periodic_payload(udev, ep);
	interval   = xhci_get_endpoint_interval(udev, ep);
	max_packet = ep_maxp(&ep->desc);
	max_burst  = xhci_get_endpoint_max_burst(udev, ep);
	avg_trb_len = max_esit;

	/* Allocate ring from pool */
	if (st->ring_pool_idx >= RING_POOL_COUNT) {
		dev_err(&st->pdev->dev, "ring pool exhausted\n");
		return -ENOMEM;
	}
	ring = &st->ring_pool[st->ring_pool_idx++];
	if (xhci_ring_init(st, ring, 2, TYPE_INTR) != 0)
		return -ENOMEM;
	virt_dev->ep_rings[dci] = ring;

	ep_ctx->ep_info  = EP_MAX_ESIT_PAYLOAD_HI(max_esit) |
			   EP_INTERVAL(interval) |
			   EP_MULT(0);
	ep_ctx->ep_info2 = EP_TYPE(endpoint_type) |
			   MAX_PACKET(max_packet) |
			   MAX_BURST(max_burst) |
			   ERROR_COUNT(3);
	ep_ctx->deq      = ring->first_seg->dma | ring->cycle_state;
	ep_ctx->tx_info  = EP_MAX_ESIT_PAYLOAD_LO(max_esit) |
			   EP_AVG_TRB_LENGTH(avg_trb_len);

	dev_info(&st->pdev->dev,
		 "ep_init slot=%u dci=%u interval=%u maxp=%u burst=%u esit=%u\n",
		 udev->slot_id, dci, interval, max_packet, max_burst, max_esit);
	dev_info(&st->pdev->dev,
		 "Transfer ring slot=%u dci=%u dma=0x%llx\n",
		 udev->slot_id, dci, (u64)ring->first_seg->dma);
	return 0;
}

/* -----------------------------------------------------------------------
 * Ring setup: DCBAA, command ring, event ring, ERST
 * ----------------------------------------------------------------------- */
static int xhci_setup_rings(struct probe_state *st)
{
	int ret;
	u64 crcr;

	/* DCBAA */
	st->dcbaa = dma_alloc_coherent(&st->pdev->dev, 256 * sizeof(u64),
				       &st->dcbaa_dma, GFP_KERNEL);
	if (!st->dcbaa)
		return -ENOMEM;
	memset(st->dcbaa, 0, 256 * sizeof(u64));
	op_write64(st, DCBAAP_OFF, st->dcbaa_dma);
	ms_kv(st, M_DATA_STRUC, "DCBAA: dma=0x%llx written=0x%llx readback=0x%llx",
	      (u64)st->dcbaa_dma, (u64)st->dcbaa_dma,
	      (u64)op_read64(st, DCBAAP_OFF));

	/* Command ring */
	ret = xhci_ring_init(st, &st->cmd_ring, 1, TYPE_COMMAND);
	if (ret)
		return ret;
	crcr = st->cmd_ring.first_seg->dma | 1;
	op_write64(st, CRCR_OFF, crcr);
	ms_kv(st, M_DATA_STRUC, "CMD_RING: dma=0x%llx CRCR_written=0x%llx readback=0x%llx",
	      (u64)st->cmd_ring.first_seg->dma, (u64)crcr,
	      (u64)op_read64(st, CRCR_OFF));

	/* Event ring */
	ret = xhci_ring_init(st, &st->event_ring, 1, TYPE_EVENT);
	if (ret)
		return ret;
	st->event_deq_seg = st->event_ring.first_seg;
	st->event_dequeue = st->event_ring.first_seg->trbs;
	st->event_cycle   = 1;

	/* ERST */
	st->erst = dma_alloc_coherent(&st->pdev->dev,
				      sizeof(struct xhci_erst_entry),
				      &st->erst_dma, GFP_KERNEL);
	if (!st->erst)
		return -ENOMEM;
	st->erst[0].seg_addr = st->event_ring.first_seg->dma;
	st->erst[0].seg_size = TRBS_PER_SEGMENT;
	st->erst[0].rsvd     = 0;

	ms_kv(st, M_DATA_STRUC, "EVT_RING: seg_dma=0x%llx",
	      (u64)st->event_ring.first_seg->dma);
	ms_kv(st, M_DATA_STRUC, "ERST: dma=0x%llx seg_addr=0x%llx seg_size=%u",
	      (u64)st->erst_dma, (u64)st->erst[0].seg_addr,
	      st->erst[0].seg_size);

	ir0_write32(st, IMOD_OFF, 0x000000a0);
	ir0_write32(st, ERSTSZ_OFF, 1);
	/* xHCI spec 4.9.4: ERDP before ERSTBA */
	ir0_write64(st, ERDP_OFF, st->event_ring.first_seg->dma);
	ir0_write64(st, ERSTBA_OFF, st->erst_dma);
	ms_kv(st, M_DATA_STRUC, "ERDP: written=0x%llx readback=0x%llx",
	      (u64)st->event_ring.first_seg->dma,
	      (u64)ir0_read64(st, ERDP_OFF));
	ms_kv(st, M_DATA_STRUC, "ERSTBA: written=0x%llx readback=0x%llx",
	      (u64)st->erst_dma, (u64)ir0_read64(st, ERSTBA_OFF));

	return 0;
}

/* -----------------------------------------------------------------------
 * Slot enable / address device / configure endpoints
 * ----------------------------------------------------------------------- */
static u8 xhci_enable_slot(struct probe_state *st)
{
	struct xhci_trb trb, ev;

	memset(&trb, 0, sizeof(trb));
	trb.field[3] = (TRB_TYPE_ENABLE_SLOT << 10);
	if (xhci_cmd(st, &trb, &ev) != 0)
		return 0;
	return (u8)((ev.field[3] >> TRB_SLOT_ID_SHIFT) & 0xff);
}

static int xhci_address_device(struct probe_state *st,
				struct xhci_virt_device *vdev,
				const struct usb_dev_min *udev)
{
	struct xhci_input_control_ctx *ctrl;
	struct xhci_slot_ctx *slot_ctx;
	struct xhci_ep_ctx *ep0;
	u32 speed_bits;
	struct xhci_trb trb, ev;

	memset(vdev->in_ctx, 0, 4096);

	ctrl = get_input_control_ctx(vdev->in_ctx);
	ctrl->add_flags  = SLOT_FLAG | EP0_FLAG;
	ctrl->drop_flags = 0;

	slot_ctx = get_slot_ctx_in(vdev->in_ctx, vdev->ctx_size);
	switch (udev->speed) {
	case USB_SPEED_SUPER_PLUS: speed_bits = SLOT_SPEED_SSP; break;
	case USB_SPEED_SUPER:      speed_bits = SLOT_SPEED_SS;  break;
	case USB_SPEED_HIGH:       speed_bits = SLOT_SPEED_HS;  break;
	case USB_SPEED_FULL:       speed_bits = SLOT_SPEED_FS;  break;
	case USB_SPEED_LOW:        speed_bits = SLOT_SPEED_LS;  break;
	default:                   speed_bits = SLOT_SPEED_SS;  break;
	}
	slot_ctx->dev_info  = speed_bits | LAST_CTX(1) | (udev->route & 0xfffff);
	slot_ctx->dev_info2 = ROOT_HUB_PORT(udev->portnum);

	ep0 = get_ep_ctx_in(vdev->in_ctx, vdev->ctx_size, 1);
	ep0->ep_info  = 0;
	ep0->ep_info2 = EP_TYPE(CTRL_EP) | MAX_PACKET(512) |
			MAX_BURST(0) | ERROR_COUNT(3);
	ep0->deq      = vdev->ep_rings[1]->first_seg->dma |
			vdev->ep_rings[1]->cycle_state;
	ep0->tx_info  = EP_AVG_TRB_LENGTH(8);

	ms_kv(st, M_ADDR_DEV, "slot=%u port=%u speed=%u",
	      udev->slot_id, udev->portnum, udev->speed);
	ms_dump(st, M_ADDR_DEV, "input_ctx", vdev->in_ctx, vdev->in_ctx_dma, 192);

	memset(&trb, 0, sizeof(trb));
	trb.field[0] = lower_32_bits(vdev->in_ctx_dma);
	trb.field[1] = upper_32_bits(vdev->in_ctx_dma);
	trb.field[3] = (TRB_TYPE_ADDRESS_DEVICE << 10) |
		       ((u32)udev->slot_id << TRB_SLOT_ID_SHIFT);

	ms_trb(st, M_ADDR_DEV, "cmd_trb", &trb);

	if (xhci_cmd(st, &trb, &ev) != 0) {
		ms_fail(st, M_ADDR_DEV, "command failed");
		return -EIO;
	}

	{
		u32 cc = (ev.field[2] >> 24) & 0xFF;

		ms_trb(st, M_ADDR_DEV, "evt_trb", &ev);
		ms_kv(st, M_ADDR_DEV, "CC=%u slot=%u", cc, udev->slot_id);
	}

	ms_dump(st, M_ADDR_DEV, "output_ctx", vdev->out_ctx, vdev->out_ctx_dma, 192);

	return 0;
}

static int xhci_configure_endpoints(struct probe_state *st,
				     struct xhci_virt_device *vdev,
				     const struct usb_dev_min *udev,
				     const struct host_endpoint *eps,
				     unsigned int ep_count)
{
	struct xhci_input_control_ctx *ctrl;
	struct xhci_slot_ctx *slot_ctx, *out_slot;
	u32 add_flags;
	u8 max_dci = 1;
	unsigned int i;
	struct xhci_trb trb, ev;

	memset(vdev->in_ctx, 0, 4096);

	ctrl = get_input_control_ctx(vdev->in_ctx);
	ctrl->drop_flags = 0;
	ctrl->add_flags  = SLOT_FLAG;

	for (i = 0; i < ep_count; i++) {
		u8 dci = (u8)(ep_num(&eps[i].desc) * 2 +
			      (ep_dir_in(&eps[i].desc) ? 1 : 0));
		ctrl->add_flags |= (1u << dci);
		if (dci > max_dci)
			max_dci = dci;
	}
	add_flags = ctrl->add_flags;

	/* Copy slot context from output, zero dev_state */
	slot_ctx = get_slot_ctx_in(vdev->in_ctx, vdev->ctx_size);
	out_slot = get_slot_ctx_out(vdev->out_ctx);
	memcpy(slot_ctx, out_slot, sizeof(*slot_ctx));
	slot_ctx->dev_state = 0;
	slot_ctx->dev_info &= ~(0x1fu << 27);
	slot_ctx->dev_info |= LAST_CTX(max_dci);

	for (i = 0; i < ep_count; i++) {
		int ret = xhci_endpoint_init(st, vdev, udev, &eps[i]);

		if (ret)
			return ret;
	}

	/* --- M8: ENDPOINT_CONFIG --- */
	ms_begin(st, M_EP_CONFIG);
	ms_kv(st, M_EP_CONFIG, "slot=%u ep_count=%u add_flags=0x%x max_dci=%u",
	      udev->slot_id, ep_count, ctrl->add_flags, max_dci);
	ms_dump(st, M_EP_CONFIG, "cfg_ep_in_ctx", vdev->in_ctx, vdev->in_ctx_dma, 256);

	memset(&trb, 0, sizeof(trb));
	trb.field[0] = lower_32_bits(vdev->in_ctx_dma);
	trb.field[1] = upper_32_bits(vdev->in_ctx_dma);
	trb.field[3] = (TRB_TYPE_CONFIGURE_ENDPOINT << 10) |
		       ((u32)udev->slot_id << TRB_SLOT_ID_SHIFT);

	ms_trb(st, M_EP_CONFIG, "cfg_ep_cmd", &trb);

	if (xhci_cmd(st, &trb, &ev) != 0) {
		ms_fail(st, M_EP_CONFIG, "ConfigureEndpoint command timeout");
		return -EIO;
	}

	{
		u32 cc = (ev.field[2] >> 24) & 0xFF;

		ms_kv(st, M_EP_CONFIG, "ConfigureEndpoint slot=%u CC=%u add_flags=0x%x",
		      udev->slot_id, cc, add_flags);
	}

	ms_trb(st, M_EP_CONFIG, "cfg_ep_evt", &ev);
	ms_dump(st, M_EP_CONFIG, "cfg_ep_out_ctx", vdev->out_ctx, vdev->out_ctx_dma, 256);

	/* Linux-style bandwidth dance: Stop Endpoint + re-ConfigureEndpoint */
	ms_kv(st, M_EP_CONFIG, "BW_dance: slot=%u ep_count=%u", udev->slot_id, ep_count);
	for (i = 0; i < ep_count; i++) {
		u8 *rc_ctx = vdev->reconfig_in_ctx;
		u8 dci = (u8)(ep_num(&eps[i].desc) * 2 +
			      (ep_dir_in(&eps[i].desc) ? 1 : 0));
		struct xhci_trb stop_trb, stop_ev;
		struct xhci_input_control_ctx *rctrl;
		struct xhci_slot_ctx *rc_slot;
		unsigned int j;
		struct xhci_trb rc_trb, rc_ev;

		/* Stop Endpoint */
		ms_kv(st, M_EP_CONFIG, "BW_stop: slot=%u dci=%u", udev->slot_id, dci);
		memset(&stop_trb, 0, sizeof(stop_trb));
		stop_trb.field[3] = (TRB_TYPE_STOP_ENDPOINT << 10) |
				    ((u32)udev->slot_id << TRB_SLOT_ID_SHIFT) |
				    ((u32)dci << TRB_EP_ID_SHIFT);
		ms_trb(st, M_EP_CONFIG, "bw_stop_cmd", &stop_trb);
		if (xhci_cmd(st, &stop_trb, &stop_ev) != 0) {
			ms_fail(st, M_EP_CONFIG, "StopEndpoint timeout");
			ms_kv(st, M_EP_CONFIG, "StopEndpoint failed slot=%u dci=%u",
			      udev->slot_id, dci);
			return -EIO;
		}
		ms_trb(st, M_EP_CONFIG, "bw_stop_evt", &stop_ev);
		{
			u32 stop_cc = (stop_ev.field[2] >> 24) & 0xFF;
			ms_kv(st, M_EP_CONFIG, "StopEndpoint slot=%u dci=%u CC=%u",
			      udev->slot_id, dci, stop_cc);
		}

		/* Rebuild input context from output context, re-configure */
		memset(rc_ctx, 0, 4096);
		rctrl = get_input_control_ctx(rc_ctx);
		rctrl->drop_flags = 0;
		rctrl->add_flags  = add_flags;

		rc_slot = get_slot_ctx_in(rc_ctx, vdev->ctx_size);
		out_slot = get_slot_ctx_out(vdev->out_ctx);
		memcpy(rc_slot, out_slot, sizeof(*rc_slot));
		rc_slot->dev_state = 0;
		rc_slot->dev_info &= ~(0x1fu << 27);
		rc_slot->dev_info |= LAST_CTX(max_dci);

		for (j = 0; j < ep_count; j++) {
			u8 ep_dci = (u8)(ep_num(&eps[j].desc) * 2 +
					 (ep_dir_in(&eps[j].desc) ? 1 : 0));
			struct xhci_ep_ctx *rc_ep =
				get_ep_ctx_in(rc_ctx, vdev->ctx_size, ep_dci);
			struct xhci_ep_ctx *out_ep =
				get_ep_ctx_out(vdev->out_ctx, vdev->ctx_size, ep_dci);
			memcpy(rc_ep, out_ep, sizeof(*rc_ep));
			rc_ep->ep_info &= ~0x7u; /* clear EP state bits */
		}

		ms_dump(st, M_EP_CONFIG, "bw_reconfig_in", rc_ctx,
			vdev->reconfig_in_ctx_dma, 256);

		memset(&rc_trb, 0, sizeof(rc_trb));
		rc_trb.field[0] = lower_32_bits(vdev->reconfig_in_ctx_dma);
		rc_trb.field[1] = upper_32_bits(vdev->reconfig_in_ctx_dma);
		rc_trb.field[3] = (TRB_TYPE_CONFIGURE_ENDPOINT << 10) |
				  ((u32)udev->slot_id << TRB_SLOT_ID_SHIFT);

		ms_trb(st, M_EP_CONFIG, "bw_reconfig_cmd", &rc_trb);

		if (xhci_cmd(st, &rc_trb, &rc_ev) != 0) {
			ms_fail(st, M_EP_CONFIG, "re-ConfigureEndpoint timeout");
			ms_kv(st, M_EP_CONFIG, "re-ConfigureEndpoint failed slot=%u dci=%u",
			      udev->slot_id, dci);
			return -EIO;
		}

		ms_trb(st, M_EP_CONFIG, "bw_reconfig_evt", &rc_ev);
		ms_dump(st, M_EP_CONFIG, "bw_reconfig_out", vdev->out_ctx,
			vdev->out_ctx_dma, 256);

		{
			u32 cc2 = (rc_ev.field[2] >> 24) & 0xFF;

			ms_kv(st, M_EP_CONFIG, "BW_dance slot=%u dci=%u CC=%u",
			      udev->slot_id, dci, cc2);
		}
	}
	ms_pass(st, M_EP_CONFIG);

	return 0;
}

/* -----------------------------------------------------------------------
 * Config descriptor parsing
 * ----------------------------------------------------------------------- */
static unsigned int parse_hid_endpoints(const u8 *buf, unsigned int len,
					struct host_endpoint *out_eps,
					unsigned int max_eps)
{
	unsigned int offset = 0, count = 0;
	u8 cur_iface = 0, cur_subclass = 0, cur_protocol = 0;
	u16 cur_report_len = 0;
	bool in_hid = false;

	while (offset + 2 <= len) {
		u8 dlen  = buf[offset];
		u8 dtype = buf[offset + 1];

		if (dlen == 0)
			break;
		if (offset + dlen > len)
			break;

		if (dtype == USB_DT_INTERFACE &&
		    dlen >= sizeof(struct usb_iface_desc)) {
			const struct usb_iface_desc *ifd =
				(const struct usb_iface_desc *)(buf + offset);
			in_hid        = (ifd->bInterfaceClass == USB_CLASS_HID);
			cur_iface     = ifd->bInterfaceNumber;
			cur_subclass  = ifd->bInterfaceSubClass;
			cur_protocol  = ifd->bInterfaceProtocol;
			cur_report_len = 0;
		} else if (in_hid && dtype == USB_DT_HID &&
			   dlen >= sizeof(struct usb_hid_desc)) {
			const struct usb_hid_desc *hd =
				(const struct usb_hid_desc *)(buf + offset);
			cur_report_len = le16_to_cpu(hd->wDescriptorLength);
		} else if (in_hid && dtype == USB_DT_ENDPOINT &&
			   dlen >= sizeof(struct usb_ep_desc)) {
			const struct usb_ep_desc *epd =
				(const struct usb_ep_desc *)(buf + offset);
			if (ep_is_int(epd) && ep_dir_in(epd) && count < max_eps) {
				memcpy(&out_eps[count].desc, epd, sizeof(*epd));
				memset(&out_eps[count].ss_ep_comp, 0,
				       sizeof(out_eps[count].ss_ep_comp));
				out_eps[count].iface_num     = cur_iface;
				out_eps[count].iface_subclass = cur_subclass;
				out_eps[count].iface_protocol = cur_protocol;
				out_eps[count].report_len    = cur_report_len;

				/* Check for SS companion */
				if (offset + dlen + 2 <= len) {
					u8 ss_len  = buf[offset + dlen];
					u8 ss_type = buf[offset + dlen + 1];

					if (ss_type == USB_DT_SS_EP_COMP &&
					    ss_len >= sizeof(struct usb_ss_ep_comp_desc)) {
						memcpy(&out_eps[count].ss_ep_comp,
						       buf + offset + dlen,
						       sizeof(struct usb_ss_ep_comp_desc));
					}
				}
				count++;
			}
		}
		offset += dlen;
	}
	return count;
}

struct hid_iface_info {
	u8  iface;
	u8  subclass;
	u8  protocol;
	u16 report_len;
};

static unsigned int build_hid_interfaces(const struct host_endpoint *eps,
					 unsigned int ep_count,
					 struct hid_iface_info *out,
					 unsigned int max_ifaces)
{
	unsigned int count = 0, i, j;

	for (i = 0; i < ep_count; i++) {
		bool seen = false;

		for (j = 0; j < count; j++) {
			if (out[j].iface == eps[i].iface_num) {
				seen = true;
				break;
			}
		}
		if (seen)
			continue;
		if (count < max_ifaces) {
			out[count].iface     = eps[i].iface_num;
			out[count].subclass  = eps[i].iface_subclass;
			out[count].protocol  = eps[i].iface_protocol;
			out[count].report_len = eps[i].report_len;
			count++;
		}
	}
	return count;
}

/* -----------------------------------------------------------------------
 * Port enumeration
 * ----------------------------------------------------------------------- */
static bool enumerate_port(struct probe_state *st, u8 port,
			   bool *port_enumerated)
{
	u32 portsc_off = st->op_off + 0x400 + (u32)port * 0x10;
	u32 portsc;
	u8 slot_id;
	struct xhci_virt_device *vdev;
	struct usb_dev_min udev;
	u32 speed_val;
	struct usb_setup_packet setup;
	struct usb_config_desc *cfg;
	u16 total_len;
	struct host_endpoint eps[MAX_HID_EPS];
	unsigned int ep_count;
	u8 config_value;

	portsc = xhci_read32(st, portsc_off);
	if (!(portsc & PORTSC_CCS))
		return false;

	/* --- M5: PORT_DETECTION --- */
	ms_begin(st, M_PORT_DET);
	ms_kv(st, M_PORT_DET, "port=%u PORTSC=0x%08x CCS=%u PED=%u",
	      port + 1, portsc, !!(portsc & PORTSC_CCS),
	      !!(portsc & PORTSC_PED));
	ms_kv(st, M_PORT_DET, "port=%u speed_raw=%u PRC=%u",
	      port + 1,
	      (portsc & PORTSC_SPEED_MASK) >> PORTSC_SPEED_SHIFT,
	      !!(portsc & PORTSC_PRC));

	if (port_enumerated[port]) {
		ms_kv(st, M_PORT_DET, "port=%u already_enumerated=true", port + 1);
		return false;
	}

	/* Reset port if not enabled */
	if (!(portsc & PORTSC_PED)) {
		unsigned int i;

		ms_kv(st, M_PORT_DET, "port=%u resetting (PED=0)", port + 1);
		xhci_write32(st, portsc_off, portsc | PORTSC_PR);
		for (i = 0; i < 100000; i++) {
			portsc = xhci_read32(st, portsc_off);
			if (!(portsc & PORTSC_PR) && (portsc & PORTSC_PED))
				break;
			udelay(10);
		}
		ms_kv(st, M_PORT_DET, "port=%u post_reset PORTSC=0x%08x PED=%u",
		      port + 1, portsc, !!(portsc & PORTSC_PED));
	}
	portsc = xhci_read32(st, portsc_off);
	{
		u8 speed_raw = (portsc & PORTSC_SPEED_MASK) >> PORTSC_SPEED_SHIFT;

		ms_kv(st, M_PORT_DET, "port=%u final PORTSC=0x%08x speed=%u",
		      port + 1, portsc, speed_raw);
	}
	ms_pass(st, M_PORT_DET);

	/* --- M6: SLOT_ENABLE --- */
	ms_begin(st, M_SLOT_EN);
	slot_id = xhci_enable_slot(st);
	if (slot_id == 0 || slot_id > MAX_SLOTS) {
		ms_fail(st, M_SLOT_EN, "EnableSlot returned 0 or out of range");
		ms_kv(st, M_SLOT_EN, "port=%u slot_id=%u", port + 1, slot_id);
		return false;
	}
	ms_kv(st, M_SLOT_EN, "port=%u slot_id=%u", port + 1, slot_id);
	ms_pass(st, M_SLOT_EN);

	vdev = &st->virt_devs[slot_id - 1];
	vdev->slot_id = slot_id;
	vdev->ctx_size = st->ctx_size;

	/* Allocate DMA contexts */
	vdev->in_ctx = dma_alloc_coherent(&st->pdev->dev, 4096,
					  &vdev->in_ctx_dma, GFP_KERNEL);
	vdev->reconfig_in_ctx = dma_alloc_coherent(&st->pdev->dev, 4096,
						   &vdev->reconfig_in_ctx_dma,
						   GFP_KERNEL);
	vdev->out_ctx = dma_alloc_coherent(&st->pdev->dev, 4096,
					   &vdev->out_ctx_dma, GFP_KERNEL);
	if (!vdev->in_ctx || !vdev->reconfig_in_ctx || !vdev->out_ctx) {
		dev_err(&st->pdev->dev, "DMA alloc failed for slot %u\n",
			slot_id);
		return false;
	}
	dev_info(&st->pdev->dev,
		 "DMA alloc: virt=%px dma=0x%llx size=%zu (in_ctx slot=%u)\n",
		 vdev->in_ctx, (u64)vdev->in_ctx_dma, (size_t)4096, slot_id);
	dev_info(&st->pdev->dev,
		 "DMA alloc: virt=%px dma=0x%llx size=%zu (reconfig_in_ctx slot=%u)\n",
		 vdev->reconfig_in_ctx, (u64)vdev->reconfig_in_ctx_dma,
		 (size_t)4096, slot_id);
	dev_info(&st->pdev->dev,
		 "DMA alloc: virt=%px dma=0x%llx size=%zu (out_ctx slot=%u)\n",
		 vdev->out_ctx, (u64)vdev->out_ctx_dma, (size_t)4096, slot_id);
	memset(vdev->in_ctx, 0, 4096);
	memset(vdev->reconfig_in_ctx, 0, 4096);
	memset(vdev->out_ctx, 0, 4096);
	memset(vdev->ep_rings, 0, sizeof(vdev->ep_rings));

	/* EP0 ring */
	if (xhci_ring_init(st, &st->ep0_ring_pool[slot_id - 1], 2, TYPE_CTRL))
		return false;
	vdev->ep_rings[1] = &st->ep0_ring_pool[slot_id - 1];

	/* Point DCBAA to output context */
	st->dcbaa[slot_id] = vdev->out_ctx_dma;

	/* Build USB device info */
	udev.slot_id = slot_id;
	udev.portnum = port + 1;
	udev.route   = 0;
	speed_val = (portsc & PORTSC_SPEED_MASK) >> PORTSC_SPEED_SHIFT;
	switch (speed_val) {
	case 5: udev.speed = USB_SPEED_SUPER_PLUS; break;
	case 4: udev.speed = USB_SPEED_SUPER;      break;
	case 3: udev.speed = USB_SPEED_HIGH;       break;
	case 2: udev.speed = USB_SPEED_FULL;       break;
	case 1: udev.speed = USB_SPEED_LOW;        break;
	default: udev.speed = USB_SPEED_SUPER;     break;
	}
	ms_kv(st, M_ADDR_DEV, "port=%u speed=%u slot=%u",
	      port + 1, udev.speed, slot_id);

	/* --- M7: DEVICE_ADDRESS --- */
	ms_begin(st, M_ADDR_DEV);
	if (xhci_address_device(st, vdev, &udev) != 0)
		return false;
	ms_pass(st, M_ADDR_DEV);

	/* GET CONFIG descriptor header (9 bytes) */
	memset(&setup, 0, sizeof(setup));
	setup.bmRequestType = 0x80;
	setup.bRequest      = 0x06;
	setup.wValue        = cpu_to_le16(0x0200);
	setup.wIndex        = 0;
	setup.wLength       = cpu_to_le16(9);

	memset(st->ctrl_buf, 0, CTRL_BUF_SIZE);
	if (control_transfer(st, slot_id, vdev->ep_rings[1], &setup,
			     st->ctrl_buf_dma, 9, true) != 0) {
		dev_warn(&st->pdev->dev, "GET_CONFIG header failed slot=%u\n",
			 slot_id);
		return false;
	}

	cfg = (struct usb_config_desc *)st->ctrl_buf;
	total_len = le16_to_cpu(cfg->wTotalLength);
	if (total_len > CTRL_BUF_SIZE)
		total_len = CTRL_BUF_SIZE;

	/* GET full config descriptor */
	setup.wLength = cpu_to_le16(total_len);
	memset(st->ctrl_buf, 0, CTRL_BUF_SIZE);
	if (control_transfer(st, slot_id, vdev->ep_rings[1], &setup,
			     st->ctrl_buf_dma, total_len, true) != 0) {
		dev_warn(&st->pdev->dev, "GET_CONFIG full failed slot=%u\n",
			 slot_id);
		return false;
	}

	ep_count = parse_hid_endpoints(st->ctrl_buf, total_len, eps, MAX_HID_EPS);
	config_value = cfg->bConfigurationValue;

	dev_info(&st->pdev->dev,
		 "Slot %u: config=%u total_len=%u hid_eps=%u\n",
		 slot_id, config_value, total_len, ep_count);

	if (ep_count > 0) {
		struct hid_iface_info hid_info[MAX_HID_EPS];
		unsigned int hid_count, idx;

		if (xhci_configure_endpoints(st, vdev, &udev, eps, ep_count))
			return false;

		/* --- M9: HID_CLASS_SETUP --- */
		ms_begin(st, M_HID_SETUP);

		/* SET_CONFIGURATION */
		{
			struct usb_setup_packet set_cfg = {};

			set_cfg.bmRequestType = 0x00;
			set_cfg.bRequest      = 0x09;
			set_cfg.wValue        = cpu_to_le16(config_value);
			control_transfer(st, slot_id, vdev->ep_rings[1],
					 &set_cfg, 0, 0, false);
			ms_kv(st, M_HID_SETUP, "SET_CONFIGURATION slot=%u config=%u",
			      slot_id, config_value);
			ms_dump(st, M_HID_SETUP, "post_set_config_out", vdev->out_ctx,
				vdev->out_ctx_dma, 256);
		}

		/* HID class setup per interface */
		hid_count = build_hid_interfaces(eps, ep_count, hid_info,
						 MAX_HID_EPS);
		ms_kv(st, M_HID_SETUP, "hid_interfaces=%u slot=%u", hid_count, slot_id);
		for (idx = 0; idx < hid_count; idx++) {
			struct usb_setup_packet pkt = {};

			ms_kv(st, M_HID_SETUP, "iface[%u]: num=%u subclass=%u protocol=%u report_len=%u",
			      idx, hid_info[idx].iface, hid_info[idx].subclass,
			      hid_info[idx].protocol, hid_info[idx].report_len);

			/* SET_INTERFACE (alt 0) */
			pkt.bmRequestType = 0x01;
			pkt.bRequest      = 0x0B;
			pkt.wValue        = 0;
			pkt.wIndex        = cpu_to_le16(hid_info[idx].iface);
			control_transfer(st, slot_id, vdev->ep_rings[1],
					 &pkt, 0, 0, false);
			ms_kv(st, M_HID_SETUP, "SET_INTERFACE slot=%u iface=%u",
			      slot_id, hid_info[idx].iface);

			/* SET_PROTOCOL (boot) for boot-class devices */
			if (hid_info[idx].subclass == 1) {
				memset(&pkt, 0, sizeof(pkt));
				pkt.bmRequestType = 0x21;
				pkt.bRequest      = 0x0B;
				pkt.wIndex        = cpu_to_le16(hid_info[idx].iface);
				control_transfer(st, slot_id, vdev->ep_rings[1],
						 &pkt, 0, 0, false);
				ms_kv(st, M_HID_SETUP, "SET_PROTOCOL (boot) slot=%u iface=%u",
				      slot_id, hid_info[idx].iface);
			}

			/* SET_IDLE */
			memset(&pkt, 0, sizeof(pkt));
			pkt.bmRequestType = 0x21;
			pkt.bRequest      = 0x0A;
			pkt.wIndex        = cpu_to_le16(hid_info[idx].iface);
			control_transfer(st, slot_id, vdev->ep_rings[1],
					 &pkt, 0, 0, false);
			ms_kv(st, M_HID_SETUP, "SET_IDLE slot=%u iface=%u",
			      slot_id, hid_info[idx].iface);

			/* GET HID Report Descriptor */
			if (hid_info[idx].report_len > 0) {
				u16 rlen = hid_info[idx].report_len;

				if (rlen > CTRL_BUF_SIZE)
					rlen = CTRL_BUF_SIZE;
				memset(&pkt, 0, sizeof(pkt));
				pkt.bmRequestType = 0x81;
				pkt.bRequest      = 0x06;
				pkt.wValue        = cpu_to_le16(0x2200);
				pkt.wIndex        = cpu_to_le16(hid_info[idx].iface);
				pkt.wLength       = cpu_to_le16(rlen);
				memset(st->ctrl_buf, 0, CTRL_BUF_SIZE);
				control_transfer(st, slot_id, vdev->ep_rings[1],
						 &pkt, st->ctrl_buf_dma,
						 rlen, true);
			}

			/* Feature reports for mouse-class HID */
			if (hid_info[idx].protocol == 2) {
				u8 fid = 0;

				if (hid_info[idx].iface == 0)
					fid = 0x11;
				else if (hid_info[idx].iface == 1)
					fid = 0x12;
				if (fid) {
					/* GET_REPORT (feature) */
					memset(&pkt, 0, sizeof(pkt));
					pkt.bmRequestType = 0xA1;
					pkt.bRequest      = 0x01;
					pkt.wValue        = cpu_to_le16((0x03 << 8) | fid);
					pkt.wIndex        = cpu_to_le16(hid_info[idx].iface);
					pkt.wLength       = cpu_to_le16(64);
					memset(st->ctrl_buf, 0, CTRL_BUF_SIZE);
					control_transfer(st, slot_id,
							 vdev->ep_rings[1],
							 &pkt, st->ctrl_buf_dma,
							 64, true);

					/* SET_REPORT (feature) */
					memset(&pkt, 0, sizeof(pkt));
					pkt.bmRequestType = 0x21;
					pkt.bRequest      = 0x09;
					pkt.wValue        = cpu_to_le16((0x03 << 8) | fid);
					pkt.wIndex        = cpu_to_le16(hid_info[idx].iface);
					pkt.wLength       = cpu_to_le16(2);
					st->ctrl_buf[0] = fid;
					st->ctrl_buf[1] = fid;
					control_transfer(st, slot_id,
							 vdev->ep_rings[1],
							 &pkt, st->ctrl_buf_dma,
							 2, false);
				}
			}

			/* LED/output report for keyboards */
			if (hid_info[idx].protocol == 1) {
				memset(&pkt, 0, sizeof(pkt));
				pkt.bmRequestType = 0x21;
				pkt.bRequest      = 0x09;
				pkt.wValue        = cpu_to_le16(0x0200);
				pkt.wIndex        = cpu_to_le16(hid_info[idx].iface);
				pkt.wLength       = cpu_to_le16(1);
				st->ctrl_buf[0] = 0;
				control_transfer(st, slot_id, vdev->ep_rings[1],
						 &pkt, st->ctrl_buf_dma,
						 1, false);
			}
		}

		ms_dump(st, M_HID_SETUP, "post_hid_setup_out", vdev->out_ctx,
			vdev->out_ctx_dma, 256);
		ms_pass(st, M_HID_SETUP);

		/* Register interrupt endpoints */
		for (idx = 0; idx < ep_count; idx++) {
			u8 dci;
			struct xhci_ring *ep_ring;

			if (st->intr_count >= MAX_INTR_ENDPOINTS)
				break;
			dci = (u8)(ep_num(&eps[idx].desc) * 2 + 1);
			ep_ring = vdev->ep_rings[dci];
			if (!ep_ring)
				continue;

			st->intr_eps[st->intr_count].slot_id    = slot_id;
			st->intr_eps[st->intr_count].dci        = dci;
			st->intr_eps[st->intr_count].ep_ring    = ep_ring;
			st->intr_eps[st->intr_count].max_packet =
				ep_maxp(&eps[idx].desc);
			ms_kv(st, M_INTR_XFER, "registered intr_ep[%u]: slot=%u dci=%u maxp=%u ring_dma=0x%llx",
			      st->intr_count, slot_id, dci,
			      ep_maxp(&eps[idx].desc),
			      (u64)ep_ring->first_seg->dma);
			st->intr_count++;
		}
	}

	port_enumerated[port] = true;
	return true;
}

/* -----------------------------------------------------------------------
 * MSI interrupt handler
 * ----------------------------------------------------------------------- */
static void requeue_intr_trb(struct probe_state *st, unsigned int idx)
{
	struct intr_ep_queue *info = &st->intr_eps[idx];
	struct xhci_trb trb;
	u32 xfer_len;

	if (!info->ep_ring || !st->intr_bufs[idx])
		return;

	xfer_len = info->max_packet;
	if (xfer_len == 0)
		xfer_len = 64;
	if (xfer_len > INTR_BUF_SIZE)
		xfer_len = INTR_BUF_SIZE;

	memset(st->intr_bufs[idx], 0xDE, INTR_BUF_SIZE);
	memset(&trb, 0, sizeof(trb));
	trb.field[0] = lower_32_bits(st->intr_bufs_dma[idx]);
	trb.field[1] = upper_32_bits(st->intr_bufs_dma[idx]);
	trb.field[2] = xfer_len;
	trb.field[3] = (TRB_TYPE_NORMAL << 10) | TRB_IOC | TRB_ISP;
	xhci_ring_enqueue_trb(info->ep_ring, &trb);
	ring_doorbell(st, info->slot_id, info->dci);
	dev_info(&st->pdev->dev,
		 "Intr TRB: slot=%u dci=%u buf_dma=0x%llx len=%u\n",
		 info->slot_id, info->dci,
		 (u64)st->intr_bufs_dma[idx], xfer_len);
}

static irqreturn_t breenix_xhci_irq(int irq, void *data)
{
	struct probe_state *st = data;
	u32 usbsts;
	int handled = 0;

	/* Check USBSTS.EINT */
	usbsts = op_read32(st, USBSTS_OFF);
	if (!(usbsts & (1u << 3)))
		return IRQ_NONE;

	/* Process all pending events */
	while (1) {
		struct xhci_trb trb = *st->event_dequeue;
		u32 cycle = trb.field[3] & TRB_CYCLE;
		u32 trb_type, cc, slot, ep_id, xfer_len;

		if ((cycle ? 1u : 0u) != st->event_cycle)
			break;

		advance_event_dequeue(st);
		handled++;

		trb_type = (trb.field[3] >> 10) & 0x3f;
		cc       = (trb.field[2] >> 24) & 0xFF;
		slot     = (trb.field[3] >> TRB_SLOT_ID_SHIFT) & 0xFF;
		ep_id    = (trb.field[3] >> TRB_EP_ID_SHIFT) & 0x1F;

		if (trb_type == TRB_TYPE_TRANSFER_EVENT) {
			/* M11: EVENT_DELIVERY — transfer event received */
			dev_info(&st->pdev->dev,
				 "[M%d] xfer_event: %08x %08x %08x %08x\n",
				 M_EVT_DELIV, trb.field[0], trb.field[1],
				 trb.field[2], trb.field[3]);
			xfer_len = trb.field[2] & 0xFFFFFF;

			if (cc == CC_SUCCESS || cc == CC_SHORT_PACKET) {
				unsigned int i;

				/* Find matching intr endpoint and print HID data */
				for (i = 0; i < st->intr_count; i++) {
					if (st->intr_eps[i].slot_id == slot &&
					    st->intr_eps[i].dci == ep_id) {
						u32 actual = st->intr_eps[i].max_packet - xfer_len;

						if (actual > 0 && actual <= INTR_BUF_SIZE) {
							dev_info(&st->pdev->dev,
								 "[M%d] HID slot=%u ep=%u CC=%u len=%u: %*ph\n",
								 M_EVT_DELIV,
								 slot, ep_id, cc, actual,
								 min_t(u32, actual, 32),
								 st->intr_bufs[i]);
						}
						/* Requeue for next event */
						requeue_intr_trb(st, i);
						break;
					}
				}
			} else {
				dev_warn(&st->pdev->dev,
					 "[M%d] Transfer event FAIL slot=%u ep=%u CC=%u\n",
					 M_EVT_DELIV, slot, ep_id, cc);
			}
		} else if (trb_type == TRB_TYPE_COMMAND_COMPLETION) {
			dev_dbg(&st->pdev->dev,
				"Command completion CC=%u slot=%u\n", cc, slot);
		} else {
			dev_dbg(&st->pdev->dev,
				"Event type=%u CC=%u slot=%u ep=%u\n",
				trb_type, cc, slot, ep_id);
		}
	}

	if (handled) {
		ack_event(st);
		return IRQ_HANDLED;
	}
	return IRQ_NONE;
}

/* -----------------------------------------------------------------------
 * Controller init
 * ----------------------------------------------------------------------- */
static int xhci_init_controller(struct probe_state *st)
{
	u32 usbcmd, usbsts;
	unsigned int i;
	int ret;

	/* Pre-HCRST register dump: capture controller state before halt+reset */
	{
		u32 pre_usbcmd = op_read32(st, USBCMD_OFF);
		u32 pre_usbsts = op_read32(st, USBSTS_OFF);
		u64 pre_dcbaap = op_read64(st, DCBAAP_OFF);
		u64 pre_crcr   = op_read64(st, CRCR_OFF);
		u32 pre_config = op_read32(st, CONFIG_OFF);

		dev_info(&st->pdev->dev, "pre-HCRST state: USBCMD=0x%08x USBSTS=0x%08x DCBAAP=0x%016llx CRCR=0x%016llx CONFIG=0x%08x\n",
			 pre_usbcmd, pre_usbsts, (u64)pre_dcbaap, (u64)pre_crcr, pre_config);
		dev_info(&st->pdev->dev, "pre-HCRST: RS=%u HCH=%u INTE=%u CNR=%u\n",
			 pre_usbcmd & 1, pre_usbsts & 1, (pre_usbcmd >> 2) & 1, (pre_usbsts >> 11) & 1);
	}

	/* Halt controller if running */
	usbcmd = op_read32(st, USBCMD_OFF);
	if (usbcmd & 1u) {
		op_write32(st, USBCMD_OFF, usbcmd & ~1u);
		for (i = 0; i < 100000; i++) {
			if (op_read32(st, USBSTS_OFF) & 1u)
				break;
			udelay(1);
		}
	}

	/* Reset — enable MMIO write tracing from HCRST onward */
	mmio_trace_idx = 0;
	mmio_trace_seq = 0;
	mmio_trace_active = true;
	op_write32(st, USBCMD_OFF, op_read32(st, USBCMD_OFF) | 2u);
	for (i = 0; i < 100000; i++) {
		if (!(op_read32(st, USBCMD_OFF) & 2u))
			break;
		udelay(1);
	}
	/* Wait for CNR to clear */
	for (i = 0; i < 100000; i++) {
		usbsts = op_read32(st, USBSTS_OFF);
		if (!(usbsts & (1u << 11)))
			break;
		udelay(1);
	}

	/* --- M2: CONTROLLER_RESET --- */
	ms_begin(st, M_RESET);
	usbsts = op_read32(st, USBSTS_OFF);
	ms_kv(st, M_RESET, "USBSTS=0x%08x HCH=%u CNR=%u",
	      usbsts, !!(usbsts & 1u), !!(usbsts & (1u << 11)));
	ms_regs(st, M_RESET);
	if ((usbsts & 1u) && !(usbsts & (1u << 11)))
		ms_pass(st, M_RESET);
	else
		ms_fail(st, M_RESET, "HCH or CNR unexpected");

	/* MaxSlotsEn */
	op_write32(st, CONFIG_OFF, st->max_slots);
	op_write32(st, DNCTRL_OFF, 0x02);

	/* --- M3: DATA_STRUCTURES --- */
	ms_begin(st, M_DATA_STRUC);
	ret = xhci_setup_rings(st);
	if (ret)
		return ret;
	ms_regs(st, M_DATA_STRUC);
	ms_pass(st, M_DATA_STRUC);

	/* Enable interrupter 0 */
	ir0_write32(st, IMAN_OFF, ir0_read32(st, IMAN_OFF) | 2u);

	/* Run + INTE */
	usbcmd = op_read32(st, USBCMD_OFF);
	op_write32(st, USBCMD_OFF, usbcmd | 1u | (1u << 2));

	/* --- M4: CONTROLLER_RUNNING --- */
	ms_begin(st, M_RUNNING);
	{
		u32 cmd = op_read32(st, USBCMD_OFF);
		u32 sts = op_read32(st, USBSTS_OFF);
		u32 iman = ir0_read32(st, IMAN_OFF);

		ms_kv(st, M_RUNNING, "USBCMD=0x%08x RS=%u INTE=%u",
		      cmd, !!(cmd & 1u), !!(cmd & (1u << 2)));
		ms_kv(st, M_RUNNING, "USBSTS=0x%08x HCH=%u", sts, !!(sts & 1u));
		ms_kv(st, M_RUNNING, "IMAN=0x%08x IE=%u", iman, !!(iman & 2u));
		ms_regs(st, M_RUNNING);
		if ((cmd & 1u) && (cmd & (1u << 2)) && (iman & 2u))
			ms_pass(st, M_RUNNING);
		else
			ms_fail(st, M_RUNNING, "RS/INTE/IE not set");
	}
	return 0;
}

/* -----------------------------------------------------------------------
 * PCI probe / remove
 * ----------------------------------------------------------------------- */
static int breenix_xhci_probe(struct pci_dev *pdev,
			       const struct pci_device_id *id)
{
	struct probe_state *st;
	u32 cap_word;
	u8 cap_length;
	u32 hcsparams1;
	bool port_enumerated[MAX_PORTS] = {};
	unsigned int i;
	int ret;

	dev_info(&pdev->dev, "breenix_xhci_probe: claiming device\n");

	/* DMA address diagnostics */
	{
		void *test_ptr = kmalloc(64, GFP_KERNEL);

		if (test_ptr) {
			dev_info(&pdev->dev,
				 "virt_to_phys test: kmalloc ptr %px -> phys 0x%llx\n",
				 test_ptr, (u64)virt_to_phys(test_ptr));
			kfree(test_ptr);
		}
	}

	dev_info(&pdev->dev, "BAR0 phys=0x%llx len=0x%lx\n",
		 (u64)pci_resource_start(pdev, 0),
		 (unsigned long)pci_resource_len(pdev, 0));

#if LINUX_VERSION_CODE >= KERNEL_VERSION(5, 5, 0)
	dev_info(&pdev->dev, "IOMMU present: %s\n",
		 device_iommu_mapped(&pdev->dev) ? "YES" : "NO");
#else
	dev_info(&pdev->dev, "IOMMU present: %s (iommu_group check)\n",
		 pdev->dev.iommu_group ? "YES" : "NO");
#endif

	st = kzalloc(sizeof(*st), GFP_KERNEL);
	if (!st)
		return -ENOMEM;
	st->pdev = pdev;
	pci_set_drvdata(pdev, st);

	ret = pci_enable_device(pdev);
	if (ret) {
		dev_err(&pdev->dev, "pci_enable_device failed: %d\n", ret);
		goto err_free;
	}

	pci_set_master(pdev);

	/* Dump PCI config space after pci_enable_device + pci_set_master */
	ms_pci_config(st, M_DISCOVERY, "pci_enabled");

	ret = dma_set_mask_and_coherent(&pdev->dev, DMA_BIT_MASK(64));
	if (ret) {
		ret = dma_set_mask_and_coherent(&pdev->dev, DMA_BIT_MASK(32));
		if (ret) {
			dev_err(&pdev->dev, "DMA mask setup failed\n");
			goto err_disable;
		}
	}

	st->bar_len = pci_resource_len(pdev, 0);
	st->bar = pci_iomap(pdev, 0, st->bar_len);
	if (!st->bar) {
		dev_err(&pdev->dev, "pci_iomap BAR0 failed\n");
		ret = -EIO;
		goto err_disable;
	}

	/* Read capability registers */
	cap_word   = readl(st->bar);
	cap_length = cap_word & 0xFF;
	st->hci_version = (cap_word >> 16) & 0xFFFF;
	st->op_off = cap_length;

	hcsparams1 = readl(st->bar + 0x04);
	st->max_slots = hcsparams1 & 0xFF;
	st->max_ports = (hcsparams1 >> 24) & 0xFF;

	/* Context size: 64 if HCCPARAMS1 bit 2 set, else 32 */
	{
		u32 hccparams1 = readl(st->bar + 0x10);

		st->ctx_size = (hccparams1 & (1u << 2)) ? 64 : 32;
	}

	/* Runtime base: RTSOFF at cap_base + 0x18 */
	st->rt_off = readl(st->bar + 0x18) & ~0x1Fu;
	/* Doorbell base: DBOFF at cap_base + 0x14 */
	st->db_off = readl(st->bar + 0x14) & ~0x3u;

	/* --- M1: CONTROLLER_DISCOVERY --- */
	ms_begin(st, M_DISCOVERY);
	ms_kv(st, M_DISCOVERY, "BAR0_phys=0x%llx BAR0_len=0x%lx",
	      (u64)pci_resource_start(pdev, 0),
	      (unsigned long)pci_resource_len(pdev, 0));
	ms_kv(st, M_DISCOVERY, "xHCI_version=0x%04x", st->hci_version);
	ms_kv(st, M_DISCOVERY, "max_slots=%u max_ports=%u ctx_size=%u",
	      st->max_slots, st->max_ports, st->ctx_size);
	ms_kv(st, M_DISCOVERY, "cap_len=%u op_off=0x%x rt_off=0x%x db_off=0x%x",
	      cap_length, st->op_off, st->rt_off, st->db_off);
	ms_kv(st, M_DISCOVERY, "HCSPARAMS1=0x%08x HCCPARAMS1=0x%08x",
	      hcsparams1, readl(st->bar + 0x10));
	ms_pass(st, M_DISCOVERY);

	/* Allocate control buffer */
	st->ctrl_buf = dma_alloc_coherent(&pdev->dev, CTRL_BUF_SIZE,
					  &st->ctrl_buf_dma, GFP_KERNEL);
	if (!st->ctrl_buf) {
		ret = -ENOMEM;
		goto err_unmap;
	}
	ms_kv(st, M_DATA_STRUC, "ctrl_buf: virt=%px dma=0x%llx size=%u",
	      st->ctrl_buf, (u64)st->ctrl_buf_dma, CTRL_BUF_SIZE);

	/* Initialize controller (M2, M3, M4 happen inside) */
	ret = xhci_init_controller(st);
	if (ret) {
		dev_err(&pdev->dev, "xHCI init failed: %d\n", ret);
		goto err_free_ctrl;
	}

	/* Enumerate all ports */
	for (i = 0; i < st->max_ports; i++)
		enumerate_port(st, i, port_enumerated);

	/* Wait for late connections, re-scan */
	msleep(2000);
	for (i = 0; i < st->max_ports; i++)
		enumerate_port(st, i, port_enumerated);

	dev_info(&pdev->dev, "Enumeration complete: %u interrupt endpoints\n",
		 st->intr_count);

	/* --- M10: INTERRUPT_TRANSFER --- */
	ms_begin(st, M_INTR_XFER);
	ms_kv(st, M_INTR_XFER, "total_intr_eps=%u", st->intr_count);

	/* Allocate interrupt transfer buffers and queue TRBs */
	for (i = 0; i < st->intr_count; i++) {
		struct intr_ep_queue *info = &st->intr_eps[i];
		struct xhci_trb trb;
		u32 xfer_len;

		st->intr_bufs[i] = dma_alloc_coherent(&pdev->dev, INTR_BUF_SIZE,
						       &st->intr_bufs_dma[i],
						       GFP_KERNEL);
		if (!st->intr_bufs[i])
			continue;
		ms_kv(st, M_INTR_XFER, "intr_buf[%u]: virt=%px dma=0x%llx size=%u",
		      i, st->intr_bufs[i], (u64)st->intr_bufs_dma[i],
		      INTR_BUF_SIZE);

		xfer_len = info->max_packet;
		if (xfer_len == 0)
			xfer_len = 64;
		if (xfer_len > INTR_BUF_SIZE)
			xfer_len = INTR_BUF_SIZE;

		memset(st->intr_bufs[i], 0xDE, INTR_BUF_SIZE);

		memset(&trb, 0, sizeof(trb));
		trb.field[0] = lower_32_bits(st->intr_bufs_dma[i]);
		trb.field[1] = upper_32_bits(st->intr_bufs_dma[i]);
		trb.field[2] = xfer_len;
		trb.field[3] = (TRB_TYPE_NORMAL << 10) | TRB_IOC | TRB_ISP;

		ms_trb(st, M_INTR_XFER, "intr_trb", &trb);
		xhci_ring_enqueue_trb(info->ep_ring, &trb);
		ring_doorbell(st, info->slot_id, info->dci);
		ms_kv(st, M_INTR_XFER, "queued+doorbell: slot=%u dci=%u buf_dma=0x%llx len=%u",
		      info->slot_id, info->dci,
		      (u64)st->intr_bufs_dma[i], xfer_len);
		{
			struct xhci_virt_device *vd =
				&st->virt_devs[info->slot_id - 1];
			struct xhci_ep_ctx *ep =
				get_ep_ctx_out(vd->out_ctx, vd->ctx_size,
					       info->dci);
			ms_kv(st, M_INTR_XFER, "ep_ctx_out: slot=%u dci=%u ep_info=0x%08x ep_info2=0x%08x deq=0x%016llx state=%u",
			      info->slot_id, info->dci,
			      ep->ep_info, ep->ep_info2, (u64)ep->deq,
			      ep->ep_info & 0x7);
		}
	}

	/* Dump MMIO write trace: HCRST through first doorbell ring */
	mmio_trace_active = false;
	dev_info(&pdev->dev, "MMIO_TRACE: %u writes captured (max %u)\n",
		 mmio_trace_idx, MMIO_TRACE_MAX);
	{
		u32 t;
		for (t = 0; t < mmio_trace_idx; t++)
			dev_info(&pdev->dev, "MMIO_TRACE[%u] @%04x = %08x\n",
				 mmio_trace_buf[t].seq,
				 mmio_trace_buf[t].offset,
				 mmio_trace_buf[t].value);
	}

	ms_regs(st, M_INTR_XFER);
	ms_pass(st, M_INTR_XFER);

	/* Setup MSI interrupt */
	ret = pci_alloc_irq_vectors(pdev, 1, 1, PCI_IRQ_MSI | PCI_IRQ_MSIX |
				    PCI_IRQ_INTX);
	if (ret < 0) {
		dev_err(&pdev->dev, "Failed to allocate IRQ vectors: %d\n", ret);
		goto err_free_ctrl;
	}

	/* Dump PCI config space after pci_alloc_irq_vectors (MSI configured) */
	ms_pci_config(st, M_DISCOVERY, "pci_msi");

	st->irq = pci_irq_vector(pdev, 0);
	ret = request_irq(st->irq, breenix_xhci_irq, IRQF_SHARED,
			  "breenix_xhci_probe", st);
	if (ret) {
		dev_err(&pdev->dev, "request_irq failed: %d\n", ret);
		pci_free_irq_vectors(pdev);
		goto err_free_ctrl;
	}

	dev_info(&pdev->dev,
		 "Probe complete, IRQ=%d. Waiting for HID events...\n",
		 st->irq);

	/* --- M11: EVENT_DELIVERY --- polled check to verify event ring works */
	ms_begin(st, M_EVT_DELIV);
	if (st->intr_count > 0) {
		struct xhci_trb noop, noop_ev, ev;

		memset(&noop, 0, sizeof(noop));
		noop.field[3] = (TRB_TYPE_NOOP << 10);
		if (xhci_cmd(st, &noop, &noop_ev) == 0) {
			ms_kv(st, M_EVT_DELIV, "NOOP command: PASS (command ring alive)");
			ms_trb(st, M_EVT_DELIV, "noop_evt", &noop_ev);
		} else {
			ms_fail(st, M_EVT_DELIV, "NOOP command timeout (command ring dead)");
		}

		/* Quick poll for any transfer events */
		ms_kv(st, M_EVT_DELIV, "polling for transfer event (5s timeout)...");
		if (xhci_wait_for_event_ms(st, &ev, TRB_TYPE_TRANSFER_EVENT,
					   5000) == 0) {
			u32 cc = (ev.field[2] >> 24) & 0xFF;
			u32 slot = (ev.field[3] >> TRB_SLOT_ID_SHIFT) & 0xFF;
			u32 ep_id = (ev.field[3] >> TRB_EP_ID_SHIFT) & 0x1F;

			ms_kv(st, M_EVT_DELIV, "transfer_event: slot=%u ep=%u CC=%u",
			      slot, ep_id, cc);
			ms_trb(st, M_EVT_DELIV, "xfer_evt", &ev);
			ms_pass(st, M_EVT_DELIV);
		} else {
			ms_kv(st, M_EVT_DELIV, "no transfer events in 5s (try pressing a key)");
		}
	} else {
		ms_kv(st, M_EVT_DELIV, "no interrupt endpoints registered, skipping poll");
	}

	/* ----- Final state dump (all under M11) ----- */
	{
		static const char * const ep_state_names[] = {
			"disabled", "running", "halted", "stopped",
			"error", "?5", "?6", "?7"
		};

		ms_regs(st, M_EVT_DELIV);

		/* Dump endpoint context state for each active interrupt EP */
		for (i = 0; i < st->intr_count; i++) {
			struct intr_ep_queue *info = &st->intr_eps[i];
			struct xhci_virt_device *vdev;
			struct xhci_ep_ctx *ep_ctx;
			u32 ep_state;

			if (info->slot_id == 0 || info->slot_id > st->max_slots)
				continue;
			vdev = &st->virt_devs[info->slot_id - 1];
			if (!vdev->out_ctx)
				continue;

			ep_ctx = get_ep_ctx_out(vdev->out_ctx,
						vdev->ctx_size, info->dci);
			ep_state = ep_ctx->ep_info & 0x7;
			ms_kv(st, M_EVT_DELIV, "EP_STATE: slot=%u dci=%u ep_info=0x%08x state=%u(%s) deq=0x%016llx",
			      info->slot_id, info->dci,
			      ep_ctx->ep_info, ep_state,
			      ep_state_names[ep_state],
			      (u64)ep_ctx->deq);
		}

		/* Dump doorbell register for each active slot */
		{
			u8 seen_slots[MAX_SLOTS] = {};

			for (i = 0; i < st->intr_count; i++) {
				u8 sid = st->intr_eps[i].slot_id;
				u32 db_val;

				if (sid == 0 || sid > st->max_slots)
					continue;
				if (seen_slots[sid - 1])
					continue;
				seen_slots[sid - 1] = 1;

				db_val = xhci_read32(st,
						     st->db_off + (u32)sid * 4);
				ms_kv(st, M_EVT_DELIV, "Doorbell[%u]=0x%08x",
				      sid, db_val);
			}
		}

		/* DCBAA dump */
		ms_kv(st, M_EVT_DELIV, "DCBAA entries:");
		for (i = 0; i <= st->max_slots && i <= MAX_SLOTS; i++) {
			if (st->dcbaa[i])
				ms_kv(st, M_EVT_DELIV, "DCBAA[%u]=0x%016llx",
				      i, (u64)st->dcbaa[i]);
		}

		/* Dump first TRB at current event ring dequeue pointer */
		if (st->event_dequeue) {
			ms_trb(st, M_EVT_DELIV, "event_deq_trb", st->event_dequeue);
			ms_kv(st, M_EVT_DELIV, "event_deq type=%u cycle=%u",
			      (st->event_dequeue->field[3] >> 10) & 0x3f,
			      st->event_dequeue->field[3] & 1);
		}
	}

	return 0;

err_free_ctrl:
	if (st->ctrl_buf)
		dma_free_coherent(&pdev->dev, CTRL_BUF_SIZE,
				  st->ctrl_buf, st->ctrl_buf_dma);
err_unmap:
	pci_iounmap(pdev, st->bar);
err_disable:
	pci_disable_device(pdev);
err_free:
	kfree(st);
	return ret;
}

static void breenix_xhci_remove(struct pci_dev *pdev)
{
	struct probe_state *st = pci_get_drvdata(pdev);
	unsigned int i;

	dev_info(&pdev->dev, "breenix_xhci_remove\n");

	/* Free IRQ */
	if (st->irq) {
		free_irq(st->irq, st);
		pci_free_irq_vectors(pdev);
	}

	/* Halt controller */
	op_write32(st, USBCMD_OFF, op_read32(st, USBCMD_OFF) & ~1u);
	msleep(10);

	/* Free interrupt buffers */
	for (i = 0; i < MAX_INTR_ENDPOINTS; i++) {
		if (st->intr_bufs[i])
			dma_free_coherent(&pdev->dev, INTR_BUF_SIZE,
					  st->intr_bufs[i],
					  st->intr_bufs_dma[i]);
	}

	/* Free device contexts */
	for (i = 0; i < MAX_SLOTS; i++) {
		struct xhci_virt_device *vdev = &st->virt_devs[i];

		if (vdev->in_ctx)
			dma_free_coherent(&pdev->dev, 4096,
					  vdev->in_ctx, vdev->in_ctx_dma);
		if (vdev->reconfig_in_ctx)
			dma_free_coherent(&pdev->dev, 4096,
					  vdev->reconfig_in_ctx,
					  vdev->reconfig_in_ctx_dma);
		if (vdev->out_ctx)
			dma_free_coherent(&pdev->dev, 4096,
					  vdev->out_ctx, vdev->out_ctx_dma);
	}

	/* Free segment TRB buffers */
	for (i = 0; i < st->seg_alloc_idx; i++) {
		if (st->seg_trb_va[i])
			dma_free_coherent(&pdev->dev,
					  TRBS_PER_SEGMENT * sizeof(struct xhci_trb),
					  st->seg_trb_va[i],
					  st->seg_trb_dma[i]);
	}

	/* Free ERST */
	if (st->erst)
		dma_free_coherent(&pdev->dev, sizeof(struct xhci_erst_entry),
				  st->erst, st->erst_dma);

	/* Free DCBAA */
	if (st->dcbaa)
		dma_free_coherent(&pdev->dev, 256 * sizeof(u64),
				  st->dcbaa, st->dcbaa_dma);

	/* Free ctrl buffer */
	if (st->ctrl_buf)
		dma_free_coherent(&pdev->dev, CTRL_BUF_SIZE,
				  st->ctrl_buf, st->ctrl_buf_dma);

	pci_iounmap(pdev, st->bar);
	pci_disable_device(pdev);
	kfree(st);
}

/* -----------------------------------------------------------------------
 * PCI device table — match xHCI controllers
 * ----------------------------------------------------------------------- */
static const struct pci_device_id breenix_xhci_ids[] = {
	/* NEC/Renesas uPD720200 (common in Parallels) */
	{ PCI_DEVICE(0x1033, 0x0194) },
	/* Intel xHCI */
	{ PCI_DEVICE_CLASS(PCI_CLASS_SERIAL_USB_XHCI, ~0) },
	{ 0 }
};
MODULE_DEVICE_TABLE(pci, breenix_xhci_ids);

static struct pci_driver breenix_xhci_driver = {
	.name     = "breenix_xhci_probe",
	.id_table = breenix_xhci_ids,
	.probe    = breenix_xhci_probe,
	.remove   = breenix_xhci_remove,
};

module_pci_driver(breenix_xhci_driver);

MODULE_LICENSE("GPL");
MODULE_AUTHOR("Ryan Breen <ryan@breenix.dev>");
MODULE_DESCRIPTION("Standalone xHCI probe for Breenix validation");
