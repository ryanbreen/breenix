/*
 * Copyright 2013 Red Hat Inc.
 *
 * Permission is hereby granted, free of charge, to any person obtaining a
 * copy of this software and associated documentation files (the "Software"),
 * to deal in the Software without restriction, including without limitation
 * the rights to use, copy, modify, merge, publish, distribute, sublicense,
 * and/or sell copies of the Software, and to permit persons to whom the
 * Software is furnished to do so, subject to the following conditions:
 *
 * The above copyright notice and this permission notice shall be included in
 * all copies or substantial portions of the Software.
 *
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
 * IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
 * FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.  IN NO EVENT SHALL
 * THE COPYRIGHT HOLDER(S) OR AUTHOR(S) BE LIABLE FOR ANY CLAIM, DAMAGES OR
 * OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE,
 * ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR
 * OTHER DEALINGS IN THE SOFTWARE.
 *
 * Authors: Ben Skeggs
 */
/*
 * Copyright 2013 Red Hat Inc.
 *
 * Permission is hereby granted, free of charge, to any person obtaining a
 * copy of this software and associated documentation files (the "Software"),
 * to deal in the Software without restriction, including without limitation
 * the rights to use, copy, modify, merge, publish, distribute, sublicense,
 * and/or sell copies of the Software, and to permit persons to whom the
 * Software is furnished to do so, subject to the following conditions:
 *
 * The above copyright notice and this permission notice shall be included in
 * all copies or substantial portions of the Software.
 *
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
 * IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
 * FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.  IN NO EVENT SHALL
 * THE COPYRIGHT HOLDER(S) OR AUTHOR(S) BE LIABLE FOR ANY CLAIM, DAMAGES OR
 * OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE,
 * ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR
 * OTHER DEALINGS IN THE SOFTWARE.
 *
 * Authors: Ben Skeggs <bskeggs@redhat.com>
 * 	    Roy Spliet <rspliet@eclipso.eu>
 */
/* Host oracle adaptation: replace ramfuc/MMIO callbacks with a map and trace.
 * Original calculations below retain #line locations. MIT; upstream source
 * hashes and revision are in docs/gui/provenance/gt710-c5a-sources.json.
 */
#include <stdint.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <errno.h>
#define ARRAY_SIZE(x) (sizeof(x)/sizeof((x)[0]))
typedef uint8_t u8;
typedef uint32_t u32;
struct nvbios_ramcfg {
    u32 timing[11];
    u32 timing_ver;
    u32 timing_hdr;
    u32 timing_10_CWL;
    u32 timing_10_CL;
    u32 timing_10_WR;
    u32 timing_10_ODT;
    u32 ramcfg_DLLoff;
    u32 ramcfg_11_02_08;
    u32 ramcfg_11_02_04;
    u32 ramcfg_11_03_f0;
    u32 ramcfg_11_03_c0;
    u32 ramcfg_11_03_30;
    u32 ramcfg_11_02_80;
    u32 ramcfg_11_02_40;
    u32 ramcfg_11_07_10;
    u32 ramcfg_11_07_08;
    u32 ramcfg_11_07_40;
    u32 rammap_11_11_0c;
    u32 ramcfg_11_09;
    u32 ramcfg_11_08_01;
    u32 ramcfg_11_01_04;
    u32 ramcfg_11_07_80;
    u32 rammap_11_08_0c;
    u32 ramcfg_11_03_0f;
    u32 timing_20_2c_003f;
    u32 timing_20_2c_1fc0;
    u32 timing_20_30_f8;
    u32 rammap_11_08_01;
};
struct nvkm_ram_data { struct nvbios_ramcfg bios; };
struct device { bool disp; };
struct subdev { struct device *device; };
struct fb { struct subdev subdev; };
struct nvkm_ram { struct nvkm_ram_data *next; u32 mr[16]; struct fb *fb; };
struct gk104_ramfuc { u32 r_funcMV[2], r_func2E[2]; };
struct gk104_ram { struct nvkm_ram base; struct gk104_ramfuc fuc; int M1,N1,P1,fN1,mode; };
static u32 addresses[256], registers[256], count;
static bool forced;
static u32 address(const char *s) {
    if (s[0]=='m') { const u32 mr[]={0x10f300,0x10f304,0x10f320}; return mr[s[3]-'0']; }
    return (u32)strtoul(s,NULL,0);
}
static u32 *reg(u32 a) {
    for (u32 i=0;i<count;i++) if (addresses[i]==a) return &registers[i];
    if (count==ARRAY_SIZE(addresses)) abort();
    addresses[count]=a;registers[count]=0;return &registers[count++];
}
static void write_word(u32 a,u32 data) { *reg(a)=data;printf("WR32 %x %x\n",a,data); }
static u32 mask_word(u32 a,u32 mask,u32 data) {
    u32 old=*reg(a), next=(old&~mask)|data;
    if (old!=next || forced) write_word(a,next);
    forced=false;return old;
}
#define ram_rd32(f,r) ((void)(f), *reg(address(#r)))
#define ram_mask(f,r,m,d) ((void)(f), mask_word(address(#r),m,d))
#define ram_wr32(f,r,d) write_word(address(#r),d)
#define ram_nuke(f,r) (forced=true)
#define ram_have(f,r) false
#define ram_nsec(f,n) printf("DELAY %x\n",(u32)(n))
#define ram_wait(f,a,m,d,n) printf("WAIT %x %x %x %x\n",(u32)(a),(u32)(m),(u32)(d),(u32)(n))
#define ram_block(f) puts("ENTER")
#define ram_unblock(f) puts("LEAVE")
#line 28 "fb/sddr3.c"
struct ramxlat {
	int id;
	u8 enc;
};

