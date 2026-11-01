/*
 * Copyright 2012 Red Hat Inc.
 * Copyright 2018 Red Hat Inc.
 * Copyright 2006 Artur Huillet, Ben Skeggs, B. R., Carlos Martin, Christoph Bumiller, Dawid Gajownik, Dmitry Baryshkov, Dmitry Eremin-Solenikov, EdB, Erik Waling, Francisco Jerez, Ilia Mirkin, jb17bsome, Jeremy Kolb, Laurent Carlier, Luca Barbieri, Maarten Maathuis, Marcelina Kościelnicka, Mark Carey, Matthieu Castet, nvidiaman, Patrice Mandin, Pekka Paalanen, Peter Popov, Richard Hughes, Rudi Cilibrasi, Serge Martin, Simon Raffeiner, Stephane Loeuillet, Stephane Marchesin, sturmflut, Sylvain Munaut, Victor Stinner, Wladmir van der Laan, Younes Manton
 *
 * Permission is hereby granted, free of charge, to any person obtaining
 * a copy of this software and associated documentation files (the
 * "Software"), to deal in the Software without restriction, including
 * without limitation the rights to use, copy, modify, merge, publish,
 * distribute, sublicense, and/or sell copies of the Software, and to
 * permit persons to whom the Software is furnished to do so, subject to
 * the following conditions:
 *
 * The above copyright notice and this permission notice (including the
 * next paragraph) shall be included in all copies or substantial
 * portions of the Software.
 *
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
 * EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
 * MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.
 * IN NO EVENT SHALL THE COPYRIGHT OWNER(S) AND/OR ITS SUPPLIERS BE
 * LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
 * OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION
 * WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
 */
//! Read-only C3b state. Raw addresses from MIT nouveau therm/g84.c:57-84,
//! 155-158, gk104.c:30-90, and MIT rnndb/pm/ptherm.xml:284-350,
//! 438-464,655-716 at f102b823. See docs/gui/gt710-c3b.md.
//! No interrupt acknowledgement, alarm programming or clock-gating writes.
use super::Access;
pub fn read(io:&mut impl Access)->[(u32,u32);31] {
    [0x20000,0x20004,0x20008,0x2000c,0x20044,0x20048,0x20100,
     0x20110,0x20114,0x20134,0x20138,0x201e0,0x201e4,0x201e8,0x201ec,
     0x20200,0x20204,0x20208,0x2020c,0x20210,0x20214,0x20218,0x2021c,
     0x20288,0x2028c,0x20414,0x20480,0x20484,0x204c0,0x204c4,0x200]
        .map(|a|(a,io.read(a)))
}
