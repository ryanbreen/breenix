/*
 * Copyright 2010 Red Hat Inc.
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
 * claim-lint:ok: MIT license condition; source recorded in THIRD_PARTY_NOTICES.
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
// Oracle: original MIT clk/pllgt215.c:31-100, nonfractional branch.
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
typedef uint32_t u32;
struct nvkm_subdev { int unused; };
struct nvbios_pll { u32 refclk; int min_p, max_p; struct {
 u32 max_freq, min_inputfreq, max_inputfreq; int min_m,max_m,min_n,max_n;
} vco1; };
#define min(a,b) ((a)<(b)?(a):(b))
#define max(a,b) ((a)>(b)?(a):(b))
#define unlikely(a) (a)
#define EINVAL 22
#define nvkm_error(subdev, message) ((void)(subdev))
int
gt215_pll_calc(struct nvkm_subdev *subdev, struct nvbios_pll *info,
	       u32 freq, int *pN, int *pfN, int *pM, int *P)
{
	u32 best_err = ~0, err;
	int M, lM, hM, N, fN;

	*P = info->vco1.max_freq / freq;
	if (*P > info->max_p)
		*P = info->max_p;
	if (*P < info->min_p)
		*P = info->min_p;

	lM = (info->refclk + info->vco1.max_inputfreq) / info->vco1.max_inputfreq;
	lM = max(lM, (int)info->vco1.min_m);
	hM = (info->refclk + info->vco1.min_inputfreq) / info->vco1.min_inputfreq;
	hM = min(hM, (int)info->vco1.max_m);
	lM = min(lM, hM);

	for (M = lM; M <= hM; M++) {
		u32 tmp = freq * *P * M;
		N  = tmp / info->refclk;
		fN = tmp % info->refclk;

		if (!pfN) {
			if (fN >= info->refclk / 2)
				N++;
		} else {
			if (fN <  info->refclk / 2)
				N--;
			fN = tmp - (N * info->refclk);
		}

		if (N < info->vco1.min_n)
			continue;
		if (N > info->vco1.max_n)
			break;

		err = abs(freq - (info->refclk * N / M / *P));
		if (err < best_err) {
			best_err = err;
			*pN = N;
			*pM = M;
		}

		if (pfN) {
			*pfN = ((fN << 13) + info->refclk / 2) / info->refclk;
			*pfN = (*pfN - 4096) & 0xffff;
			return freq;
		}
	}

	if (unlikely(best_err == ~0)) {
		nvkm_error(subdev, "unable to find matching pll values\n");
		return -EINVAL;
	}

	return info->refclk * *pN / *pM / *P;
}

int main(int argc, char **argv) {
 if (argc != 3) return 2;
 struct nvbios_pll p = {.refclk=(u32)strtoul(argv[1],0,10),.min_p=1,.max_p=63,
 .vco1={.max_freq=2200000,.min_inputfreq=25000,.max_inputfreq=100000,
 .min_m=17,.max_m=32,.min_n=8,.max_n=255}};
 int n=0,m=0,d=0;
 int f=gt215_pll_calc(NULL,&p,(u32)strtoul(argv[2],0,10),&n,NULL,&m,&d);
 if (f<=0) return 1;
 printf("%u %u\n",(unsigned)((d<<16)|(n<<8)|m),(unsigned)f);
 return 0;
}