static inline int
ramxlat(const struct ramxlat *xlat, int id)
{
	while (xlat->id >= 0) {
		if (xlat->id == id)
			return xlat->enc;
		xlat++;
	}
	return -EINVAL;
}

static const struct ramxlat
ramddr3_cl[] = {
	{ 5, 2 }, { 6, 4 }, { 7, 6 }, { 8, 8 }, { 9, 10 }, { 10, 12 },
	{ 11, 14 },
	/* the below are mentioned in some, but not all, ddr3 docs */
	{ 12, 1 }, { 13, 3 }, { 14, 5 },
	{ -1 }
};

static const struct ramxlat
ramddr3_wr[] = {
	{ 5, 1 }, { 6, 2 }, { 7, 3 }, { 8, 4 }, { 10, 5 }, { 12, 6 },
	/* the below are mentioned in some, but not all, ddr3 docs */
	{ 14, 7 }, { 15, 7 }, { 16, 0 },
	{ -1 }
};

static const struct ramxlat
ramddr3_cwl[] = {
	{ 5, 0 }, { 6, 1 }, { 7, 2 }, { 8, 3 },
	/* the below are mentioned in some, but not all, ddr3 docs */
	{ 9, 4 }, { 10, 5 },
	{ -1 }
};

int
nvkm_sddr3_calc(struct nvkm_ram *ram)
{
	int CWL, CL, WR, DLL = 0, ODT = 0;

	DLL = !ram->next->bios.ramcfg_DLLoff;

	switch (ram->next->bios.timing_ver) {
	case 0x10:
		if (ram->next->bios.timing_hdr < 0x17) {
			/* XXX: NV50: Get CWL from the timing register */
			return -ENOSYS;
		}
		CWL = ram->next->bios.timing_10_CWL;
		CL  = ram->next->bios.timing_10_CL;
		WR  = ram->next->bios.timing_10_WR;
		ODT = ram->next->bios.timing_10_ODT;
		break;
	case 0x20:
		CWL = (ram->next->bios.timing[1] & 0x00000f80) >> 7;
		CL  = (ram->next->bios.timing[1] & 0x0000001f) >> 0;
		WR  = (ram->next->bios.timing[2] & 0x007f0000) >> 16;
		/* XXX: Get these values from the VBIOS instead */
		ODT =   (ram->mr[1] & 0x004) >> 2 |
			(ram->mr[1] & 0x040) >> 5 |
		        (ram->mr[1] & 0x200) >> 7;
		break;
	default:
		return -ENOSYS;
	}

	CWL = ramxlat(ramddr3_cwl, CWL);
	CL  = ramxlat(ramddr3_cl, CL);
	WR  = ramxlat(ramddr3_wr, WR);
	if (CL < 0 || CWL < 0 || WR < 0)
		return -EINVAL;

	ram->mr[0] &= ~0xf74;
	ram->mr[0] |= (WR & 0x07) << 9;
	ram->mr[0] |= (CL & 0x0e) << 3;
	ram->mr[0] |= (CL & 0x01) << 2;

	ram->mr[1] &= ~0x245;
	ram->mr[1] |= (ODT & 0x1) << 2;
	ram->mr[1] |= (ODT & 0x2) << 5;
	ram->mr[1] |= (ODT & 0x4) << 7;
	ram->mr[1] |= !DLL;

	ram->mr[2] &= ~0x038;
	ram->mr[2] |= (CWL & 0x07) << 3;
	return 0;
}
#line 680 "fb/ramgk104.c"
static void
nvkm_sddr3_dll_reset(struct gk104_ramfuc *fuc)
{
	ram_nuke(fuc, mr[0]);
	ram_mask(fuc, mr[0], 0x100, 0x100);
	ram_mask(fuc, mr[0], 0x100, 0x000);
}

static void
nvkm_sddr3_dll_disable(struct gk104_ramfuc *fuc)
{
	u32 mr1_old = ram_rd32(fuc, mr[1]);

	if (!(mr1_old & 0x1)) {
		ram_mask(fuc, mr[1], 0x1, 0x1);
		ram_nsec(fuc, 1000);
	}
}

static int
gk104_ram_calc_sddr3(struct gk104_ram *ram, u32 freq)
{
	struct gk104_ramfuc *fuc = &ram->fuc;
	const u32 rcoef = ((  ram->P1 << 16) | (ram->N1 << 8) | ram->M1);
	const u32 runk0 = ram->fN1 << 16;
	const u32 runk1 = ram->fN1;
	struct nvkm_ram_data *next = ram->base.next;
	int vc = !next->bios.ramcfg_11_02_08;
	int mv = !next->bios.ramcfg_11_02_04;
	u32 mask, data;

	ram_mask(fuc, 0x10f808, 0x40000000, 0x40000000);
	ram_block(fuc);

	if (ram->base.fb->subdev.device->disp)
		ram_wr32(fuc, 0x62c000, 0x0f0f0000);

	if (vc == 1 && ram_have(fuc, gpio2E)) {
		u32 temp  = ram_mask(fuc, gpio2E, 0x3000, fuc->r_func2E[1]);
		if (temp != ram_rd32(fuc, gpio2E)) {
			ram_wr32(fuc, gpiotrig, 1);
			ram_nsec(fuc, 20000);
		}
	}

	ram_mask(fuc, 0x10f200, 0x00000800, 0x00000000);
	if (next->bios.ramcfg_11_03_f0)
		ram_mask(fuc, 0x10f808, 0x04000000, 0x04000000);

	ram_wr32(fuc, 0x10f314, 0x00000001); /* PRECHARGE */

	if (next->bios.ramcfg_DLLoff)
		nvkm_sddr3_dll_disable(fuc);

	ram_wr32(fuc, 0x10f210, 0x00000000); /* REFRESH_AUTO = 0 */
	ram_wr32(fuc, 0x10f310, 0x00000001); /* REFRESH */
	ram_mask(fuc, 0x10f200, 0x80000000, 0x80000000);
	ram_wr32(fuc, 0x10f310, 0x00000001); /* REFRESH */
	ram_mask(fuc, 0x10f200, 0x80000000, 0x00000000);
	ram_nsec(fuc, 1000);

	ram_wr32(fuc, 0x10f090, 0x00000060);
	ram_wr32(fuc, 0x10f090, 0xc000007e);

	/*XXX: there does appear to be some kind of condition here, simply
	 *     modifying these bits in the vbios from the default pl0
	 *     entries shows no change.  however, the data does appear to
	 *     be correct and may be required for the transition back
	 */
	mask = 0x00010000;
	data = 0x00010000;

	if (1) {
		mask |= 0x800807e0;
		data |= 0x800807e0;
		switch (next->bios.ramcfg_11_03_c0) {
		case 3: data &= ~0x00000040; break;
		case 2: data &= ~0x00000100; break;
		case 1: data &= ~0x80000000; break;
		case 0: data &= ~0x00000400; break;
		}

		switch (next->bios.ramcfg_11_03_30) {
		case 3: data &= ~0x00000020; break;
		case 2: data &= ~0x00000080; break;
		case 1: data &= ~0x00080000; break;
		case 0: data &= ~0x00000200; break;
		}
	}

	if (next->bios.ramcfg_11_02_80)
		mask |= 0x03000000;
	if (next->bios.ramcfg_11_02_40)
		mask |= 0x00002000;
	if (next->bios.ramcfg_11_07_10)
		mask |= 0x00004000;
	if (next->bios.ramcfg_11_07_08)
		mask |= 0x00000003;
	else
		mask |= 0x14000000;
	ram_mask(fuc, 0x10f824, mask, data);

	ram_mask(fuc, 0x132040, 0x00010000, 0x00000000);

	ram_mask(fuc, 0x1373f4, 0x00000000, 0x00010010);
	data  = ram_rd32(fuc, 0x1373ec) & ~0x00030000;
	data |= next->bios.ramcfg_11_03_30 << 16;
	ram_wr32(fuc, 0x1373ec, data);
	ram_mask(fuc, 0x1373f4, 0x00000003, 0x00000000);
	ram_mask(fuc, 0x1373f4, 0x00000010, 0x00000000);

	/* (re)program refpll, if required */
	if ((ram_rd32(fuc, 0x132024) & 0xffffffff) != rcoef ||
	    (ram_rd32(fuc, 0x132034) & 0x0000ffff) != runk1) {
		ram_mask(fuc, 0x132000, 0x00000001, 0x00000000);
		ram_mask(fuc, 0x132020, 0x00000001, 0x00000000);
		ram_wr32(fuc, 0x137320, 0x00000000);
		ram_mask(fuc, 0x132030, 0xffff0000, runk0);
		ram_mask(fuc, 0x132034, 0x0000ffff, runk1);
		ram_wr32(fuc, 0x132024, rcoef);
		ram_mask(fuc, 0x132028, 0x00080000, 0x00080000);
		ram_mask(fuc, 0x132020, 0x00000001, 0x00000001);
		ram_wait(fuc, 0x137390, 0x00020000, 0x00020000, 64000);
		ram_mask(fuc, 0x132028, 0x00080000, 0x00000000);
	}

	ram_mask(fuc, 0x1373f4, 0x00000010, 0x00000010);
	ram_mask(fuc, 0x1373f4, 0x00000003, 0x00000001);
	ram_mask(fuc, 0x1373f4, 0x00010000, 0x00000000);

	if (ram_have(fuc, gpioMV)) {
		u32 temp  = ram_mask(fuc, gpioMV, 0x3000, fuc->r_funcMV[mv]);
		if (temp != ram_rd32(fuc, gpioMV)) {
			ram_wr32(fuc, gpiotrig, 1);
			ram_nsec(fuc, 64000);
		}
	}

	if (next->bios.ramcfg_11_02_40 ||
	    next->bios.ramcfg_11_07_10) {
		ram_mask(fuc, 0x132040, 0x00010000, 0x00010000);
		ram_nsec(fuc, 20000);
	}

	if (ram->mode != 2) /*XXX*/ {
		if (next->bios.ramcfg_11_07_40)
			ram_mask(fuc, 0x10f670, 0x80000000, 0x80000000);
	}

	ram_wr32(fuc, 0x10f65c, 0x00000011 * next->bios.rammap_11_11_0c);
	ram_wr32(fuc, 0x10f6b8, 0x01010101 * next->bios.ramcfg_11_09);
	ram_wr32(fuc, 0x10f6bc, 0x01010101 * next->bios.ramcfg_11_09);

	mask = 0x00010000;
	data = 0x00000000;
	if (!next->bios.ramcfg_11_02_80)
		data |= 0x03000000;
	if (!next->bios.ramcfg_11_02_40)
		data |= 0x00002000;
	if (!next->bios.ramcfg_11_07_10)
		data |= 0x00004000;
	if (!next->bios.ramcfg_11_07_08)
		data |= 0x00000003;
	else
		data |= 0x14000000;
	ram_mask(fuc, 0x10f824, mask, data);
	ram_nsec(fuc, 1000);

	if (next->bios.ramcfg_11_08_01)
		data = 0x00100000;
	else
		data = 0x00000000;
	ram_mask(fuc, 0x10f82c, 0x00100000, data);

	/* PFB timing */
	ram_mask(fuc, 0x10f248, 0xffffffff, next->bios.timing[10]);
	ram_mask(fuc, 0x10f290, 0xffffffff, next->bios.timing[0]);
	ram_mask(fuc, 0x10f294, 0xffffffff, next->bios.timing[1]);
	ram_mask(fuc, 0x10f298, 0xffffffff, next->bios.timing[2]);
	ram_mask(fuc, 0x10f29c, 0xffffffff, next->bios.timing[3]);
	ram_mask(fuc, 0x10f2a0, 0xffffffff, next->bios.timing[4]);
	ram_mask(fuc, 0x10f2a4, 0xffffffff, next->bios.timing[5]);
	ram_mask(fuc, 0x10f2a8, 0xffffffff, next->bios.timing[6]);
	ram_mask(fuc, 0x10f2ac, 0xffffffff, next->bios.timing[7]);
	ram_mask(fuc, 0x10f2cc, 0xffffffff, next->bios.timing[8]);
	ram_mask(fuc, 0x10f2e8, 0xffffffff, next->bios.timing[9]);

	mask = 0x33f00000;
	data = 0x00000000;
	if (!next->bios.ramcfg_11_01_04)
		data |= 0x20200000;
	if (!next->bios.ramcfg_11_07_80)
		data |= 0x12800000;
	/*XXX: see note above about there probably being some condition
	 *     for the 10f824 stuff that uses ramcfg 3...
	 */
	if (next->bios.ramcfg_11_03_f0) {
		if (next->bios.rammap_11_08_0c) {
			if (!next->bios.ramcfg_11_07_80)
				mask |= 0x00000020;
			else
				data |= 0x00000020;
			mask |= 0x08000004;
		}
		data |= 0x04000000;
	} else {
		mask |= 0x44000020;
		data |= 0x08000004;
	}

	ram_mask(fuc, 0x10f808, mask, data);

	ram_wr32(fuc, 0x10f870, 0x11111111 * next->bios.ramcfg_11_03_0f);

	ram_mask(fuc, 0x10f250, 0x000003f0, next->bios.timing_20_2c_003f << 4);

	data = (next->bios.timing[10] & 0x7f000000) >> 24;
	if (data < next->bios.timing_20_2c_1fc0)
		data = next->bios.timing_20_2c_1fc0;
	ram_mask(fuc, 0x10f24c, 0x7f000000, data << 24);

	ram_mask(fuc, 0x10f224, 0x001f0000, next->bios.timing_20_30_f8 << 16);

	ram_wr32(fuc, 0x10f090, 0x4000007f);
	ram_nsec(fuc, 1000);

	ram_wr32(fuc, 0x10f314, 0x00000001); /* PRECHARGE */
	ram_wr32(fuc, 0x10f310, 0x00000001); /* REFRESH */
	ram_wr32(fuc, 0x10f210, 0x80000000); /* REFRESH_AUTO = 1 */
	ram_nsec(fuc, 1000);

	if (!next->bios.ramcfg_DLLoff) {
		ram_mask(fuc, mr[1], 0x1, 0x0);
		nvkm_sddr3_dll_reset(fuc);
	}

	ram_mask(fuc, mr[2], 0x00000fff, ram->base.mr[2]);
	ram_mask(fuc, mr[1], 0xffffffff, ram->base.mr[1]);
	ram_wr32(fuc, mr[0], ram->base.mr[0]);
	ram_nsec(fuc, 1000);

	if (!next->bios.ramcfg_DLLoff) {
		nvkm_sddr3_dll_reset(fuc);
		ram_nsec(fuc, 1000);
	}

	if (vc == 0 && ram_have(fuc, gpio2E)) {
		u32 temp  = ram_mask(fuc, gpio2E, 0x3000, fuc->r_func2E[0]);
		if (temp != ram_rd32(fuc, gpio2E)) {
			ram_wr32(fuc, gpiotrig, 1);
			ram_nsec(fuc, 20000);
		}
	}

	if (ram->mode != 2) {
		ram_mask(fuc, 0x10f830, 0x01000000, 0x01000000);
		ram_mask(fuc, 0x10f830, 0x01000000, 0x00000000);
	}

	ram_mask(fuc, 0x10f200, 0x80000000, 0x80000000);
	ram_wr32(fuc, 0x10f318, 0x00000001); /* NOP? */
	ram_mask(fuc, 0x10f200, 0x80000000, 0x00000000);
	ram_nsec(fuc, 1000);

	ram_unblock(fuc);

	if (ram->base.fb->subdev.device->disp)
		ram_wr32(fuc, 0x62c000, 0x0f0f0f00);

	if (next->bios.rammap_11_08_01)
		data = 0x00000800;
	else
		data = 0x00000000;
	ram_mask(fuc, 0x10f200, 0x00000800, data);
	return 0;
}


#line 1 "host-oracle-main"
int main(void) {
    unsigned n, phases, a, data, display;
    if (scanf("%u %u %u",&n,&phases,&display)!=3) return 2;
    for (unsigned i=0;i<n;i++) {
        if (scanf("%x %x",&a,&data)!=2) return 2;
        *reg(a)=data;
    }
    struct device device={.disp=display};struct fb fb={.subdev={.device=&device}};
    struct nvkm_ram_data next={0};struct gk104_ram ram={.base={.next=&next,.fb=&fb}};
    for (unsigned i=0;i<phases;i++) {
        if (scanf("%d %d %d %d %d",&ram.M1,&ram.N1,&ram.P1,&ram.fN1,&ram.mode)!=5) return 2;
        if (scanf("%u",&next.bios.timing_ver)!=1) return 2;
        if (scanf("%u",&next.bios.timing_hdr)!=1) return 2;
        if (scanf("%u",&next.bios.timing_10_CWL)!=1) return 2;
        if (scanf("%u",&next.bios.timing_10_CL)!=1) return 2;
        if (scanf("%u",&next.bios.timing_10_WR)!=1) return 2;
        if (scanf("%u",&next.bios.timing_10_ODT)!=1) return 2;
        if (scanf("%u",&next.bios.ramcfg_DLLoff)!=1) return 2;
        if (scanf("%u",&next.bios.ramcfg_11_02_08)!=1) return 2;
        if (scanf("%u",&next.bios.ramcfg_11_02_04)!=1) return 2;
        if (scanf("%u",&next.bios.ramcfg_11_03_f0)!=1) return 2;
        if (scanf("%u",&next.bios.ramcfg_11_03_c0)!=1) return 2;
        if (scanf("%u",&next.bios.ramcfg_11_03_30)!=1) return 2;
        if (scanf("%u",&next.bios.ramcfg_11_02_80)!=1) return 2;
        if (scanf("%u",&next.bios.ramcfg_11_02_40)!=1) return 2;
        if (scanf("%u",&next.bios.ramcfg_11_07_10)!=1) return 2;
        if (scanf("%u",&next.bios.ramcfg_11_07_08)!=1) return 2;
        if (scanf("%u",&next.bios.ramcfg_11_07_40)!=1) return 2;
        if (scanf("%u",&next.bios.rammap_11_11_0c)!=1) return 2;
        if (scanf("%u",&next.bios.ramcfg_11_09)!=1) return 2;
        if (scanf("%u",&next.bios.ramcfg_11_08_01)!=1) return 2;
        if (scanf("%u",&next.bios.ramcfg_11_01_04)!=1) return 2;
        if (scanf("%u",&next.bios.ramcfg_11_07_80)!=1) return 2;
        if (scanf("%u",&next.bios.rammap_11_08_0c)!=1) return 2;
        if (scanf("%u",&next.bios.ramcfg_11_03_0f)!=1) return 2;
        if (scanf("%u",&next.bios.timing_20_2c_003f)!=1) return 2;
        if (scanf("%u",&next.bios.timing_20_2c_1fc0)!=1) return 2;
        if (scanf("%u",&next.bios.timing_20_30_f8)!=1) return 2;
        if (scanf("%u",&next.bios.rammap_11_08_01)!=1) return 2;

        for (unsigned j=0;j<11;j++) if (scanf("%u",&next.bios.timing[j])!=1) return 2;
        ram.base.mr[0]=*reg(0x10f300);ram.base.mr[1]=*reg(0x10f304);ram.base.mr[2]=*reg(0x10f320);
        if (nvkm_sddr3_calc(&ram.base)!=0) return 3;
        puts("PHASE");
        if (gk104_ram_calc_sddr3(&ram,0)!=0) return 4;
    }
    return 0;
}
